use super::*;
use crate::{CustomForm, PendingForm};

async fn saved(c: &Controller, condition: impl Fn(&SavedData) -> bool) -> Result<()> {
    let start = std::time::Instant::now();
    loop {
        if condition(&c.data.lock().unwrap()) {
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(20) {
            bail!("Autoteste: persistência do catálogo/pendências não concluída");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
pub(super) async fn run(c: &Arc<Controller>, ui: &slint::Weak<AppWindow>) -> Result<()> {
    invoke(ui, |ui| {
        ui.window().set_size(slint::LogicalSize::new(1220.0, 820.0))
    })
    .await?;
    navigate(c, ui, 8).await?;
    invoke(ui, |ui| ui.invoke_action("custom-new".into(), "".into())).await?;
    wait(c, |v| v.custom.form.is_some()).await?;
    snapshot(c, ui, "11-custom-github-form").await?;
    invoke(ui, |ui| {
        ui.invoke_custom_save(CustomForm {
            name: "Manual QA".into(),
            website: "https://example.com/manual-qa".into(),
            instructions: "Registro exclusivo do autoteste; não instalar programas.".into(),
            source: 2,
            ..Default::default()
        })
    })
    .await?;
    saved(c, |d| d.custom_apps.iter().any(|a| a.name == "Manual QA")).await?;
    let app = c
        .data
        .lock()
        .unwrap()
        .custom_apps
        .iter()
        .find(|a| a.name == "Manual QA")
        .cloned()
        .context("Cadastro QA ausente")?;
    let key = app.reference().key();
    let action_key = key.clone();
    invoke(ui, move |ui| ui.invoke_entry_action(action_key.into(), 1)).await?;
    saved(c, |d| d.favorites.iter().any(|r| r.key() == key)).await?;
    let action_key = key.clone();
    invoke(ui, move |ui| {
        ui.invoke_toggle(action_key.into());
        ui.invoke_action("selection-profile".into(), "".into());
    })
    .await?;
    saved(c, |d| {
        d.profiles
            .iter()
            .any(|p| p.packages.iter().any(|r| r.key() == key))
    })
    .await?;
    snapshot(c, ui, "12-custom-catalog").await?;
    invoke(ui, |ui| {
        ui.invoke_action("custom-example".into(), "".into())
    })
    .await?;
    wait(c, |v| {
        v.custom.form.as_ref().is_some_and(|f| f.name == "DuoDesk")
    })
    .await?;
    snapshot(c, ui, "13-duodesk-before-query").await?;
    invoke(ui, |ui| ui.invoke_custom_query(ui.get_custom_form())).await?;
    wait(c, |v| {
        v.custom
            .release
            .as_ref()
            .is_some_and(|(f, _)| f.repository == "0xCodez/DuoDesk")
    })
    .await?;
    let selected = {
        let v = c.view.lock().unwrap();
        let (_, r) = v.custom.release.as_ref().unwrap();
        winit::providers::github::candidates(r, Architecture::X64, "")
            .first()
            .context("DuoDesk sem asset x64")?
            .name
            .clone()
    };
    invoke(ui, move |ui| {
        ui.invoke_custom_asset_selected(ui.get_custom_form(), selected.into())
    })
    .await?;
    snapshot(c, ui, "14-duodesk-dynamic-asset").await?;
    invoke(ui, |ui| ui.invoke_custom_save(ui.get_custom_form())).await?;
    wait(c, |v| v.custom.form.is_none()).await?;
    invoke(ui, |ui| ui.invoke_action("custom-new".into(), "".into())).await?;
    wait(c, |v| v.custom.form.is_some()).await?;
    invoke(ui, |ui| {
        ui.set_custom_form(CustomForm {
            name: "URL QA (não executar)".into(),
            website: "https://example.com/".into(),
            source: 1,
            url: "https://example.com/setup.exe".into(),
            filename: "setup.exe".into(),
            instructions: "Dados de teste isolados; nenhum download neste cenário.".into(),
            ..Default::default()
        })
    })
    .await?;
    snapshot(c, ui, "15-direct-url-form").await?;
    invoke(ui, |ui| ui.invoke_custom_save(ui.get_custom_form())).await?;
    saved(c, |d| d.custom_apps.len() == 3).await?;
    let profile = Profile {
        schema_version: 1,
        name: "Manual QA isolado".into(),
        description: "Autoteste sem instalar programas".into(),
        packages: vec![app.reference()],
        custom_apps: vec![app],
    };
    navigate(c, ui, 5).await?;
    invoke(ui, |ui| ui.invoke_action("profile-new".into(), "".into())).await?;
    wait(c, |v| v.selected_profile.is_none()).await?;
    let json = serde_json::to_string_pretty(&profile)?;
    invoke(ui, move |ui| ui.invoke_save_profile(json.into())).await?;
    saved(c, |d| {
        d.profiles.iter().any(|p| p.name == "Manual QA isolado")
    })
    .await?;
    let json = serde_json::to_string_pretty(&profile)?;
    invoke(ui, move |ui| {
        ui.invoke_action("profile-run".into(), json.into())
    })
    .await?;
    wait(c, |v| v.dialog.is_some()).await?;
    snapshot(c, ui, "16-manual-profile-plan").await?;
    invoke(ui, |ui| ui.invoke_dialog_result(true, false)).await?;
    saved(c, |d| {
        d.manual_pending
            .iter()
            .any(|p| p.profile.as_deref() == Some("Manual QA isolado"))
    })
    .await?;
    snapshot(c, ui, "17-manual-profile-report").await?;
    navigate(c, ui, 9).await?;
    let id = c
        .data
        .lock()
        .unwrap()
        .manual_pending
        .iter()
        .find(|p| p.profile.as_deref() == Some("Manual QA isolado"))
        .context("Pendência QA ausente")?
        .id;
    snapshot(c, ui, "18-manual-pending").await?;
    invoke(ui, move |ui| {
        ui.invoke_entry_action(format!("pending:{id}").into(), 1)
    })
    .await?;
    saved(c, |d| {
        d.manual_pending
            .iter()
            .any(|p| p.id == id && p.status == ManualStatus::Completed)
    })
    .await?;
    snapshot(c, ui, "19-manual-completed").await?;
    invoke(ui, move |ui| {
        ui.invoke_entry_action(format!("pending:{id}").into(), 1)
    })
    .await?;
    saved(c, |d| {
        d.manual_pending
            .iter()
            .any(|p| p.id == id && p.status == ManualStatus::Pending)
    })
    .await?;
    invoke(ui, move |ui| {
        ui.invoke_entry_action(format!("pending:{id}").into(), 2)
    })
    .await?;
    wait(c, |v| v.custom.pending.is_some()).await?;
    snapshot(c, ui, "20-pending-editor").await?;
    invoke(ui, move |ui| {
        ui.invoke_pending_save(PendingForm {
            id: id.to_string().into(),
            name: "Manual QA editado".into(),
            website: "https://example.com/manual-qa".into(),
            profile: "Manual QA isolado".into(),
            instructions: "Instruções editadas pelo autoteste".into(),
        })
    })
    .await?;
    saved(c, |d| {
        d.manual_pending
            .iter()
            .any(|p| p.id == id && p.name == "Manual QA editado")
    })
    .await?;
    if !c.data.lock().unwrap().history.is_empty() {
        bail!("O cenário de cadastro não deveria executar instaladores ou downloads");
    }
    let catalog_path = c.storage.root.join("catalog-roundtrip.json");
    let data = c.data.lock().unwrap().clone();
    winit::services::catalog::export(&catalog_path, &data.custom_apps)?;
    if winit::services::catalog::import(&catalog_path)?.apps.len() != 3 {
        bail!("Catálogo não persistiu corretamente");
    }
    Ok(())
}
