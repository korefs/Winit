use winit::{
    domain::*,
    infrastructure::storage::{SavedData, Storage, atomic_write},
    providers::{
        SoftwareProvider,
        github::{asset_safe_name, compatible, select_asset, validate_asset_url},
        winget::{TableKind, is_empty_result, mutation_args, parse_table},
    },
    services::profiles,
};

fn reference(id: &str) -> PackageRef {
    PackageRef {
        provider: ProviderId::Winget,
        id: id.into(),
    }
}
fn profile() -> Profile {
    Profile::parse(include_str!("../assets/essentials.json")).unwrap()
}
fn asset(name: &str) -> Asset {
    Asset {
        name: name.into(),
        browser_download_url: format!("https://github.com/owner/repo/releases/download/v1/{name}"),
        size: 42,
        digest: None,
    }
}

#[test]
fn profile_round_trip() {
    let p = profile();
    assert_eq!(
        Profile::parse(&serde_json::to_string(&p).unwrap())
            .unwrap()
            .packages,
        p.packages
    );
}
#[test]
fn rejects_unknown_schema() {
    let mut p = profile();
    p.schema_version = 2;
    assert!(p.validate().is_err());
}
#[test]
fn rejects_invalid_and_unknown_fields() {
    assert!(Profile::parse("{garbage").is_err());
    assert!(
        Profile::parse(r#"{"schema_version":1,"name":"x","packages":[],"script":"evil"}"#).is_err()
    );
}
#[test]
fn duplicates_are_case_insensitive() {
    let mut p = profile();
    p.packages.push(reference("7ZIP.7ZIP"));
    assert!(p.validate().is_err());
}
#[test]
fn identifiers_reject_injection_and_truncation() {
    for id in [
        "--help",
        "x; calc.exe",
        "x\n--silent",
        "x…",
        "",
        "../file",
        "a\\b",
    ] {
        assert!(validate_winget_id(id).is_err(), "{id}");
    }
    for id in [
        "7zip.7zip",
        "Notepad++.Notepad++",
        "Microsoft.VisualStudioCode",
    ] {
        assert!(validate_winget_id(id).is_ok());
    }
}
#[test]
fn plans_only_missing_packages() {
    let p = profile();
    let installed = vec![Package {
        reference: reference("7ZIP.7ZIP"),
        name: "7zip".into(),
        version: "1".into(),
        available: String::new(),
        source: "winget".into(),
        manageable: true,
    }];
    let plan = p.plan(&installed, true).unwrap();
    assert_eq!(plan.skipped.len(), 1);
    assert_eq!(plan.missing.len(), 3);
    assert!(!plan.missing.iter().any(|p| p.id == "7zip.7zip"));
}
#[test]
fn plan_checks_provider_availability() {
    assert!(profile().plan(&[], false).is_err());
    let mut p = profile();
    p.packages = vec![PackageRef {
        provider: ProviderId::Github,
        id: "owner/repo".into(),
    }];
    assert_eq!(p.plan(&[], false).unwrap().missing.len(), 1);
}
#[test]
fn empty_profile_is_valid() {
    let mut p = profile();
    p.packages.clear();
    assert!(p.validate().is_ok());
    assert!(p.plan(&[], false).unwrap().missing.is_empty());
}
#[test]
fn bom_import_is_supported() {
    assert!(
        Profile::parse(&format!(
            "\u{feff}{}",
            include_str!("../assets/essentials.json")
        ))
        .is_ok()
    );
}
#[test]
fn profile_names_are_unique() {
    let mut profiles = vec![profile()];
    assert!(profiles::save(&mut profiles, profile(), None).is_err());
    assert!(profiles::save(&mut profiles, profile(), Some(0)).is_ok());
}
#[test]
fn state_machine_rejects_terminal_transitions() {
    use OperationState::*;
    assert_eq!(Pending.transition(Running).unwrap(), Running);
    assert!(Running.transition(Completed).is_ok());
    assert!(Pending.transition(Cancelled).is_ok());
    assert!(Completed.transition(Running).is_err());
    assert!(Failed.transition(Completed).is_err());
    assert!(Pending.transition(Completed).is_err());
}
#[test]
fn winget_args_are_exact_and_shell_free() {
    let args = mutation_args(Action::Install, "Notepad++.Notepad++", false, true).unwrap();
    assert_eq!(
        &args[..6],
        &[
            "install",
            "--id",
            "Notepad++.Notepad++",
            "--exact",
            "--source",
            "winget"
        ]
    );
    assert!(args.contains(&"--no-upgrade".into()));
    assert!(!args.contains(&"--silent".into()));
    assert!(!args.contains(&"--allow-reboot".into()));
    assert!(mutation_args(Action::Install, "x & calc", true, true).is_err());
}
#[test]
fn uninstall_avoids_unsupported_package_agreements() {
    let a = mutation_args(Action::Uninstall, "7zip.7zip", true, true).unwrap();
    assert!(a.contains(&"--silent".into()));
    assert!(!a.contains(&"--accept-package-agreements".into()));
    assert!(!a.contains(&"--no-upgrade".into()));
}
#[test]
fn unsupported_actions_are_explicit() {
    assert!(mutation_args(Action::Download, "7zip.7zip", false, false).is_err());
    let github = winit::providers::github::GitHubReleaseProvider::new().unwrap();
    assert!(!github.capabilities().actions.contains(&Action::Install));
}
#[test]
fn parses_portuguese_table_with_multiword_versions() {
    let text = "Nome                     ID                    Versão          Disponível\n------------------------------------------------------------------------------\nMeu Aplicativo           Owner.App             > 8.1.2         9.2\nOutro                    Other.App             Unknown         \n2 atualizações disponíveis.\n";
    let p = parse_table(text, TableKind::Installed).unwrap();
    assert_eq!(p.len(), 2);
    assert_eq!(p[0].reference.id, "Owner.App");
    assert_eq!(p[0].version, "> 8.1.2");
    assert_eq!(p[0].available, "9.2");
}
#[test]
fn parses_unicode_and_ansi_without_byte_slicing() {
    let text = "\u{1b}[?25lName                 Id             Version\n------------------------------------------------\n编辑器               Owner.Editor   1.0\nCafé                 Other.Cafe     2.0\n";
    let p = parse_table(text, TableKind::Search).unwrap();
    assert_eq!(p.len(), 2);
    assert_eq!(p[0].name, "编辑器");
    assert_eq!(p[1].reference.id, "Other.Cafe");
}
#[test]
fn parser_fails_closed_on_changed_format() {
    assert!(parse_table("{\"new_output\":true}", TableKind::Search).is_err());
}
#[test]
fn no_results_exit_codes_are_recognized() {
    assert!(is_empty_result(Some(0x8A150014u32 as i32)));
    assert!(!is_empty_result(Some(1)));
}
#[test]
fn github_prefers_explicit_windows_x64_exe() {
    let assets = vec![
        asset("app-arm64.exe"),
        asset("app-linux-x64.zip"),
        asset("app-win-x64.zip"),
        asset("app-win-x64.exe"),
    ];
    assert_eq!(select_asset(&assets).unwrap().name, "app-win-x64.exe");
}
#[test]
fn github_rejects_scripts_and_other_platforms() {
    for name in [
        "app-arm64.exe",
        "setup.ps1",
        "app-x86.exe",
        "app-linux-x64.zip",
        "app-macos.zip",
    ] {
        assert!(!compatible(&asset(name)), "{name}");
    }
}
#[test]
fn github_handles_releases_without_installers() {
    assert!(select_asset(&[asset("source.tar.gz"), asset("checksums.txt")]).is_none());
}
#[test]
fn rejects_unsafe_filenames() {
    for name in [
        "../evil.exe",
        "..\\evil.exe",
        "file:ads.exe",
        "CON.exe",
        "evil.exe.",
        "evil.exe ",
        "x\n.exe",
    ] {
        assert!(!asset_safe_name(name), "{name}");
    }
}
#[test]
fn restricts_asset_to_configured_repository() {
    let mut a = asset("app.exe");
    assert!(validate_asset_url("owner/repo", &a).is_ok());
    a.browser_download_url = "https://evil.example/app.exe".into();
    assert!(validate_asset_url("owner/repo", &a).is_err());
    a.browser_download_url = "http://github.com/owner/repo/releases/download/v1/app.exe".into();
    assert!(validate_asset_url("owner/repo", &a).is_err());
}
#[test]
fn validates_repo_instead_of_accepting_urls() {
    for repo in [
        "https://github.com/o/r",
        "o/r/extra",
        "../repo",
        "owner/..",
        "o/",
        "o/r?token=x",
    ] {
        assert!(validate_repo(repo).is_err());
    }
    assert!(validate_repo("itsfatduck/optimizerDuck").is_ok());
}
#[test]
fn atomic_storage_replaces_and_recovers_interrupted_operations() {
    let root = std::env::temp_dir().join(format!("winit-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let store = Storage {
        root: root.clone(),
        portable: true,
    };
    let mut data = SavedData::default();
    store.save(&data).unwrap();
    data.settings.dark_theme = false;
    store.save(&data).unwrap();
    assert!(!store.load().unwrap().settings.dark_theme);
    assert!(root.join("state.backup.json").exists());
    atomic_write(&root.join("state.json"), b"invalid").unwrap();
    assert!(store.load().is_err());
    std::fs::remove_file(root.join("state.json")).unwrap();
    std::fs::remove_file(root.join("state.backup.json")).unwrap();
    std::fs::remove_dir(root).unwrap();
}
