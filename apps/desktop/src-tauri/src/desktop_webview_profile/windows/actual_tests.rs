use super::*;

#[test]
fn original_manager_navigation_remains_sealed_until_actual_profile_is_verified() {
    let navigation = Navigation::default();
    let original = Url::parse("http://127.0.0.1:1420/original?keep=1").unwrap();
    assert!(!navigation.allows(&original));
    assert!(!navigation.allows(&Url::parse("https://example.com/").unwrap()));
    navigation.activate();
    assert!(navigation.allows(&original));
    assert!(!navigation.allows(&Url::parse("http://127.0.0.1:49152/").unwrap()));
}

#[test]
fn expected_and_actual_directory_comparison_uses_real_complete_file_identity() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-manager-udf-identity-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let profile = profile::ManagerProfile::initialize(&root).unwrap();
    profile
        .verify_actual_directory(profile.directory())
        .unwrap();
    let foreign = root.join("foreign-directory");
    std::fs::create_dir(&foreign).unwrap();
    std::fs::write(foreign.join("sentinel"), b"retain foreign directory").unwrap();
    assert_eq!(
        profile.verify_actual_directory(&foreign).unwrap_err().code,
        "MANAGER_PROFILE_ACTUAL_MISMATCH"
    );
    assert_eq!(
        std::fs::read(foreign.join("sentinel")).unwrap(),
        b"retain foreign directory"
    );
    assert_eq!(
        profile
            .verify_actual_directory(&root.join("missing"))
            .unwrap_err()
            .code,
        "MANAGER_PROFILE_ACTUAL_UNKNOWN"
    );
    assert!(!root.join("missing").exists());
}

#[test]
fn production_frontend_directory_must_not_be_parsed_as_a_windows_drive_url() {
    use tauri::utils::config::FrontendDist;

    let relative = "../../../target/production-windows/candidate-test/stage/frontend";
    let configured: FrontendDist = serde_json::from_value(serde_json::json!(relative)).unwrap();
    match configured {
        FrontendDist::Directory(path) => assert_eq!(path, PathBuf::from(relative)),
        other => panic!("Bundled production frontend must be a directory: {other:?}"),
    }

    // FrontendDist is untagged and tries Url first. A Windows drive letter is
    // a valid URL scheme, so this old candidate spelling embeds no frontend
    // assets and must remain outside the manager's privileged origin policy.
    let absolute = r"D:\dev\postgresUI\target\production-windows\candidate-test\stage\frontend";
    let configured: FrontendDist = serde_json::from_value(serde_json::json!(absolute)).unwrap();
    match configured {
        FrontendDist::Url(url) => {
            assert_eq!(url.scheme(), "d");
            assert!(!crate::desktop_webview_profile::is_manager_url(&url, false));
            assert!(!crate::desktop_webview_profile::is_manager_url(&url, true));
        }
        other => panic!("Record the locked Tauri drive-path ambiguity: {other:?}"),
    }
}
