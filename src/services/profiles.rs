use crate::domain::Profile;
use crate::{
    domain::*,
    infrastructure::storage::SavedData,
    providers::{Providers, github},
};
use anyhow::{Result, bail};
use std::path::Path;

#[derive(Clone)]
pub struct PreparedProfile {
    pub name: String,
    pub batch: String,
    pub requests: Vec<OperationRequest>,
    pub manual: Vec<ManualPending>,
    pub summary: String,
    pub save_last_profile: bool,
}
pub async fn prepare(
    profile: &Profile,
    data: &SavedData,
    installed: &[Package],
    winget_available: bool,
    providers: &Providers,
) -> Result<PreparedProfile> {
    let plan = profile.plan(installed, winget_available)?;
    let batch = format!("{} · {}", profile.name, uuid::Uuid::new_v4());
    let mut prepared = PreparedProfile {
        name: profile.name.clone(),
        batch: batch.clone(),
        requests: vec![],
        manual: vec![],
        save_last_profile: true,
        summary: format!(
            "Perfil: {}\n{}\n\n{} já instalados serão ignorados (evidência Winget):\n{}\n",
            profile.name,
            profile.description,
            plan.skipped.len(),
            plan.skipped
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    for reference in plan.missing {
        let mut request = OperationRequest::new(
            reference.clone(),
            if reference.provider == ProviderId::Winget {
                Action::Install
            } else {
                Action::Download
            },
        );
        request.batch = Some(batch.clone());
        request.profile = Some(profile.name.clone());
        request.silent = data.settings.silent_installers;
        match reference.provider {
            ProviderId::Winget => {
                prepared
                    .summary
                    .push_str(&format!("\n• Instalar {} [Winget]", reference.id));
                prepared.requests.push(request);
            }
            ProviderId::Custom => {
                let app=data.custom_apps.iter().chain(profile.custom_apps.iter()).find(|a|a.id.to_string().eq_ignore_ascii_case(&reference.id)).ok_or_else(||anyhow::anyhow!("Cadastro personalizado ausente: {}. Importe o catálogo ou o perfil com suas definições.",reference.id))?;
                app.validate()?;
                let mut pending =
                    ManualPending::for_app(app, Some(profile.name.clone()), Some(batch.clone()));
                request.custom_app = Some(app.clone());
                match &app.source {
                    CustomSource::Manual => {
                        prepared.summary.push_str(&format!(
                            "\n• Instalação manual: {} — {}",
                            app.name, app.website
                        ));
                    }
                    CustomSource::DirectUrl {
                        url,
                        filename,
                        sha256,
                    } => {
                        prepared.summary.push_str(&format!(
                            "\n• Baixar {} [URL direta; atualização não verificável]\n  Arquivo: {} · Origem: {} · {}",
                            app.name,filename,crate::domain::validate_https(url)?.host_str().unwrap_or(""),if sha256.is_some(){"SHA-256 configurado"}else{"Sem hash para comparação"}
                        ));
                        prepared.requests.push(request);
                    }
                    CustomSource::Github {
                        repository,
                        prerelease,
                        architecture,
                        asset_pattern,
                    } => {
                        let result = providers
                            .github
                            .resolve(repository, *prerelease)
                            .await
                            .and_then(|r| {
                                let asset = github::select_configured_asset(
                                    &r,
                                    *architecture,
                                    asset_pattern,
                                )?
                                .clone();
                                Ok((r, asset))
                            });
                        match result {
                            Ok((release, asset)) => {
                                prepared.summary.push_str(&format!(
                                    "\n• Baixar {} [GitHub: {}] — {} / {}",
                                    app.name, repository, release.tag_name, asset.name
                                ));
                                request.asset = Some(asset);
                                prepared.requests.push(request);
                            }
                            Err(e) => {
                                pending.instructions = format!(
                                    "Resolução automática indisponível: {e:#}\n\n{}",
                                    app.instructions
                                );
                                pending.instructions =
                                    pending.instructions.chars().take(11000).collect();
                                prepared.summary.push_str(&format!(
                                    "\n• Pendência manual: {} — {e:#}",
                                    app.name
                                ));
                            }
                        }
                    }
                }
                prepared.manual.push(pending);
            }
            ProviderId::Github => {
                let mut pending=ManualPending {id:uuid::Uuid::new_v4(),app_id:None,name:reference.id.clone(),website:format!("https://github.com/{}",reference.id),instructions:"Revise o download e execute o instalador somente após confirmação. Download não comprova instalação.".into(),profile:Some(profile.name.clone()),batch:Some(batch.clone()),status:ManualStatus::Pending,created_at:chrono::Utc::now(),updated_at:chrono::Utc::now()};
                match providers.github.latest(&reference.id).await {
                    Ok(release) => {
                        if let Some(asset) = github::select_asset(&release.assets) {
                            prepared.summary.push_str(&format!(
                                "\n• Baixar {} — {}",
                                reference.id, release.tag_name
                            ));
                            request.asset = Some(asset.clone());
                            prepared.requests.push(request);
                        } else {
                            pending.instructions="Release sem asset Windows x64. Consulte o website/repositório para instalar manualmente.".into();
                            prepared.summary.push_str(&format!(
                                "\n• Pendência manual: {} (release sem instalador)",
                                reference.id
                            ));
                        }
                    }
                    Err(e) => {
                        pending.instructions = format!("GitHub indisponível: {e:#}");
                        prepared
                            .summary
                            .push_str(&format!("\n• Pendência manual: {} — {e:#}", reference.id));
                    }
                }
                prepared.manual.push(pending);
            }
            ProviderId::Windows => {
                bail!("Ferramentas do Windows não podem ser instaladas por perfil")
            }
        }
    }
    prepared.summary.push_str(&format!("\n\n{} operações na fila. {} pendências de instalação/acompanhamento.\n\nDownloads e conclusões manuais NÃO comprovam instalação. Aplicativos personalizados serão resolvidos novamente na próxima execução; não são ignorados apenas por já terem sido baixados. Instaladores só podem ser abertos após uma confirmação adicional em Operações ou Pendências.",prepared.requests.len(),prepared.manual.len()));
    Ok(prepared)
}

pub fn import(path: &Path) -> Result<Profile> {
    if std::fs::metadata(path)?.len() > 1_048_576 {
        bail!("Perfil maior que 1 MB");
    }
    Profile::parse(&std::fs::read_to_string(path)?)
}
pub fn export(path: &Path, profile: &Profile) -> Result<()> {
    profile.validate()?;
    crate::infrastructure::storage::atomic_write(path, &serde_json::to_vec_pretty(profile)?)
}
pub fn save(
    profiles: &mut Vec<Profile>,
    profile: Profile,
    replace: Option<usize>,
) -> Result<usize> {
    profile.validate()?;
    if profiles
        .iter()
        .enumerate()
        .any(|(i, p)| Some(i) != replace && p.name.eq_ignore_ascii_case(&profile.name))
    {
        bail!("Já existe um perfil com esse nome");
    }
    match replace {
        Some(i) if i < profiles.len() => {
            profiles[i] = profile;
            Ok(i)
        }
        Some(_) => bail!("Perfil não encontrado"),
        None => {
            if profiles.len() >= 100 {
                bail!("Limite de 100 perfis atingido");
            }
            profiles.push(profile);
            Ok(profiles.len() - 1)
        }
    }
}
