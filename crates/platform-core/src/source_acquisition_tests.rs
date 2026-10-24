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
fn source_branch_is_optional_and_restricted_to_literal_local_branch_names() {
    let input: AcquireSourceInput =
        serde_json::from_value(json!({"url":"https://github.com/example/disposable"})).unwrap();
    assert!(input.branch.is_none());
    for branch in [
        "main",
        "codex/tts-providers",
        "release/v1.2",
        "topic_2/test-3",
    ] {
        assert!(validate_branch(branch).is_ok(), "{branch}");
    }
    for branch in [
        "",
        "HEAD",
        "refs/heads/main",
        "--upload-pack=bad",
        "-main",
        "main~1",
        "main^{commit}",
        "a..b",
        "a//b",
        "a/.hidden",
        "topic.lock",
        "topic/branch.lock",
        "a/",
        "a.",
        "a@{1}",
        "a\\b",
        "a b",
        "main\nnext",
        "main:other",
    ] {
        assert_eq!(
            validate_branch(branch).unwrap_err().code,
            "SOURCE_BRANCH_INVALID",
            "{branch}"
        );
    }
    assert!(validate_branch(&"1".repeat(40)).is_err());
    assert!(validate_branch(&"b".repeat(129)).is_err());
    assert!(serde_json::from_value::<AcquireSourceInput>(
        json!({"url":"https://github.com/example/disposable","branch":true})
    )
    .is_err());
}

#[tokio::test]
async fn invalid_branch_is_rejected_before_any_task_or_source_slot_is_created() {
    let (_temp, _paths, store, tasks, manager) = fixture();
    let (stop, _) = watch::channel(false);
    let issue = manager
        .submit(
            AcquireSourceInput {
                url: "https://github.com/example/disposable".into(),
                branch: Some("--upload-pack=forbidden".into()),
                name: None,
            },
            stop.subscribe(),
        )
        .await
        .unwrap_err();
    assert_eq!(issue.code, "SOURCE_BRANCH_INVALID");
    assert!(manager.list().unwrap().is_empty());
    assert!(!manager.root().exists());
    assert!(!store.has_active_tasks());
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn selected_branch_is_one_clone_argument_and_persists_without_changing_url_rules() {
    let (_temp, paths, store, tasks, manager) = fixture();
    let arguments = paths.data_root().join("disposable-clone-args");
    fake_git(&paths,&format!("for arg do\ncase \"$arg\" in clone) printf '%s\\0' \"$@\" > '{}'; exit 128;; esac\ndone\nexit 128",arguments.display()));
    let (stop, _) = watch::channel(false);
    let task = manager
        .submit(
            AcquireSourceInput {
                url: "https://github.com/example/disposable".into(),
                branch: Some("codex/tts-providers".into()),
                name: None,
            },
            stop.subscribe(),
        )
        .await
        .unwrap();
    let done = final_task(&tasks, &task.task_id).await;
    assert_eq!(done.error.unwrap().code, "SOURCE_GIT_FAILED");
    let args = fs::read(arguments).unwrap();
    let args: Vec<_> = args
        .split(|byte| *byte == 0)
        .filter(|argument| !argument.is_empty())
        .map(|argument| std::str::from_utf8(argument).unwrap())
        .collect();
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--branch", "codex/tts-providers"]));
    assert!(args
        .windows(2)
        .any(|pair| pair == ["--", "https://github.com/example/disposable.git"]));
    let candidate = manager.list().unwrap().pop().unwrap();
    assert_eq!(candidate.branch.as_deref(), Some("codex/tts-providers"));
    let record = candidate.evidence_path.join("source.json");
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    legacy.as_object_mut().unwrap().remove("branch");
    atomic_json(&record, &legacy).unwrap();
    assert_eq!(manager.info(&candidate.source_id).unwrap().branch, None);
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn explicit_branch_never_silently_accepts_a_tag_or_another_branch() {
    let (_temp, paths, store, tasks, manager) = fixture();
    fake_git(&paths,"operation=''\nfor arg do\ncase \"$arg\" in clone|symbolic-ref) operation=\"$arg\";; esac\nlast=\"$arg\"\ndone\ncase \"$operation\" in clone) mkdir \"$last\"; exit 0;; symbolic-ref) printf 'refs/heads/unrequested\\n'; exit 0;; esac\nexit 1");
    let (stop, _) = watch::channel(false);
    let task = manager
        .submit(
            AcquireSourceInput {
                url: "https://github.com/example/disposable".into(),
                branch: Some("codex/tts-providers".into()),
                name: None,
            },
            stop.subscribe(),
        )
        .await
        .unwrap();
    let done = final_task(&tasks, &task.task_id).await;
    assert_eq!(done.error.unwrap().code, "SOURCE_BRANCH_UNAVAILABLE");
    let candidate = manager.list().unwrap().pop().unwrap();
    assert_eq!(candidate.status, "failed");
    assert!(candidate.source_path.is_none());
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
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
pub(super) fn fixture() -> (
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
    let git =
        GitEnvironmentManager::with_processes(paths.data_root().into(), None, store.processes());
    let manager =
        SourceAcquisitionManager::new(paths.clone(), store.clone(), tasks.clone(), git).unwrap();
    (temp, paths, store, tasks, manager)
}
pub(super) fn fake_git(paths: &RuntimePaths, body: &str) -> PathBuf {
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
pub(super) async fn final_task(tasks: &BuildManager, id: &str) -> TaskRecord {
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
                    branch: None,
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
                branch: None,
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
                    branch: None,
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
                branch: None,
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
                branch: None,
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
        legacy_build_inspection: false,
        branch: None,
        schema_version: 1,
        source_id: id.clone(),
        task_id: task.task_id,
        name: "Interrupted disposable".into(),
        status: "acquiring".into(),
        phase: "downloading".into(),
        url: "https://github.com/example/disposable.git".into(),
        provider: "github".into(),
        local_source_path: None,
        capture_policy: None,
        build_source_digest: None,
        excluded_paths: Vec::new(),
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
        processes: Default::default(),
        scope: None,
        hooks: Default::default(),
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
    assert!(control.processes.shutdown().await.is_empty());
    assert!(control
        .processes
        .snapshot()
        .iter()
        .all(|process| process.resource_closed()));
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
                branch: None,
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

fn capture_control(root: &Path) -> (watch::Sender<bool>, watch::Sender<bool>, RunControl) {
    let (stop, stop_rx) = watch::channel(false);
    let (cancel, cancel_rx) = watch::channel(false);
    (
        stop,
        cancel,
        RunControl {
            stop: stop_rx,
            cancel: cancel_rx,
            deadline: Instant::now() + TIMEOUT,
            monitor: root.to_path_buf(),
            processes: Default::default(),
            scope: None,
            hooks: Default::default(),
        },
    )
}
fn disposable_node_source(root: &Path, web: &str) {
    fs::create_dir_all(root.join(web)).unwrap();
    fs::create_dir(root.join("server")).unwrap();
    fs::write(
        root.join(web).join("index.html"),
        "<!doctype html><title>Disposable capture</title>",
    )
    .unwrap();
    fs::write(
        root.join("server/main.mjs"),
        "throw new Error('source must not execute');",
    )
    .unwrap();
    fs::write(root.join("package.json"), br#"{"name":"disposable-local-source","version":"1.0.0","scripts":{"postinstall":"touch must-not-execute"}}"#).unwrap();
    fs::write(root.join("package-lock.json"), br#"{"lockfileVersion":3,"packages":{"":{"name":"disposable-local-source","version":"1.0.0"}}}"#).unwrap();
    fs::write(root.join("local-app.json"),serde_json::to_vec(&json!({"packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service","app":{"id":"org.example.disposable","name":"Disposable local source","version":"1.0.0"},"web":{"root":web,"entry":"index.html","routing":"files"},"backend":{"root":"server","entry":"main.mjs","protocolVersion":1,"nodeMajor":24}})).unwrap()).unwrap();
}

#[tokio::test]
async fn local_capture_preserves_public_source_reports_exclusions_and_never_executes_or_registers()
{
    let (_temp, paths, store, tasks, manager) = fixture();
    let original = sample();
    let root = original.path().canonicalize().unwrap();
    disposable_node_source(&root, "public");
    let forbidden = paths.data_root().join("capture-must-not-run-git");
    fake_git(&paths, &format!("touch '{}'\nexit 1", forbidden.display()));
    for name in [
        ".git",
        ".platform",
        "node_modules",
        "data",
        "credentials",
        "secrets",
        "tokens",
        "user",
        "work",
        ".env",
        "private.key",
        "business.sqlite",
        "business.sqlite-wal",
        "business.db-shm",
    ] {
        symlink("/nonexistent-must-not-be-opened", root.join(name)).unwrap();
    }
    let (stop, _) = watch::channel(false);
    let task = manager
        .capture(
            CaptureLocalSourceInput {
                source_path: root.clone(),
                name: Some("Disposable copied Node source".into()),
            },
            stop.subscribe(),
        )
        .await
        .unwrap();
    assert_eq!(task.task_type, "sourceCapture");
    let result = final_task(&tasks, &task.task_id).await;
    assert_eq!(result.status, crate::TaskStatus::Succeeded, "{result:?}");
    let candidate = manager.list().unwrap().pop().unwrap();
    assert_eq!(candidate.provider, "local");
    assert_eq!(candidate.url, "");
    assert!(!candidate.shallow);
    assert!(candidate.git.is_none() && candidate.commit.is_none());
    assert_eq!(candidate.local_source_path, Some(root.clone()));
    assert_eq!(candidate.capture_policy.as_deref(), Some(CAPTURE_POLICY));
    assert!(candidate
        .capture_notice
        .unwrap()
        .contains("未知文件中的秘密无法通用识别"));
    assert_eq!(candidate.inspection.unwrap().kind, "node-service-source");
    assert_eq!(candidate.execution_state, "not-executed");
    assert!(!candidate.registration_available);
    let copied = candidate.source_path.unwrap();
    assert!(copied.join("public/index.html").is_file());
    assert_eq!(
        fs::read(copied.join("server/main.mjs")).unwrap(),
        fs::read(root.join("server/main.mjs")).unwrap()
    );
    for name in [".git", "data", ".env", "private.key", "business.sqlite"] {
        assert!(candidate.excluded_paths.contains(&name.to_owned()));
        assert!(!copied.join(name).exists());
        assert!(fs::symlink_metadata(root.join(name))
            .unwrap()
            .file_type()
            .is_symlink());
    }
    assert!(!forbidden.exists());
    assert!(!root.join("must-not-execute").exists());
    assert!(!copied.join("must-not-execute").exists());
    assert_eq!(
        directory_digest(&copied).unwrap(),
        candidate.content_digest.unwrap()
    );
    assert!(store.list().await.unwrap().is_empty());
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

#[tokio::test]
async fn local_ready_verification_requires_exact_source_task_lease_and_rechecks_bytes_and_modes() {
    let (_temp, _paths, store, tasks, manager) = fixture();
    let source = sample();
    let source = source.path().canonicalize().unwrap();
    fs::write(
        source.join("README.md"),
        "ordinary source requiring adaptation",
    )
    .unwrap();
    let (stop, _) = watch::channel(false);
    let task = manager
        .capture(
            CaptureLocalSourceInput {
                source_path: source,
                name: None,
            },
            stop.subscribe(),
        )
        .await
        .unwrap();
    assert_eq!(
        final_task(&tasks, &task.task_id).await.status,
        crate::TaskStatus::Succeeded
    );
    let candidate = manager.list().unwrap().pop().unwrap();
    let digest = candidate.content_digest.clone().unwrap();
    assert_eq!(
        manager
            .verify_ready_leased(&candidate.source_id, &digest, "missing")
            .unwrap_err()
            .code,
        "SOURCE_LEASE_REQUIRED"
    );
    let owner = tasks
        .create_workspace_operation("nodeInstallPrepare")
        .unwrap();
    let lease = store
        .reserve_task(&format!("source:{}", candidate.source_id), &owner.task_id)
        .unwrap();
    assert_eq!(
        manager
            .verify_ready_leased(&candidate.source_id, &digest, "different")
            .unwrap_err()
            .code,
        "SOURCE_LEASE_REQUIRED"
    );
    store.begin_workspace_migration().unwrap();
    assert!(
        manager
            .verify_ready_leased(&candidate.source_id, &digest, &owner.task_id)
            .is_ok(),
        "an already-leased operation can finish before migration"
    );
    assert_eq!(
        manager
            .verify_ready(&candidate.source_id, &digest)
            .unwrap_err()
            .code,
        "WORKSPACE_MIGRATING"
    );
    store.resume_workspace();
    let file = candidate.source_path.unwrap().join("README.md");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        manager
            .verify_ready_leased(&candidate.source_id, &digest, &owner.task_id)
            .unwrap_err()
            .code,
        "SOURCE_CHANGED"
    );
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&file, "changed source").unwrap();
    assert_eq!(
        manager
            .verify_ready_leased(&candidate.source_id, &digest, &owner.task_id)
            .unwrap_err()
            .code,
        "SOURCE_CHANGED"
    );
    drop(lease);
    tasks.finish(&owner.task_id, Ok(json!({}))).unwrap();
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

#[test]
fn local_capture_rejects_data_roots_links_hardlinks_and_cross_root_dependencies() {
    let (_temp, paths) = RuntimePaths::for_test();
    for root in [paths.data_root(), paths.data_root().parent().unwrap()] {
        assert!(validate_local_root(root, &paths).is_err());
    }
    if let Some(home) = std::env::var_os("HOME") {
        assert!(validate_local_root(Path::new(&home), &paths).is_err());
    }
    assert!(validate_local_root(Path::new("relative"), &paths).is_err());
    let source = sample();
    let root = source.path().canonicalize().unwrap();
    let (_stop, _cancel, control) = capture_control(&root);
    symlink("/no-follow", root.join("source.js")).unwrap();
    assert!(local_plan(&root, &control).is_err());
    fs::remove_file(root.join("source.js")).unwrap();
    fs::write(root.join("source.js"), "ordinary").unwrap();
    fs::hard_link(root.join("source.js"), root.join("alias.js")).unwrap();
    assert!(local_plan(&root, &control).is_err());
    fs::remove_file(root.join("alias.js")).unwrap();
    for value in [
        json!({"dependencies":{"unsafe":"file:../outside"}}),
        json!({"dependencies":{"unsafe":"~"}}),
        json!({"dependencies":{"unsafe":"~/outside"}}),
        json!({"dependencies":{"unsafe":"~\\outside"}}),
        json!({"dependencies":{"unsafe":"file:~/outside"}}),
        json!({"dependencies":{"unsafe":"link:~/outside"}}),
        json!({"packages":{"node_modules/p":{"link":true,"resolved":"~2.1.34"}}}),
        json!({"overrides":{"nested":{"unsafe":"file:../../outside"}}}),
        json!({"packages":{"node_modules/p":{"link":true,"resolved":"../outside"}}}),
        json!({"dependencies":{"unsafe":{"version":"file:%2e%2e/outside"}}}),
        json!({"workspaces":["../outside/*"]}),
    ] {
        fs::write(
            root.join("package.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        let plan = local_plan(&root, &control).unwrap();
        assert!(
            validate_local_dependencies(&root, &plan, &control).is_err(),
            "{value}"
        );
    }
}

#[test]
fn local_capture_distinguishes_registry_tilde_ranges_from_home_paths() {
    let source = sample();
    let root = source.path().canonicalize().unwrap();
    let (_stop, _cancel, control) = capture_control(&root);
    fs::write(
        root.join("package.json"),
        serde_json::to_vec(&json!({
            "dependencies":{"regular":"~2.1.34"},
            "devDependencies":{"dev":"~1.0"},
            "optionalDependencies":{"optional":"~0.9.0-beta.1"},
            "peerDependencies":{"peer":"~4"},
            "overrides":{"nested":{"child":"~2.1.34"}}
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("package-lock.json"),
        serde_json::to_vec(&json!({
            "lockfileVersion":3,
            "packages":{
                "":{"dependencies":{"regular":"~2.1.34"}},
                "node_modules/regular":{"version":"2.1.34","dependencies":{"nested":"~1.0.0"}}
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let plan = local_plan(&root, &control).unwrap();
    assert!(validate_local_dependencies(&root, &plan, &control)
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn public_capture_accepts_registry_tilde_ranges_without_executing_source() {
    let (_temp, _paths, store, tasks, manager) = fixture();
    let source = sample();
    let root = source.path().canonicalize().unwrap();
    disposable_node_source(&root, "public");
    let path = root.join("package.json");
    let mut package: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    package["dependencies"] = json!({"regular":"~2.1.34"});
    fs::write(&path, serde_json::to_vec(&package).unwrap()).unwrap();
    let path = root.join("package-lock.json");
    let mut lock: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    lock["packages"][""]["dependencies"] = json!({"regular":"~2.1.34"});
    lock["packages"]["node_modules/regular"] =
        json!({"version":"2.1.34","dependencies":{"nested":"~1.0.0"}});
    fs::write(&path, serde_json::to_vec(&lock).unwrap()).unwrap();
    let (stop, _) = watch::channel(false);
    let task = manager
        .capture(
            CaptureLocalSourceInput {
                source_path: root.clone(),
                name: None,
            },
            stop.subscribe(),
        )
        .await
        .unwrap();
    let completed = final_task(&tasks, &task.task_id).await;
    assert_eq!(
        completed.status,
        crate::TaskStatus::Succeeded,
        "{completed:?}"
    );
    let candidate = manager.list().unwrap().pop().unwrap();
    assert_eq!(candidate.inspection.unwrap().kind, "node-service-source");
    assert_eq!(candidate.execution_state, "not-executed");
    assert!(!root.join("must-not-execute").exists());
    assert!(!candidate
        .source_path
        .unwrap()
        .join("must-not-execute")
        .exists());
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

#[test]
fn local_capture_internal_files_and_workspaces_are_bounded_to_copied_material() {
    let source = sample();
    let root = source.path().canonicalize().unwrap();
    fs::create_dir_all(root.join("packages/inner")).unwrap();
    fs::create_dir(root.join("vendor")).unwrap();
    fs::write(root.join("vendor/sdk.tgz"), "disposable archive bytes").unwrap();
    fs::write(
        root.join("package.json"),
        br#"{"workspaces":["packages/*"]}"#,
    )
    .unwrap();
    fs::write(
        root.join("packages/inner/package.json"),
        br#"{"dependencies":{"sdk":"file:../../vendor/sdk.tgz"}}"#,
    )
    .unwrap();
    fs::write(
        root.join("package-lock.json"),
        br#"{"packages":{"node_modules/inner":{"link":true,"resolved":"packages/inner"}}}"#,
    )
    .unwrap();
    let (_stop, _cancel, control) = capture_control(&root);
    let plan = local_plan(&root, &control).unwrap();
    assert!(validate_local_dependencies(&root, &plan, &control)
        .unwrap()
        .is_empty());
    fs::write(
        root.join("packages/inner/package.json"),
        br#"{"dependencies":{"unsafe":"file:../../data/excluded.tgz"}}"#,
    )
    .unwrap();
    let plan = local_plan(&root, &control).unwrap();
    assert!(validate_local_dependencies(&root, &plan, &control).is_err());
}

#[tokio::test]
async fn local_capture_reports_config_adaptation_and_keeps_declared_node_web_dist() {
    let (_temp, _paths, store, tasks, manager) = fixture();
    let source = sample();
    let root = source.path().canonicalize().unwrap();
    disposable_node_source(&root, "dist");
    fs::create_dir(root.join("config")).unwrap();
    fs::write(
        root.join("config/settings.js"),
        "this is code but must be reviewed after exclusion",
    )
    .unwrap();
    let (stop, _) = watch::channel(false);
    let task = manager
        .capture(
            CaptureLocalSourceInput {
                source_path: root,
                name: None,
            },
            stop.subscribe(),
        )
        .await
        .unwrap();
    assert_eq!(
        final_task(&tasks, &task.task_id).await.status,
        crate::TaskStatus::Succeeded
    );
    let candidate = manager.list().unwrap().pop().unwrap();
    assert!(candidate
        .source_path
        .unwrap()
        .join("dist/index.html")
        .is_file());
    assert!(candidate.excluded_paths.contains(&"config".to_owned()));
    assert_eq!(candidate.inspection.unwrap().kind, "needs-adaptation");
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}

#[test]
fn local_copy_detects_source_edits_and_stops_before_publication() {
    let source = sample();
    let root = source.path().canonicalize().unwrap();
    let candidate = sample();
    let candidate = candidate.path().canonicalize().unwrap();
    fs::write(root.join("source.js"), "before").unwrap();
    let (_stop, cancel, control) = capture_control(&candidate);
    let plan = local_plan(&root, &control).unwrap();
    fs::write(root.join("source.js"), "after").unwrap();
    assert_eq!(
        capture_plan(&root, &candidate, plan, &control)
            .err()
            .unwrap()
            .code,
        "SOURCE_CHANGED"
    );
    assert_eq!(fs::read(root.join("source.js")).unwrap(), b"after");
    let plan = local_plan(&root, &control).unwrap();
    cancel.send(true).unwrap();
    assert_eq!(
        capture_plan(&root, &candidate, plan, &control)
            .err()
            .unwrap()
            .code,
        "SOURCE_ACQUISITION_CANCELLED"
    );
}

#[test]
fn local_capture_excludes_declared_project_output_and_bounds_metadata_issues() {
    let source = sample();
    let root = source.path().canonicalize().unwrap();
    fs::write(
        root.join("project.json"),
        br#"{"outputDir":"generated-web"}"#,
    )
    .unwrap();
    for excluded in ["generated-web", "attachments", "session-wal", "session-shm"] {
        // These nonexistent targets prove exclusion uses only the entry name.
        symlink(
            "/disposable-target-that-must-not-be-read",
            root.join(excluded),
        )
        .unwrap();
    }
    fs::create_dir(root.join("public")).unwrap();
    fs::write(root.join("public/index.html"), "runtime material").unwrap();
    for directory in ["one", "two"] {
        fs::create_dir(root.join(directory)).unwrap();
        fs::write(root.join(directory).join("package.json"), "invalid json").unwrap();
    }
    let (_stop, _cancel, control) = capture_control(&root);
    let plan = local_plan(&root, &control).unwrap();
    assert_eq!(plan.excluded.len(), 4);
    assert!(plan.files.contains_key("public/index.html"));
    assert_eq!(
        validate_local_dependencies(&root, &plan, &control)
            .unwrap()
            .len(),
        1
    );
}
