use super::*;
use crate::{Entry, Palette};
use slint::{ModelRc, SharedString, VecModel};
use winit::{infrastructure::system, providers::github::compatible};

pub(super) fn render(
    ui: &AppWindow,
    data: &SharedData,
    view: &Arc<Mutex<View>>,
    storage: &Storage,
) {
    let d = data.lock().unwrap().clone();
    let v = view.lock().unwrap().clone();
    if ui.get_page() != v.page {
        ui.set_query(v.query.clone().into());
    }
    ui.set_page(v.page);
    ui.set_dark(d.settings.dark_theme);
    ui.global::<Palette>().set_dark(d.settings.dark_theme);
    ui.global::<crate::WidgetPalette>()
        .set_color_scheme(if d.settings.dark_theme {
            slint::language::ColorScheme::Dark
        } else {
            slint::language::ColorScheme::Light
        });
    ui.set_system_info(system::summary().into());
    ui.set_installed_count(
        v.installed
            .as_ref()
            .map(|p| {
                p.iter()
                    .filter(|p| p.reference.provider == ProviderId::Winget)
                    .count()
                    .to_string()
            })
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    ui.set_updates_count(
        v.updates
            .as_ref()
            .map(|p| p.len().to_string())
            .unwrap_or_else(|| "—".into())
            .into(),
    );
    ui.set_profiles_count(d.profiles.len().to_string().into());
    ui.set_profile_names(ModelRc::new(VecModel::from(
        d.profiles
            .iter()
            .map(|p| SharedString::from(&p.name))
            .collect::<Vec<_>>(),
    )));
    ui.set_repositories(ModelRc::new(VecModel::from(
        d.settings
            .repositories
            .iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    ui.set_winget_status(v.winget_status.clone().into());
    ui.set_winget_available(v.winget_available);
    ui.set_status(v.status.clone().into());
    ui.set_error(v.error);
    ui.set_busy(v.working > 0);
    ui.set_downloads_path(d.settings.downloads.to_string_lossy().into_owned().into());
    ui.set_data_path(storage.root.to_string_lossy().into_owned().into());
    ui.set_storage_mode(
        if storage.portable {
            "Modo portátil"
        } else {
            "Dados do usuário"
        }
        .into(),
    );
    ui.set_silent_installers(d.settings.silent_installers);
    ui.set_source_agreements(d.settings.accept_source_agreements);
    ui.set_last_profile(
        d.settings
            .last_profile
            .clone()
            .or_else(|| d.profiles.first().map(|p| p.name.clone()))
            .unwrap_or_else(|| "Nenhum perfil salvo".into())
            .into(),
    );
    if ui.get_editor_revision() != v.editor_revision {
        ui.set_profile_json(v.editor.clone().into());
        ui.set_profile_index(v.selected_profile.map(|i| i as i32).unwrap_or(-1));
        ui.set_editor_revision(v.editor_revision);
    }
    ui.set_filter(v.filter.clone().into());
    let active = d
        .history
        .iter()
        .filter(|o| matches!(o.state, OperationState::Pending | OperationState::Running))
        .count();
    let complete = d
        .history
        .iter()
        .filter(|o| o.state == OperationState::Completed)
        .count();
    let failed = d
        .history
        .iter()
        .filter(|o| o.state == OperationState::Failed)
        .count();
    let cancelled = d
        .history
        .iter()
        .filter(|o| o.state == OperationState::Cancelled)
        .count();
    let mut summary = format!(
        "{active} na fila ou em execução · {complete} concluídas · {failed} falhas · {cancelled} canceladas"
    );
    if let Some(batch) = d.last_batch.as_ref().or_else(|| {
        d.history
            .iter()
            .rev()
            .find_map(|o| o.request.batch.as_ref())
    }) {
        let batch_ops: Vec<_> = d
            .history
            .iter()
            .filter(|o| o.request.batch.as_ref() == Some(batch))
            .collect();
        let ok = batch_ops
            .iter()
            .filter(|o| o.state == OperationState::Completed)
            .count();
        let err = batch_ops
            .iter()
            .filter(|o| o.state == OperationState::Failed)
            .count();
        if batch_ops.is_empty() {
            summary.push_str("\nÚltimo plano: somente pendências manuais.");
        } else {
            summary.push_str(&format!(
                "\nÚltimo lote: {ok}/{} concluídas, {err} falhas.",
                batch_ops.len()
            ));
        }
        let manual: Vec<_> = d
            .manual_pending
            .iter()
            .filter(|p| p.batch.as_ref() == Some(batch))
            .collect();
        if !manual.is_empty() {
            summary.push_str(&format!("\nInstalações manuais/acompanhamento: {} pendentes, {} concluídas manualmente (não comprovam instalação).",manual.iter().filter(|p|p.status==ManualStatus::Pending).count(),manual.iter().filter(|p|p.status==ManualStatus::Completed).count()));
            summary.push_str(&format!(
                "\n{}",
                manual
                    .iter()
                    .take(8)
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            if manual.len() > 8 {
                summary.push_str(&format!(
                    " e mais {}. Veja a lista completa em Pendências.",
                    manual.len() - 8
                ));
            }
        }
    }
    ui.set_activity_summary(summary.into());
    let installed: HashSet<_> = v
        .installed
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|p| p.reference.key())
        .collect();
    let favorites: HashSet<_> = d.favorites.iter().map(PackageRef::key).collect();
    let mut entries = vec![];
    if matches!(v.page, 1..=3) {
        let packages = package_source(&v, &d);
        let query = v.query.to_lowercase();
        for p in packages {
            let key = p.reference.key();
            let is_installed = installed.contains(&key);
            let favorite = favorites.contains(&key);
            if v.filter == "favorites" && !favorite
                || v.filter == "missing" && is_installed
                || v.filter == "installed" && !is_installed
            {
                continue;
            }
            if (v.page != 1 || v.filter == "favorites")
                && !query.is_empty()
                && !format!("{} {}", p.name, p.reference.id)
                    .to_lowercase()
                    .contains(&query)
            {
                continue;
            }
            entries.push(Entry {
                key: key.clone().into(),
                title: p.name.clone().into(),
                subtitle: format!("{}  ·  {}", p.reference.id, p.source).into(),
                version: if p.available.is_empty() {
                    format!("Versão {}", p.version)
                } else {
                    format!("{}  →  {}", p.version, p.available)
                }
                .into(),
                badge: if p.reference.provider == ProviderId::Windows {
                    "REGISTRO"
                } else if !p.manageable {
                    "ID truncado"
                } else if is_installed {
                    "INSTALADO"
                } else {
                    "WINGET"
                }
                .into(),
                selected: v.selected.contains(&key),
                favorite,
                can_select: p.manageable,
                primary: if !p.manageable {
                    ""
                } else if v.page == 3 {
                    "Atualizar"
                } else if v.page == 2 {
                    "Remover"
                } else if is_installed {
                    ""
                } else {
                    "Instalar"
                }
                .into(),
                secondary: if !p.manageable {
                    ""
                } else if v.page == 2 && !p.available.is_empty() {
                    "Atualizar"
                } else if favorite {
                    "★"
                } else {
                    "☆"
                }
                .into(),
                extra: "Detalhes".into(),
                ..Default::default()
            });
        }
        ui.set_result_label(format!("{} aplicativos", entries.len()).into());
        ui.set_selection_label(format!("{} selecionados", v.selected.len()).into());
        let (title, description) = if v.working > 0 {
            (
                "Consultando o Winget…",
                "Os resultados aparecerão assim que a consulta terminar.",
            )
        } else if v.error {
            (
                "A consulta precisa de atenção",
                "Veja a mensagem abaixo ou abra Detalhes para o diagnóstico.",
            )
        } else if v.page == 1 && v.query.len() < 2 {
            (
                "O que vamos instalar hoje?",
                "Digite ao menos dois caracteres para pesquisar no catálogo Winget.",
            )
        } else if v.page == 3 {
            (
                "Nenhuma atualização nesta lista",
                "Atualize a consulta ou revise os filtros. Pacotes fixados ou sem versão conhecida podem ser omitidos pelo Winget.",
            )
        } else {
            (
                "Nenhum aplicativo encontrado",
                "Tente outra pesquisa, atualize a lista ou revise os filtros.",
            )
        };
        ui.set_empty_title(title.into());
        ui.set_empty_description(description.into());
    } else if v.page == 4 {
        if let Some((repo, release)) = &v.release {
            let count = release.assets.iter().filter(|a| compatible(a)).count();
            ui.set_release_info(
                format!(
                    "{repo} · {} · {} assets, {count} candidatos Windows x64. {}",
                    release.tag_name,
                    release.assets.len(),
                    if count == 0 {
                        "Nenhum arquivo compatível: use a página oficial."
                    } else {
                        "A execução nunca é automática."
                    }
                )
                .into(),
            );
            for a in &release.assets {
                let supported = compatible(a);
                entries.push(Entry {
                    key: format!("asset:{}", a.name).into(),
                    title: a.name.clone().into(),
                    subtitle: format!("GitHub · {repo} · {:.1} MB", a.size as f64 / 1_048_576.0)
                        .into(),
                    version: a
                        .digest
                        .clone()
                        .unwrap_or_else(|| "Sem hash publicado pela release".into())
                        .into(),
                    badge: if supported {
                        "DOWNLOAD"
                    } else {
                        "NÃO COMPATÍVEL"
                    }
                    .into(),
                    primary: if supported { "Baixar" } else { "" }.into(),
                    secondary: "Ver release".into(),
                    ..Default::default()
                });
            }
        }
        for t in TOOLS {
            entries.push(Entry {
                key: format!("tool:{}", t.id).into(),
                title: t.name.into(),
                subtitle: t.description.into(),
                badge: format!("WINDOWS · {}", t.category).into(),
                primary: "Abrir".into(),
                ..Default::default()
            });
        }
    } else if v.page == 7 {
        for o in d.history.iter().rev() {
            let primary = match o.state {
                OperationState::Pending => "Cancelar",
                OperationState::Running if o.request.action == Action::Download => "Cancelar",
                OperationState::Failed | OperationState::Cancelled => "Tentar novamente",
                OperationState::Completed if o.file.is_some() => "Abrir pasta",
                _ => "",
            };
            let executable = o.file.as_ref().is_some_and(|p| {
                [".exe", ".msi", ".msix"]
                    .iter()
                    .any(|ext| p.to_lowercase().ends_with(ext))
            });
            let subtitle = if o.downloaded > 0 {
                format!(
                    "{:.1} MB recebidos{} · {}",
                    o.downloaded as f64 / 1_048_576.0,
                    o.total
                        .map(|t| format!(" / {:.1} MB", t as f64 / 1_048_576.0))
                        .unwrap_or_default(),
                    o.message
                )
            } else {
                o.message.clone()
            };
            entries.push(Entry {
                key: o.id.to_string().into(),
                title: format!(
                    "{} · {}",
                    o.request.action.label(),
                    o.request
                        .custom_app
                        .as_ref()
                        .map(|a| a.name.as_str())
                        .unwrap_or(&o.request.package.id)
                )
                .into(),
                subtitle: subtitle.into(),
                version: format!(
                    "{} · {}",
                    o.created_at
                        .with_timezone(&chrono::Local)
                        .format("%d/%m %H:%M"),
                    o.request.package.provider.label()
                )
                .into(),
                badge: o.state.label().into(),
                primary: primary.into(),
                secondary: if o.state == OperationState::Completed && executable {
                    "Executar"
                } else {
                    ""
                }
                .into(),
                extra: "Detalhes".into(),
                has_progress: o.request.action == Action::Download
                    && o.state == OperationState::Running
                    && o.total.is_some(),
                progress: o
                    .total
                    .filter(|t| *t > 0)
                    .map(|t| (o.downloaded as f64 / t as f64).clamp(0.0, 1.0) as f32)
                    .unwrap_or(0.0),
                state: match o.state {
                    OperationState::Failed => 3,
                    OperationState::Completed => 2,
                    OperationState::Running => 1,
                    _ => 0,
                },
                ..Default::default()
            });
        }
    }
    custom::render(ui, &d, &v, &mut entries);
    ui.set_entries(ModelRc::new(VecModel::from(entries)));
    ui.set_dialog_visible(v.dialog.is_some());
    if let Some(dialog) = v.dialog {
        if ui.get_dialog_title().as_str() != dialog.title
            || ui.get_dialog_text().as_str() != dialog.text
        {
            ui.set_dialog_terms(false);
        }
        ui.set_dialog_title(dialog.title.into());
        ui.set_dialog_text(dialog.text.into());
        ui.set_dialog_confirm_label(dialog.label.into());
        ui.set_dialog_needs_terms(dialog.terms);
        ui.set_dialog_confirmable(dialog.confirmation.is_some());
    }
}
