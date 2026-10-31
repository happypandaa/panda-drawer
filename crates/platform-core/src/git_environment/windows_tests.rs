use super::*;
use platform_native::windows::{process::resolve_environment, system_paths::SystemPaths};
use std::ffi::OsString;

#[test]
fn offline_probe_uses_actual_system_paths_native_path_separator_and_explicit_environment() {
    let system = SystemPaths::current().unwrap();
    let executable = Path::new(r"C:\工具 中文 space\cmd\git.exe");
    let path = tool_path(executable).unwrap();
    assert_eq!(
        std::env::split_paths(&path).collect::<Vec<_>>(),
        vec![
            executable.parent().unwrap().to_path_buf(),
            system.system.clone(),
            system.windows.clone()
        ]
    );
    let command = isolated_command(executable, &path).unwrap();
    let command = command.as_std();
    assert_eq!(command.get_current_dir(), Some(system.system.as_path()));
    let changes = command
        .get_envs()
        .map(|(k, v)| (k.to_owned(), v.map(OsString::from)))
        .collect::<Vec<_>>();
    let environment = resolve_environment(false, &changes).unwrap();
    let get = |key: &str| {
        environment
            .iter()
            .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone())
    };
    assert_eq!(get("SystemRoot"), Some(system.windows.into_os_string()));
    assert_eq!(get("HOME"), Some("NUL".into()));
    assert_eq!(get("GIT_CONFIG_GLOBAL"), Some("NUL".into()));
    assert_eq!(get("GIT_TERMINAL_PROMPT"), Some("0".into()));
    assert!(get("PANDA_GIT_MUST_NOT_INHERIT").is_none());
}

#[tokio::test]
async fn missing_private_git_never_discovers_a_host_git_or_creates_settings() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w4-git-no-discovery-")
        .tempdir()
        .unwrap();
    let manager = GitEnvironmentManager::new(temporary.path().to_path_buf(), None);
    assert!(manager.resolve().await.is_err());
    let info = manager.doctor().await;
    assert!(!info.ready);
    assert!(info.git.is_none());
    assert_eq!(info.authentication.state, "not-checked");
    assert!(!temporary.path().join("git-toolchain.json").exists());
}

#[test]
fn windows_vendor_version_is_identification_only_and_does_not_discard_the_suffix() {
    assert_eq!(
        git_version(b"git version 2.56.0.windows.4\n").unwrap(),
        "2.56.0.windows.4"
    );
    assert!(git_version(b"not git\n").is_err());
}

#[tokio::test]
#[ignore = "Requires PANDA_W4_GIT_SAMPLE_ROOT naming a verified disposable official MinGit copy"]
async fn official_windows_mingit_is_verified_probed_and_selected_without_mutating_its_tree() {
    let sample = PathBuf::from(
        std::env::var_os("PANDA_W4_GIT_SAMPLE_ROOT").expect("explicit disposable MinGit sample"),
    );
    assert!(sample.is_absolute());
    assert!(sample.components().any(|c| c
        .as_os_str()
        .to_string_lossy()
        .starts_with("disposable-w4-git-")));
    let before = crate::managed_git::verify(sample.clone()).await.unwrap();
    let manifest = std::fs::read(sample.join("development-git.json")).unwrap();
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w4-git-settings-")
        .tempdir()
        .unwrap();
    let processes = OwnedProcessRegistry::default();
    let manager = GitEnvironmentManager::with_processes(
        temporary.path().to_path_buf(),
        Some(sample.clone()),
        processes.clone(),
    );
    let info = manager.doctor().await;
    assert!(info.ready, "{:?}", info.issues);
    assert_eq!(info.selection, "managed");
    assert_eq!(info.authentication.state, "not-checked");
    assert_eq!(info.git.unwrap().version, "2.56.0.windows.4");
    let resolved = manager.resolve().await.unwrap();
    assert_eq!(resolved.exec_path, Some(before.exec_path));
    assert_eq!(resolved.template_path, Some(before.template_path));
    assert_eq!(resolved.git.path, before.git);
    assert!(!temporary.path().join("git-toolchain.json").exists());
    let explicit = manager
        .configure_git(Some(resolved.git.path))
        .await
        .unwrap();
    assert!(explicit.ready, "{:?}", explicit.issues);
    assert_eq!(explicit.selection, "explicit");
    crate::managed_git::verify(sample.clone()).await.unwrap();
    assert_eq!(
        std::fs::read(sample.join("development-git.json")).unwrap(),
        manifest
    );
    assert!(processes.shutdown().await.is_empty());
    assert!(processes
        .snapshot()
        .iter()
        .all(|s| s.phase == crate::owned_process::ProcessPhase::Closed));
}
