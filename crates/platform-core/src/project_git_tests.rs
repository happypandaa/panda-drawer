use super::*;
use std::os::unix::fs::symlink;

#[test]
fn version_parser_never_truncates_or_accepts_malformed_commit_metadata() {
    let row = format!(
        "{}\0\0作者\0author@localhost\u{0}2026-10-22T01:00:00+08:00\0A local source version\0",
        "a".repeat(40)
    );
    let parsed = parse_versions(row.as_bytes(), 2).unwrap();
    assert_eq!(parsed.len(), 1);
    assert!(parsed[0].parents.is_empty());
    assert_eq!(parsed[0].author.name, "作者");
    assert!(parse_versions(row.trim_end_matches('\0').as_bytes(), 2).is_err());
    assert!(parse_versions(row.as_bytes(), 0).is_err());
    assert!(parse_versions(format!("{row}{row}").as_bytes(), 2).is_err());
    for malformed in [
        row.replace("A local source version", "unexpected\ncontrol"),
        row.replace("A local source version", &"x".repeat(4001)),
        row.replace("author@localhost", &"a".repeat(255)),
        row.replace("2026-10-22T01:00:00+08:00", "not-a-date"),
        row.replacen(&"a".repeat(40), "HEAD", 1),
        row.replace("\0\0作者", "\0bad-parent\0作者"),
    ] {
        assert_eq!(
            parse_versions(malformed.as_bytes(), 2).unwrap_err().code,
            "PROJECT_GIT_OUTPUT_INVALID"
        );
    }
    assert!(parse_versions(&[0xff, 0], 1).is_err());
}

#[tokio::test]
async fn existing_source_only_repository_is_reported_and_never_wrapped_or_changed() {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap();
    let created = store
        .create(crate::projects::CreateProjectInput {
            name: "Disposable source-only Git marker".into(),
            description: None,
        })
        .await
        .unwrap();
    let project = store.project(&created.id).unwrap();
    private_dir(&project.root.join("app/.git")).unwrap();
    fs::write(
        project.root.join("app/.git/HEAD"),
        "ref: refs/heads/existing\n",
    )
    .unwrap();
    let (_stop, stop) = watch::channel(false);
    let (_cancel, cancel) = watch::channel(false);
    let mut context = Context::new(stop, cancel).unwrap();
    let status = inspect(&project, None, &mut context).await.unwrap().view;
    assert_eq!(status.repository.state, "source-only");
    assert!(status.repository.head.is_none());
    assert!(!status.can_initialize);
    assert!(!status.can_save);
    assert!(!project.root.join(".git").exists());
    assert_eq!(
        fs::read(project.root.join("app/.git/HEAD")).unwrap(),
        b"ref: refs/heads/existing\n"
    );
    store.shutdown().await;
}

#[tokio::test]
async fn status_reserves_transient_operation_until_stopped_even_while_waiting_for_project_gate() {
    preview_lease_test(false).await;
}
#[tokio::test]
async fn versions_reserves_transient_operation_and_stops_while_waiting_for_project_gate() {
    preview_lease_test(true).await;
}
async fn preview_lease_test(versions: bool) {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
    let created = store
        .create(crate::projects::CreateProjectInput {
            name: "Disposable Git preview migration lease".into(),
            description: None,
        })
        .await
        .unwrap();
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into()).unwrap();
    let git = GitEnvironmentManager::new(paths.data_root().to_path_buf(), None);
    let manager = ProjectGitManager::new(paths, store.clone(), tasks.clone(), git).unwrap();
    let gate = store.gate(&created.id);
    let guard = gate.write().await;
    let (stop, receiver) = watch::channel(false);
    let reader = manager.clone();
    let id = created.id.clone();
    let pending = tokio::spawn(async move {
        if versions {
            reader.versions(&id, 50, receiver).await.map(|_| ())
        } else {
            reader.status(&id, receiver).await.map(|_| ())
        }
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !store.has_active_tasks() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        store.active_task(&created.id).is_none(),
        "preview is not a persistent task"
    );
    assert_eq!(
        store
            .reserve_task(&created.id, "another-operation")
            .err()
            .unwrap()
            .code,
        "PROJECT_BUSY"
    );
    store.begin_workspace_migration().unwrap();
    assert!(
        store.has_active_tasks(),
        "migration must wait before pause/copy"
    );
    stop.send(true).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(3), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "PROJECT_GIT_INTERRUPTED");
    assert!(
        !store.has_active_tasks(),
        "stop releases the transient lease without needing the gate"
    );
    let (_second_stop, second_receiver) = watch::channel(false);
    assert_eq!(
        manager
            .status(&created.id, second_receiver)
            .await
            .unwrap_err()
            .code,
        "WORKSPACE_MIGRATING"
    );
    store.resume_workspace();
    drop(guard);
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

#[test]
fn repository_scan_blocks_external_links_and_preserves_existing_locks() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    private_dir(&root.join("objects/info")).unwrap();
    fs::write(root.join("objects/info/alternates"), "do not follow me").unwrap();
    fs::write(root.join("index.lock"), "user lock").unwrap();
    let mut issues = vec![];
    inspect_repository_files(&root, &mut issues).unwrap();
    assert!(issues
        .iter()
        .any(|error| error.code == "PROJECT_GIT_REPOSITORY_UNSUPPORTED"));
    assert!(issues
        .iter()
        .any(|error| error.code == "PROJECT_GIT_LOCKED"));
    symlink(root.join("outside"), root.join("config")).unwrap();
    assert_eq!(
        inspect_repository_files(&root, &mut vec![])
            .unwrap_err()
            .code,
        "PROJECT_GIT_UNSAFE_PATH"
    );
    assert_eq!(fs::read(root.join("index.lock")).unwrap(), b"user lock");
}
#[tokio::test]
async fn stopped_command_kills_descendants_and_never_returns_sensitive_stderr() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let marker = root.join("ticks");
    let script = root.join("probe.sh");
    fs::write(&script,format!("#!/bin/sh\nwhile true; do printf x >> '{}'; /bin/sleep 0.02; done &\necho SENSITIVE_TEST_STDERR >&2\nwait\n",marker.display())).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let (stop, receiver) = watch::channel(false);
    let (_cancel, cancel) = watch::channel(false);
    let mut context = Context::new(receiver, cancel).unwrap();
    let tools = GitToolchain {
        git: ExecutableInfo {
            path: script,
            version: "2.56.0".into(),
        },
        path: "/usr/bin:/bin".into(),
        exec_path: None,
        template_path: None,
    };
    let command = git_command(&tools, context.root(), None);
    let task = tokio::spawn(async move { context.run(command, Vec::new()).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        while !marker.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    stop.send(true).unwrap();
    let error = task.await.unwrap().err().unwrap();
    assert_eq!(error.code, "PROJECT_GIT_INTERRUPTED");
    assert!(!error.message.contains("SENSITIVE"));
    let bytes = fs::read(&marker).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(fs::read(marker).unwrap(), bytes);
}
