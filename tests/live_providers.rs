use winit::{
    domain::Profile,
    providers::{Providers, SoftwareProvider, github::select_asset},
};

#[tokio::test]
#[ignore = "Download HTTPS real (~90 MB), sem executar arquivos; execute explicitamente"]
async fn live_official_download() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    };
    use winit::{domain::*, providers::ExecuteContext};
    let providers = Providers::new().unwrap();
    let release = providers
        .github
        .latest("itsfatduck/optimizerDuck")
        .await
        .unwrap();
    let asset = select_asset(&release.assets).unwrap().clone();
    let root = std::env::temp_dir().join(format!("winit-download-test-{}", uuid::Uuid::new_v4()));
    let count = Arc::new(AtomicU64::new(0));
    let progress = count.clone();
    let result = providers
        .github
        .execute(
            &OperationRequest {
                package: PackageRef {
                    provider: ProviderId::Github,
                    id: "itsfatduck/optimizerDuck".into(),
                },
                action: Action::Download,
                asset: Some(asset.clone()),
                custom_app: None,
                profile: None,
                batch: None,
                accept_agreements: false,
                silent: false,
            },
            ExecuteContext {
                silent: false,
                accept_agreements: false,
                downloads: root.clone(),
                cancel: Arc::new(AtomicBool::new(false)),
                progress: Arc::new(move |p| {
                    progress.store(p.downloaded, Ordering::Relaxed);
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(count.load(Ordering::Relaxed), asset.size);
    assert_eq!(
        format!("sha256:{}", result.sha256.unwrap()),
        asset.digest.unwrap()
    );
    let file = std::path::PathBuf::from(result.file.unwrap());
    assert!(file.exists());
    println!(
        "Download real verificado: {} bytes; {}",
        asset.size, result.message
    );
    std::fs::remove_file(&file).unwrap();
    std::fs::remove_dir(file.parent().unwrap()).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[tokio::test]
#[ignore = "Consulta serviços reais e o Winget local; execute explicitamente no Windows"]
async fn live_read_only_providers() {
    let providers = Providers::new().unwrap();
    let version = providers.winget.diagnose().await.unwrap();
    println!("Winget: {version}");
    let installed = providers.winget.installed(false).await.unwrap();
    assert!(!installed.is_empty());
    let search = providers.winget.search("7zip", false).await.unwrap();
    assert!(search.iter().any(|p| p.reference.id == "7zip.7zip"));
    let updates = providers.winget.updates(false).await.unwrap();
    println!(
        "{} instalados, {} resultados, {} atualizações",
        installed.len(),
        search.len(),
        updates.len()
    );
    let profile = Profile::parse(include_str!("../assets/essentials.json")).unwrap();
    for p in &profile.packages {
        let details = providers.winget.details(&p.id, false).await.unwrap();
        assert!(!details.trim().is_empty());
        println!("ID validado: {}", p.id);
    }
    let plan = profile.plan(&installed, true).unwrap();
    println!(
        "Perfil: {} ignorados, {} ausentes",
        plan.skipped.len(),
        plan.missing.len()
    );
    let release = providers
        .github
        .latest("itsfatduck/optimizerDuck")
        .await
        .unwrap();
    let asset = select_asset(&release.assets).expect("Asset Windows x64 oficial");
    assert!(asset.name.contains("x64"));
    assert!(asset.digest.as_deref().unwrap_or("").starts_with("sha256:"));
    println!(
        "GitHub {}: {} ({} bytes)",
        release.tag_name, asset.name, asset.size
    );
}
