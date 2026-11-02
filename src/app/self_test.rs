use super::*;
use slint::Model;
mod custom_scenario;

type TestResult = Arc<Mutex<Option<Result<()>>>>;
pub(super) fn start(c: Arc<Controller>, ui: slint::Weak<AppWindow>) -> TestResult {
    let result = Arc::new(Mutex::new(None));
    let output = result.clone();
    tokio::spawn(async move {
        let outcome = run(c.clone(), ui.clone()).await;
        let report = c.storage.root.join("result.json");
        let text = match &outcome {
            Ok(()) => {
                serde_json::json!({"passed":true,"screenshots":20,"scope":"Janela Slint, Winget real, catálogo GitHub/URL/manual, DuoDesk dinâmico, favoritos, perfis existentes e personalizados, planejamento, pendências com conclusão/reabertura/edição, tema e erros. Sem instalar ou remover programas."})
            }
            Err(e) => serde_json::json!({"passed":false,"error":format!("{e:#}")}),
        };
        let _ = std::fs::write(
            &report,
            serde_json::to_vec_pretty(&text).unwrap_or_default(),
        );
        eprintln!("Autoteste: {}", report.display());
        *output.lock().unwrap() = Some(outcome);
        let _ = ui.upgrade_in_event_loop(|_| {
            let _ = slint::quit_event_loop();
        });
    });
    result
}
async fn wait(c: &Controller, condition: impl Fn(&View) -> bool) -> Result<()> {
    let start = std::time::Instant::now();
    loop {
        if condition(&c.view.lock().unwrap()) {
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(150) {
            let view = c.view.lock().unwrap().clone();
            bail!(
                "Autoteste excedeu o tempo: página {}, tarefas {}, status {}",
                view.page,
                view.working,
                view.status
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
async fn invoke(
    ui: &slint::Weak<AppWindow>,
    f: impl FnOnce(AppWindow) + Send + 'static,
) -> Result<()> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    ui.upgrade_in_event_loop(move |ui| {
        f(ui);
        let _ = tx.send(());
    })?;
    rx.await?;
    Ok(())
}
async fn snapshot(c: &Controller, ui: &slint::Weak<AppWindow>, name: &str) -> Result<()> {
    // Wait for a render turn after the tested state change; this is an app-level QA harness.
    tokio::time::sleep(Duration::from_millis(350)).await;
    let (tx, rx) = tokio::sync::oneshot::channel();
    ui.upgrade_in_event_loop(move |ui| {
        let shot = ui
            .window()
            .take_snapshot()
            .map(|p| (p.width(), p.height(), p.as_bytes().to_vec()))
            .map_err(|e| e.to_string());
        let _ = tx.send(shot);
    })?;
    let (w, h, bytes) = rx.await?.map_err(anyhow::Error::msg)?;
    image::save_buffer(
        c.storage.root.join(format!("{name}.png")),
        &bytes,
        w,
        h,
        image::ColorType::Rgba8,
    )?;
    Ok(())
}
async fn navigate(c: &Controller, ui: &slint::Weak<AppWindow>, page: i32) -> Result<()> {
    eprintln!("Autoteste: navegar para {page}");
    invoke(ui, move |ui| ui.invoke_navigate(page)).await?;
    wait(c, |v| v.page == page && v.working == 0).await
}
async fn run(c: Arc<Controller>, ui: slint::Weak<AppWindow>) -> Result<()> {
    wait(&c, |v| {
        v.working == 0 && v.winget_available && v.installed.is_some()
    })
    .await?;
    snapshot(&c, &ui, "01-dashboard-dark").await?;
    eprintln!("Autoteste: dashboard capturado");
    navigate(&c, &ui, 1).await?;
    invoke(&ui, |ui| {
        ui.set_query("7zip".into());
        ui.invoke_query_edited("7zip".into());
    })
    .await?;
    wait(&c, |v| {
        v.search.iter().any(|p| p.reference.id == "7zip.7zip")
    })
    .await?;
    snapshot(&c, &ui, "02-winget-search").await?;
    let key = "winget:7zip.7zip";
    invoke(&ui, |ui| {
        ui.invoke_entry_action("winget:7zip.7zip".into(), 1)
    })
    .await?;
    let start = std::time::Instant::now();
    while !c
        .data
        .lock()
        .unwrap()
        .favorites
        .iter()
        .any(|p| p.key() == key)
    {
        if start.elapsed() > Duration::from_secs(5) {
            bail!("Favorito não persistido");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    invoke(&ui, |ui| {
        ui.invoke_toggle("winget:7zip.7zip".into());
        ui.invoke_action("selection-profile".into(), "".into());
    })
    .await?;
    wait(&c, |v| v.status.contains("adicionados ao perfil")).await?;
    navigate(&c, &ui, 2).await?;
    snapshot(&c, &ui, "03-installed").await?;
    navigate(&c, &ui, 3).await?;
    snapshot(&c, &ui, "04-updates").await?;
    navigate(&c, &ui, 5).await?;
    invoke(&ui, |ui| ui.invoke_select_profile(0)).await?;
    wait(&c, |v| v.selected_profile == Some(0)).await?;
    snapshot(&c, &ui, "05-profiles").await?;
    invoke(&ui, |ui| {
        let json = ui.get_profile_json();
        ui.invoke_action("profile-run".into(), json);
    })
    .await?;
    wait(&c, |v| v.dialog.is_some()).await?;
    snapshot(&c, &ui, "06-profile-plan").await?;
    invoke(&ui, |ui| ui.invoke_dialog_result(false, false)).await?;
    navigate(&c, &ui, 4).await?;
    invoke(&ui, |ui| {
        ui.invoke_action("github-query".into(), "itsfatduck/optimizerDuck".into())
    })
    .await?;
    wait(&c, |v| v.release.is_some()).await?;
    snapshot(&c, &ui, "07-github-tools").await?;
    navigate(&c, &ui, 6).await?;
    invoke(&ui, |ui| ui.invoke_theme(false)).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    snapshot(&c, &ui, "08-settings-light").await?;
    invoke(&ui, |ui| ui.invoke_theme(true)).await?;
    navigate(&c, &ui, 5).await?;
    invoke(&ui, |ui| ui.invoke_save_profile("{invalid json".into())).await?;
    wait(&c, |v| v.error && v.dialog.is_some()).await?;
    snapshot(&c, &ui, "09-invalid-profile").await?;
    invoke(&ui, |ui| {
        assert!(ui.get_entries().row_count() <= 500);
        ui.invoke_dialog_result(false, false);
    })
    .await?;
    navigate(&c, &ui, 1).await?;
    invoke(&ui, |ui| {
        ui.window().set_size(slint::LogicalSize::new(960.0, 680.0))
    })
    .await?;
    snapshot(&c, &ui, "10-small-window-empty").await?;
    custom_scenario::run(&c, &ui).await?;
    Ok(())
}
