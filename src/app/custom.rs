use super::*;
use crate::{CustomForm, Entry, PendingForm};
use slint::{ModelRc, SharedString, VecModel};
use winit::{
    providers::github,
    services::{catalog, manual},
};

#[derive(Clone, Default)]
pub(super) struct CustomView {
    pub(super) form: Option<CustomForm>,
    revision: i32,
    pub(super) release: Option<(CustomForm, Release)>,
    release_info: String,
    pub(super) pending: Option<PendingForm>,
    pending_revision: i32,
}
pub(super) fn connect(ui: &AppWindow, controller: &Arc<Controller>) {
    let c = controller.clone();
    ui.on_custom_save(move |form| {
        let c = c.clone();
        tokio::spawn(async move {
            if let Err(e) = c.save_custom(form) {
                c.report(e);
            }
        });
    });
    let c = controller.clone();
    ui.on_custom_query(move |form| {
        let c = c.clone();
        tokio::spawn(async move {
            if let Err(e) = c.query_custom(form).await {
                c.report(e);
            }
        });
    });
    let c = controller.clone();
    ui.on_custom_asset_selected(move |form, name| {
        let c = c.clone();
        tokio::spawn(async move {
            if let Err(e) = c.select_custom_asset(form, name.as_str()) {
                c.report(e);
            }
        });
    });
    let c = controller.clone();
    ui.on_pending_save(move |form| {
        let c = c.clone();
        tokio::spawn(async move {
            if let Err(e) = c.save_pending(form) {
                c.report(e);
            }
        });
    });
}
fn architecture(index: i32) -> Result<Architecture> {
    match index {
        0 => Ok(Architecture::X64),
        1 => Ok(Architecture::Arm64),
        2 => Ok(Architecture::X86),
        _ => bail!("Arquitetura inválida"),
    }
}
fn form_for(app: &CustomApp) -> CustomForm {
    let mut form = CustomForm {
        id: app.id.to_string().into(),
        name: app.name.clone().into(),
        description: app.description.clone().into(),
        category: app.category.clone().into(),
        website: app.website.clone().into(),
        instructions: app.instructions.clone().into(),
        ..Default::default()
    };
    match &app.source {
        CustomSource::Github {
            repository,
            prerelease,
            architecture,
            asset_pattern,
        } => {
            form.source = 0;
            form.repository = repository.clone().into();
            form.prerelease = *prerelease;
            form.architecture = match architecture {
                Architecture::X64 => 0,
                Architecture::Arm64 => 1,
                Architecture::X86 => 2,
            };
            form.pattern = asset_pattern.clone().into();
        }
        CustomSource::DirectUrl {
            url,
            filename,
            sha256,
        } => {
            form.source = 1;
            form.url = url.clone().into();
            form.filename = filename.clone().into();
            form.sha256 = sha256.clone().unwrap_or_default().into();
        }
        CustomSource::Manual => form.source = 2,
    }
    form
}
pub(super) fn app_from_form(form: &CustomForm) -> Result<CustomApp> {
    let source = match form.source {
        0 => CustomSource::Github {
            repository: form.repository.trim().into(),
            prerelease: form.prerelease,
            architecture: architecture(form.architecture)?,
            asset_pattern: form.pattern.trim().into(),
        },
        1 => CustomSource::DirectUrl {
            url: form.url.trim().into(),
            filename: form.filename.trim().into(),
            sha256: if form.sha256.trim().is_empty() {
                None
            } else {
                Some(form.sha256.trim().to_lowercase())
            },
        },
        2 => CustomSource::Manual,
        _ => bail!("Origem inválida"),
    };
    let app = CustomApp {
        id: if form.id.is_empty() {
            Uuid::new_v4()
        } else {
            Uuid::parse_str(form.id.as_str())?
        },
        name: form.name.trim().into(),
        description: form.description.trim().into(),
        category: form.category.trim().into(),
        website: form.website.trim().into(),
        instructions: form.instructions.trim().into(),
        source,
    };
    app.validate()?;
    Ok(app)
}
impl Controller {
    fn update_data<T>(&self, change: impl FnOnce(&mut SavedData) -> Result<T>) -> Result<T> {
        let mut data = self.data.lock().unwrap();
        let mut copy = data.clone();
        let result = change(&mut copy)?;
        self.storage.save(&copy)?;
        *data = copy;
        Ok(result)
    }
    fn open_custom(&self, form: CustomForm) {
        self.custom_revision.fetch_add(1, Ordering::SeqCst);
        {
            let mut v = self.view.lock().unwrap();
            v.custom.form = Some(form);
            v.custom.revision += 1;
            v.custom.release = None;
            v.custom.release_info="Consulte o repositório para ver os assets atuais. A seleção será salva como padrão de nome, sem fixar a versão.".into();
            v.custom.pending = None;
        }
        (self.notify)();
    }
    fn save_custom(&self, form: CustomForm) -> Result<()> {
        let app = app_from_form(&form)?;
        self.update_data(|d| {
            if !form.id.is_empty() && !d.custom_apps.iter().any(|a| a.id == app.id) {
                bail!("Este cadastro não existe mais. Abra-o novamente ou crie um novo.");
            }
            catalog::save(d, app)
        })?;
        {
            let mut v = self.view.lock().unwrap();
            v.custom.form = None;
            v.custom.release = None;
            if v.page != 8 {
                v.query.clear();
            }
            v.page = 8;
            v.selected.clear();
        }
        self.status(
            "Aplicativo personalizado salvo; suas referências em perfis foram preservadas",
            false,
        );
        Ok(())
    }
    async fn query_custom(&self, form: CustomForm) -> Result<()> {
        if form.source != 0 {
            bail!("Selecione GitHub Releases como origem");
        }
        validate_repo(form.repository.trim())?;
        let arch = architecture(form.architecture)?;
        let revision = self.custom_revision.fetch_add(1, Ordering::SeqCst) + 1;
        self.working(true);
        let result = self
            .providers
            .github
            .resolve(form.repository.trim(), form.prerelease)
            .await;
        self.working(false);
        if self.custom_revision.load(Ordering::SeqCst) != revision
            || self.view.lock().unwrap().custom.form.is_none()
        {
            return Ok(());
        }
        let release = result?;
        let assets = github::candidates(&release, arch, "");
        let info = format!(
            "{} · {} assets na release, {} compatíveis. {}",
            release.tag_name,
            release.assets.len(),
            assets.len(),
            if assets.is_empty() {
                "Sem instalador compatível: ajuste o cadastro ou use origem Manual."
            } else {
                "Selecione abaixo para gerar um padrão dinâmico de nome."
            }
        );
        {
            let mut v = self.view.lock().unwrap();
            v.custom.release = Some((form, release));
            v.custom.release_info = info;
        }
        (self.notify)();
        Ok(())
    }
    fn select_custom_asset(&self, mut form: CustomForm, name: &str) -> Result<()> {
        let (queried, release) = self
            .view
            .lock()
            .unwrap()
            .custom
            .release
            .clone()
            .context("Consulte as releases primeiro")?;
        if form.repository.trim().to_lowercase() != queried.repository.trim().to_lowercase()
            || form.prerelease != queried.prerelease
            || form.architecture != queried.architecture
        {
            bail!("O repositório, canal ou arquitetura mudou. Consulte as releases novamente.");
        }
        let arch = architecture(form.architecture)?;
        if !github::candidates(&release, arch, "")
            .iter()
            .any(|a| a.name == name)
        {
            bail!("Asset incompatível");
        }
        let pattern = github::asset_pattern(&release, name)?;
        github::select_configured_asset(&release, arch, &pattern)?;
        form.pattern = pattern.into();
        {
            let mut v = self.view.lock().unwrap();
            v.custom.form = Some(form);
            v.custom.revision += 1;
        }
        (self.notify)();
        Ok(())
    }
    fn selected_custom_ids(&self) -> Result<Vec<Uuid>> {
        let selected = self.view.lock().unwrap().selected.clone();
        let ids: Vec<_> = selected
            .iter()
            .filter_map(|s| s.strip_prefix("custom:"))
            .map(Uuid::parse_str)
            .collect::<std::result::Result<_, _>>()?;
        if ids.is_empty() {
            bail!("Selecione ao menos um aplicativo personalizado");
        }
        Ok(ids)
    }
    fn selected_pending_ids(&self) -> Result<Vec<Uuid>> {
        let selected = self.view.lock().unwrap().selected.clone();
        let ids: Vec<_> = selected
            .iter()
            .filter_map(|s| s.strip_prefix("pending:"))
            .map(Uuid::parse_str)
            .collect::<std::result::Result<_, _>>()?;
        if ids.is_empty() {
            bail!("Selecione ao menos uma pendência");
        }
        Ok(ids)
    }
    pub(super) fn ask_prepared(&self, prepared: profiles::PreparedProfile) -> Result<()> {
        if prepared.requests.is_empty() && prepared.manual.is_empty() {
            self.info("Seu perfil já está pronto", &prepared.summary);
            return Ok(());
        }
        let terms = prepared
            .requests
            .iter()
            .any(|r| r.package.provider == ProviderId::Winget);
        self.ask(
            "Revise as operações",
            prepared.summary.clone(),
            "Confirmar plano",
            terms,
            Confirmation::Prepared(prepared),
        );
        Ok(())
    }
    async fn obtain_custom(&self, ids: Vec<Uuid>) -> Result<()> {
        let data = self.data.lock().unwrap().clone();
        let apps = ids
            .iter()
            .map(|id| catalog::get(&data, &id.to_string()).cloned())
            .collect::<Result<Vec<_>>>()?;
        let profile=Profile {schema_version:1,name:"Obter aplicativos personalizados".into(),description:"Os arquivos serão baixados e acompanhados em Pendências; nenhuma instalação automática.".into(),packages:apps.iter().map(CustomApp::reference).collect(),custom_apps:apps};
        self.working(true);
        let result = profiles::prepare(&profile, &data, &[], false, &self.providers).await;
        self.working(false);
        let mut prepared = result?;
        prepared.save_last_profile = false;
        for r in &mut prepared.requests {
            r.profile = None;
        }
        for p in &mut prepared.manual {
            p.profile = None;
        }
        self.ask_prepared(prepared)
    }
    pub(super) async fn custom_action(&self, name: &str, payload: &str) -> Result<()> {
        match name {
            "custom-new" => self.open_custom(CustomForm {
                filename: "instalador.exe".into(),
                ..Default::default()
            }),
            "custom-close" => {
                self.custom_revision.fetch_add(1, Ordering::SeqCst);
                self.view.lock().unwrap().custom.form = None;
                (self.notify)();
            }
            "custom-example" => {
                let id = self.update_data(catalog::add_example)?;
                let app = catalog::get(&self.data.lock().unwrap(), &id.to_string())?.clone();
                self.view.lock().unwrap().page = 8;
                self.open_custom(form_for(&app));
                self.status(
                    "Sugestão DuoDesk disponível. Cadastros existentes foram preservados.",
                    false,
                );
            }
            "custom-open-website" => {
                validate_https(payload)?;
                process::open_url(payload)?;
            }
            "custom-delete" => {
                let ids = self.selected_custom_ids()?;
                let data = self.data.lock().unwrap();
                let names = data
                    .custom_apps
                    .iter()
                    .filter(|a| ids.contains(&a.id))
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join("\n• ");
                let profiles = data
                    .profiles
                    .iter()
                    .filter(|p| {
                        p.packages.iter().any(|r| {
                            r.provider == ProviderId::Custom
                                && ids
                                    .iter()
                                    .any(|id| id.to_string().eq_ignore_ascii_case(&r.id))
                        })
                    })
                    .count();
                drop(data);
                self.ask("Excluir aplicativos do catálogo?",format!("• {names}\n\nSerão removidos do catálogo, dos favoritos e de {profiles} perfis associados. Pendências e histórico serão preservados. Nenhum aplicativo instalado ou arquivo baixado será removido."),"Excluir cadastros",false,Confirmation::DeleteCustom(ids));
            }
            "custom-obtain-selected" => self.obtain_custom(self.selected_custom_ids()?).await?,
            "custom-import" => {
                if let Some(file) = rfd::AsyncFileDialog::new()
                    .add_filter("Catálogo JSON", &["json"])
                    .pick_file()
                    .await
                {
                    let document = catalog::import(file.path())?;
                    let mut preview = self.data.lock().unwrap().clone();
                    catalog::merge(&mut preview, document.clone())?;
                    self.ask("Importar catálogo personalizado?",format!("{} aplicativos no arquivo. Cadastros com o mesmo ID serão atualizados; novos IDs serão adicionados. Nomes ou origens duplicados com IDs diferentes são rejeitados.\n\n{}",document.apps.len(),document.apps.iter().map(|a|format!("• {} — {}",a.name,a.source_label())).collect::<Vec<_>>().join("\n")),"Importar catálogo",false,Confirmation::ImportCatalog(document));
                }
            }
            "custom-export" => {
                if let Some(file) = rfd::AsyncFileDialog::new()
                    .add_filter("Catálogo JSON", &["json"])
                    .set_file_name("meus-aplicativos.json")
                    .save_file()
                    .await
                {
                    let apps = self.data.lock().unwrap().custom_apps.clone();
                    catalog::export(file.path(), &apps)?;
                    self.status(
                        "Catálogo exportado com seus identificadores estáveis",
                        false,
                    );
                }
            }
            "pending-new" => {
                let mut v = self.view.lock().unwrap();
                v.custom.pending = Some(PendingForm::default());
                v.custom.pending_revision += 1;
                drop(v);
                (self.notify)();
            }
            "pending-close" => {
                self.view.lock().unwrap().custom.pending = None;
                (self.notify)();
            }
            "pending-delete" => {
                let ids = self.selected_pending_ids()?;
                self.ask("Remover pendências?",format!("{} pendências serão removidas. Isso não desinstala aplicativos nem exclui os arquivos baixados ou registros de operações.",ids.len()),"Remover pendências",false,Confirmation::DeletePending(ids));
            }
            "pending-open-file" | "pending-open-folder" => {
                let ids = self.selected_pending_ids()?;
                if ids.len() != 1 {
                    bail!("Selecione apenas uma pendência");
                }
                let data = self.data.lock().unwrap();
                let pending = data
                    .manual_pending
                    .iter()
                    .find(|p| p.id == ids[0])
                    .context("Pendência não encontrada")?;
                let op=pending_download(&data,pending).cloned().context("Ainda não há um download concluído para esta pendência. Use o website ou repita o download pelo catálogo.")?;
                drop(data);
                if name == "pending-open-file" {
                    self.ask_execute_download(op.id)?;
                } else {
                    process::open_path(
                        PathBuf::from(op.file.context("Arquivo indisponível")?)
                            .parent()
                            .context("Diretório inválido")?,
                    )?;
                }
            }
            _ => bail!("Ação personalizada desconhecida: {name}"),
        }
        Ok(())
    }
    pub(super) async fn custom_entry_action(&self, key: &str, index: i32) -> Result<()> {
        if let Some(id) = key.strip_prefix("custom:") {
            let app = catalog::get(&self.data.lock().unwrap(), id)?.clone();
            match index {
                0 => self.obtain_custom(vec![app.id]).await?,
                1 => {
                    self.update_data(|d| {
                        let reference = app.reference();
                        if let Some(i) = d.favorites.iter().position(|r| r.key() == reference.key())
                        {
                            d.favorites.remove(i);
                        } else {
                            d.favorites.push(reference);
                        }
                        Ok(())
                    })?;
                    (self.notify)();
                }
                2 => self.open_custom(form_for(&app)),
                _ => {}
            }
        } else if let Some(id) = key.strip_prefix("pending:") {
            let id = Uuid::parse_str(id)?;
            let pending = self
                .data
                .lock()
                .unwrap()
                .manual_pending
                .iter()
                .find(|p| p.id == id)
                .cloned()
                .context("Pendência não encontrada")?;
            match index {
                0 => process::open_url(&pending.website)?,
                1 => {
                    self.update_data(|d| {
                        manual::set_completed(d, id, pending.status != ManualStatus::Completed)
                    })?;
                    self.status(
                        "Status manual salvo. Isso não comprova instalação no Windows.",
                        false,
                    );
                }
                2 => {
                    let form = PendingForm {
                        id: id.to_string().into(),
                        name: pending.name.into(),
                        website: pending.website.into(),
                        instructions: pending.instructions.into(),
                        profile: pending.profile.unwrap_or_default().into(),
                    };
                    let mut v = self.view.lock().unwrap();
                    v.custom.pending = Some(form);
                    v.custom.pending_revision += 1;
                    drop(v);
                    (self.notify)();
                }
                _ => {}
            }
        } else {
            bail!("Item personalizado não encontrado");
        }
        Ok(())
    }
    fn save_pending(&self, form: PendingForm) -> Result<()> {
        self.update_data(|d|{
            let mut pending=if form.id.is_empty() {ManualPending {id:Uuid::new_v4(),app_id:None,name:String::new(),website:String::new(),instructions:String::new(),profile:None,batch:None,status:ManualStatus::Pending,created_at:chrono::Utc::now(),updated_at:chrono::Utc::now()}}else{let id=Uuid::parse_str(form.id.as_str())?;d.manual_pending.iter().find(|p|p.id==id).cloned().context("Pendência não encontrada")?};
            let profile=if form.profile.trim().is_empty(){None}else{Some(form.profile.trim().to_owned())};
            if profile!=pending.profile && profile.as_ref().is_some_and(|name|!d.profiles.iter().any(|p|&p.name==name)){bail!("O perfil associado não existe. Use o nome exato de um perfil salvo ou deixe vazio.");}
            pending.name=form.name.trim().into();pending.website=form.website.trim().into();pending.instructions=form.instructions.trim().into();pending.profile=profile;manual::save(d,pending)?;Ok(())
        })?;
        {
            let mut v = self.view.lock().unwrap();
            v.custom.pending = None;
            v.page = 9;
        }
        self.status("Pendência manual salva", false);
        Ok(())
    }
    pub(super) fn delete_custom_confirmed(&self, ids: Vec<Uuid>) -> Result<()> {
        self.update_data(|d| catalog::remove(d, &ids.into_iter().collect()))?;
        {
            let mut v = self.view.lock().unwrap();
            v.selected.clear();
            v.editor_revision += 1;
        }
        let selected = self.view.lock().unwrap().selected_profile;
        if let Some(i) = selected {
            self.select_profile(i)?;
        }
        self.status(
            "Cadastros e referências removidos; pendências históricas preservadas",
            false,
        );
        Ok(())
    }
    pub(super) fn delete_pending_confirmed(&self, ids: Vec<Uuid>) -> Result<()> {
        self.update_data(|d| {
            d.manual_pending.retain(|p| !ids.contains(&p.id));
            Ok(())
        })?;
        self.view.lock().unwrap().selected.clear();
        self.status("Pendências removidas", false);
        Ok(())
    }
    pub(super) fn import_catalog_confirmed(&self, doc: catalog::CatalogDocument) -> Result<()> {
        self.update_data(|d| catalog::merge(d, doc))?;
        self.status("Catálogo importado e validado", false);
        Ok(())
    }
    pub(super) fn import_profile_data(&self, mut profile: Profile) -> Result<usize> {
        self.update_data(|d| {
            catalog::import_profile_definitions(d, &mut profile)?;
            profiles::save(&mut d.profiles, profile, None)
        })
    }
    pub(super) fn ask_execute_download(&self, id: Uuid) -> Result<()> {
        let op = self
            .data
            .lock()
            .unwrap()
            .history
            .iter()
            .find(|o| o.id == id)
            .cloned()
            .context("Operação não encontrada")?;
        if op.state != OperationState::Completed {
            bail!("O download não foi concluído");
        }
        let file = op.file.context("Arquivo indisponível")?;
        let extension = PathBuf::from(&file)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(extension.as_str(), "exe" | "msi" | "msix") {
            bail!(
                "Este arquivo é um arquivo compactado, não um instalador executável. Use Abrir pasta e revise seu conteúdo manualmente."
            );
        }
        let provenance = op
            .download_metadata
            .as_ref()
            .map(|m| {
                format!(
                    "Origem: {}\nArquivo: {}\nVersão: {}\nIntegridade: {}\nURL: {}",
                    m.origin,
                    m.asset_name,
                    m.version
                        .as_deref()
                        .unwrap_or("Atualização não verificável"),
                    if m.sha256_verified {
                        "SHA-256 comparado com o hash publicado/configurado"
                    } else {
                        "hash calculado sem referência para comparação"
                    },
                    m.source_url
                )
            })
            .unwrap_or_else(|| format!("Origem: {}", op.request.package.id));
        self.ask("Executar arquivo baixado?",format!("{file}\n\n{provenance}\nSHA-256 registrado: {}\n\nA procedência foi informada pelo usuário; integridade por hash não comprova a identidade do fornecedor. Assinatura Authenticode não verificada. O hash será recalculado para detectar alterações. SmartScreen e UAC permanecem ativos. O programa externo pode modificar ou remover dados. Executar o arquivo não comprova que a instalação foi concluída.",op.sha256.as_deref().unwrap_or("indisponível")),"Executar arquivo",false,Confirmation::ExecuteFile(id));
        Ok(())
    }
}

fn pending_download<'a>(data: &'a SavedData, pending: &ManualPending) -> Option<&'a Operation> {
    data.history.iter().rev().find(|op| {
        op.state == OperationState::Completed
            && op.file.is_some()
            && op.request.profile == pending.profile
            && op.request.batch == pending.batch
            && (op
                .request
                .custom_app
                .as_ref()
                .is_some_and(|a| Some(a.id) == pending.app_id)
                || (pending.app_id.is_none()
                    && op.request.package.provider == ProviderId::Github
                    && pending.website == format!("https://github.com/{}", op.request.package.id)))
    })
}

pub(super) fn operation_details(op: &Operation) -> String {
    let metadata = op
        .download_metadata
        .as_ref()
        .and_then(|m| serde_json::to_string_pretty(m).ok())
        .unwrap_or_else(|| "Sem metadados adicionais nesta operação".into());
    format!(
        "{} {}\nEstado: {}\nCriado: {}\nFinalizado: {:?}\nPerfil: {}\nLote: {}\nCódigo: {:?}\n\n{}\n\nArquivo: {}\nSHA-256: {}\n\nMetadados do download (não comprovam instalação)\n{}\n\nSTDOUT\n{}\n\nSTDERR\n{}",
        op.request.action.label(),
        op.request
            .custom_app
            .as_ref()
            .map(|a| a.name.as_str())
            .unwrap_or(&op.request.package.id),
        op.state.label(),
        op.created_at,
        op.finished_at,
        op.request.profile.as_deref().unwrap_or("Sem perfil"),
        op.request.batch.as_deref().unwrap_or("individual"),
        op.exit_code,
        op.message,
        op.file.as_deref().unwrap_or("—"),
        op.sha256.as_deref().unwrap_or("—"),
        metadata,
        op.stdout,
        op.stderr
    )
}

pub(super) fn render(ui: &AppWindow, data: &SavedData, view: &View, entries: &mut Vec<Entry>) {
    ui.set_custom_count(data.custom_apps.len().to_string().into());
    ui.set_pending_count(
        data.manual_pending
            .iter()
            .filter(|p| p.status == ManualStatus::Pending)
            .count()
            .to_string()
            .into(),
    );
    ui.set_custom_editor_open(view.custom.form.is_some());
    if let Some(form) = &view.custom.form
        && ui.get_custom_revision() != view.custom.revision
    {
        ui.set_custom_form(form.clone());
        ui.set_custom_revision(view.custom.revision);
        ui.set_custom_asset_index(-1);
    }
    ui.set_custom_release_info(view.custom.release_info.clone().into());
    let assets = view
        .custom
        .release
        .as_ref()
        .map(|(f, r)| {
            github::candidates(r, architecture(f.architecture).unwrap_or_default(), "")
                .iter()
                .map(|a| SharedString::from(&a.name))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    ui.set_custom_assets(ModelRc::new(VecModel::from(assets)));
    ui.set_pending_editor_open(view.custom.pending.is_some());
    if let Some(form) = &view.custom.pending
        && ui.get_pending_revision() != view.custom.pending_revision
    {
        ui.set_pending_form(form.clone());
        ui.set_pending_revision(view.custom.pending_revision);
    }
    let query = view.query.to_lowercase();
    if view.page == 8 {
        for app in &data.custom_apps {
            let reference = app.reference();
            let favorite = data.favorites.iter().any(|r| r.key() == reference.key());
            if view.filter == "favorites" && !favorite {
                continue;
            }
            if !format!(
                "{} {} {} {}",
                app.name,
                app.description,
                app.category,
                app.source_label()
            )
            .to_lowercase()
            .contains(&query)
            {
                continue;
            }
            let latest = data.history.iter().rev().find(|o| {
                o.request
                    .custom_app
                    .as_ref()
                    .is_some_and(|a| a.id == app.id)
                    && o.state == OperationState::Completed
            });
            let version = match &app.source {
                CustomSource::DirectUrl { .. } => {
                    "Atualização não verificável · instalação não verificada".into()
                }
                CustomSource::Manual => "Acompanhamento manual · sem detecção de instalação".into(),
                CustomSource::Github { repository, .. } => format!(
                    "{repository} · {}",
                    latest
                        .and_then(|o| o.download_metadata.as_ref())
                        .and_then(|m| m.version.as_deref())
                        .map(|v| format!("último download: {v}; instalação não verificada"))
                        .unwrap_or_else(|| "versão resolvida ao consultar/baixar".into())
                ),
            };
            entries.push(Entry {
                key: reference.key().into(),
                title: app.name.clone().into(),
                subtitle: format!(
                    "{}{}{}",
                    if app.category.is_empty() {
                        ""
                    } else {
                        &app.category
                    },
                    if app.category.is_empty() { "" } else { " · " },
                    if app.description.is_empty() {
                        &app.website
                    } else {
                        &app.description
                    }
                )
                .into(),
                version: version.into(),
                badge: app.source_label().into(),
                primary: if matches!(app.source, CustomSource::Manual) {
                    "Registrar pendência"
                } else {
                    "Baixar"
                }
                .into(),
                secondary: if favorite { "★" } else { "☆" }.into(),
                extra: "Editar".into(),
                selected: view.selected.contains(&reference.key()),
                favorite,
                can_select: true,
                ..Default::default()
            });
        }
    } else if view.page == 9 {
        for pending in data.manual_pending.iter().rev() {
            if view.filter == "pending" && pending.status != ManualStatus::Pending
                || view.filter == "completed" && pending.status != ManualStatus::Completed
            {
                continue;
            }
            if !format!(
                "{} {} {}",
                pending.name,
                pending.website,
                pending.profile.as_deref().unwrap_or("")
            )
            .to_lowercase()
            .contains(&query)
            {
                continue;
            }
            let key = format!("pending:{}", pending.id);
            let file = pending_download(data, pending).and_then(|o| o.file.as_ref());
            entries.push(Entry {
                key: key.clone().into(),
                title: pending.name.clone().into(),
                subtitle: format!(
                    "{} · {}",
                    pending.profile.as_deref().unwrap_or("Sem perfil"),
                    pending.website
                )
                .into(),
                version: if file.is_some() {
                    "Download disponível · selecione para abrir pasta ou instalador"
                } else if pending.instructions.is_empty() {
                    "Conclusão manual não comprova instalação"
                } else {
                    &pending.instructions
                }
                .into(),
                badge: if pending.status == ManualStatus::Completed {
                    "Concluída manualmente"
                } else {
                    "Pendente"
                }
                .into(),
                primary: "Abrir website".into(),
                secondary: if pending.status == ManualStatus::Completed {
                    "Reabrir"
                } else {
                    "Concluir"
                }
                .into(),
                extra: "Editar".into(),
                can_select: true,
                selected: view.selected.contains(&key),
                ..Default::default()
            });
        }
    }
    if matches!(view.page, 8 | 9) {
        ui.set_selection_label(format!("{} selecionados", view.selected.len()).into());
    }
}
