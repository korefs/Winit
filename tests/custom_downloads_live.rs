use std::sync::{Arc, atomic::AtomicBool};
use uuid::Uuid;
use winit::{
    domain::*,
    providers::{ExecuteContext, Providers, SoftwareProvider, github},
    services::downloads,
};

#[tokio::test]
#[ignore = "Consulta GitHub e baixa DuoDesk por GitHub e URL direta (~200 MB), sem executar instaladores"]
async fn live_custom_github_direct_url_hash_and_http_failures() {
    let providers = Providers::new().unwrap();
    let app = CustomApp::duodesk_example();
    let release = providers
        .github
        .resolve("0xCodez/DuoDesk", false)
        .await
        .unwrap();
    let asset = github::select_configured_asset(&release, Architecture::X64, "DuoDesk-Setup-*.exe")
        .unwrap()
        .clone();
    assert!(!asset.name.to_lowercase().contains("arm64"));
    let root = std::env::temp_dir().join(format!("winit-custom-live-{}", Uuid::new_v4()));
    let ctx = ExecuteContext {
        silent: false,
        accept_agreements: false,
        downloads: root.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        progress: Arc::new(|_| {}),
    };
    let mut request = OperationRequest::new(app.reference(), Action::Download);
    request.custom_app = Some(app.clone());
    let result = providers
        .custom
        .execute(&request, ctx.clone())
        .await
        .unwrap();
    let metadata = result.download_metadata.as_ref().unwrap();
    assert!(metadata.sha256_verified);
    assert_eq!(metadata.version.as_deref(), Some(release.tag_name.as_str()));
    assert!(!metadata.reused_existing);
    let github_file = std::path::PathBuf::from(result.file.unwrap());
    let reused = providers
        .custom
        .execute(&request, ctx.clone())
        .await
        .unwrap();
    assert!(reused.download_metadata.unwrap().reused_existing);
    assert_eq!(reused.sha256, result.sha256);
    println!(
        "GitHub personalizado: {} / {} / {} bytes; SHA-256 verificado; reutilização verificada sem reinstalar",
        release.tag_name, asset.name, asset.size
    );
    let mut direct = app.clone();
    direct.id = Uuid::new_v4();
    direct.name = "DuoDesk por URL direta (teste)".into();
    direct.source = CustomSource::DirectUrl {
        url: asset.browser_download_url.clone(),
        filename: asset.name.clone(),
        sha256: asset
            .digest
            .as_deref()
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(String::from),
    };
    let mut direct_request = OperationRequest::new(direct.reference(), Action::Download);
    direct_request.custom_app = Some(direct.clone());
    let result = providers
        .custom
        .execute(&direct_request, ctx.clone())
        .await
        .unwrap();
    let metadata = result.download_metadata.unwrap();
    assert!(metadata.sha256_verified);
    assert!(metadata.version.is_none());
    assert!(!metadata.final_url.is_empty());
    let direct_file = std::path::PathBuf::from(result.file.unwrap());
    println!(
        "URL direta: HTTPS com redirecionamento real, SHA-256 verificado, versão não verificável."
    );
    let client = downloads::direct_client().unwrap();
    let bad_http = downloads::DownloadSpec {
        url: "https://api.github.com/repos/0xCodez/DuoDesk/winit-nonexistent-download-test".into(),
        filename: "failure.exe".into(),
        namespace: "failure".into(),
        size: None,
        sha256: None,
        version: None,
        release_url: None,
        origin: "teste de erro HTTP".into(),
    };
    assert!(
        downloads::download(&client, bad_http, ctx.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("HTTP")
    );
    assert!(!root.join("failure/failure.exe").exists());
    // Small official API payload used only to exercise a rejected hash; never published/executed.
    let bad_hash = downloads::DownloadSpec {
        url: "https://api.github.com/repos/0xCodez/DuoDesk/releases/latest".into(),
        filename: "rejected.exe".into(),
        namespace: "bad-hash".into(),
        size: None,
        sha256: Some("0".repeat(64)),
        version: None,
        release_url: None,
        origin: "payload oficial de teste, rejeitado antes de publicação".into(),
    };
    assert!(
        downloads::download(&client, bad_hash, ctx.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("SHA-256")
    );
    assert_eq!(std::fs::read_dir(root.join("bad-hash")).unwrap().count(), 0);
    println!("Falhas reais HTTP e SHA-256: tratadas; arquivo inválido e .partial ausentes.");
    std::fs::remove_file(&github_file).unwrap();
    std::fs::remove_file(&direct_file).unwrap();
    std::fs::remove_dir(github_file.parent().unwrap()).unwrap();
    std::fs::remove_dir(direct_file.parent().unwrap()).unwrap();
    std::fs::remove_dir(root.join("failure")).unwrap();
    std::fs::remove_dir(root.join("bad-hash")).unwrap();
    std::fs::remove_dir(root).unwrap();
}
