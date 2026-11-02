pub mod custom;
pub mod github;
pub mod windows;
pub mod winget;

use crate::domain::*;
use anyhow::{Result, bail};
use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

pub type ProgressSink = Arc<dyn Fn(Progress) + Send + Sync>;
#[derive(Clone)]
pub struct ExecuteContext {
    pub silent: bool,
    pub accept_agreements: bool,
    pub downloads: PathBuf,
    pub cancel: Arc<AtomicBool>,
    pub progress: ProgressSink,
}

#[async_trait]
pub trait SoftwareProvider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn capabilities(&self) -> Capabilities;
    async fn search(&self, _query: &str, _agreements: bool) -> Result<Vec<Package>> {
        bail!("Pesquisa não suportada")
    }
    async fn installed(&self, _agreements: bool) -> Result<Vec<Package>> {
        bail!("Listagem não suportada")
    }
    async fn updates(&self, _agreements: bool) -> Result<Vec<Package>> {
        bail!("Atualizações não suportadas")
    }
    async fn execute(
        &self,
        request: &OperationRequest,
        context: ExecuteContext,
    ) -> Result<ExecutionResult>;
}

pub struct Providers {
    pub winget: winget::WingetProvider,
    pub github: github::GitHubReleaseProvider,
    pub windows: windows::WindowsToolsProvider,
    pub custom: custom::CustomAppProvider,
}
impl Providers {
    pub fn new() -> Result<Self> {
        let github = github::GitHubReleaseProvider::new()?;
        let custom = custom::CustomAppProvider::new(github.clone())?;
        Ok(Self {
            winget: winget::WingetProvider,
            github,
            custom,
            windows: windows::WindowsToolsProvider,
        })
    }
    pub fn get(&self, id: ProviderId) -> &dyn SoftwareProvider {
        match id {
            ProviderId::Winget => &self.winget,
            ProviderId::Github => &self.github,
            ProviderId::Windows => &self.windows,
            ProviderId::Custom => &self.custom,
        }
    }
}
