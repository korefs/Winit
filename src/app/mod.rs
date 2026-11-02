mod custom;
mod render;
mod self_test;
use crate::AppWindow;
use anyhow::{Context, Result, bail};
use slint::ComponentHandle;
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;
use winit::{
    domain::*,
    infrastructure::{
        process,
        storage::{SavedData, Storage},
    },
    providers::{Providers, SoftwareProvider, github::Release, windows::TOOLS},
    services::{
        operations::{Notify, OperationManager, SharedData},
        profiles,
    },
};

#[derive(Clone)]
enum Confirmation {
    Queue(Vec<OperationRequest>),
    DeleteProfile(usize),
    ExecuteFile(Uuid),
    Tool(String),
    Prepared(profiles::PreparedProfile),
    DeleteCustom(Vec<Uuid>),
    DeletePending(Vec<Uuid>),
    ImportCatalog(winit::services::catalog::CatalogDocument),
}
#[derive(Clone)]
struct Dialog {
    title: String,
    text: String,
    label: String,
    terms: bool,
    confirmation: Option<Confirmation>,
}
#[derive(Clone)]
struct View {
    page: i32,
    query: String,
    filter: String,
    selected: HashSet<String>,
    search: Vec<Package>,
    installed: Option<Vec<Package>>,
    updates: Option<Vec<Package>>,
    winget_available: bool,
    winget_status: String,
    working: usize,
    status: String,
    error: bool,
    release: Option<(String, Release)>,
    selected_profile: Option<usize>,
    editor: String,
    editor_revision: i32,
    dialog: Option<Dialog>,
    custom: custom::CustomView,
}
impl Default for View {
    fn default() -> Self {
        Self {
            page: 0,
            query: String::new(),
            filter: "all".into(),
            selected: HashSet::new(),
            search: vec![],
            installed: None,
            updates: None,
            winget_available: false,
            winget_status: "Verificando…".into(),
            working: 0,
            status: "Verificando providers e aplicativos instalados…".into(),
            error: false,
            release: None,
            selected_profile: None,
            editor: String::new(),
            editor_revision: 0,
            dialog: None,
            custom: custom::CustomView::default(),
        }
    }
}
struct Controller {
    data: SharedData,
    storage: Arc<Storage>,
    providers: Arc<Providers>,
    view: Arc<Mutex<View>>,
    operations: Arc<OperationManager>,
    notify: Notify,
    search_revision: AtomicU64,
    custom_revision: AtomicU64,
}

pub fn run() -> Result<()> {
    let self_test = std::env::args().any(|a| a == "--self-test");
    let storage = Arc::new(if self_test {
        let root = std::env::current_dir()?
            .join("target")
            .join("qa")
            .join(format!("session-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        Storage {
            root,
            portable: true,
        }
    } else {
        Storage::discover()?
    });
    let instance = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(storage.root.join("instance.lock"))?;
    instance.try_lock().map_err(|_|anyhow::anyhow!("Outra instância do Winit está usando este diretório de dados. Feche-a antes de continuar."))?;
    let data = Arc::new(Mutex::new(storage.load()?));
    let providers = Arc::new(Providers::new()?);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let guard = runtime.enter();
    let ui = AppWindow::new()?;
    let view = Arc::new(Mutex::new(View::default()));
    let weak = ui.as_weak();
    let notify: Notify = {
        let data = data.clone();
        let view = view.clone();
        let storage = storage.clone();
        Arc::new(move || {
            let data = data.clone();
            let view = view.clone();
            let storage = storage.clone();
            let _ =
                weak.upgrade_in_event_loop(move |ui| render::render(&ui, &data, &view, &storage));
        })
    };
    let operations = OperationManager::start(
        data.clone(),
        storage.clone(),
        providers.clone(),
        notify.clone(),
    );
    let controller = Arc::new(Controller {
        data,
        storage,
        providers,
        view,
        operations,
        notify,
        search_revision: AtomicU64::new(0),
        custom_revision: AtomicU64::new(0),
    });
    {
        let c = controller.clone();
        let weak = ui.as_weak();
        ui.on_navigate(move |page| {
            if let Some(ui) = weak.upgrade() {
                ui.set_query("".into());
                ui.set_filter("all".into());
            }
            let c = c.clone();
            tokio::spawn(async move {
                c.navigate(page).await;
            });
        });
    }
    {
        let c = controller.clone();
        ui.on_action(move |name, payload| {
            let c = c.clone();
            tokio::spawn(async move {
                if let Err(e) = c.action(name.as_str(), payload.as_str()).await {
                    c.report(e);
                }
            });
        });
    }
    {
        let c = controller.clone();
        ui.on_entry_action(move |key, index| {
            let c = c.clone();
            tokio::spawn(async move {
                if let Err(e) = c.entry_action(key.as_str(), index).await {
                    c.report(e);
                }
            });
        });
    }
    {
        let c = controller.clone();
        ui.on_toggle(move |key| {
            {
                let mut v = c.view.lock().unwrap();
                if !v.selected.remove(key.as_str()) {
                    v.selected.insert(key.to_string());
                }
            }
            (c.notify)();
        });
    }
    {
        let c = controller.clone();
        ui.on_query_edited(move |query| {
            let revision = c.search_revision.fetch_add(1, Ordering::SeqCst) + 1;
            let c = c.clone();
            let query = query.to_string();
            tokio::spawn(async move {
                c.search(query, revision).await;
            });
        });
    }
    {
        let c = controller.clone();
        ui.on_select_profile(move |index| {
            let c = c.clone();
            tokio::spawn(async move {
                if let Err(e) = c.select_profile(index as usize) {
                    c.report(e);
                }
            });
        });
    }
    {
        let c = controller.clone();
        ui.on_save_profile(move |json| {
            let c = c.clone();
            tokio::spawn(async move {
                if let Err(e) = c.save_profile(json.as_str(), false) {
                    c.report(e);
                }
            });
        });
    }
    {
        let c = controller.clone();
        ui.on_settings(move |silent, agreements| {
            let c = c.clone();
            tokio::spawn(async move {
                {
                    let mut d = c.data.lock().unwrap();
                    d.settings.silent_installers = silent;
                    d.settings.accept_source_agreements = agreements;
                }
                if let Err(e) = c.save() {
                    c.report(e);
                } else {
                    c.status("Preferências salvas", false);
                }
            });
        });
    }
    {
        let c = controller.clone();
        ui.on_theme(move |dark| {
            let c = c.clone();
            tokio::spawn(async move {
                c.data.lock().unwrap().settings.dark_theme = dark;
                if let Err(e) = c.save() {
                    c.report(e);
                } else {
                    (c.notify)();
                }
            });
        });
    }
    {
        let c = controller.clone();
        ui.on_dialog_result(move |confirmed, terms| {
            let c = c.clone();
            tokio::spawn(async move {
                if let Err(e) = c.confirm(confirmed, terms).await {
                    c.report(e);
                }
            });
        });
    }
    {
        let c = controller.clone();
        ui.window().on_close_requested(move || {
            if c.operations.has_active() {
                c.info("Operações em andamento","Aguarde os instaladores terminarem antes de fechar. Você pode cancelar tarefas pendentes e downloads em Operações. Instaladores em execução não são encerrados à força.");
                slint::CloseRequestResponse::KeepWindowShown
            } else {slint::CloseRequestResponse::HideWindow}
        });
    }
    render::render(&ui, &controller.data, &controller.view, &controller.storage);
    custom::connect(&ui, &controller);
    let test_result = if self_test {
        Some(self_test::start(controller.clone(), ui.as_weak()))
    } else {
        None
    };
    {
        let c = controller.clone();
        tokio::spawn(async move {
            c.initialize().await;
        });
    }
    ui.run()?;
    if let Some(result) = test_result {
        result
            .lock()
            .unwrap()
            .take()
            .context("Autoteste não finalizou")??;
    }
    drop(guard);
    runtime.shutdown_timeout(Duration::from_secs(2));
    Ok(())
}

impl Controller {
    fn status(&self, message: impl Into<String>, error: bool) {
        {
            let mut v = self.view.lock().unwrap();
            v.status = message.into();
            v.error = error;
        }
        (self.notify)();
    }
    fn report(&self, error: anyhow::Error) {
        self.status(format!("{error:#}"), true);
        self.info("Não foi possível concluir", &format!("{error:#}"));
    }
    fn info(&self, title: &str, text: &str) {
        self.view.lock().unwrap().dialog = Some(Dialog {
            title: title.into(),
            text: text.into(),
            label: "".into(),
            terms: false,
            confirmation: None,
        });
        (self.notify)();
    }
    fn ask(&self, title: &str, text: String, label: &str, terms: bool, confirmation: Confirmation) {
        self.view.lock().unwrap().dialog = Some(Dialog {
            title: title.into(),
            text,
            label: label.into(),
            terms,
            confirmation: Some(confirmation),
        });
        (self.notify)();
    }
    fn save(&self) -> Result<()> {
        self.storage.save(&self.data.lock().unwrap())
    }
    fn agreements(&self) -> bool {
        self.data.lock().unwrap().settings.accept_source_agreements
    }
    fn working(&self, start: bool) {
        {
            let mut v = self.view.lock().unwrap();
            if start {
                v.working += 1;
            } else {
                v.working = v.working.saturating_sub(1);
            }
        }
        (self.notify)();
    }
    async fn initialize(&self) {
        self.working(true);
        let registry = self
            .providers
            .windows
            .installed(false)
            .await
            .unwrap_or_default();
        self.view.lock().unwrap().installed = Some(registry.clone());
        match self.providers.winget.diagnose().await {
            Ok(version) => {
                {
                    let mut v = self.view.lock().unwrap();
                    v.winget_available = true;
                    v.winget_status = format!("Winget {version}");
                }
                let accept = self.agreements();
                let (installed, updates) = tokio::join!(
                    self.providers.winget.installed(accept),
                    self.providers.winget.updates(accept)
                );
                let mut failures = vec![];
                match installed {
                    Ok(p) => {
                        self.view.lock().unwrap().installed = Some(merge_installed(p, registry))
                    }
                    Err(e) => failures.push(format!("Listagem: {e:#}")),
                }
                match updates {
                    Ok(p) => self.view.lock().unwrap().updates = Some(p),
                    Err(e) => failures.push(format!("Atualizações: {e:#}")),
                }
                if failures.is_empty() {
                    self.status("Tudo pronto. Escolha seu próximo passo.", false);
                } else {
                    self.status(failures.join("\n"), true);
                }
            }
            Err(e) => {
                {
                    let mut v = self.view.lock().unwrap();
                    v.winget_available = false;
                    v.winget_status = "Winget indisponível".into();
                }
                self.status(format!("{e:#}"), true);
            }
        }
        self.working(false);
    }
    async fn navigate(&self, page: i32) {
        self.search_revision.fetch_add(1, Ordering::SeqCst);
        {
            let mut v = self.view.lock().unwrap();
            v.page = page;
            v.query.clear();
            v.filter = "all".into();
            v.selected.clear();
            v.error = false;
            v.status = "Pronto para a próxima ação".into();
            if page == 1 {
                v.search.clear();
            }
        }
        if page == 5 {
            let index = self.view.lock().unwrap().selected_profile;
            if index.is_none() && !self.data.lock().unwrap().profiles.is_empty() {
                let _ = self.select_profile(0);
            }
        }
        (self.notify)();
        if matches!(page, 2 | 3)
            && let Err(e) = self.refresh(page).await
        {
            self.report(e);
        }
    }
    async fn refresh(&self, page: i32) -> Result<()> {
        self.working(true);
        let result = match page {
            2 => {
                let registry = self.providers.windows.installed(false).await?;
                let available = self.view.lock().unwrap().winget_available;
                if available {
                    self.providers
                        .winget
                        .installed(self.agreements())
                        .await
                        .map(|p| {
                            self.view.lock().unwrap().installed =
                                Some(merge_installed(p, registry));
                        })
                } else {
                    self.view.lock().unwrap().installed = Some(registry);
                    Ok(())
                }
            }
            3 => self
                .providers
                .winget
                .updates(self.agreements())
                .await
                .map(|p| {
                    self.view.lock().unwrap().updates = Some(p);
                }),
            1 => {
                let q = self.view.lock().unwrap().query.clone();
                self.providers
                    .winget
                    .search(&q, self.agreements())
                    .await
                    .map(|p| {
                        self.view.lock().unwrap().search = p;
                    })
            }
            _ => Ok(()),
        };
        self.working(false);
        result?;
        self.status("Listagem atualizada", false);
        Ok(())
    }
    async fn search(&self, query: String, revision: u64) {
        let page = {
            let mut v = self.view.lock().unwrap();
            v.query = query.clone();
            v.page
        };
        if page != 1 {
            (self.notify)();
            return;
        }
        tokio::time::sleep(Duration::from_millis(420)).await;
        if self.search_revision.load(Ordering::SeqCst) != revision {
            return;
        }
        if query.trim().len() < 2 {
            self.view.lock().unwrap().search.clear();
            (self.notify)();
            return;
        }
        self.working(true);
        let result = self
            .providers
            .winget
            .search(&query, self.agreements())
            .await;
        self.working(false);
        if self.search_revision.load(Ordering::SeqCst) != revision {
            return;
        }
        match result {
            Ok(p) => {
                let n = p.len();
                self.view.lock().unwrap().search = p;
                self.status(
                    format!("{n} resultados reais do Winget (máximo de 100 por busca)"),
                    false,
                );
            }
            Err(e) => {
                self.view.lock().unwrap().search.clear();
                self.status(format!("{e:#}"), true);
            }
        }
    }
    fn select_profile(&self, index: usize) -> Result<()> {
        let json = {
            let d = self.data.lock().unwrap();
            serde_json::to_string_pretty(d.profiles.get(index).context("Perfil não encontrado")?)?
        };
        {
            let mut v = self.view.lock().unwrap();
            v.selected_profile = Some(index);
            v.editor = json;
            v.editor_revision += 1;
        }
        (self.notify)();
        Ok(())
    }
    fn save_profile(&self, json: &str, duplicate: bool) -> Result<()> {
        let mut p = Profile::parse(json)?;
        let replace = if duplicate {
            None
        } else {
            self.view.lock().unwrap().selected_profile
        };
        if duplicate {
            p.name = format!("{} — cópia", p.name);
        }
        let index = {
            let mut d = self.data.lock().unwrap();
            let mut copy = d.clone();
            let old_name = replace
                .and_then(|i| copy.profiles.get(i))
                .map(|p| p.name.clone());
            winit::services::catalog::import_profile_definitions(&mut copy, &mut p)?;
            let new_name = p.name.clone();
            let index = profiles::save(&mut copy.profiles, p, replace)?;
            if let Some(old) = old_name
                && old != new_name
            {
                for pending in &mut copy.manual_pending {
                    if pending.profile.as_ref() == Some(&old) {
                        pending.profile = Some(new_name.clone());
                    }
                }
            }
            self.storage.save(&copy)?;
            *d = copy;
            index
        };
        self.select_profile(index)?;
        self.status("Perfil salvo e validado", false);
        Ok(())
    }
    fn request(&self, package: PackageRef, action: Action) -> OperationRequest {
        OperationRequest {
            package,
            action,
            asset: None,
            custom_app: None,
            profile: None,
            batch: None,
            accept_agreements: false,
            silent: self.data.lock().unwrap().settings.silent_installers,
        }
    }
    fn packages(&self) -> Vec<Package> {
        let d = self.data.lock().unwrap().clone();
        let v = self.view.lock().unwrap();
        package_source(&v, &d)
    }
    fn selected_packages(&self) -> Vec<PackageRef> {
        let selected = self.view.lock().unwrap().selected.clone();
        self.packages()
            .into_iter()
            .filter(|p| p.manageable && selected.contains(&p.reference.key()))
            .map(|p| p.reference)
            .collect()
    }
    fn confirm_queue(&self, requests: Vec<OperationRequest>, prefix: &str) -> Result<()> {
        if requests.is_empty() {
            bail!("Selecione ao menos um aplicativo elegível");
        }
        let terms = requests
            .iter()
            .any(|r| r.package.provider == ProviderId::Winget && r.action != Action::Uninstall);
        let list = requests
            .iter()
            .map(|r| {
                format!(
                    "• {} {} [{}]",
                    r.action.label(),
                    r.package.id,
                    r.package.provider.label()
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let warning = if requests.iter().any(|r| r.action == Action::Uninstall) {
            "\nA desinstalação remove o aplicativo e pode remover configurações e dados. Revise antes de continuar."
        } else {
            "\nInstaladores podem solicitar elevação pelo UAC. O suporte ao modo silencioso depende do pacote. As tarefas serão executadas uma por vez. Não será solicitado reinício automático."
        };
        self.ask("Revise as operações",format!("{prefix}\n\n{list}\n{warning}\n\nOs termos e licenças são definidos por cada fornecedor. Use Detalhes no catálogo para consultar o pacote."),"Confirmar e enfileirar",terms,Confirmation::Queue(requests));
        Ok(())
    }
    async fn plan_profile(&self, profile: Profile) -> Result<()> {
        profile.validate()?;
        self.working(true);
        let result = async {
            let needs_winget = profile
                .packages
                .iter()
                .any(|p| p.provider == ProviderId::Winget);
            let installed = if needs_winget {
                self.providers.winget.diagnose().await?;
                self.providers.winget.installed(self.agreements()).await?
            } else {
                vec![]
            };
            if needs_winget {
                let registry = self.providers.windows.installed(false).await?;
                self.view.lock().unwrap().installed =
                    Some(merge_installed(installed.clone(), registry));
            }
            let data = self.data.lock().unwrap().clone();
            let prepared =
                profiles::prepare(&profile, &data, &installed, needs_winget, &self.providers)
                    .await?;
            self.ask_prepared(prepared)
        }
        .await;
        self.working(false);
        result
    }
    async fn github_query(&self, repo: &str) -> Result<()> {
        validate_repo(repo)?;
        self.working(true);
        let result = self.providers.github.latest(repo).await;
        self.working(false);
        let release = result?;
        let count = release
            .assets
            .iter()
            .filter(|a| winit::providers::github::compatible(a))
            .count();
        self.view.lock().unwrap().release = Some((repo.into(), release));
        self.status(format!("Release consultada: {count} candidatos para Windows x64. Confira o nome e a arquitetura."),false);
        Ok(())
    }

    async fn action(&self, name: &str, payload: &str) -> Result<()> {
        if name.starts_with("custom-") || name.starts_with("pending-") {
            return self.custom_action(name, payload).await;
        }
        match name {
            "refresh" => {
                let page = self.view.lock().unwrap().page;
                self.refresh(page).await?;
            }
            "filter" => {
                self.view.lock().unwrap().filter = payload.into();
                (self.notify)();
            }
            "clear-selection" => {
                self.view.lock().unwrap().selected.clear();
                (self.notify)();
            }
            "install-selected" | "update-selected" => {
                let a = if name == "install-selected" {
                    Action::Install
                } else {
                    Action::Update
                };
                let installed: HashSet<_> = self
                    .view
                    .lock()
                    .unwrap()
                    .installed
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|p| p.reference.key())
                    .collect();
                self.confirm_queue(
                    self.selected_packages()
                        .into_iter()
                        .filter(|p| a != Action::Install || !installed.contains(&p.key()))
                        .map(|p| self.request(p, a))
                        .collect(),
                    "Operação em lote",
                )?;
            }
            "update-all" => {
                let packages = self
                    .view
                    .lock()
                    .unwrap()
                    .updates
                    .clone()
                    .unwrap_or_default();
                let batch = format!("Atualizações · {}", Uuid::new_v4());
                let requests = packages
                    .into_iter()
                    .filter(|p| p.manageable)
                    .map(|p| {
                        let mut r = self.request(p.reference, Action::Update);
                        r.batch = Some(batch.clone());
                        r
                    })
                    .collect();
                self.confirm_queue(
                    requests,
                    "Atualizar todos os aplicativos elegíveis da lista atual",
                )?;
            }
            "selection-profile" => {
                let selected = self.selected_packages();
                if selected.is_empty() {
                    bail!("Selecione aplicativos para adicionar ao perfil");
                }
                let index = self.view.lock().unwrap().selected_profile;
                let (index, count) = {
                    let mut current = self.data.lock().unwrap();
                    let mut d = current.clone();
                    let apps = d.custom_apps.clone();
                    let i = match index.filter(|i| *i < d.profiles.len()) {
                        Some(i) => i,
                        None => {
                            let n = d.profiles.len() + 1;
                            d.profiles.push(Profile {
                                schema_version: 1,
                                name: format!("Meu setup {n}"),
                                description: String::new(),
                                packages: vec![],
                                custom_apps: vec![],
                            });
                            d.profiles.len() - 1
                        }
                    };
                    let p = &mut d.profiles[i];
                    let mut keys: HashSet<_> = p.packages.iter().map(PackageRef::key).collect();
                    let mut count = 0;
                    for reference in selected {
                        if keys.insert(reference.key()) {
                            p.packages.push(reference);
                            count += 1;
                        }
                    }
                    p.validate()?;
                    winit::services::catalog::hydrate_profile(p, &apps)?;
                    self.storage.save(&d)?;
                    *current = d;
                    (i, count)
                };
                self.select_profile(index)?;
                self.status(
                    format!("{count} aplicativos adicionados ao perfil selecionado em Meus perfis"),
                    false,
                );
            }
            "profile-new" => {
                let n = self.data.lock().unwrap().profiles.len() + 1;
                let p = Profile {
                    schema_version: 1,
                    name: format!("Meu setup {n}"),
                    description: "Meu ambiente de aplicativos".into(),
                    packages: vec![],
                    custom_apps: vec![],
                };
                {
                    let mut v = self.view.lock().unwrap();
                    v.selected_profile = None;
                    v.editor = serde_json::to_string_pretty(&p)?;
                    v.editor_revision += 1;
                }
                (self.notify)();
            }
            "profile-duplicate" => self.save_profile(payload, true)?,
            "profile-delete" => {
                let i = self
                    .view
                    .lock()
                    .unwrap()
                    .selected_profile
                    .context("Selecione um perfil salvo")?;
                let name = self
                    .data
                    .lock()
                    .unwrap()
                    .profiles
                    .get(i)
                    .context("Perfil não encontrado")?
                    .name
                    .clone();
                self.ask("Excluir perfil?",format!("O perfil “{name}” será removido dos dados locais. Os aplicativos instalados não serão removidos."),"Excluir perfil",false,Confirmation::DeleteProfile(i));
            }
            "profile-import" => {
                if let Some(file) = rfd::AsyncFileDialog::new()
                    .add_filter("Perfil JSON", &["json"])
                    .pick_file()
                    .await
                {
                    let p = profiles::import(file.path())?;
                    let i = self.import_profile_data(p)?;
                    self.select_profile(i)?;
                    self.status("Perfil importado e validado", false);
                }
            }
            "profile-export" => {
                let mut p = Profile::parse(payload)?;
                let apps = self.data.lock().unwrap().custom_apps.clone();
                winit::services::catalog::hydrate_profile(&mut p, &apps)?;
                if let Some(file) = rfd::AsyncFileDialog::new()
                    .add_filter("Perfil JSON", &["json"])
                    .set_file_name("meu-perfil.json")
                    .save_file()
                    .await
                {
                    profiles::export(file.path(), &p)?;
                    self.status("Perfil exportado", false);
                }
            }
            "profile-run" => self.plan_profile(Profile::parse(payload)?).await?,
            "last-profile" => {
                let p = {
                    let d = self.data.lock().unwrap();
                    d.settings
                        .last_profile
                        .as_ref()
                        .and_then(|n| d.profiles.iter().find(|p| &p.name == n))
                        .or(d.profiles.first())
                        .cloned()
                        .context("Crie ou importe um perfil em Meus perfis")?
                };
                self.plan_profile(p).await?;
            }
            "github-query" => self.github_query(payload).await?,
            "repository-save" => {
                validate_repo(payload)?;
                {
                    let mut d = self.data.lock().unwrap();
                    if !d
                        .settings
                        .repositories
                        .iter()
                        .any(|r| r.eq_ignore_ascii_case(payload))
                    {
                        if d.settings.repositories.len() >= 8 {
                            bail!(
                                "Limite de 8 repositórios. Edite os dados locais para remover um repositório."
                            );
                        }
                        d.settings.repositories.push(payload.into());
                    }
                }
                self.save()?;
                self.status("Repositório salvo", false);
            }
            "download-directory" => {
                if let Some(dir) = rfd::AsyncFileDialog::new().pick_folder().await {
                    self.data.lock().unwrap().settings.downloads = dir.path().into();
                    self.save()?;
                    (self.notify)();
                }
            }
            "open-downloads" => {
                let dir = self.data.lock().unwrap().settings.downloads.clone();
                std::fs::create_dir_all(&dir)?;
                process::open_path(&dir)?;
            }
            "open-data" => process::open_path(&self.storage.root)?,
            "diagnose" => {
                self.initialize().await;
                let v = self.view.lock().unwrap().clone();
                self.info("Diagnóstico Winget",&format!("{}\n\n{}\n\nSe a origem solicitar termos, revise a preferência de aceitação nesta tela. O Winit usa a origem winget e não altera fontes automaticamente.",v.winget_status,v.status));
            }
            "recovery" => process::open_url(
                "https://learn.microsoft.com/windows/package-manager/winget/#install-winget",
            )?,
            "status-details" => {
                let text = self.view.lock().unwrap().status.clone();
                self.info("Detalhes do status", &text);
            }
            _ => bail!("Ação desconhecida: {name}"),
        }
        Ok(())
    }

    async fn entry_action(&self, key: &str, index: i32) -> Result<()> {
        let page = self.view.lock().unwrap().page;
        if matches!(page, 8 | 9) {
            return self.custom_entry_action(key, index).await;
        }
        if matches!(page, 1..=3) {
            let p = self
                .packages()
                .into_iter()
                .find(|p| p.reference.key() == key)
                .context("O resultado mudou. Atualize a lista.")?;
            if p.reference.provider == ProviderId::Windows {
                self.info(&p.name,&format!("ID: {}\nOrigem: {}\nVersão: {}\n\nEste aplicativo foi detectado no registro e não foi associado a um pacote Winget. Para removê-lo, use Configurações do Windows > Aplicativos. O Winit não executa comandos de desinstalação extraídos do registro.",p.reference.id,p.source,p.version));
                return Ok(());
            }
            if !p.manageable {
                bail!("O Winget truncou este identificador. Refine a busca pelo ID completo.");
            }
            match index {
                0 => {
                    let a = if page == 3 {
                        Action::Update
                    } else if page == 2 {
                        Action::Uninstall
                    } else {
                        Action::Install
                    };
                    self.confirm_queue(vec![self.request(p.reference, a)], "Operação individual")?;
                }
                1 => {
                    if page == 2 && !p.available.is_empty() {
                        self.confirm_queue(
                            vec![self.request(p.reference, Action::Update)],
                            "Atualização individual",
                        )?;
                    } else {
                        {
                            let mut d = self.data.lock().unwrap();
                            if let Some(i) = d.favorites.iter().position(|p| p.key() == key) {
                                d.favorites.remove(i);
                            } else {
                                d.favorites.push(p.reference);
                            }
                        }
                        self.save()?;
                        (self.notify)();
                    }
                }
                2 => {
                    self.working(true);
                    let r = self
                        .providers
                        .winget
                        .details(&p.reference.id, self.agreements())
                        .await;
                    self.working(false);
                    self.info(&p.name, &r?);
                }
                _ => {}
            }
        } else if page == 4 {
            if let Some(id) = key.strip_prefix("tool:") {
                let tool = TOOLS
                    .iter()
                    .find(|t| t.id == id)
                    .context("Ferramenta não encontrada")?;
                if tool.modifies {
                    self.ask(tool.name,format!("{}\n\nSerá aberta a ferramenta oficial do Windows. Revise as alterações antes de aplicá-las. O Winit não executa mudanças automaticamente.",tool.description),"Abrir ferramenta",false,Confirmation::Tool(id.into()));
                } else {
                    self.operations.enqueue(vec![self.request(
                        PackageRef {
                            provider: ProviderId::Windows,
                            id: id.into(),
                        },
                        Action::Launch,
                    )])?;
                    self.status(
                        format!("Abertura de {} adicionada à fila", tool.name),
                        false,
                    );
                }
            } else if let Some(name) = key.strip_prefix("asset:") {
                let (repo, release) = self
                    .view
                    .lock()
                    .unwrap()
                    .release
                    .clone()
                    .context("Consulte uma release")?;
                let asset = release
                    .assets
                    .iter()
                    .find(|a| a.name == name)
                    .cloned()
                    .context("Asset não encontrado")?;
                if index == 0 {
                    let mut r = self.request(
                        PackageRef {
                            provider: ProviderId::Github,
                            id: repo,
                        },
                        Action::Download,
                    );
                    r.asset = Some(asset.clone());
                    self.ask("Baixar release oficial?",format!("Arquivo: {}\nTamanho: {:.1} MB\nOrigem: {}\n\n{}\n\nO arquivo será apenas baixado. A execução exige nova confirmação. ZIP não é extraído automaticamente. Nomes sem arquitetura explícita exigem conferência do usuário.",asset.name,asset.size as f64/1_048_576.0,asset.browser_download_url,asset.digest.as_deref().unwrap_or("Sem hash publicado. O SHA-256 será calculado, sem verificação de autenticidade.")),"Baixar arquivo",false,Confirmation::Queue(vec![r]));
                } else {
                    process::open_url(&release.html_url)?;
                }
            }
        } else if page == 7 {
            let id = Uuid::parse_str(key)?;
            let op = self
                .data
                .lock()
                .unwrap()
                .history
                .iter()
                .find(|o| o.id == id)
                .cloned()
                .context("Operação não encontrada")?;
            match index {
                2 => self.info("Detalhes da operação", &custom::operation_details(&op)),
                0 => {
                    if matches!(op.state, OperationState::Failed | OperationState::Cancelled) {
                        self.confirm_queue(
                            vec![op.request],
                            "Tentar novamente. Verifique o estado do aplicativo antes de repetir.",
                        )?;
                    } else if op.state == OperationState::Pending
                        || (op.state == OperationState::Running
                            && op.request.action == Action::Download)
                    {
                        self.operations.cancel(id)?;
                    } else if let Some(file) = op.file {
                        process::open_path(
                            PathBuf::from(file).parent().context("Diretório inválido")?,
                        )?;
                    }
                }
                1 => {
                    self.ask_execute_download(id)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn confirm(&self, confirmed: bool, terms: bool) -> Result<()> {
        let dialog = self.view.lock().unwrap().dialog.take();
        (self.notify)();
        if !confirmed {
            return Ok(());
        }
        let Some(dialog) = dialog else { return Ok(()) };
        if dialog.terms && !terms {
            bail!("É necessário aceitar os termos para executar esta operação");
        }
        match dialog.confirmation {
            Some(Confirmation::Queue(mut requests)) => {
                for r in &mut requests {
                    r.accept_agreements = terms || self.agreements();
                }
                let manual = requests
                    .iter()
                    .filter_map(|r| {
                        r.custom_app
                            .as_ref()
                            .map(|a| ManualPending::for_app(a, r.profile.clone(), r.batch.clone()))
                    })
                    .collect();
                let n = self
                    .operations
                    .enqueue_prepared(requests, manual, None, None)?;
                {
                    let mut v = self.view.lock().unwrap();
                    v.page = 7;
                    v.selected.clear();
                }
                self.status(
                    format!("{n} operações adicionadas à fila. Acompanhe o relatório nesta tela."),
                    false,
                );
            }
            Some(Confirmation::DeleteProfile(i)) => {
                {
                    let mut d = self.data.lock().unwrap();
                    if i < d.profiles.len() {
                        d.profiles.remove(i);
                    }
                }
                self.save()?;
                if self.data.lock().unwrap().profiles.is_empty() {
                    let mut v = self.view.lock().unwrap();
                    v.selected_profile = None;
                    v.editor = String::new();
                    v.editor_revision += 1;
                } else {
                    self.select_profile(0)?;
                }
                self.status("Perfil excluído", false);
            }
            Some(Confirmation::ExecuteFile(id)) => {
                let op = self
                    .data
                    .lock()
                    .unwrap()
                    .history
                    .iter()
                    .find(|o| o.id == id)
                    .cloned()
                    .context("Operação não encontrada")?;
                let path = PathBuf::from(op.file.context("Arquivo indisponível")?);
                let ext = path
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !matches!(ext.as_str(), "exe" | "msi" | "msix") {
                    bail!("Abra este arquivo manualmente pelo Explorador");
                }
                let expected = op
                    .sha256
                    .context("Hash ausente. Baixe o arquivo novamente.")?;
                self.working(true);
                let result = async {
                    use sha2::{Digest, Sha256};
                    use tokio::io::AsyncReadExt;
                    let mut file = tokio::fs::File::open(&path).await?;
                    let mut hash = Sha256::new();
                    let mut buf = [0; 65536];
                    loop {
                        let n = file.read(&mut buf).await?;
                        if n == 0 {
                            break;
                        }
                        hash.update(&buf[..n]);
                    }
                    if hex::encode(hash.finalize()) != expected {
                        bail!("O arquivo mudou após o download. Execução bloqueada.");
                    }
                    process::open_path(&path)
                }
                .await;
                self.working(false);
                result?;
                self.status("Arquivo enviado ao Windows para execução", false);
            }
            Some(Confirmation::Tool(id)) => {
                self.operations.enqueue(vec![self.request(
                    PackageRef {
                        provider: ProviderId::Windows,
                        id,
                    },
                    Action::Launch,
                )])?;
                self.status("Abertura da ferramenta oficial adicionada à fila", false);
            }
            Some(Confirmation::Prepared(mut prepared)) => {
                for r in &mut prepared.requests {
                    r.accept_agreements = terms || self.agreements();
                }
                let manual_count = prepared.manual.len();
                let profile = if prepared.save_last_profile {
                    Some(prepared.name)
                } else {
                    None
                };
                let n = self.operations.enqueue_prepared(
                    prepared.requests,
                    prepared.manual,
                    Some(prepared.batch),
                    profile,
                )?;
                {
                    let mut v = self.view.lock().unwrap();
                    v.page = 7;
                    v.selected.clear();
                }
                self.status(format!("{n} operações enfileiradas; {manual_count} pendências de instalação/acompanhamento. Consulte Pendências ao final."),false);
            }
            Some(Confirmation::DeleteCustom(ids)) => self.delete_custom_confirmed(ids)?,
            Some(Confirmation::DeletePending(ids)) => self.delete_pending_confirmed(ids)?,
            Some(Confirmation::ImportCatalog(doc)) => self.import_catalog_confirmed(doc)?,
            None => {}
        }
        Ok(())
    }
}

fn package_source(v: &View, d: &SavedData) -> Vec<Package> {
    match v.page {
        1 if v.filter == "favorites" => d
            .favorites
            .iter()
            .filter(|p| p.provider == ProviderId::Winget)
            .map(|reference| {
                v.search
                    .iter()
                    .chain(v.installed.as_deref().unwrap_or(&[]))
                    .find(|p| p.reference.key() == reference.key())
                    .cloned()
                    .unwrap_or_else(|| Package {
                        reference: reference.clone(),
                        name: reference.id.clone(),
                        version: "não consultada".into(),
                        available: String::new(),
                        source: "winget · favorito salvo".into(),
                        manageable: true,
                    })
            })
            .collect(),
        1 => v.search.clone(),
        8 => d
            .custom_apps
            .iter()
            .map(|a| Package {
                reference: a.reference(),
                name: a.name.clone(),
                version: "instalação não verificada".into(),
                available: String::new(),
                source: a.source_label().into(),
                manageable: true,
            })
            .collect(),
        2 => v.installed.clone().unwrap_or_default(),
        3 => v.updates.clone().unwrap_or_default(),
        _ => vec![],
    }
}

fn merge_installed(mut managed: Vec<Package>, registry: Vec<Package>) -> Vec<Package> {
    let names: HashSet<_> = managed.iter().map(|p| p.name.to_lowercase()).collect();
    managed.extend(
        registry
            .into_iter()
            .filter(|p| !names.contains(&p.name.to_lowercase())),
    );
    managed.sort_by_key(|p| p.name.to_lowercase());
    managed
}
