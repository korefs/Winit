use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;
pub mod custom;
pub use custom::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Winget,
    Github,
    Windows,
    Custom,
}

impl ProviderId {
    pub fn label(self) -> &'static str {
        match self {
            Self::Winget => "winget",
            Self::Github => "github",
            Self::Windows => "windows",
            Self::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Install,
    Update,
    Uninstall,
    Download,
    Launch,
}
impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::Install => "Instalar",
            Self::Update => "Atualizar",
            Self::Uninstall => "Desinstalar",
            Self::Download => "Baixar",
            Self::Launch => "Abrir",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    pub search: bool,
    pub installed: bool,
    pub updates: bool,
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(deny_unknown_fields)]
pub struct PackageRef {
    pub provider: ProviderId,
    pub id: String,
}
impl PackageRef {
    pub fn key(&self) -> String {
        format!("{}:{}", self.provider.label(), self.id.to_lowercase())
    }
    pub fn validate(&self) -> Result<()> {
        match self.provider {
            ProviderId::Winget => validate_winget_id(&self.id),
            ProviderId::Github => validate_repo(&self.id),
            ProviderId::Windows => bail!("Ferramentas do Windows não são pacotes de perfil"),
            ProviderId::Custom => {
                let id = Uuid::parse_str(&self.id)
                    .map_err(|_| anyhow::anyhow!("ID de aplicativo personalizado inválido"))?;
                if id.is_nil() {
                    bail!("ID personalizado vazio");
                }
                Ok(())
            }
        }
    }
}

pub fn validate_winget_id(id: &str) -> Result<()> {
    if id.len() > 256
        || id.is_empty()
        || id.starts_with('-')
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-_+".contains(c))
    {
        bail!("ID Winget inválido ou truncado: {id}");
    }
    Ok(())
}

pub fn validate_repo(repo: &str) -> Result<()> {
    let parts: Vec<_> = repo.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|p| {
            p.is_empty()
                || p.len() > 100
                || p.starts_with('.')
                || p.contains("..")
                || !p
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        })
    {
        bail!("Repositório inválido. Use owner/repository, sem URL.");
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub reference: PackageRef,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub available: String,
    #[serde(default)]
    pub source: String,
    #[serde(default = "yes")]
    pub manageable: bool,
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub schema_version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub packages: Vec<PackageRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_apps: Vec<CustomApp>,
}
impl Profile {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            bail!("Versão de perfil não suportada: {}", self.schema_version);
        }
        if self.name.trim().is_empty() || self.name.len() > 120 {
            bail!("O nome precisa ter entre 1 e 120 bytes");
        }
        if self.description.len() > 4000 || self.packages.len() > 500 {
            bail!("Perfil excede os limites (500 pacotes, descrição de 4000 bytes)");
        }
        let mut keys = HashSet::new();
        let mut app_ids = HashSet::new();
        if self.custom_apps.len() > 500 {
            bail!("Perfil excede 500 definições personalizadas");
        }
        for app in &self.custom_apps {
            app.validate()?;
            if !app_ids.insert(app.id) {
                bail!("Definição personalizada duplicada: {}", app.name);
            }
        }
        for p in &self.packages {
            p.validate()?;
            if !keys.insert(p.key()) {
                bail!("Pacote duplicado: {}", p.id);
            }
        }
        if serde_json::to_vec_pretty(self)?.len() > 1_048_576 {
            bail!("Perfil maior que 1 MB, incluindo definições personalizadas");
        }
        Ok(())
    }
    pub fn parse(json: &str) -> Result<Self> {
        if json.len() > 1_048_576 {
            bail!("Perfil maior que 1 MB");
        }
        let p: Self = serde_json::from_str(json.trim_start_matches('\u{feff}')).map_err(|e| {
            anyhow::anyhow!(
                "JSON de perfil inválido na linha {}, coluna {}: {}",
                e.line(),
                e.column(),
                e
            )
        })?;
        p.validate()?;
        Ok(p)
    }
    pub fn plan(&self, installed: &[Package], winget_available: bool) -> Result<ProfilePlan> {
        self.validate()?;
        let keys: HashSet<_> = installed.iter().map(|p| p.reference.key()).collect();
        let mut plan = ProfilePlan::default();
        for p in &self.packages {
            if p.provider == ProviderId::Winget && !winget_available {
                bail!("Winget indisponível. Verifique o diagnóstico em Configurações.");
            }
            if p.provider == ProviderId::Winget && keys.contains(&p.key()) {
                plan.skipped.push(p.clone());
            } else {
                plan.missing.push(p.clone());
            }
        }
        Ok(plan)
    }
}
#[derive(Debug, Default)]
pub struct ProfilePlan {
    pub missing: Vec<PackageRef>,
    pub skipped: Vec<PackageRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    #[serde(default)]
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationRequest {
    pub package: PackageRef,
    pub action: Action,
    #[serde(default)]
    pub asset: Option<Asset>,
    #[serde(default)]
    pub batch: Option<String>,
    #[serde(default)]
    pub accept_agreements: bool,
    #[serde(default)]
    pub silent: bool,
    #[serde(default)]
    pub custom_app: Option<CustomApp>,
    #[serde(default)]
    pub profile: Option<String>,
}
impl OperationRequest {
    pub fn new(package: PackageRef, action: Action) -> Self {
        Self {
            package,
            action,
            asset: None,
            batch: None,
            accept_agreements: false,
            silent: false,
            custom_app: None,
            profile: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}
impl OperationState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pendente",
            Self::Running => "Executando",
            Self::Completed => "Concluído",
            Self::Failed => "Falhou",
            Self::Cancelled => "Cancelado",
        }
    }
    pub fn transition(self, next: Self) -> Result<Self> {
        if matches!(
            (self, next),
            (Self::Pending, Self::Running | Self::Cancelled)
                | (
                    Self::Running,
                    Self::Completed | Self::Failed | Self::Cancelled
                )
        ) {
            Ok(next)
        } else {
            bail!("Transição inválida: {self:?} → {next:?}")
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: Uuid,
    pub request: OperationRequest,
    pub state: OperationState,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub message: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub file: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub download_metadata: Option<DownloadMetadata>,
}
impl Operation {
    pub fn new(request: OperationRequest) -> Self {
        Self {
            id: Uuid::new_v4(),
            request,
            state: OperationState::Pending,
            created_at: Utc::now(),
            finished_at: None,
            message: "Aguardando na fila".into(),
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
            downloaded: 0,
            total: None,
            file: None,
            sha256: None,
            download_metadata: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub message: String,
    pub downloaded: u64,
    pub total: Option<u64>,
}
#[derive(Debug, Clone, Default)]
pub struct ExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub file: Option<String>,
    pub sha256: Option<String>,
    pub message: String,
    pub download_metadata: Option<DownloadMetadata>,
}
