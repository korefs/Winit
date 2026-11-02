use super::*;
use crate::services::downloads::{self, DownloadSpec};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    pub assets: Vec<Asset>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub published_at: Option<String>,
}
#[derive(Clone)]
pub struct GitHubReleaseProvider {
    client: reqwest::Client,
}
pub fn asset_safe_name(name: &str) -> bool {
    safe_filename(name)
}
pub fn compatible(asset: &Asset) -> bool {
    compatible_for(asset, Architecture::X64)
}
pub fn compatible_for(asset: &Asset, architecture: Architecture) -> bool {
    let n = asset.name.to_lowercase();
    let source_archive = n.ends_with(".zip")
        && n.split(['-', '_', '.', ' '])
            .any(|part| matches!(part, "source" | "sources" | "src" | "sourcecode"));
    if source_archive
        || !safe_filename(&asset.name)
        || !download_extension(&asset.name)
        || [
            "linux",
            "darwin",
            "macos",
            ".blockmap",
            "sources",
            "source-code",
            "source_code",
            "-symbols",
            "-src.",
        ]
        .iter()
        .any(|s| n.contains(s))
        || n == "source.zip"
    {
        return false;
    }
    let arm = n.contains("arm64") || n.contains("aarch64");
    let x64 =
        n.contains("x64") || n.contains("amd64") || n.contains("win64") || n.contains("x86_64");
    let x86 = (n.contains("x86") && !n.contains("x86_64"))
        || n.contains("i686")
        || n.contains("win32")
        || n.contains("32bit")
        || n.contains("ia32");
    match architecture {
        Architecture::X64 => !arm && !x86,
        Architecture::Arm64 => arm,
        Architecture::X86 => x86 && !arm && !x64,
    }
}
pub fn pattern_matches(pattern: &str, value: &str) -> bool {
    let p: Vec<_> = pattern.to_lowercase().chars().collect();
    let v: Vec<_> = value.to_lowercase().chars().collect();
    let mut previous = vec![false; v.len() + 1];
    previous[0] = true;
    for token in p {
        let mut next = vec![false; v.len() + 1];
        if token == '*' {
            next[0] = previous[0];
        }
        for i in 1..=v.len() {
            next[i] = if token == '*' {
                previous[i] || next[i - 1]
            } else {
                previous[i - 1] && (token == '?' || token == v[i - 1])
            };
        }
        previous = next;
    }
    previous[v.len()]
}
pub fn candidates<'a>(
    release: &'a Release,
    architecture: Architecture,
    pattern: &str,
) -> Vec<&'a Asset> {
    release
        .assets
        .iter()
        .filter(|a| {
            compatible_for(a, architecture)
                && (pattern.trim().is_empty() || pattern_matches(pattern.trim(), &a.name))
        })
        .collect()
}
pub fn select_configured_asset<'a>(
    release: &'a Release,
    architecture: Architecture,
    pattern: &str,
) -> Result<&'a Asset> {
    let assets = candidates(release, architecture, pattern);
    match assets.as_slice() {
        [asset] => Ok(asset),
        [] => bail!(
            "A release {} não contém instalador/arquivo Windows compatível com a arquitetura e o padrão configurados. Escolha outro padrão ou registre uma pendência manual.",
            release.tag_name
        ),
        _ => bail!(
            "Há {} assets compatíveis. Consulte a release no cadastro e selecione o arquivo desejado ou refine o padrão.",
            assets.len()
        ),
    }
}
pub fn asset_pattern(release: &Release, name: &str) -> Result<String> {
    if !release.assets.iter().any(|a| a.name == name) {
        bail!("Asset não pertence à release consultada");
    }
    let version = release.tag_name.trim_start_matches(['v', 'V']);
    let numeric_suffix = release
        .tag_name
        .char_indices()
        .find(|(_, c)| c.is_ascii_digit())
        .map(|(i, _)| &release.tag_name[i..]);
    let pattern = if name.contains(&release.tag_name) {
        name.replace(&release.tag_name, "*")
    } else if !version.is_empty() && name.contains(version) {
        name.replace(version, "*")
    } else if let Some(version) = numeric_suffix.filter(|version| name.contains(version)) {
        name.replace(version, "*")
    } else {
        name.to_owned()
    };
    Ok(pattern)
}
pub fn select_asset(assets: &[Asset]) -> Option<&Asset> {
    assets.iter().filter(|a| compatible(a)).max_by_key(|a| {
        let n = a.name.to_lowercase();
        (if n.contains("x64") || n.contains("amd64") {
            20
        } else {
            0
        }) + if n.ends_with(".exe") {
            5
        } else if n.ends_with(".msi") {
            4
        } else {
            0
        }
    })
}
pub fn validate_asset_url(repo: &str, asset: &Asset) -> Result<()> {
    validate_repo(repo)?;
    if !safe_filename(&asset.name) {
        bail!("Nome de arquivo inseguro");
    }
    let u = validate_https(&asset.browser_download_url)?;
    if u.host_str() != Some("github.com")
        || !u
            .path()
            .to_lowercase()
            .starts_with(&format!("/{}/releases/download/", repo.to_lowercase()))
    {
        bail!("Asset fora da origem oficial do repositório");
    }
    Ok(())
}
pub fn select_release(releases: Vec<Release>, allow_prerelease: bool) -> Result<Release> {
    releases
        .into_iter()
        .filter(|r| !r.draft && (allow_prerelease || !r.prerelease))
        .max_by(|a, b| a.published_at.cmp(&b.published_at))
        .context("O repositório não contém releases públicas compatíveis com o canal escolhido")
}
impl GitHubReleaseProvider {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .https_only(true)
            .user_agent(concat!("Winit/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(60))
            .redirect(reqwest::redirect::Policy::custom(|a| {
                if a.previous().len() > 6 {
                    return a.error("Redirecionamentos excessivos");
                }
                let h = a.url().host_str().unwrap_or("");
                if validate_https(a.url().as_str()).is_ok()
                    && (h == "github.com" || h.ends_with(".githubusercontent.com"))
                {
                    a.follow()
                } else {
                    a.error("Redirecionamento fora do GitHub HTTPS")
                }
            }))
            .build()?;
        Ok(Self { client })
    }
    async fn response(&self, url: &str) -> Result<reqwest::Response> {
        let response = self
            .client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .timeout(Duration::from_secs(40))
            .send()
            .await?;
        if !response.status().is_success() {
            bail!(
                "GitHub HTTP {}. Verifique repositório, canal e disponibilidade da rede. HTTP 403/429 indica possível limite da API; aguarde antes de tentar novamente.",
                response.status()
            );
        }
        Ok(response)
    }
    pub async fn latest(&self, repo: &str) -> Result<Release> {
        self.resolve(repo, false).await
    }
    pub async fn resolve(&self, repo: &str, prerelease: bool) -> Result<Release> {
        validate_repo(repo)?;
        let release = if prerelease {
            let list: Vec<Release> = self
                .response(&format!(
                    "https://api.github.com/repos/{repo}/releases?per_page=100"
                ))
                .await?
                .json()
                .await?;
            select_release(list, true)?
        } else {
            self.response(&format!(
                "https://api.github.com/repos/{repo}/releases/latest"
            ))
            .await?
            .json()
            .await?
        };
        if release.draft || (!prerelease && release.prerelease) || release.assets.len() > 500 {
            bail!("Metadados de release inválidos ou incompatíveis com o canal");
        }
        Ok(release)
    }
    pub async fn download_asset(
        &self,
        repo: &str,
        release: &Release,
        asset: &Asset,
        architecture: Architecture,
        context: ExecuteContext,
    ) -> Result<ExecutionResult> {
        validate_asset_url(repo, asset)?;
        if !compatible_for(asset, architecture) {
            bail!("Asset incompatível com a arquitetura configurada");
        }
        let hash = asset
            .digest
            .as_deref()
            .map(|d| {
                d.strip_prefix("sha256:")
                    .context("Algoritmo de hash da release não suportado")
            })
            .transpose()?
            .map(String::from);
        downloads::download(
            &self.client,
            DownloadSpec {
                url: asset.browser_download_url.clone(),
                filename: asset.name.clone(),
                namespace: repo.replace('/', "_"),
                size: Some(asset.size).filter(|s| *s > 0),
                sha256: hash,
                version: Some(release.tag_name.clone()),
                release_url: Some(release.html_url.clone()),
                origin: format!("GitHub Releases · {repo}"),
            },
            context,
        )
        .await
    }
}
#[async_trait]
impl SoftwareProvider for GitHubReleaseProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Github
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            search: true,
            actions: vec![Action::Download],
            ..Default::default()
        }
    }
    async fn search(&self, query: &str, _: bool) -> Result<Vec<Package>> {
        let r = self.latest(query).await?;
        Ok(vec![Package {
            reference: PackageRef {
                provider: ProviderId::Github,
                id: query.into(),
            },
            name: query.into(),
            version: r.tag_name,
            available: String::new(),
            source: "GitHub Releases".into(),
            manageable: true,
        }])
    }
    async fn execute(
        &self,
        request: &OperationRequest,
        context: ExecuteContext,
    ) -> Result<ExecutionResult> {
        if request.action != Action::Download {
            bail!("GitHub oferece download, sem instalação automática");
        }
        let release = self.latest(&request.package.id).await?;
        let selected = if let Some(asset) = &request.asset {
            release
                .assets
                .iter()
                .find(|a| {
                    a.browser_download_url == asset.browser_download_url && a.name == asset.name
                })
                .context(
                    "A release mudou ou o asset não pertence à última release. Consulte novamente.",
                )?
        } else {
            select_asset(&release.assets)
                .context("A release não contém asset compatível com Windows x64")?
        };
        self.download_asset(
            &request.package.id,
            &release,
            selected,
            Architecture::X64,
            context,
        )
        .await
    }
}
