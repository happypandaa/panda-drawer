use super::*;
use std::os::unix::fs::symlink;

async fn fixture() -> (TempDir, RuntimePaths, ProjectHandle) {
    let (temp, paths) = RuntimePaths::for_test();
    let store = ProjectStore::new(paths.workspace_root().into()).unwrap();
    let created = store
        .create(crate::CreateProjectInput {
            name: "Disposable build Git gate".into(),
            description: None,
        })
        .await
        .unwrap();
    (temp, paths, store.project(&created.id).unwrap())
}
fn repository(root: &Path) -> PathBuf {
    let path = root.join(".git");
    fs::create_dir_all(path.join("objects")).unwrap();
    fs::create_dir_all(path.join("refs")).unwrap();
    fs::write(path.join("HEAD"), "ref: refs/heads/main\n").unwrap();
    fs::write(path.join("config"), "[core]\nrepositoryformatversion=0\n").unwrap();
    path
}
fn synthetic(paths: &RuntimePaths, behavior: &str) -> GitEnvironmentManager {
    let script = paths.data_root().join("synthetic-build-git");
    fs::write(&script, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'git version 2.56.0'; exit; fi\nfor arg do\ncase \"$arg\" in config) exit 1;; ls-files) {behavior};; esac\ndone\nexit 98\n")).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        paths.data_root().join("git-toolchain.json"),
        serde_json::to_vec(&json!({"schemaVersion":1,"gitPath":script})).unwrap(),
    )
    .unwrap();
    GitEnvironmentManager::new(paths.data_root().into(), None)
}
#[tokio::test]
async fn no_repository_does_not_read_invalid_git_choice_or_probe_missing_managed_git() {
    let (_temp, paths, project) = fixture().await;
    fs::write(
        paths.data_root().join("git-toolchain.json"),
        "malformed settings",
    )
    .unwrap();
    let environment = GitEnvironmentManager::new(
        paths.data_root().into(),
        Some(paths.data_root().join("missing-managed-git")),
    );
    let (_stop, receiver) = watch::channel(false);
    let guard = BuildGitGuard::capture(&project, &environment, receiver.clone())
        .await
        .unwrap();
    guard.recheck(&project, receiver.clone()).await.unwrap();
    repository(&project.root);
    assert_eq!(
        guard.recheck(&project, receiver).await.unwrap_err().code,
        "BUILD_GIT_STATE_CHANGED"
    );
}
#[tokio::test]
async fn unfinished_operations_are_reported_before_tool_resolution_without_opening_marker() {
    let (_temp, paths, project) = fixture().await;
    let git = repository(&project.root);
    let environment = GitEnvironmentManager::new(paths.data_root().into(), None);
    let (_stop, receiver) = watch::channel(false);
    for name in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ] {
        // Even a link is inspected by name only, never followed.
        symlink(paths.data_root().join("must-not-read"), git.join(name)).unwrap();
        let result = BuildGitGuard::capture(&project, &environment, receiver.clone()).await;
        assert_eq!(
            result.err().unwrap().code,
            "BUILD_GIT_OPERATION_IN_PROGRESS",
            "{name}"
        );
        fs::remove_file(git.join(name)).unwrap();
    }
}
#[tokio::test]
async fn marked_project_never_falls_back_from_broken_explicit_selection() {
    let (_temp, paths, project) = fixture().await;
    repository(&project.root);
    fs::write(
        paths.data_root().join("git-toolchain.json"),
        serde_json::to_vec(
            &json!({"schemaVersion":1,"gitPath":paths.data_root().join("lost-explicit-git")}),
        )
        .unwrap(),
    )
    .unwrap();
    let environment = GitEnvironmentManager::new(
        paths.data_root().into(),
        Some(paths.data_root().join("also-absent-managed")),
    );
    let (_stop, receiver) = watch::channel(false);
    let error = BuildGitGuard::capture(&project, &environment, receiver)
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, "BUILD_GIT_CHECK_UNAVAILABLE");
    assert_eq!(
        error.details["causeCode"],
        "PROJECT_GIT_ENVIRONMENT_UNAVAILABLE"
    );
}
#[tokio::test]
async fn source_repository_is_supported_but_gitfiles_links_ancestors_and_nested_repositories_are_not(
) {
    let (_temp, paths, project) = fixture().await;
    let app = project.root.join("app");
    let git = repository(&app);
    let environment = synthetic(&paths, "exit 0");
    let (_stop, receiver) = watch::channel(false);
    let guard = BuildGitGuard::capture(&project, &environment, receiver.clone())
        .await
        .unwrap();
    fs::write(git.join("index.lock"), "another Git writer").unwrap();
    assert_eq!(
        guard
            .recheck(&project, receiver.clone())
            .await
            .unwrap_err()
            .code,
        "BUILD_GIT_CHECK_UNAVAILABLE"
    );
    fs::remove_file(git.join("index.lock")).unwrap();
    fs::remove_dir_all(&git).unwrap();
    fs::write(&git, "gitdir: ../must-not-open").unwrap();
    assert_eq!(
        discover(&project).unwrap_err().code,
        "BUILD_GIT_CHECK_UNAVAILABLE"
    );
    fs::remove_file(&git).unwrap();
    symlink(paths.data_root().join("outside"), &git).unwrap();
    assert_eq!(
        discover(&project).unwrap_err().code,
        "BUILD_GIT_CHECK_UNAVAILABLE"
    );
    fs::remove_file(&git).unwrap();
    repository(&app.join("src"));
    assert_eq!(
        discover(&project).unwrap_err().code,
        "BUILD_GIT_CHECK_UNAVAILABLE"
    );
    fs::remove_dir_all(app.join("src/.git")).unwrap();
    repository(project.root.parent().unwrap());
    assert_eq!(
        discover(&project).unwrap_err().code,
        "BUILD_GIT_CHECK_UNAVAILABLE"
    );
}
#[tokio::test]
async fn index_change_is_detected_and_shutdown_reaps_stuck_git_process() {
    let (_temp, paths, project) = fixture().await;
    let git = repository(&project.root);
    let environment = synthetic(&paths, "exit 0");
    let (stop, receiver) = watch::channel(false);
    let guard = BuildGitGuard::capture(&project, &environment, receiver.clone())
        .await
        .unwrap();
    fs::write(git.join("index"), "different index bytes").unwrap();
    assert_eq!(
        guard
            .recheck(&project, receiver.clone())
            .await
            .unwrap_err()
            .code,
        "BUILD_GIT_STATE_CHANGED"
    );
    let pid_path = paths.data_root().join("stuck-git-pid");
    let environment = synthetic(
        &paths,
        &format!(
            "echo $$ > '{}'; sleep 60 & wait; exit 0",
            pid_path.display()
        ),
    );
    let future = BuildGitGuard::capture(&project, &environment, receiver);
    tokio::pin!(future);
    tokio::select! {
        result = &mut future => panic!("unexpected completion: {:?}", result.err()),
        _ = tokio::time::timeout(Duration::from_secs(5), async { while !pid_path.exists() { tokio::time::sleep(Duration::from_millis(10)).await; } }) => {}
    }
    let pid: i32 = fs::read_to_string(pid_path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    stop.send_replace(true);
    let error = tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .unwrap()
        .err()
        .unwrap();
    assert_eq!(error.code, "BUILD_INTERRUPTED");
    assert_eq!(
        unsafe { libc::kill(-pid, 0) },
        -1,
        "probe descendants must be gone"
    );
}
#[test]
fn unmerged_output_requires_complete_stage_records_and_never_exposes_paths() {
    let valid = format!("100644 {} 2\tdocs/conflict.md\0", "a".repeat(40));
    assert!(validate_unmerged(valid.as_bytes()).is_ok());
    for bad in [
        "arbitrary\0".to_owned(),
        valid.replace(" 2\t", " 0\t"),
        valid.replace('\0', ""),
    ] {
        assert!(validate_unmerged(bad.as_bytes()).is_err());
    }
}

#[tokio::test]
async fn unversioned_deep_source_and_dependency_git_metadata_add_no_new_build_requirement() {
    let (_temp, paths, project) = fixture().await;
    let mut deep = project.root.join("app");
    for _ in 0..80 {
        deep.push("d");
    }
    fs::create_dir_all(deep).unwrap();
    let dependency = project.root.join("app/node_modules/dependency");
    fs::create_dir_all(&dependency).unwrap();
    symlink(
        paths.data_root().join("never-read"),
        dependency.join(".git"),
    )
    .unwrap();
    fs::write(paths.data_root().join("git-toolchain.json"), "invalid").unwrap();
    let environment = GitEnvironmentManager::new(paths.data_root().into(), None);
    let (_stop, receiver) = watch::channel(false);
    assert!(BuildGitGuard::capture(&project, &environment, receiver)
        .await
        .is_ok());
}
#[tokio::test]
async fn opaque_tool_failure_never_becomes_a_success_or_discloses_stderr() {
    let (_temp, paths, project) = fixture().await;
    repository(&project.root);
    let environment = synthetic(&paths, "echo credential-like-private-error >&2; exit 79");
    let (_stop, receiver) = watch::channel(false);
    let error = BuildGitGuard::capture(&project, &environment, receiver)
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, "BUILD_GIT_CHECK_UNAVAILABLE");
    assert!(!serde_json::to_string(&error)
        .unwrap()
        .contains("credential-like-private-error"));
}

#[tokio::test]
async fn intermediate_repository_above_multilevel_source_is_never_treated_as_unversioned() {
    let (_temp, paths, mut project) = fixture().await;
    fs::create_dir(project.root.join("client")).unwrap();
    fs::rename(project.root.join("app"), project.root.join("client/app")).unwrap();
    project.manifest.source_dir = "client/app".into();
    crate::projects::validate_manifest(&project.manifest, &project.manifest.id, &project.root)
        .unwrap();
    let git = repository(&project.root.join("client"));
    fs::write(
        git.join("MERGE_HEAD"),
        "ongoing merge must not be overlooked",
    )
    .unwrap();
    let environment = GitEnvironmentManager::new(paths.data_root().into(), None);
    let (_stop, receiver) = watch::channel(false);
    let error = BuildGitGuard::capture(&project, &environment, receiver)
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, "BUILD_GIT_CHECK_UNAVAILABLE");
    assert!(
        !error.details.to_string().contains("ENVIRONMENT"),
        "reject unsupported scope before tool resolution"
    );
    assert_eq!(
        fs::read(git.join("MERGE_HEAD")).unwrap(),
        b"ongoing merge must not be overlooked"
    );
}
