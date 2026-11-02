use std::{
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool},
};
use uuid::Uuid;
use winit::{
    domain::*,
    infrastructure::storage::{SavedData, Storage, atomic_write, publish_new_file},
    providers::{
        ExecuteContext, Providers, SoftwareProvider,
        github::{self, Release},
    },
    services::{catalog, downloads, manual, operations::OperationManager, profiles},
};

fn app() -> CustomApp {
    CustomApp::duodesk_example()
}
fn manual_app() -> CustomApp {
    let mut a = app();
    a.id = Uuid::new_v4();
    a.name = "Aplicativo manual de teste".into();
    a.website = "https://example.com/app".into();
    a.source = CustomSource::Manual;
    a
}
fn direct_app() -> CustomApp {
    let mut a = manual_app();
    a.name = "Instalador direto de teste".into();
    a.source = CustomSource::DirectUrl {
        url: "https://example.com/setup.exe".into(),
        filename: "setup.exe".into(),
        sha256: None,
    };
    a
}
fn profile(packages: Vec<PackageRef>) -> Profile {
    Profile {
        schema_version: 1,
        name: "Perfil misto".into(),
        description: String::new(),
        packages,
        custom_apps: vec![],
    }
}
fn asset(name: &str) -> Asset {
    Asset {
        name: name.into(),
        browser_download_url: format!("https://github.com/owner/repo/releases/download/v1/{name}"),
        size: 40,
        digest: None,
    }
}
fn release(tag: &str, pre: bool, date: &str, assets: Vec<Asset>) -> Release {
    Release {
        tag_name: tag.into(),
        prerelease: pre,
        draft: false,
        published_at: Some(date.into()),
        html_url: "https://github.com/owner/repo/releases".into(),
        body: None,
        assets,
    }
}
fn root() -> PathBuf {
    let p = std::env::temp_dir().join(format!("winit-custom-test-{}", Uuid::new_v4()));
    std::fs::create_dir(&p).unwrap();
    p
}

#[tokio::test]
async fn existing_download_is_reused_only_after_hash_verification() {
    use sha2::{Digest, Sha256};
    let dir = root();
    let folder = dir.join("test");
    std::fs::create_dir(&folder).unwrap();
    let bytes = b"offline test payload, never executed";
    std::fs::write(folder.join("setup.exe"), bytes).unwrap();
    let hash = hex::encode(Sha256::digest(bytes));
    let client = downloads::direct_client().unwrap();
    let context = ExecuteContext {
        silent: false,
        accept_agreements: false,
        downloads: dir.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        progress: Arc::new(|_| {}),
    };
    let make_spec = |hash| downloads::DownloadSpec {
        url: "https://example.com/setup.exe".into(),
        filename: "setup.exe".into(),
        namespace: "test".into(),
        size: Some(bytes.len() as u64),
        sha256: Some(hash),
        version: None,
        release_url: None,
        origin: "offline unit test".into(),
    };
    let result = downloads::download(&client, make_spec(hash), context.clone())
        .await
        .unwrap();
    assert!(result.download_metadata.unwrap().reused_existing);
    assert_eq!(result.exit_code, Some(0));
    assert!(
        downloads::download(&client, make_spec("0".repeat(64)), context)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(folder.join("setup.exe")).unwrap(), bytes);
    std::fs::remove_file(folder.join("setup.exe")).unwrap();
    std::fs::remove_dir(folder).unwrap();
    cleanup(&dir);
}
fn cleanup(p: &std::path::Path) {
    for entry in std::fs::read_dir(p).unwrap() {
        let entry = entry.unwrap();
        assert!(entry.file_type().unwrap().is_file());
        std::fs::remove_file(entry.path()).unwrap();
    }
    std::fs::remove_dir(p).unwrap();
}

#[test]
fn create_edit_preserve_ids_profiles_and_favorites() {
    let mut data = SavedData::default();
    let mut a = app();
    catalog::save(&mut data, a.clone()).unwrap();
    let mut p = profile(vec![a.reference()]);
    catalog::hydrate_profile(&mut p, &data.custom_apps).unwrap();
    data.profiles.push(p);
    data.favorites.push(a.reference());
    a.name = "DuoDesk personalizado pelo usuário".into();
    a.instructions = "Minhas instruções".into();
    catalog::save(&mut data, a.clone()).unwrap();
    assert_eq!(data.custom_apps.len(), 1);
    assert_eq!(data.profiles.last().unwrap().packages[0], a.reference());
    assert_eq!(data.profiles.last().unwrap().custom_apps[0].name, a.name);
    assert_eq!(data.favorites[0], a.reference());
}
#[test]
fn rejects_duplicate_names_and_origins_transactionally() {
    let mut d = SavedData::default();
    catalog::save(&mut d, app()).unwrap();
    let original = serde_json::to_string(&d).unwrap();
    let mut duplicate = manual_app();
    duplicate.name = " duodesk  ".into();
    assert!(catalog::save(&mut d, duplicate).is_err());
    let mut duplicate = app();
    duplicate.id = Uuid::new_v4();
    duplicate.name = "Outro nome".into();
    assert!(catalog::save(&mut d, duplicate).is_err());
    assert_eq!(serde_json::to_string(&d).unwrap(), original);
}
#[test]
fn idempotent_suggestion_never_overwrites_user_registration() {
    let mut d = SavedData::default();
    let id = catalog::add_example(&mut d).unwrap();
    d.custom_apps[0].name = "Meu DuoDesk".into();
    d.custom_apps[0].instructions = "Preservar".into();
    assert_eq!(catalog::add_example(&mut d).unwrap(), id);
    assert_eq!(d.custom_apps.len(), 1);
    assert_eq!(d.custom_apps[0].instructions, "Preservar");
}
#[test]
fn seed_reconciles_same_repository_with_another_id() {
    let mut d = SavedData::default();
    let mut a = app();
    a.id = Uuid::new_v4();
    let id = a.id;
    catalog::save(&mut d, a).unwrap();
    assert_eq!(catalog::add_example(&mut d).unwrap(), id);
    assert_eq!(d.custom_apps.len(), 1);
}
#[test]
fn custom_apps_and_pending_roundtrip_on_disk() {
    let dir = root();
    let storage = Storage {
        root: dir.clone(),
        portable: true,
    };
    let mut d = SavedData::default();
    catalog::save(&mut d, app()).unwrap();
    manual::register(
        &mut d,
        ManualPending::for_app(&app(), Some("Meu perfil".into()), None),
    )
    .unwrap();
    storage.save(&d).unwrap();
    let restored = storage.load().unwrap();
    assert_eq!(restored.custom_apps, d.custom_apps);
    assert_eq!(restored.manual_pending.len(), 1);
    assert_eq!(restored.schema_version, 2);
    cleanup(&dir);
}
#[test]
fn v1_migration_preserves_original_and_all_existing_fields() {
    let dir = root();
    let json = serde_json::json!({"schema_version":1,"settings":{"dark_theme":false,"repositories":["itsfatduck/optimizerDuck"]},"profiles":[serde_json::from_str::<serde_json::Value>(include_str!("../assets/essentials.json")).unwrap()],"favorites":[{"provider":"winget","id":"7zip.7zip"}],"history":[]});
    let bytes = serde_json::to_vec_pretty(&json).unwrap();
    atomic_write(&dir.join("state.json"), &bytes).unwrap();
    let storage = Storage {
        root: dir.clone(),
        portable: true,
    };
    let data = storage.load().unwrap();
    assert_eq!(data.schema_version, 2);
    assert!(!data.settings.dark_theme);
    assert_eq!(data.favorites.len(), 1);
    assert!(data.custom_apps.is_empty());
    assert_eq!(
        std::fs::read(dir.join("state.v1.backup.json")).unwrap(),
        bytes
    );
    assert_eq!(std::fs::read(dir.join("state.json")).unwrap(), bytes);
    storage.save(&data).unwrap();
    assert_eq!(
        std::fs::read(dir.join("state.v1.backup.json")).unwrap(),
        bytes
    );
    cleanup(&dir);
}
#[test]
fn old_profiles_remain_valid_and_keep_identifiers() {
    let p = Profile::parse(include_str!("../assets/essentials.json")).unwrap();
    assert!(p.custom_apps.is_empty());
    assert_eq!(p.plan(&[], true).unwrap().missing.len(), 4);
    assert!(!serde_json::to_string(&p).unwrap().contains("custom_apps"));
}
#[test]
fn profile_export_carries_definitions_and_import_reconciles_local_ids() {
    let source = app();
    let mut p = profile(vec![source.reference()]);
    catalog::hydrate_profile(&mut p, std::slice::from_ref(&source)).unwrap();
    let dir = root();
    profiles::export(&dir.join("profile.json"), &p).unwrap();
    let mut p = profiles::import(&dir.join("profile.json")).unwrap();
    let mut local = source.clone();
    local.id = Uuid::new_v4();
    local.name = "DuoDesk local".into();
    local.instructions = "Configuração local preservada".into();
    let mut d = SavedData::default();
    catalog::save(&mut d, local.clone()).unwrap();
    catalog::import_profile_definitions(&mut d, &mut p).unwrap();
    assert_eq!(d.custom_apps.len(), 1);
    assert_eq!(p.packages[0], local.reference());
    assert_eq!(p.custom_apps[0].instructions, local.instructions);
    cleanup(&dir);
}
#[test]
fn catalog_import_export_and_failed_merge_are_atomic() {
    let dir = root();
    catalog::export(&dir.join("catalog.json"), &[app(), manual_app()]).unwrap();
    let doc = catalog::import(&dir.join("catalog.json")).unwrap();
    let mut d = SavedData::default();
    catalog::merge(&mut d, doc).unwrap();
    assert_eq!(d.custom_apps.len(), 2);
    let original = d.clone();
    let mut bad = direct_app();
    bad.name = "DuoDesk".into();
    assert!(
        catalog::merge(
            &mut d,
            catalog::CatalogDocument {
                schema_version: 1,
                apps: vec![manual_app(), bad]
            }
        )
        .is_err()
    );
    assert_eq!(d.custom_apps, original.custom_apps);
    assert!(catalog::parse("{bad").is_err());
    cleanup(&dir);
}
#[test]
fn delete_removes_profile_refs_and_favorites_but_preserves_manual_snapshots() {
    let mut d = SavedData::default();
    let a = app();
    catalog::save(&mut d, a.clone()).unwrap();
    let mut p = profile(vec![a.reference()]);
    catalog::hydrate_profile(&mut p, &d.custom_apps).unwrap();
    d.profiles.push(p);
    d.favorites.push(a.reference());
    manual::register(&mut d, ManualPending::for_app(&a, None, None)).unwrap();
    catalog::remove(&mut d, &[a.id].into_iter().collect()).unwrap();
    assert!(d.custom_apps.is_empty());
    assert!(d.favorites.is_empty());
    assert!(d.profiles.last().unwrap().packages.is_empty());
    assert_eq!(d.manual_pending[0].name, "DuoDesk");
}
#[test]
fn custom_download_or_manual_completion_is_never_installation_evidence() {
    let a = app();
    let p = profile(vec![a.reference()]);
    let fake_installed = Package {
        reference: a.reference(),
        name: a.name,
        version: "downloaded".into(),
        available: String::new(),
        source: "custom".into(),
        manageable: true,
    };
    let plan = p.plan(&[fake_installed], false).unwrap();
    assert!(plan.skipped.is_empty());
    assert_eq!(plan.missing.len(), 1);
}
#[test]
fn manual_completion_reopens_without_creating_duplicates() {
    let a = manual_app();
    let mut d = SavedData::default();
    let pending = ManualPending::for_app(&a, Some("Setup".into()), Some("batch1".into()));
    let id = manual::register(&mut d, pending).unwrap();
    manual::set_completed(&mut d, id, true).unwrap();
    assert_eq!(d.manual_pending[0].status, ManualStatus::Completed);
    let again = ManualPending::for_app(&a, Some("Setup".into()), Some("batch2".into()));
    assert_eq!(manual::register(&mut d, again).unwrap(), id);
    assert_eq!(d.manual_pending.len(), 1);
    assert_eq!(d.manual_pending[0].status, ManualStatus::Pending);
    assert_eq!(d.manual_pending[0].batch.as_deref(), Some("batch2"));
    assert!(d.history.is_empty());
}
#[test]
fn manual_items_are_separate_for_different_profiles() {
    let a = manual_app();
    let mut d = SavedData::default();
    for name in ["Casa", "Trabalho"] {
        manual::register(&mut d, ManualPending::for_app(&a, Some(name.into()), None)).unwrap();
    }
    assert_eq!(d.manual_pending.len(), 2);
}
#[test]
fn urls_hashes_and_filenames_are_validated() {
    for url in [
        "http://example.com/setup.exe",
        "file:///C:/setup.exe",
        "https://user:pass@example.com/x",
        "https://example.com/#script",
        "https://example.com/\n",
    ] {
        assert!(validate_https(url).is_err(), "{url}");
    }
    for name in [
        "../setup.exe",
        "x\\setup.exe",
        "file.exe:zone",
        "COM9.exe",
        "LPT8.zip",
        "NUL.msi",
        "setup.exe.",
    ] {
        assert!(!safe_filename(name), "{name}");
    }
    let mut a = direct_app();
    a.source = CustomSource::DirectUrl {
        url: "https://example.com/x".into(),
        filename: "script.ps1".into(),
        sha256: None,
    };
    assert!(a.validate().is_err());
    assert!(validate_sha256(&"a".repeat(64)).is_ok());
    assert!(validate_sha256("xx").is_err());
}
#[test]
fn release_selection_respects_channel_and_ignores_drafts() {
    let stable = release("v1", false, "2026-01-01", vec![]);
    let beta = release("v2-beta", true, "2026-02-01", vec![]);
    let mut draft = release("v3", false, "2026-03-01", vec![]);
    draft.draft = true;
    assert_eq!(
        github::select_release(vec![stable.clone(), beta.clone(), draft.clone()], false)
            .unwrap()
            .tag_name,
        "v1"
    );
    assert_eq!(
        github::select_release(vec![stable, beta, draft], true)
            .unwrap()
            .tag_name,
        "v2-beta"
    );
}
#[test]
fn duodesk_selection_is_dynamic_and_excludes_arm64_for_x64() {
    let r = release(
        "v2.5.0",
        false,
        "2026-10-31",
        vec![
            asset("DuoDesk-Setup-2.5.0-arm64.exe"),
            asset("DuoDesk-Setup-2.5.0.exe"),
            asset("DuoDesk-2.5.0-arm64.dmg"),
        ],
    );
    let selected =
        github::select_configured_asset(&r, Architecture::X64, "DuoDesk-Setup-*.exe").unwrap();
    assert_eq!(selected.name, "DuoDesk-Setup-2.5.0.exe");
    assert_eq!(
        github::asset_pattern(&r, &selected.name).unwrap(),
        "DuoDesk-Setup-*.exe"
    );
    let next = release(
        "v9.9.0",
        false,
        "2027-01-01",
        vec![asset("DuoDesk-Setup-9.9.0.exe")],
    );
    assert!(
        github::select_configured_asset(&next, Architecture::X64, "DuoDesk-Setup-*.exe").is_ok()
    );
}
#[test]
fn ambiguous_assets_require_selection_and_sources_are_ignored() {
    let r = release(
        "v1",
        false,
        "2026",
        vec![
            asset("App-x64.exe"),
            asset("App-x64.msi"),
            asset("source.zip"),
        ],
    );
    assert!(github::select_configured_asset(&r, Architecture::X64, "").is_err());
    assert_eq!(
        github::select_configured_asset(&r, Architecture::X64, "*.msi")
            .unwrap()
            .name,
        "App-x64.msi"
    );
    let r = release(
        "v1",
        false,
        "2026",
        vec![
            asset("source.zip"),
            asset("app-linux-x64.zip"),
            asset("code.tar.gz"),
        ],
    );
    assert!(github::select_configured_asset(&r, Architecture::X64, "").is_err());
}
#[test]
fn architecture_and_glob_matching_are_case_insensitive() {
    assert!(github::pattern_matches(
        "app-setup-*.EXE",
        "App-Setup-2.5.exe"
    ));
    assert!(github::pattern_matches("app-?.exe", "app-1.exe"));
    assert!(!github::pattern_matches("app-?.exe", "app-12.exe"));
    assert!(github::compatible_for(
        &asset("App-arm64.exe"),
        Architecture::Arm64
    ));
    assert!(!github::compatible_for(
        &asset("App-x64.exe"),
        Architecture::Arm64
    ));
    assert!(github::compatible_for(
        &asset("App-x86.exe"),
        Architecture::X86
    ));
    assert!(!github::compatible_for(
        &asset("App-win32.exe"),
        Architecture::X64
    ));
    assert!(github::compatible_for(
        &asset("App-windows-x86_64.exe"),
        Architecture::X64
    ));
}

#[test]
fn named_source_archives_are_not_installers_and_prefixed_tags_stay_dynamic() {
    let r = release(
        "release/v2.5.0",
        false,
        "2026",
        vec![
            asset("App-2.5.0-x64.exe"),
            asset("App-source.zip"),
            asset("App_sources.zip"),
            asset("app-src.zip"),
        ],
    );
    assert_eq!(github::candidates(&r, Architecture::X64, "").len(), 1);
    assert_eq!(
        github::asset_pattern(&r, "App-2.5.0-x64.exe").unwrap(),
        "App-*-x64.exe"
    );
}
#[test]
fn hash_failures_and_unverifiable_integrity_are_distinct() {
    let hash = "ab".repeat(32);
    assert!(downloads::verify_hash(&hash, Some(&hash.to_uppercase())).unwrap());
    assert!(!downloads::verify_hash(&hash, None).unwrap());
    assert!(downloads::verify_hash(&hash, Some(&"00".repeat(32))).is_err());
    assert!(downloads::verify_hash(&hash, Some("bad")).is_err());
}
#[test]
fn empty_partial_and_oversized_downloads_fail() {
    assert!(downloads::verify_size(0, None).is_err());
    assert!(downloads::verify_size(100, Some(101)).is_err());
    assert!(downloads::verify_size(downloads::MAX_DOWNLOAD_SIZE + 1, None).is_err());
    assert!(downloads::verify_size(100, None).is_ok());
}
#[test]
fn atomic_publish_cannot_overwrite_an_existing_file() {
    let dir = root();
    std::fs::write(dir.join("part"), "new").unwrap();
    std::fs::write(dir.join("file.exe"), "original").unwrap();
    assert!(publish_new_file(&dir.join("part"), &dir.join("file.exe")).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.join("file.exe")).unwrap(),
        "original"
    );
    std::fs::remove_file(dir.join("file.exe")).unwrap();
    publish_new_file(&dir.join("part"), &dir.join("file.exe")).unwrap();
    assert!(!dir.join("part").exists());
    cleanup(&dir);
}

#[tokio::test]
async fn profile_plans_existing_winget_direct_and_manual_together_without_installing() {
    let mut d = SavedData::default();
    let a = manual_app();
    let b = direct_app();
    catalog::save(&mut d, a.clone()).unwrap();
    catalog::save(&mut d, b.clone()).unwrap();
    let winget = PackageRef {
        provider: ProviderId::Winget,
        id: "7zip.7zip".into(),
    };
    let p = profile(vec![winget.clone(), a.reference(), b.reference()]);
    let providers = Providers::new().unwrap();
    let prepared = profiles::prepare(&p, &d, &[], true, &providers)
        .await
        .unwrap();
    assert_eq!(prepared.requests.len(), 2);
    assert_eq!(prepared.manual.len(), 2);
    assert_eq!(prepared.requests[0].action, Action::Install);
    assert_eq!(prepared.requests[1].action, Action::Download);
    assert!(prepared.requests[1].custom_app.is_some());
    assert!(prepared.summary.contains("NÃO comprovam"));
}
#[tokio::test]
async fn manual_only_profile_persists_in_existing_operation_manager() {
    let dir = root();
    let storage = Arc::new(Storage {
        root: dir.clone(),
        portable: true,
    });
    let data = Arc::new(Mutex::new(SavedData::default()));
    let queue = OperationManager::start(
        data.clone(),
        storage.clone(),
        Arc::new(Providers::new().unwrap()),
        Arc::new(|| {}),
    );
    let pending = ManualPending::for_app(&manual_app(), Some("Setup".into()), Some("batch".into()));
    assert_eq!(
        queue
            .enqueue_prepared(
                vec![],
                vec![pending],
                Some("batch".into()),
                Some("Setup".into())
            )
            .unwrap(),
        0
    );
    assert!(!queue.has_active());
    let loaded = storage.load().unwrap();
    assert_eq!(loaded.manual_pending.len(), 1);
    assert_eq!(loaded.last_batch.as_deref(), Some("batch"));
    assert!(loaded.history.is_empty());
    cleanup(&dir);
}
#[tokio::test]
async fn invalid_download_inputs_fail_before_creating_files_or_network_requests() {
    let dir = root();
    let ctx = ExecuteContext {
        silent: false,
        accept_agreements: false,
        downloads: dir.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        progress: Arc::new(|_| {}),
    };
    let client = downloads::direct_client().unwrap();
    let mut spec = downloads::DownloadSpec {
        url: "http://example.com/file".into(),
        filename: "setup.exe".into(),
        namespace: "test".into(),
        size: None,
        sha256: None,
        version: None,
        release_url: None,
        origin: "test".into(),
    };
    assert!(
        downloads::download(&client, spec, ctx.clone())
            .await
            .is_err()
    );
    spec = downloads::DownloadSpec {
        url: "https://example.com/file".into(),
        filename: "../setup.exe".into(),
        namespace: "test".into(),
        size: None,
        sha256: None,
        version: None,
        release_url: None,
        origin: "test".into(),
    };
    assert!(
        downloads::download(&client, spec, ctx.clone())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    cleanup(&dir);
}
#[tokio::test]
async fn custom_provider_never_executes_manual_installers_or_unsupported_actions() {
    let a = manual_app();
    let providers = Providers::new().unwrap();
    let mut request = OperationRequest::new(a.reference(), Action::Download);
    request.custom_app = Some(a);
    let ctx = ExecuteContext {
        silent: false,
        accept_agreements: false,
        downloads: std::env::temp_dir(),
        cancel: Arc::new(AtomicBool::new(false)),
        progress: Arc::new(|_| {}),
    };
    assert!(
        providers
            .custom
            .execute(&request, ctx.clone())
            .await
            .is_err()
    );
    request.action = Action::Install;
    assert!(providers.custom.execute(&request, ctx).await.is_err());
}
