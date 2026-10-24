use super::*;
use std::os::unix::fs::symlink;

#[test]
fn branch_names_are_bounded_and_inputs_require_exact_preview_fields() {
    for name in ["custom", "custom/feature-1", "a_b.c", "1"] {
        assert!(validate_branch_name(name).is_ok(), "{name}");
    }
    for name in [
        "", "HEAD", "-option", ".hidden", "a//b", "a/", "a.", "a..b", "a/.b", "a/b.lock", "a@{b",
        "中文", "a\nb",
    ] {
        assert!(validate_branch_name(name).is_err(), "{name}");
    }
    assert!(validate_branch_name(&"a".repeat(129)).is_err());
    let value = json!({"name":"custom","expectedHead":"a".repeat(40),"expectedBranch":"refs/heads/main","expectedSnapshot":"b".repeat(64)});
    assert!(serde_json::from_value::<GitCreateBranchInput>(value.clone()).is_ok());
    for field in ["name", "expectedHead", "expectedBranch", "expectedSnapshot"] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<GitCreateBranchInput>(missing).is_err());
    }
    let mut extra = value;
    extra["checkout"] = json!(true);
    assert!(serde_json::from_value::<GitCreateBranchInput>(extra).is_err());
    assert!(branch_version_supported("2.56.0"));
    for version in ["2.54.0", "2.54.0 (Apple Git-154)", "2.56.1", "2.57.0"] {
        assert!(!branch_version_supported(version));
    }
    assert!(supported_version("2.54.0 (Apple Git-154)"));
}

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

#[test]
fn old_branch_lock_never_overwrites_or_removes_another_writers_lock() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("refs/heads/nested/main.lock");
    let lock = RefLock::acquire(&path).unwrap();
    assert!(RefLock::acquire(&path).is_err());
    let original = path.with_extension("moved");
    fs::rename(&path, original).unwrap();
    fs::write(&path, b"another writer").unwrap();
    drop(lock);
    assert_eq!(fs::read(&path).unwrap(), b"another writer");
}

#[test]
fn save_requires_explicit_nullable_head_and_valid_snapshot() {
    let base = json!({"expectedSnapshot":"a".repeat(64),"message":"save","author":{"name":"Local Projects","email":"local-projects@localhost"}});
    assert!(serde_json::from_value::<GitSaveInput>(base.clone()).is_err());
    let mut supplied = base;
    supplied["expectedHead"] = Value::Null;
    assert!(serde_json::from_value::<GitSaveInput>(supplied.clone()).is_ok());
    supplied["unexpected"] = json!(true);
    assert!(serde_json::from_value::<GitSaveInput>(supplied).is_err());
    assert!(validate_digest(&"a".repeat(64)).is_ok());
    assert!(validate_digest("HEAD").is_err());
}
fn manifest() -> ProjectManifest {
    serde_json::from_value(json!({"manifestVersion":1,"id":"test","name":"test","description":"","sdkApiVersion":1,"sourceDir":"app","outputDir":"dist","entry":"dist/index.html","database":"data/main.sqlite","attachmentsDir":"data/attachments","build":{"script":"build"},"docs":{"readme":"README.md","database":"docs/database.md","platform":"docs/platform.md"}})).unwrap()
}
#[test]
fn scope_includes_generated_instructions_and_metadata_but_never_live_data_or_credentials() {
    let manifest = manifest();
    let roots = included_roots(&manifest);
    for path in [
        "project.json",
        "app/src/main.ts",
        "app/vendor/sdk.tgz",
        "AGENTS.md",
        "CLAUDE.md",
        ".agents/skills/project/SKILL.md",
        "meta/icons/icon.png",
        "docs/custom.md",
    ] {
        assert!(included(path, &roots), "{path}");
        assert!(protected(path, &manifest).is_none(), "{path}");
    }
    for path in [
        "data/main.sqlite",
        "data/main.sqlite-wal",
        "data/attachments/photo.png",
        ".platform/tasks/record",
        "dist/index.html",
        "app/node_modules/package/x.js",
        "app/.env.local",
        "app/credentials/token",
        "app/.npmrc",
        "app/private.key",
        "DATA/main.sqlite",
        ".Platform/task",
        "app/Node_Modules/module/index.js",
        "app/.GiT/config",
        ".DS_Store",
    ] {
        assert!(protected(path, &manifest).is_some(), "{path}");
    }
    assert!(!included("assets/logo.png", &roots));
    assert!(!included("root-script.js", &roots));
}
#[test]
fn scan_reports_unknown_roots_nested_git_and_unsafe_source_without_reading_excluded_targets() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    private_dir(&root.join("app/src")).unwrap();
    private_dir(&root.join("assets")).unwrap();
    private_dir(&root.join("app/vendor/nested/.git")).unwrap();
    fs::write(root.join("app/src/main.ts"), "export const value=1;").unwrap();
    symlink(
        root.join("not-existing-secret"),
        root.join("app/.env.local"),
    )
    .unwrap();
    symlink(root.join("not-existing-data"), root.join("data")).unwrap();
    symlink(root.join("not-existing-source"), root.join("app/link")).unwrap();
    let manifest = manifest();
    let scan = scan_source(&root, &manifest, &included_roots(&manifest)).unwrap();
    assert_eq!(scan.files.len(), 1);
    assert!(scan.unclassified.contains(&"assets".into()));
    assert_eq!(scan.markers.len(), 1);
    assert!(scan.markers[0].path.ends_with("app/vendor/nested/.git"));
    assert_eq!(scan.issues.len(), 1);
    assert_eq!(scan.issues[0].code, "PROJECT_GIT_UNSAFE_PATH");
    assert!(scan.excluded.iter().any(|path| path.path == "data"));
    assert!(scan
        .excluded
        .iter()
        .any(|path| path.path == "app/.env.local"));
}
#[test]
fn tracked_parser_reports_symlink_submodule_conflict_and_keeps_protected_names_for_policy() {
    let oid = "a".repeat(40);
    let mut issues = vec![];
    let rows = format!("100644 {oid} 0\tdata/main.sqlite\0")
        + &format!("120000 {oid} 0\tapp/link\0")
        + &format!("160000 {oid} 0\tapp/submodule\0")
        + &format!("100644 {oid} 2\tapp/conflict\0");
    let entries = parse_tree(rows.as_bytes(), true, &mut issues).unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(issues.len(), 3);
    assert!(protected("data/main.sqlite", &manifest()).is_some());
    assert_eq!(
        parse_tree(
            format!("100644 {oid} 0\t../outside\0").as_bytes(),
            true,
            &mut vec![]
        )
        .unwrap_err()
        .code,
        "PROJECT_GIT_UNSAFE_PATH"
    );
}
#[test]
fn index_lock_never_removes_an_existing_or_replaced_lock() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let path = root.join("index.lock");
    fs::write(&path, "existing user lock").unwrap();
    assert!(IndexLock::acquire(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"existing user lock");
    fs::remove_file(&path).unwrap();
    let lock = IndexLock::acquire(&path).unwrap();
    fs::rename(&path, root.join("owned-retained")).unwrap();
    fs::write(&path, "replacement").unwrap();
    drop(lock);
    assert_eq!(fs::read(&path).unwrap(), b"replacement");
}
#[test]
fn index_publish_preserves_external_changes_and_tampered_candidate() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let index = root.join("index");
    let lock_path = root.join("index.lock");
    fs::write(&index, "previous index").unwrap();
    let mut lock = IndexLock::acquire(&lock_path).unwrap();
    lock.write(b"candidate index").unwrap();
    fs::write(&index, "external staged state").unwrap();
    assert_eq!(
        lock.publish(&index).unwrap_err().code,
        "PROJECT_GIT_RECOVERY_REQUIRED"
    );
    assert_eq!(fs::read(&index).unwrap(), b"external staged state");
    drop(lock);
    let mut lock = IndexLock::acquire(&lock_path).unwrap();
    lock.write(b"candidate index").unwrap();
    fs::write(&lock_path, "externally modified candidate").unwrap();
    assert_eq!(
        lock.publish(&index).unwrap_err().code,
        "PROJECT_GIT_RECOVERY_REQUIRED"
    );
    assert_eq!(fs::read(&index).unwrap(), b"external staged state");
}

#[test]
fn index_publish_installs_exact_candidate_when_original_is_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    let index = root.join("index");
    let mut lock = IndexLock::acquire(&root.join("index.lock")).unwrap();
    lock.write(b"candidate index").unwrap();
    lock.publish(&index).unwrap();
    drop(lock);
    assert_eq!(fs::read(index).unwrap(), b"candidate index");
    assert!(!root.join("index.lock").exists());
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
    let status = inspect(&project, None, &mut context, None)
        .await
        .unwrap()
        .view;
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
    inspect_repository_files(&root, None, &mut issues).unwrap();
    assert!(issues
        .iter()
        .any(|error| error.code == "PROJECT_GIT_REPOSITORY_UNSUPPORTED"));
    assert!(issues
        .iter()
        .any(|error| error.code == "PROJECT_GIT_LOCKED"));
    symlink(root.join("outside"), root.join("config")).unwrap();
    assert_eq!(
        inspect_repository_files(&root, None, &mut vec![])
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

async fn wait(manager: &BuildManager, id: &str) -> TaskRecord {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let task = manager.task(id).unwrap();
            if task.status.is_final() {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}
async fn ordinary(
    manager: &ProjectGitManager,
    project: &ProjectHandle,
    tools: &GitToolchain,
    args: &[&str],
) -> Vec<u8> {
    let (_stop, stop) = watch::channel(false);
    let (_cancel, cancel) = watch::channel(false);
    let mut context = Context::new(stop, cancel).unwrap();
    let mut command = git_command(tools, context.root(), Some(project));
    command.args(args);
    let output = context.run(command, Vec::new()).await.unwrap();
    assert_eq!(output.code, 0);
    let _ = manager;
    output.stdout
}

/// Uses only the explicit disposable official toolchain. The environment variable
/// is opt-in and never changes the user's Git selection or any registered project.
#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified .test-artifacts development-git payload"]
async fn official_git_initialization_source_save_and_protection_are_standard_and_local() {
    let tool_root = PathBuf::from(
        std::env::var("LOCAL_PROJECTS_TEST_GIT_ROOT")
            .expect("explicit disposable toolchain required"),
    )
    .canonicalize()
    .unwrap();
    let evidence = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.test-artifacts")
        .canonicalize()
        .unwrap();
    assert!(tool_root.starts_with(&evidence));
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
    let created = store
        .create(crate::projects::CreateProjectInput {
            name: "Disposable project Git source versions".into(),
            description: None,
        })
        .await
        .unwrap();
    let project = store.project(&created.id).unwrap();
    fs::write(project.root.join("app/src/example.txt"), "first source\n").unwrap();
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into()).unwrap();
    let git = GitEnvironmentManager::new(paths.data_root().to_path_buf(), Some(tool_root));
    let tools = git.resolve().await.unwrap();
    let manager = ProjectGitManager::new(paths.clone(), store.clone(), tasks.clone(), git).unwrap();
    let (stop, receiver) = watch::channel(false);
    let preview = manager.status(&created.id, receiver.clone()).await.unwrap();
    assert!(preview.can_initialize, "{preview:?}");
    let task = manager
        .initialize(
            &created.id,
            GitInitInput {
                expected_snapshot: preview.snapshot_digest,
                initial_branch: "main".into(),
            },
            receiver.clone(),
        )
        .await
        .unwrap();
    let done = wait(&tasks, &task.task_id).await;
    assert_eq!(done.status, crate::tasks::TaskStatus::Succeeded, "{done:?}");
    let hooks = paths.data_root().join("synthetic-user-hooks");
    private_dir(&hooks).unwrap();
    let invoked = paths.data_root().join("user-program-invoked");
    let probe = hooks.join("program");
    fs::write(
        &probe,
        format!(
            "#!/bin/sh\nprintf forbidden > '{}'\nexit 1\n",
            invoked.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&probe, fs::Permissions::from_mode(0o700)).unwrap();
    for name in ["pre-commit", "commit-msg", "reference-transaction"] {
        fs::copy(&probe, hooks.join(name)).unwrap();
    }
    for (key, value) in [
        ("core.hooksPath", hooks.to_str().unwrap()),
        ("core.fsmonitor", probe.to_str().unwrap()),
        ("gpg.program", probe.to_str().unwrap()),
        ("commit.gpgSign", "true"),
        ("user.name", "Never use this local default"),
        ("user.email", "ignored@localhost"),
    ] {
        ordinary(
            &manager,
            &project,
            &tools,
            &["config", "--local", key, value],
        )
        .await;
    }
    let config = project.root.join(".git/config");
    let original_config = fs::read(&config).unwrap();
    let unrelated = paths.data_root().join("external-config-must-not-be-opened");
    fs::write(
        &unrelated,
        "this is deliberately invalid Git configuration\n",
    )
    .unwrap();
    let mut with_include = original_config.clone();
    with_include
        .extend_from_slice(format!("\n[include]\n\tpath = {}\n", unrelated.display()).as_bytes());
    fs::write(&config, with_include).unwrap();
    let included_config = manager.status(&created.id, receiver.clone()).await.unwrap();
    assert!(!included_config.can_save);
    assert!(included_config
        .issues
        .iter()
        .any(|issue| issue.code == "PROJECT_GIT_CONFIGURATION_UNSUPPORTED"));
    assert_eq!(included_config.repository.index_state, "not-inspected");
    fs::write(&config, original_config).unwrap();
    let before_data = crate::builds::tree_digest(&project.root.join("data"), false).unwrap();
    let preview = manager.status(&created.id, receiver.clone()).await.unwrap();
    assert!(preview.can_save, "{preview:?}");
    assert_eq!(preview.database["state"], "supported");
    let author = GitAuthor {
        name: "Explicit Disposable Author".into(),
        email: "disposable@localhost".into(),
    };
    let task = manager
        .save(
            &created.id,
            GitSaveInput {
                expected_head: None,
                expected_snapshot: preview.snapshot_digest.clone(),
                message: "First source version".into(),
                author: author.clone(),
            },
            receiver.clone(),
        )
        .await
        .unwrap();
    let done = wait(&tasks, &task.task_id).await;
    assert_eq!(done.status, crate::tasks::TaskStatus::Succeeded, "{done:?}");
    let commit = done.result.as_ref().unwrap()["commit"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(valid_oid(&commit));
    let raw = ordinary(&manager, &project, &tools, &["cat-file", "commit", &commit]).await;
    assert!(String::from_utf8(raw)
        .unwrap()
        .contains("Explicit Disposable Author <disposable@localhost>"));
    assert!(
        !invoked.exists(),
        "a user hook, fsmonitor or signing program ran"
    );
    let clean = manager.status(&created.id, receiver.clone()).await.unwrap();
    assert!(clean.changes.is_empty(), "{clean:?}");
    assert!(clean.can_save);
    assert_eq!(
        ordinary(&manager, &project, &tools, &["status", "--porcelain=v1"]).await,
        b""
    );
    assert_eq!(
        crate::builds::tree_digest(&project.root.join("data"), false).unwrap(),
        before_data
    );
    // A changed symbolic target can share the same OID. The interactive prepare
    // gate must abort while Git holds both ref locks, without moving either branch.
    ordinary(
        &manager,
        &project,
        &tools,
        &["update-ref", "refs/heads/other", &commit],
    )
    .await;
    ordinary(
        &manager,
        &project,
        &tools,
        &["symbolic-ref", "HEAD", "refs/heads/other"],
    )
    .await;
    let (_probe_stop, probe_stop) = watch::channel(false);
    let (_probe_cancel, probe_cancel) = watch::channel(false);
    let mut context = Context::new(probe_stop, probe_cancel).unwrap();
    let mut command = git_command(&tools, context.root(), Some(&project));
    command.args(["update-ref", "--stdin", "-z"]);
    author_environment(&mut command, &author);
    let aborted = context
        .commit_ref(command, &commit, Some(&commit), || {
            if fs::read(project.root.join(".git/HEAD"))? != b"ref: refs/heads/main\n" {
                return Err(stale());
            }
            Ok(())
        })
        .await
        .unwrap_err();
    assert_eq!(aborted.code, "PROJECT_GIT_STALE_PREVIEW");
    assert_eq!(aborted.details["safeAbort"], true);
    assert_eq!(aborted.details["commitSent"], false);
    for name in ["HEAD.lock", "refs/heads/main.lock", "refs/heads/other.lock"] {
        assert!(!project.root.join(".git").join(name).exists());
    }
    ordinary(
        &manager,
        &project,
        &tools,
        &["symbolic-ref", "HEAD", "refs/heads/main"],
    )
    .await;
    let stale_task = manager
        .save(
            &created.id,
            GitSaveInput {
                expected_head: None,
                expected_snapshot: preview.snapshot_digest,
                message: "Stale save".into(),
                author: author.clone(),
            },
            receiver.clone(),
        )
        .await
        .unwrap();
    let stale_result = wait(&tasks, &stale_task.task_id).await;
    assert_eq!(
        stale_result.error.unwrap().code,
        "PROJECT_GIT_STALE_PREVIEW"
    );
    fs::write(
        project.root.join("root-script.js"),
        "this must not silently disappear",
    )
    .unwrap();
    let unknown = manager.status(&created.id, receiver.clone()).await.unwrap();
    assert!(!unknown.can_save);
    assert!(unknown
        .scope
        .unclassified_paths
        .contains(&"root-script.js".into()));
    fs::remove_file(project.root.join("root-script.js")).unwrap();
    ordinary(
        &manager,
        &project,
        &tools,
        &[
            "config",
            "--local",
            "filter.synthetic.clean",
            probe.to_str().unwrap(),
        ],
    )
    .await;
    fs::write(
        project.root.join(".gitattributes"),
        "app/src/*.txt filter=synthetic\n",
    )
    .unwrap();
    let attributes = manager.status(&created.id, receiver.clone()).await.unwrap();
    assert!(!attributes.can_save);
    assert!(attributes
        .issues
        .iter()
        .any(|issue| issue.code == "PROJECT_GIT_ATTRIBUTES_UNSUPPORTED"));
    assert!(!invoked.exists(), "a user content filter ran during status");
    fs::remove_file(project.root.join(".gitattributes")).unwrap();
    // Force a protected file into the ordinary index to prove .gitignore alone
    // cannot confer protection. The fixture is a disposable artificial file.
    fs::write(project.root.join("data/secret.txt"), "artificial test data").unwrap();
    ordinary(
        &manager,
        &project,
        &tools,
        &["add", "--force", "--", "data/secret.txt"],
    )
    .await;
    let protected = manager.status(&created.id, receiver.clone()).await.unwrap();
    assert!(!protected.can_save);
    assert!(protected
        .scope
        .protected_tracked_paths
        .contains(&"data/secret.txt".into()));
    let blocked = manager
        .save(
            &created.id,
            GitSaveInput {
                expected_head: Some(commit.clone()),
                expected_snapshot: protected.snapshot_digest,
                message: "Must fail".into(),
                author,
            },
            receiver.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        wait(&tasks, &blocked.task_id).await.error.unwrap().code,
        "PROJECT_GIT_SCOPE_BLOCKED"
    );
    assert_eq!(
        ordinary(&manager, &project, &tools, &["rev-parse", "HEAD"]).await,
        format!("{commit}\n").as_bytes()
    );
    drop(stop);
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified .test-artifacts development-git payload"]
async fn official_git_versions_and_new_branch_keep_source_index_output_and_data_unchanged() {
    let tool_root = PathBuf::from(std::env::var("LOCAL_PROJECTS_TEST_GIT_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    let evidence = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.test-artifacts")
        .canonicalize()
        .unwrap();
    assert!(tool_root.starts_with(evidence));
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
    let created = store
        .create(crate::projects::CreateProjectInput {
            name: "Disposable local Git branch acceptance".into(),
            description: None,
        })
        .await
        .unwrap();
    let project = store.project(&created.id).unwrap();
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into()).unwrap();
    let git = GitEnvironmentManager::new(paths.data_root().to_path_buf(), Some(tool_root));
    let tools = git.resolve().await.unwrap();
    let manager = ProjectGitManager::new(paths.clone(), store.clone(), tasks.clone(), git).unwrap();
    let (_stop, receiver) = watch::channel(false);
    for limit in [0, 101] {
        assert_eq!(
            manager
                .versions(&created.id, limit, receiver.clone())
                .await
                .unwrap_err()
                .code,
            "PROJECT_GIT_INPUT_INVALID"
        );
    }
    let preview = manager.status(&created.id, receiver.clone()).await.unwrap();
    let initialized = manager
        .initialize(
            &created.id,
            GitInitInput {
                expected_snapshot: preview.snapshot_digest,
                initial_branch: "main".into(),
            },
            receiver.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        wait(&tasks, &initialized.task_id).await.status,
        crate::tasks::TaskStatus::Succeeded
    );
    let unborn = manager.status(&created.id, receiver.clone()).await.unwrap();
    assert!(!unborn.can_create_branch && !unborn.can_list_versions);
    let author = GitAuthor {
        name: "Explicit Branch Fixture Author".into(),
        email: "fixture@localhost".into(),
    };
    let source_path = project.root.join("app/src/branch-fixture.txt");
    for message in ["First local version", "Second local version"] {
        fs::write(&source_path, message).unwrap();
        let preview = manager.status(&created.id, receiver.clone()).await.unwrap();
        let saved = manager
            .save(
                &created.id,
                GitSaveInput {
                    expected_head: head_oid(&preview).map(str::to_owned),
                    expected_snapshot: preview.snapshot_digest,
                    message: message.into(),
                    author: author.clone(),
                },
                receiver.clone(),
            )
            .await
            .unwrap();
        let done = wait(&tasks, &saved.task_id).await;
        assert_eq!(done.status, crate::tasks::TaskStatus::Succeeded, "{done:?}");
    }
    let invoked = paths.data_root().join("forbidden-user-program");
    let hooks = paths.data_root().join("synthetic-hooks");
    private_dir(&hooks).unwrap();
    let hook = hooks.join("reference-transaction");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\nprintf forbidden > '{}'\nexit 1\n",
            invoked.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
    for (name, value) in [
        ("core.hooksPath", hooks.to_str().unwrap()),
        ("core.fsmonitor", hook.to_str().unwrap()),
        ("gpg.program", hook.to_str().unwrap()),
        ("log.showSignature", "true"),
        ("user.name", "Unselected identity"),
        ("user.email", "unselected@localhost"),
    ] {
        ordinary(
            &manager,
            &project,
            &tools,
            &["config", "--local", name, value],
        )
        .await;
    }
    let index = project.root.join(".git/index");
    let before_index = index_fingerprint(&index).unwrap();
    let before_source = fs::read(&source_path).unwrap();
    private_dir(&project.root.join("dist")).unwrap();
    fs::write(
        project.root.join("dist/index.html"),
        "disposable previous running output",
    )
    .unwrap();
    let before_data = crate::builds::tree_digest(&project.root.join("data"), false).unwrap();
    let before_output = crate::builds::tree_digest(&project.root.join("dist"), false).unwrap();
    let listed = manager
        .versions(&created.id, 1, receiver.clone())
        .await
        .unwrap();
    assert_eq!(listed.versions.len(), 1);
    assert!(listed.has_more);
    assert_eq!(listed.versions[0].subject, "Second local version");
    assert_eq!(listed.versions[0].author.name, author.name);
    let head = listed.head.oid.unwrap();
    let full = manager
        .versions(&created.id, 100, receiver.clone())
        .await
        .unwrap();
    assert_eq!(full.versions.len(), 2);
    assert!(!full.has_more);
    assert_eq!(full.versions[0].parents, vec![full.versions[1].oid.clone()]);
    assert_eq!(
        index_fingerprint(&index).unwrap(),
        before_index,
        "listing refreshed the index"
    );
    let input_for = |view: &ProjectGitStatus, name: &str| GitCreateBranchInput {
        name: name.into(),
        expected_head: head_oid(view).unwrap().into(),
        expected_branch: view
            .repository
            .head
            .as_ref()
            .unwrap()
            .branch
            .clone()
            .unwrap(),
        expected_snapshot: view.snapshot_digest.clone(),
    };
    let clean = manager.status(&created.id, receiver.clone()).await.unwrap();
    assert!(
        clean.can_create_branch && clean.can_list_versions,
        "{clean:?}"
    );
    let mut older = clean.clone();
    older.repository.git.as_mut().unwrap().version = "2.54.0 (Apple Git-154)".into();
    finalize_capabilities(&mut older);
    assert!(older.can_save && older.can_list_versions);
    assert!(!older.can_create_branch);
    assert_eq!(
        older.branch_creation_issue.unwrap().code,
        "PROJECT_GIT_BRANCH_VERSION_UNSUPPORTED"
    );
    let mut wrong_branch = input_for(&clean, "wrong-preview");
    wrong_branch.expected_branch = "refs/heads/other".into();
    let task = manager
        .create_branch(&created.id, wrong_branch, receiver.clone())
        .await
        .unwrap();
    let error = wait(&tasks, &task.task_id).await.error.unwrap();
    assert_eq!(error.code, "PROJECT_GIT_STALE_PREVIEW");
    assert_eq!(error.details["operationOutcome"], "not_committed");
    for scenario in ["dirty", "staged", "unknown"] {
        match scenario {
            "dirty" | "staged" => fs::write(&source_path, "uncommitted content").unwrap(),
            _ => fs::write(project.root.join("unclassified-script.js"), "unclassified").unwrap(),
        }
        if scenario == "staged" {
            ordinary(
                &manager,
                &project,
                &tools,
                &["add", "--", "app/src/branch-fixture.txt"],
            )
            .await;
        }
        let view = manager.status(&created.id, receiver.clone()).await.unwrap();
        assert!(!view.can_create_branch, "{scenario}: {view:?}");
        assert!(
            view.can_list_versions,
            "{scenario}: read-only history must stay available"
        );
        let input = input_for(&view, &format!("blocked-{scenario}"));
        let task = manager
            .create_branch(&created.id, input, receiver.clone())
            .await
            .unwrap();
        let done = wait(&tasks, &task.task_id).await;
        assert_eq!(
            done.error.as_ref().unwrap().details["operationOutcome"],
            "not_committed"
        );
        assert!(!project
            .root
            .join(format!(".git/refs/heads/blocked-{scenario}"))
            .exists());
        if scenario == "unknown" {
            fs::remove_file(project.root.join("unclassified-script.js")).unwrap();
        } else {
            fs::write(&source_path, &before_source).unwrap();
        }
        if scenario == "staged" {
            ordinary(&manager, &project, &tools, &["read-tree", "HEAD"]).await;
        }
    }
    ordinary(
        &manager,
        &project,
        &tools,
        &["update-ref", "refs/heads/taken", &head],
    )
    .await;
    let view = manager.status(&created.id, receiver.clone()).await.unwrap();
    let task = manager
        .create_branch(&created.id, input_for(&view, "taken"), receiver.clone())
        .await
        .unwrap();
    assert_eq!(
        wait(&tasks, &task.task_id).await.error.unwrap().code,
        "PROJECT_GIT_BRANCH_EXISTS"
    );
    // Exercise a packed old branch and a nested new branch. No checkout/reset,
    // read-tree, or index publication is used by the operation under test.
    ordinary(&manager, &project, &tools, &["pack-refs", "--all"]).await;
    let view = manager.status(&created.id, receiver.clone()).await.unwrap();
    let index_before_branch = index_fingerprint(&index).unwrap();
    let task = manager
        .create_branch(
            &created.id,
            input_for(&view, "custom/feature-1"),
            receiver.clone(),
        )
        .await
        .unwrap();
    assert_eq!(task.task_type, "gitBranchCreate");
    let done = wait(&tasks, &task.task_id).await;
    assert_eq!(done.status, crate::tasks::TaskStatus::Succeeded, "{done:?}");
    let result = done.result.unwrap();
    assert_eq!(result["operationOutcome"], "committed");
    assert_eq!(result["head"], head);
    assert_eq!(result["previousBranch"], "refs/heads/main");
    assert_eq!(result["branch"], "refs/heads/custom/feature-1");
    assert_eq!(result["database"]["state"], "not-supported");
    assert_eq!(
        ordinary(&manager, &project, &tools, &["symbolic-ref", "HEAD"]).await,
        b"refs/heads/custom/feature-1\n"
    );
    assert_eq!(
        ordinary(
            &manager,
            &project,
            &tools,
            &["rev-parse", "refs/heads/main"]
        )
        .await,
        format!("{head}\n").as_bytes()
    );
    assert_eq!(index_fingerprint(&index).unwrap(), index_before_branch);
    assert_eq!(fs::read(&source_path).unwrap(), before_source);
    assert_eq!(
        crate::builds::tree_digest(&project.root.join("data"), false).unwrap(),
        before_data
    );
    assert_eq!(
        crate::builds::tree_digest(&project.root.join("dist"), false).unwrap(),
        before_output
    );
    assert!(!invoked.exists(), "a configured user program was executed");
    for name in [
        "HEAD.lock",
        "index.lock",
        "refs/heads/main.lock",
        "refs/heads/custom/feature-1.lock",
    ] {
        assert!(
            !project.root.join(".git").join(name).exists(),
            "left own lock {name}"
        );
    }
    let after = manager.status(&created.id, receiver.clone()).await.unwrap();
    assert!(
        after.can_create_branch && after.changes.is_empty(),
        "{after:?}"
    );
    let journal: Journal = serde_json::from_slice(
        &fs::read(manager.journal_path(&created.id, &task.task_id)).unwrap(),
    )
    .unwrap();
    assert_eq!(journal.phase, "completed");
    assert_eq!(journal.previous_branch.as_deref(), Some("refs/heads/main"));
    // A validation failure after prepare aborts both new ref and symbolic HEAD.
    let (_probe_stop, probe_stop) = watch::channel(false);
    let (_probe_cancel, probe_cancel) = watch::channel(false);
    let mut context = Context::new(probe_stop, probe_cancel).unwrap();
    let mut command = git_command(&tools, context.root(), Some(&project));
    command.args(["update-ref", "--stdin", "-z"]);
    author_environment(&mut command, &author);
    let commands = format!("start\0create refs/heads/must-abort\0{head}\0option no-deref\0symref-update HEAD\0refs/heads/must-abort\0oid\0{head}\0prepare\0").into_bytes();
    let error = context
        .commit_transaction(command, commands, || Err(stale()))
        .await
        .unwrap_err();
    assert_eq!(error.details["safeAbort"], true);
    assert_eq!(error.details["commitSent"], false);
    assert!(!project.root.join(".git/refs/heads/must-abort").exists());
    assert_eq!(
        fs::read(project.root.join(".git/HEAD")).unwrap(),
        b"ref: refs/heads/custom/feature-1\n"
    );
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn new_branch_cancellation_releases_task_lease_without_waiting_for_the_project_gate() {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
    let project = store
        .create(crate::projects::CreateProjectInput {
            name: "Disposable branch cancel gate".into(),
            description: None,
        })
        .await
        .unwrap();
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into()).unwrap();
    let git = GitEnvironmentManager::new(paths.data_root().to_path_buf(), None);
    let manager = ProjectGitManager::new(paths, store.clone(), tasks.clone(), git).unwrap();
    let gate = store.gate(&project.id);
    let held = gate.write().await;
    let (_stop, receiver) = watch::channel(false);
    let task = manager
        .create_branch(
            &project.id,
            GitCreateBranchInput {
                name: "never-created".into(),
                expected_head: "a".repeat(40),
                expected_branch: "refs/heads/main".into(),
                expected_snapshot: "b".repeat(64),
            },
            receiver,
        )
        .await
        .unwrap();
    assert_eq!(
        store.active_task(&project.id).as_deref(),
        Some(task.task_id.as_str())
    );
    store.begin_workspace_migration().unwrap();
    manager.cancel(&task.task_id).unwrap();
    let completed = tokio::time::timeout(Duration::from_secs(3), wait(&tasks, &task.task_id))
        .await
        .unwrap();
    assert_eq!(completed.status, crate::tasks::TaskStatus::Interrupted);
    let error = completed.error.unwrap();
    assert_eq!(error.code, "PROJECT_GIT_INTERRUPTED");
    assert_eq!(error.details["operationOutcome"], "not_committed");
    assert!(!store.has_active_tasks());
    store.resume_workspace();
    assert!(!store
        .project(&project.id)
        .unwrap()
        .root
        .join(".git")
        .exists());
    drop(held);
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}
