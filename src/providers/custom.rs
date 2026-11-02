use super::*;
use crate::services::downloads::{self, DownloadSpec};
use anyhow::{Context, Result, bail};

pub struct CustomAppProvider {
    github: github::GitHubReleaseProvider,
    client: reqwest::Client,
}
impl CustomAppProvider {
    pub fn new(github: github::GitHubReleaseProvider) -> Result<Self> {
        Ok(Self {
            github,
            client: downloads::direct_client()?,
        })
    }
}
#[async_trait]
impl SoftwareProvider for CustomAppProvider {
    fn id(&self) -> ProviderId {
        ProviderId::Custom
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            actions: vec![Action::Download],
            ..Default::default()
        }
    }
    async fn execute(
        &self,
        request: &OperationRequest,
        context: ExecuteContext,
    ) -> Result<ExecutionResult> {
        if request.action != Action::Download {
            bail!(
                "Aplicativos personalizados oferecem download ou instalação manual, sem execução automática"
            );
        }
        let app = request
            .custom_app
            .as_ref()
            .context("Configuração do aplicativo personalizado ausente da operação")?;
        app.validate()?;
        if app.reference().key() != request.package.key() {
            bail!("O identificador da operação não corresponde ao cadastro");
        }
        match &app.source {
            CustomSource::Github {
                repository,
                prerelease,
                architecture,
                asset_pattern,
            } => {
                let release = self.github.resolve(repository, *prerelease).await?;
                let asset =
                    github::select_configured_asset(&release, *architecture, asset_pattern)?;
                self.github
                    .download_asset(repository, &release, asset, *architecture, context)
                    .await
            }
            CustomSource::DirectUrl {
                url,
                filename,
                sha256,
            } => {
                downloads::download(
                    &self.client,
                    DownloadSpec {
                        url: url.clone(),
                        filename: filename.clone(),
                        namespace: app.id.to_string(),
                        size: None,
                        sha256: sha256.clone(),
                        version: None,
                        release_url: None,
                        origin: "URL direta configurada pelo usuário · atualização não verificável"
                            .into(),
                    },
                    context,
                )
                .await
            }
            CustomSource::Manual => bail!(
                "Aplicativo de instalação manual. Use a lista de Pendências para abrir o website e acompanhar a instalação."
            ),
        }
    }
}
