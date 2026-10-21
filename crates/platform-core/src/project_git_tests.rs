use super::*;
use std::os::unix::fs::symlink;

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
    let pending = tokio::spawn(async move { reader.status(&id, receiver).await });
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
    assert_eq!(preview.database["state"], "not-supported");
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
