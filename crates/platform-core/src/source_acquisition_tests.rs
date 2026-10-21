use super::*;
use serde_json::json;
use std::os::unix::fs::symlink;
use tempfile::TempDir;

fn sample() -> TempDir {
    tempfile::Builder::new()
        .prefix("disposable-source-acquisition-")
        .tempdir()
        .unwrap()
}
fn row(mode: &str, path: &str, size: u64) -> Vec<u8> {
    format!("{mode} blob {} {size}\t{path}\0", "1".repeat(40)).into_bytes()
}

#[test]
fn only_canonical_public_provider_urls_are_accepted() {
    assert_eq!(
        validate_url("https://github.com/git/git/").unwrap().0,
        "https://github.com/git/git.git"
    );
    assert_eq!(
        validate_url("https://gitee.com/mirrors/git.git").unwrap().1,
        "gitee"
    );
    for value in [
        "http://github.com/a/b",
        "https://github.com:443/a/b",
        "https://user@github.com/a/b",
        "https://github.com/a/b?token=private",
        "https://github.com/a/b#main",
        "https://github.com/a/b/tree/main",
        "https://github.com/a/%62",
        "https://github.com/a/../b",
        "git@github.com:a/b",
        "file:///tmp/repo",
        " https://github.com/a/b",
    ] {
        assert!(validate_url(value).is_err(), "{value}");
    }
}

#[test]
fn tree_validation_rejects_links_paths_collisions_and_size_without_materializing() {
    let files =
        parse_tree(&[row("100644", "a.txt", 0), row("100755", "目录/run.sh", 4)].concat()).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files[1].executable);
    for path in [
        "../bad",
        "/absolute",
        ".git/config",
        "dir/.GiT/config",
        "a//b",
        "a\\b",
        "a\nb",
        "a/./b",
        "trailing.",
        "dir:stream/file",
    ] {
        assert!(parse_tree(&row("100644", path, 1)).is_err(), "{path}");
    }
    for mode in ["120000", "160000", "040000"] {
        assert!(parse_tree(&row(mode, "unsafe", 1)).is_err());
    }
    for paths in [
        ["A/file", "a/other"],
        ["é/file", "e\u{301}/other"],
        ["a", "a/b"],
        ["a/b", "a"],
        ["same", "same"],
    ] {
        assert!(
            parse_tree(
                &paths
                    .iter()
                    .flat_map(|p| row("100644", p, 1))
                    .collect::<Vec<_>>()
            )
            .is_err(),
            "{paths:?}"
        );
    }
    assert!(parse_tree(&row("100644", "large", MAX_BLOB + 1)).is_err());
    assert!(parse_tree(&[0xff, 0]).is_err());
    assert!(parse_tree(b"unfinished").is_err());
    assert!(parse_tree(&[]).unwrap().is_empty());
}

#[test]
fn local_inspection_is_metadata_only_and_does_not_register_or_read_secrets() {
    let temp = sample();
    let root = temp.path().canonicalize().unwrap();
    fs::write(
        root.join("package.json"),
        "{\"scripts\":{\"postinstall\":\"touch must-not-run\"}}",
    )
    .unwrap();
    symlink("/this-secret-file-must-not-be-opened", root.join(".env")).unwrap();
    let generic = inspect_directory(&root).unwrap();
    assert_eq!(generic.kind, "needs-adaptation");
    assert_eq!(generic.execution_state, "not-executed");
    assert!(!root.join("must-not-run").exists());
    fs::create_dir(root.join("app")).unwrap();
    fs::write(root.join("app/package.json"), "{}").unwrap();
    fs::write(
        root.join("app/package-lock.json"),
        "{\"lockfileVersion\":3}",
    )
    .unwrap();
    let manifest = json!({"manifestVersion":1,"id":"prj_disposable","name":"Disposable source","description":"","sdkApiVersion":1,"sourceDir":"app","outputDir":"dist","entry":"dist/index.html","database":"data/main.sqlite","attachmentsDir":"data/attachments","build":{"script":"build"},"docs":{"readme":"README.md","database":"docs/database.md","platform":"docs/platform.md"}});
    fs::write(
        root.join("project.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let info = inspect_directory(&root).unwrap();
    assert_eq!(info.kind, "project-v1-source");
    assert!(!root.join("data").exists());
    assert!(!root.join("dist").exists());
    fs::remove_file(root.join("project.json")).unwrap();
    fs::create_dir(root.join("public")).unwrap();
    fs::write(root.join("public/index.html"), "<!doctype html>").unwrap();
    fs::create_dir(root.join("server")).unwrap();
    fs::write(
        root.join("server/managed.mjs"),
        "throw new Error('must not run');",
    )
    .unwrap();
    fs::write(root.join("local-app.json"),serde_json::to_vec(&json!({"packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service","app":{"id":"org.example.disposable","name":"Disposable Node","version":"1.0.0"},"web":{"root":"public","entry":"index.html","routing":"files"},"backend":{"root":"server","entry":"managed.mjs","protocolVersion":1,"nodeMajor":24}})).unwrap()).unwrap();
    assert_eq!(
        inspect_directory(&root).unwrap().kind,
        "node-service-source"
    );
    assert!(!root.join("configuration").exists());
    assert!(inspect_directory(Path::new("relative")).is_err());
}

#[test]
fn local_declaration_symlink_cannot_open_external_file() {
    let temp = sample();
    let root = temp.path().canonicalize().unwrap();
    let external = root.join("external-secret");
    fs::write(&external, "not a project").unwrap();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    symlink(&external, source.join("project.json")).unwrap();
    let inspection = inspect_directory(&source).unwrap();
    assert_eq!(inspection.kind, "needs-adaptation");
    assert!(!inspection.issues.is_empty());
    assert_eq!(fs::read(external).unwrap(), b"not a project");
}

#[allow(clippy::type_complexity)]
fn fixture() -> (
    TempDir,
    RuntimePaths,
    Arc<ProjectStore>,
    Arc<BuildManager>,
    Arc<SourceAcquisitionManager>,
) {
    let (temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1".into()).unwrap();
    let git = GitEnvironmentManager::new(paths.data_root().into(), None);
    let manager =
        SourceAcquisitionManager::new(paths.clone(), store.clone(), tasks.clone(), git).unwrap();
    (temp, paths, store, tasks, manager)
}
fn fake_git(paths: &RuntimePaths, body: &str) -> PathBuf {
    let file = paths.data_root().join("disposable-git");
    fs::write(&file,format!("#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'git version 2.56.0\\n'; exit 0; fi\n{body}\n")).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    atomic_json(
        &paths.data_root().join("git-toolchain.json"),
        &json!({"schemaVersion":1,"gitPath":file}),
    )
    .unwrap();
    file
}
async fn final_task(tasks: &BuildManager, id: &str) -> TaskRecord {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let task = tasks.task(id).unwrap();
            if task.status.is_final() {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
async fn wait_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fs::read_to_string(path).is_ok_and(|s| s.ends_with('\n')) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn failed_acquisition_is_a_persisted_task_and_retry_never_reuses_slot() {
    let (_temp, paths, store, tasks, manager) = fixture();
    fake_git(
        &paths,
        "printf 'private raw stderr must not escape' >&2; exit 128",
    );
    let (stop, _) = watch::channel(false);
    for _ in 0..2 {
        let task = manager
            .submit(
                AcquireSourceInput {
                    url: "https://github.com/example/disposable.git".into(),
                    name: Some("Disposable".into()),
                },
                stop.subscribe(),
            )
            .await
            .unwrap();
        assert!(task.project_id.is_none());
        let outcome = final_task(&tasks, &task.task_id).await;
        assert_eq!(outcome.error.unwrap().code, "SOURCE_GIT_FAILED");
        assert!(!tasks
            .logs(&task.task_id, 0)
            .unwrap()
            .text
            .contains("private raw stderr"));
    }
    let slots = manager.list().unwrap();
    assert_eq!(slots.len(), 2);
    assert_ne!(slots[0].source_id, slots[1].source_id);
    assert!(slots
        .iter()
        .all(|s| s.status == "failed" && s.source_path.is_none() && !s.registration_available));
    assert!(store.list().await.unwrap().is_empty());
    assert!(!store.has_active_tasks());
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn cancellation_kills_process_group_and_holds_migration_lease_until_final() {
    let (_temp, paths, store, tasks, manager) = fixture();
    let marker = paths.data_root().join("disposable-child.pid");
    fake_git(
        &paths,
        &format!(
            "/bin/sleep 30 &\nprintf '%s\\n' \"$!\" > '{}'\nwait",
            marker.display()
        ),
    );
    let (stop, _) = watch::channel(false);
    let task = manager
        .submit(
            AcquireSourceInput {
                url: "https://github.com/example/disposable".into(),
                name: None,
            },
            stop.subscribe(),
        )
        .await
        .unwrap();
    wait_file(&marker).await;
    assert!(store.has_active_tasks());
    assert_eq!(
        manager
            .submit(
                AcquireSourceInput {
                    url: "https://github.com/example/another".into(),
                    name: None
                },
                stop.subscribe()
            )
            .await
            .unwrap_err()
            .code,
        "SOURCE_ACQUISITION_BUSY"
    );
    store.begin_workspace_migration().unwrap();
    assert!(store.has_active_tasks());
    assert!(manager
        .submit(
            AcquireSourceInput {
                url: "https://github.com/example/another".into(),
                name: None
            },
            stop.subscribe()
        )
        .await
        .is_err());
    manager.cancel(&task.task_id).unwrap();
    let outcome = final_task(&tasks, &task.task_id).await;
    assert_eq!(outcome.error.unwrap().code, "SOURCE_ACQUISITION_CANCELLED");
    manager.shutdown().await;
    assert!(!store.has_active_tasks());
    store.resume_workspace();
    let pid = fs::read_to_string(marker)
        .unwrap()
        .trim()
        .parse::<i32>()
        .unwrap();
    // A just-killed child may briefly be a zombie; it cannot keep a running pipe.
    let status = std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .unwrap();
    assert!(
        status.stdout.is_empty()
            || String::from_utf8_lossy(&status.stdout)
                .trim()
                .starts_with('Z')
    );
    let candidate = manager.list().unwrap().pop().unwrap();
    assert_eq!(candidate.status, "cancelled");
    assert!(candidate.evidence_path.join(".acquiring").is_dir());
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn stopped_service_interrupts_without_publishing_or_auto_retry() {
    let (_temp, paths, store, tasks, manager) = fixture();
    let marker = paths.data_root().join("started");
    fake_git(
        &paths,
        &format!(
            "printf 'started\\n' > '{}'\n/bin/sleep 30",
            marker.display()
        ),
    );
    let (stop, _) = watch::channel(false);
    let task = manager
        .submit(
            AcquireSourceInput {
                url: "https://gitee.com/example/disposable".into(),
                name: None,
            },
            stop.subscribe(),
        )
        .await
        .unwrap();
    wait_file(&marker).await;
    stop.send(true).unwrap();
    let result = final_task(&tasks, &task.task_id).await;
    assert_eq!(result.error.unwrap().code, "SOURCE_ACQUISITION_INTERRUPTED");
    manager.shutdown().await;
    let record = manager.list().unwrap().pop().unwrap();
    assert_eq!(record.status, "interrupted");
    assert!(record.source_path.is_none());
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn restart_marks_unfinished_source_and_retains_evidence_without_project_reservation() {
    let (_temp, paths, store, tasks, manager) = fixture();
    let task = tasks.create_workspace_operation("sourceAcquire").unwrap();
    let id = format!("src_{}", Uuid::new_v4().simple());
    private_dir(&manager.root()).unwrap();
    new_dir(&manager.root().join(&id)).unwrap();
    new_dir(&manager.root().join(&id).join(".acquiring")).unwrap();
    fs::write(
        manager.root().join(&id).join(".acquiring/evidence"),
        "partial input",
    )
    .unwrap();
    let record = SourceRecord {
        schema_version: 1,
        source_id: id.clone(),
        task_id: task.task_id,
        name: "Interrupted disposable".into(),
        status: "acquiring".into(),
        phase: "downloading".into(),
        url: "https://github.com/example/disposable.git".into(),
        provider: "github".into(),
        created_at: 1,
        completed_at: None,
        repository: "repository".into(),
        shallow: true,
        git: None,
        commit: None,
        file_count: None,
        total_bytes: None,
        content_digest: None,
        inspection: None,
        error: None,
    };
    manager.save_record(&record).unwrap();
    let restarted = SourceAcquisitionManager::new(
        paths.clone(),
        store.clone(),
        tasks.clone(),
        GitEnvironmentManager::new(paths.data_root().into(), None),
    )
    .unwrap();
    let result = restarted.info(&id).unwrap();
    assert_eq!(result.status, "interrupted");
    assert_eq!(
        fs::read(result.evidence_path.join(".acquiring/evidence")).unwrap(),
        b"partial input"
    );
    assert!(!store.has_active_tasks());
    assert!(!paths
        .workspace_root()
        .join(".platform/project-state/reservations")
        .exists());
    restarted.shutdown().await;
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn unsafe_slot_metadata_and_tampered_payload_are_not_followed() {
    let (_temp, _paths, store, tasks, manager) = fixture();
    private_dir(&manager.root()).unwrap();
    let id = format!("src_{}", Uuid::new_v4().simple());
    let external = sample();
    symlink(external.path(), manager.root().join(&id)).unwrap();
    assert!(manager.info(&id).is_err());
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

fn real_toolchain() -> GitToolchain {
    let git = PathBuf::from(
        std::env::var_os("PLATFORM_TEST_SOURCE_GIT")
            .expect("PLATFORM_TEST_SOURCE_GIT must name official disposable Git"),
    );
    let git = git.canonicalize().unwrap();
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .canonicalize()
        .unwrap();
    assert!(git.starts_with(workspace.join(".test-artifacts")));
    assert!(git.components().any(|part| part
        .as_os_str()
        .to_string_lossy()
        .starts_with("disposable-")));
    GitToolchain {
        git: ExecutableInfo {
            path: git.clone(),
            version: "2.56.0".into(),
        },
        path: format!(
            "{}:/usr/bin:/bin:/usr/sbin:/sbin",
            git.parent().unwrap().display()
        ),
        exec_path: None,
        template_path: None,
    }
}

#[tokio::test]
#[ignore = "Opt-in: official disposable PLATFORM_TEST_SOURCE_GIT; offline, no user repository/configuration"]
async fn actual_git_blob_materialization_index_and_filters_are_non_executing() {
    let toolchain = real_toolchain();
    let temp = sample();
    let root = temp.path().canonicalize().unwrap();
    let repository = root.join("repository");
    new_dir(&repository).unwrap();
    let home = root.join("home");
    new_dir(&home).unwrap();
    let template = root.join("template");
    new_dir(&template).unwrap();
    let (stop, _) = watch::channel(false);
    let (cancel, _) = watch::channel(false);
    let mut control = RunControl {
        stop: stop.subscribe(),
        cancel: cancel.subscribe(),
        deadline: Instant::now() + Duration::from_secs(30),
        monitor: root.clone(),
    };
    let mut init = isolated_command(&toolchain, &repository, &home, &template);
    init.args(["init", "--initial-branch=main"]);
    execute(init, &mut control, MAX_OUTPUT).await.unwrap();
    fs::write(
        repository.join(".gitattributes"),
        "*.txt filter=must-not-run\n",
    )
    .unwrap();
    fs::write(repository.join("safe.txt"), "中文 materialized exactly\n").unwrap();
    fs::write(
        repository.join("script.sh"),
        "#!/bin/sh\ntouch must-not-execute\n",
    )
    .unwrap();
    fs::set_permissions(
        repository.join("script.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let mut add = isolated_command(&toolchain, &repository, &home, &template);
    add.args(["add", "--", ".gitattributes", "safe.txt", "script.sh"]);
    execute(add, &mut control, MAX_OUTPUT).await.unwrap();
    let mut commit = isolated_command(&toolchain, &repository, &home, &template);
    commit.args([
        "-c",
        "user.name=Disposable",
        "-c",
        "user.email=disposable@example.invalid",
        "-c",
        "commit.gpgSign=false",
        "commit",
        "-m",
        "disposable source",
    ]);
    execute(commit, &mut control, MAX_OUTPUT).await.unwrap();
    let mut list = isolated_command(&toolchain, &repository, &home, &template);
    list.args(["ls-tree", "-r", "-z", "--long", "HEAD"]);
    let entries = parse_tree(&execute(list, &mut control, MAX_TREE_OUTPUT).await.unwrap()).unwrap();
    // Configure a hostile filter in this test-owned repository. Raw cat-file and
    // read-tree must still never run it. No production network exception exists.
    let mut config = isolated_command(&toolchain, &repository, &home, &template);
    config.args([
        "config",
        "filter.must-not-run.smudge",
        "touch must-not-execute",
    ]);
    execute(config, &mut control, MAX_OUTPUT).await.unwrap();
    for name in [".gitattributes", "safe.txt", "script.sh"] {
        fs::remove_file(repository.join(name)).unwrap();
    }
    let digest = materialize(
        &toolchain,
        &repository,
        &home,
        &template,
        &entries,
        &mut control,
    )
    .await
    .unwrap();
    assert_eq!(digest.len(), 64);
    assert_eq!(directory_digest(&repository).unwrap(), digest);
    let mut index = isolated_command(&toolchain, &repository, &home, &template);
    index.args(["read-tree", "HEAD"]);
    execute(index, &mut control, MAX_OUTPUT).await.unwrap();
    assert_eq!(
        fs::read_to_string(repository.join("safe.txt")).unwrap(),
        "中文 materialized exactly\n"
    );
    assert!(!repository.join("must-not-execute").exists());
    let mut status = isolated_command(&toolchain, &repository, &home, &template);
    status.args(["status", "--porcelain=v1", "--untracked-files=all"]);
    assert!(execute(status, &mut control, MAX_OUTPUT)
        .await
        .unwrap()
        .is_empty());
    fs::set_permissions(
        repository.join("script.sh"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert_ne!(
        directory_digest(&repository).unwrap(),
        digest,
        "executable permissions participate in source identity"
    );
}

#[tokio::test]
#[ignore = "Opt-in public HTTPS download: PLATFORM_TEST_SOURCE_GIT plus PLATFORM_TEST_SOURCE_URL; creates only disposable slots"]
async fn actual_public_https_download_records_commit_and_keeps_projects_empty() {
    let tools = real_toolchain();
    let url =
        std::env::var("PLATFORM_TEST_SOURCE_URL").expect("explicit public repository URL required");
    validate_url(&url).unwrap();
    let (_temp, paths, store, tasks, manager) = fixture();
    atomic_json(
        &paths.data_root().join("git-toolchain.json"),
        &json!({"schemaVersion":1,"gitPath":tools.git.path}),
    )
    .unwrap();
    let (stop, _) = watch::channel(false);
    let task = manager
        .submit(
            AcquireSourceInput {
                url,
                name: Some("Disposable public download".into()),
            },
            stop.subscribe(),
        )
        .await
        .unwrap();
    let final_result = tokio::time::timeout(TIMEOUT + Duration::from_secs(5), async {
        loop {
            let task = tasks.task(&task.task_id).unwrap();
            if task.status.is_final() {
                break task;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert!(final_result.error.is_none(), "{:?}", final_result.error);
    let result = manager.list().unwrap().pop().unwrap();
    assert_eq!(result.status, "ready");
    manager
        .verify_ready(&result.source_id, result.content_digest.as_ref().unwrap())
        .unwrap();
    assert!(valid_oid(result.commit.as_deref().unwrap()));
    assert!(result.source_path.unwrap().join(".git/index").is_file());
    assert!(store.list().await.unwrap().is_empty());
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

#[test]
fn disk_monitor_and_manifest_reads_reject_limits_and_hard_links() {
    let temp = sample();
    let root = temp.path().canonicalize().unwrap();
    let large = root.join("disposable-sparse-payload");
    fs::File::create(&large)
        .unwrap()
        .set_len(MAX_DISK + 1)
        .unwrap();
    assert_eq!(check_disk(&root).unwrap_err().code, "SOURCE_LIMIT_EXCEEDED");
    fs::remove_file(&large).unwrap();
    let declaration = root.join("project.json");
    fs::File::create(&declaration)
        .unwrap()
        .set_len(MAX_METADATA + 1)
        .unwrap();
    assert_eq!(
        read_small(&declaration).unwrap_err().code,
        "SOURCE_LIMIT_EXCEEDED"
    );
    fs::remove_file(&declaration).unwrap();
    fs::write(&declaration, "{}").unwrap();
    fs::hard_link(&declaration, root.join("second-link")).unwrap();
    assert!(read_small(&declaration).is_err());
    assert!(check_disk(&root).is_err());
}

#[test]
fn tree_limits_include_file_count_total_bytes_and_hfs_git_aliases() {
    let too_many = (0..=MAX_FILES)
        .flat_map(|i| row("100644", &format!("file-{i}"), 0))
        .collect::<Vec<_>>();
    assert_eq!(
        parse_tree(&too_many).unwrap_err().code,
        "SOURCE_LIMIT_EXCEEDED"
    );
    let too_large = (0..5)
        .flat_map(|i| row("100644", &format!("file-{i}"), MAX_BLOB))
        .collect::<Vec<_>>();
    assert_eq!(
        parse_tree(&too_large).unwrap_err().code,
        "SOURCE_LIMIT_EXCEEDED"
    );
    assert!(parse_tree(&row("100644", ".g\u{200c}it/config", 1)).is_err());
}
