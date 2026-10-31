use super::*;
use platform_native::windows::process::resolve_environment;

#[test]
fn windows_drive_cwd_and_verified_vendor_versions_use_native_policy() {
    assert!(valid_cwd(Path::new(r"C:\临时 中文 space\remote")));
    assert!(!valid_cwd(Path::new("C:\\bad\npath")));
    assert!(supported_version("2.56.0.windows.4"));
    for invalid in [
        "2.56.0.windows.",
        "2.56.0.windows.4 extra",
        "2.56.1.windows.4",
        "2.56.0.rc1",
    ] {
        assert!(!supported_version(invalid));
    }
}

#[test]
fn remote_command_explicitly_blocks_host_configuration_and_prompts() {
    let tools = GitToolchain {
        git: ExecutableInfo {
            path: r"C:\工具 中文 space\cmd\git.exe".into(),
            version: "2.56.0.windows.4".into(),
        },
        path: r"C:\工具 中文 space\cmd;C:\Windows\System32".into(),
        exec_path: Some(r"C:\工具 中文 space\ucrt64\libexec\git-core".into()),
        template_path: None,
    };
    let cwd = Path::new(r"C:\临时 中文 space\remote");
    let command = command(&tools, cwd).unwrap();
    let standard = command.as_std();
    assert_eq!(standard.get_current_dir(), Some(cwd));
    let environment = resolve_environment(
        false,
        &standard
            .get_envs()
            .map(|(k, v)| (k.to_owned(), v.map(ToOwned::to_owned)))
            .collect::<Vec<_>>(),
    )
    .unwrap();
    let value = |key: &str| {
        environment
            .iter()
            .find(|(k, _)| k.to_string_lossy().eq_ignore_ascii_case(key))
            .map(|(_, v)| v.to_string_lossy().into_owned())
    };
    assert_eq!(value("GIT_CONFIG_SYSTEM").as_deref(), Some("NUL"));
    assert_eq!(value("GIT_ALLOW_PROTOCOL").as_deref(), Some("https"));
    assert_eq!(value("GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
    assert!(value("SystemRoot").is_some());
    assert_eq!(value("TEMP").as_deref(), cwd.to_str());
    let arguments = standard
        .get_args()
        .map(|v| v.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(arguments.iter().any(|a| a == "credential.helper="));
    assert!(arguments.iter().any(|a| a == "core.longpaths=true"));
    assert!(arguments.iter().any(|a| a == "http.sslVerify=true"));
}

#[tokio::test]
#[ignore = "Explicit PANDA_W4_GIT_SAMPLE_ROOT and public GitHub network access required"]
async fn official_mingit_reads_public_github_head_and_closes_actual_job() {
    let sample = std::path::PathBuf::from(
        std::env::var_os("PANDA_W4_GIT_SAMPLE_ROOT").expect("explicit official MinGit sample"),
    );
    assert!(sample.components().any(|c| c
        .as_os_str()
        .to_string_lossy()
        .starts_with("disposable-w4-git-")));
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w4-git-remote-")
        .tempdir()
        .unwrap();
    let processes = OwnedProcessRegistry::default();
    let environment = GitEnvironmentManager::with_processes(
        temporary.path().into(),
        Some(sample.clone()),
        processes.clone(),
    );
    let checker = GitRemoteChecker::with_processes(processes.clone());
    let (_stop, rx) = watch::channel(false);
    let result = checker
        .check(
            &environment,
            GitRemoteCheckInput {
                url: "https://github.com/happypandaa/panda-drawer-demo-node.git".into(),
            },
            rx,
        )
        .await
        .unwrap();
    assert_eq!(result.state, "readable");
    assert_eq!(result.provider, "github");
    assert_eq!(result.transport, "public-https");
    let head = result.head.unwrap();
    assert_eq!(
        head.symbolic_ref.as_deref(),
        Some("refs/heads/codex/manual-demo")
    );
    assert!(valid_oid_for_test(&head.oid));
    assert!(processes.shutdown().await.is_empty());
    assert!(processes
        .snapshot()
        .iter()
        .all(|s| s.phase == crate::owned_process::ProcessPhase::Closed));
    crate::managed_git::verify(sample).await.unwrap();
    assert!(!temporary.path().join("git-toolchain.json").exists());
}

fn valid_oid_for_test(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
