use crate::{
    domain::*,
    infrastructure::storage::{SavedData, Storage},
    providers::{ExecuteContext, Providers},
};
use anyhow::{Result, bail};
use chrono::Utc;
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::mpsc;
use uuid::Uuid;

pub type SharedData = Arc<Mutex<SavedData>>;
pub type Notify = Arc<dyn Fn() + Send + Sync>;
pub struct OperationManager {
    data: SharedData,
    storage: Arc<Storage>,
    tx: mpsc::UnboundedSender<Uuid>,
    cancellations: Arc<Mutex<HashMap<Uuid, Arc<AtomicBool>>>>,
    notify: Notify,
}
impl OperationManager {
    pub fn start(
        data: SharedData,
        storage: Arc<Storage>,
        providers: Arc<Providers>,
        notify: Notify,
    ) -> Arc<Self> {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let manager = Arc::new(Self {
            data,
            storage,
            tx,
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            notify,
        });
        let worker = manager.clone();
        tokio::spawn(async move {
            while let Some(id) = rx.recv().await {
                worker.execute(id, &providers).await;
            }
        });
        manager
    }
    pub fn enqueue(&self, requests: Vec<OperationRequest>) -> Result<usize> {
        self.enqueue_prepared(requests, vec![], None, None)
    }
    pub fn enqueue_prepared(
        &self,
        requests: Vec<OperationRequest>,
        manual: Vec<ManualPending>,
        batch: Option<String>,
        profile: Option<String>,
    ) -> Result<usize> {
        let mut current = self.data.lock().unwrap();
        let mut data = current.clone();
        let active = data
            .history
            .iter()
            .filter(|o| matches!(o.state, OperationState::Pending | OperationState::Running))
            .count();
        if active + requests.len() > 500 {
            bail!("A fila suporta até 500 operações");
        }
        let mut ids = vec![];
        let request_batch = requests.iter().rev().find_map(|r| r.batch.clone());
        for item in manual {
            crate::services::manual::register(&mut data, item)?;
        }
        if let Some(batch) = batch.or(request_batch) {
            data.last_batch = Some(batch);
        }
        if let Some(profile) = profile {
            data.settings.last_profile = Some(profile);
        }
        for request in requests {
            if data.history.iter().any(|o| {
                matches!(o.state, OperationState::Pending | OperationState::Running)
                    && o.request.package.key() == request.package.key()
                    && o.request.action == request.action
                    && (request.package.provider != ProviderId::Custom
                        || (o.request.profile == request.profile
                            && o.request.batch == request.batch
                            && o.request.custom_app == request.custom_app))
            }) {
                continue;
            }
            let op = Operation::new(request);
            ids.push(op.id);
            data.history.push(op);
        }
        // Persist before dispatch so queued changes are always recoverable.
        self.storage.save(&data)?;
        *current = data;
        drop(current);
        for id in &ids {
            self.tx
                .send(*id)
                .map_err(|_| anyhow::anyhow!("Fila indisponível"))?;
        }
        (self.notify)();
        Ok(ids.len())
    }
    pub fn cancel(&self, id: Uuid) -> Result<()> {
        let mut data = self.data.lock().unwrap();
        let op = data
            .history
            .iter_mut()
            .find(|o| o.id == id)
            .ok_or_else(|| anyhow::anyhow!("Operação não encontrada"))?;
        if op.state == OperationState::Pending {
            op.state = op.state.transition(OperationState::Cancelled)?;
            op.message = "Cancelado antes da execução".into();
            op.finished_at = Some(Utc::now());
            self.storage.save(&data)?;
        } else if op.state == OperationState::Running && op.request.action == Action::Download {
            if let Some(flag) = self.cancellations.lock().unwrap().get(&id) {
                flag.store(true, Ordering::Relaxed);
            }
        } else {
            bail!(
                "Instaladores em execução não podem ser interrompidos com segurança. Cancele na janela do próprio instalador, se disponível."
            );
        }
        drop(data);
        (self.notify)();
        Ok(())
    }
    pub fn has_active(&self) -> bool {
        self.data
            .lock()
            .unwrap()
            .history
            .iter()
            .any(|o| matches!(o.state, OperationState::Running | OperationState::Pending))
    }
    fn persist(&self) {
        let mut data = self.data.lock().unwrap();
        if let Err(e) = self.storage.save(&data)
            && let Some(op) = data.history.last_mut()
        {
            op.message
                .push_str(&format!("\nFalha ao salvar histórico: {e:#}"));
        }
    }
    async fn execute(&self, id: Uuid, providers: &Providers) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (request, downloads) = {
            let mut data = self.data.lock().unwrap();
            let downloads = data.settings.downloads.clone();
            let Some(op) = data.history.iter_mut().find(|o| o.id == id) else {
                return;
            };
            if op.state != OperationState::Pending {
                return;
            }
            op.state = OperationState::Running;
            op.message = "Iniciando operação".into();
            self.cancellations
                .lock()
                .unwrap()
                .insert(id, cancel.clone());
            (op.request.clone(), downloads)
        };
        self.persist();
        (self.notify)();
        let data = self.data.clone();
        let notify = self.notify.clone();
        let context = ExecuteContext {
            silent: request.silent,
            accept_agreements: request.accept_agreements,
            downloads,
            cancel: cancel.clone(),
            progress: Arc::new(move |p| {
                if let Some(op) = data.lock().unwrap().history.iter_mut().find(|o| o.id == id) {
                    op.message = p.message;
                    op.downloaded = p.downloaded;
                    op.total = p.total;
                }
                notify();
            }),
        };
        let provider = providers.get(request.package.provider);
        let result = if provider.capabilities().actions.contains(&request.action) {
            provider.execute(&request, context).await
        } else {
            Err(anyhow::anyhow!("O provider não suporta esta operação"))
        };
        self.cancellations.lock().unwrap().remove(&id);
        {
            let mut data = self.data.lock().unwrap();
            if let Some(op) = data.history.iter_mut().find(|o| o.id == id) {
                match result {
                    Ok(r) => {
                        op.state = if r.exit_code == Some(0) {
                            OperationState::Completed
                        } else {
                            OperationState::Failed
                        };
                        op.message = r.message;
                        op.stdout = bounded_log(&r.stdout);
                        op.stderr = bounded_log(&r.stderr);
                        op.exit_code = r.exit_code;
                        op.file = r.file;
                        op.sha256 = r.sha256;
                        op.download_metadata = r.download_metadata;
                    }
                    Err(e) => {
                        op.state = if cancel.load(Ordering::Relaxed) {
                            OperationState::Cancelled
                        } else {
                            OperationState::Failed
                        };
                        op.message = format!("{e:#}");
                    }
                }
                op.finished_at = Some(Utc::now());
            }
            while data.history.len() > 300 {
                if let Some(i) = data.history.iter().position(|o| {
                    !matches!(o.state, OperationState::Pending | OperationState::Running)
                }) {
                    data.history.remove(i);
                } else {
                    break;
                }
            }
        }
        self.persist();
        (self.notify)();
    }
}

fn bounded_log(text: &str) -> String {
    const LIMIT: usize = 16 * 1024;
    if text.len() <= LIMIT {
        return text.to_owned();
    }
    let mut start = text.len() - LIMIT;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!(
        "[Saída anterior truncada; consulte os logs do Winget para a saída integral]\n{}",
        &text[start..]
    )
}
