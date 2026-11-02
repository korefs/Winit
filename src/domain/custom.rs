use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Architecture {
    #[default]
    X64,
    Arm64,
    X86,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CustomSource {
    Github {
        repository: String,
        #[serde(default)]
        prerelease: bool,
        #[serde(default)]
        architecture: Architecture,
        #[serde(default)]
        asset_pattern: String,
    },
    DirectUrl {
        url: String,
        filename: String,
        #[serde(default)]
        sha256: Option<String>,
    },
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomApp {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub category: String,
    pub website: String,
    #[serde(default)]
    pub instructions: String,
    pub source: CustomSource,
}
impl CustomApp {
    pub fn reference(&self) -> PackageRef {
        PackageRef {
            provider: ProviderId::Custom,
            id: self.id.to_string(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.id.is_nil() {
            bail!("O identificador do aplicativo não pode ser vazio");
        }
        validate_name(&self.name)?;
        if self.description.len() > 4000
            || self.category.len() > 120
            || self.instructions.len() > 8000
        {
            bail!("Descrição, categoria ou instruções excedem o limite permitido");
        }
        validate_https(&self.website)?;
        match &self.source {
            CustomSource::Github {
                repository,
                asset_pattern,
                ..
            } => {
                validate_repo(repository)?;
                if asset_pattern.len() > 200
                    || asset_pattern
                        .chars()
                        .any(|c| c.is_control() || "/\\:<>|\"".contains(c))
                {
                    bail!(
                        "Padrão de asset inválido. Use apenas nome de arquivo, com * e ? opcionais."
                    );
                }
            }
            CustomSource::DirectUrl {
                url,
                filename,
                sha256,
            } => {
                validate_https(url)?;
                if !safe_filename(filename) || !download_extension(filename) {
                    bail!(
                        "Nome do instalador inválido. Use .exe, .msi, .msix ou .zip, sem caminhos."
                    );
                }
                if let Some(hash) = sha256 {
                    validate_sha256(hash)?;
                }
            }
            CustomSource::Manual => {}
        }
        Ok(())
    }
    pub fn source_label(&self) -> &'static str {
        match self.source {
            CustomSource::Github { .. } => "GitHub Releases",
            CustomSource::DirectUrl { .. } => "URL direta",
            CustomSource::Manual => "Manual",
        }
    }
    pub fn origin_key(&self) -> String {
        match &self.source {
            CustomSource::Github {
                repository,
                architecture,
                ..
            } => format!("github:{}:{architecture:?}", repository.to_lowercase()),
            CustomSource::DirectUrl { url, .. } => format!(
                "url:{}",
                reqwest::Url::parse(url)
                    .map(|u| u.to_string())
                    .unwrap_or_else(|_| url.clone())
            ),
            CustomSource::Manual => format!(
                "manual:{}:{}",
                normalized_name(&self.name),
                self.website.to_lowercase()
            ),
        }
    }
    pub fn duodesk_example() -> Self {
        Self {id:Uuid::from_u128(0x268e963e04734a759f13d00de5c00001),name:"DuoDesk".into(),description:"Compartilhe teclado, mouse e recursos entre dispositivos.".into(),category:"Produtividade".into(),website:"https://getduodesk.vercel.app/".into(),instructions:"Após baixar, revise a origem e execute o instalador apenas se confiar no fornecedor. Feche o DuoDesk antes de atualizar. A conclusão manual não comprova instalação no Windows.".into(),source:CustomSource::Github{repository:"0xCodez/DuoDesk".into(),prerelease:false,architecture:Architecture::X64,asset_pattern:"DuoDesk-Setup-*.exe".into()}}
    }
}

pub fn normalized_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
pub fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() || name.len() > 160 || name.chars().any(char::is_control) {
        bail!("Informe um nome de até 160 bytes, sem caracteres de controle");
    }
    Ok(())
}
pub fn validate_https(value: &str) -> Result<reqwest::Url> {
    if value.len() > 4096 || value.chars().any(char::is_control) {
        bail!("URL inválida ou maior que 4096 bytes");
    }
    let url = reqwest::Url::parse(value)
        .map_err(|_| anyhow::anyhow!("Informe uma URL HTTPS completa e válida"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        bail!("Use HTTPS, sem credenciais embutidas nem fragmentos na URL");
    }
    Ok(url)
}
pub fn validate_sha256(hash: &str) -> Result<()> {
    if hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
        bail!("SHA-256 deve conter exatamente 64 dígitos hexadecimais");
    }
    Ok(())
}
pub fn download_extension(name: &str) -> bool {
    let n = name.to_lowercase();
    [".exe", ".msi", ".msix", ".zip"]
        .iter()
        .any(|e| n.ends_with(e))
}
pub fn safe_filename(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or("").to_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&base.as_str())
        || ((base.starts_with("COM") || base.starts_with("LPT"))
            && base.len() == 4
            && matches!(base.as_bytes()[3], b'1'..=b'9'));
    !name.is_empty()
        && name.len() < 200
        && name != "."
        && name != ".."
        && !reserved
        && !name.ends_with(['.', ' '])
        && !name
            .chars()
            .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManualStatus {
    Pending,
    Completed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualPending {
    pub id: Uuid,
    #[serde(default)]
    pub app_id: Option<Uuid>,
    pub name: String,
    pub website: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub batch: Option<String>,
    pub status: ManualStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
impl ManualPending {
    pub fn for_app(app: &CustomApp, profile: Option<String>, batch: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            app_id: Some(app.id),
            name: app.name.clone(),
            website: app.website.clone(),
            instructions: app.instructions.clone(),
            profile,
            batch,
            status: ManualStatus::Pending,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        if self.id.is_nil() {
            bail!("ID de pendência inválido");
        }
        validate_name(&self.name)?;
        validate_https(&self.website)?;
        if self.instructions.len() > 12000 || self.profile.as_ref().is_some_and(|p| p.len() > 120) {
            bail!("Pendência excede os limites permitidos");
        }
        Ok(())
    }
    pub fn set_completed(&mut self, completed: bool) {
        self.status = if completed {
            ManualStatus::Completed
        } else {
            ManualStatus::Pending
        };
        self.updated_at = Utc::now();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadMetadata {
    pub source_url: String,
    pub final_url: String,
    pub asset_name: String,
    pub version: Option<String>,
    pub release_url: Option<String>,
    pub sha256_verified: bool,
    pub origin: String,
    #[serde(default)]
    pub reused_existing: bool,
}
