//! Disposable native NTFS source snapshots; real Git uses the fixed official
//! development payload and is opt-in. No existing project or global Git is used.
use super::*;
use platform_host::WindowsWorkspace;
use std::fs;

struct Fixture {
    parent: WindowsSourceDirectory,
    workspace: WindowsWorkspace,
    tasks: Arc<DevelopmentTasks>,
    manager: Arc<SourceManager>,
}
impl Fixture {
    fn new() -> Self {
        let artifacts = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(".test-artifacts");
        fs::create_dir_all(&artifacts).unwrap();
        let parent = WindowsSourceDirectory::create_private_in(
            &artifacts,
            &format!(
                "source-disposable-{}",
                &Uuid::new_v4().simple().to_string()[..16]
            ),
        )
        .unwrap();
        let workspace = platform_host::disposable_node_workspace(
            &parent.path().join("windows-node-disposable-data"),
        )
        .unwrap();
        workspace
            .root()
            .create_directory("workspace")
            .unwrap()
            .create_directory(".platform")
            .unwrap();
        let tasks = DevelopmentTasks::new(workspace.root()).unwrap();
        let manager = SourceManager::new(
            workspace.root(),
            DevelopmentTools::new(parent.path().join("no-tools-used")),
            tasks.clone(),
        )
        .unwrap();
        Self {
            parent,
            workspace,
            tasks,
            manager,
        }
    }
    fn source(&self) -> WindowsSourceDirectory {
        let source = self
            .parent
            .create_private_directory("原始 中文 源码")
            .unwrap();
        source.write_new("project.json",&serde_json::to_vec(&json!({"manifestVersion":1,"id":"disposable_source","name":"可丢弃源码","sdkApiVersion":1,"sourceDir":"app","outputDir":"dist","entry":"dist/index.html","database":"data/main.sqlite","attachmentsDir":"data/attachments","build":{"script":"build"},"docs":{"readme":"README.md","database":"docs/database.md","platform":"docs/platform.md"}})).unwrap()).unwrap();
        let app = source.create_private_directory("app").unwrap();
        app.write_new(
            "package.json",
            br#"{"name":"disposable","version":"1.0.0","scripts":{"build":"node build.cjs"}}"#,
        )
        .unwrap();
        app.write_new(
            "package-lock.json",
            br#"{"name":"disposable","version":"1.0.0","lockfileVersion":3,"packages":{}}"#,
        )
        .unwrap();
        app.write_new(
            "build.cjs",
            b"require('fs').writeFileSync('must-not-run','executed')",
        )
        .unwrap();
        app.write_new("source.txt", b"original source bytes")
            .unwrap();
        source
            .write_new("README.md", b"disposable declaration")
            .unwrap();
        let docs = source.create_private_directory("docs").unwrap();
        docs.write_new("database.md", b"database contract").unwrap();
        docs.write_new("platform.md", b"platform contract").unwrap();
        source
    }
    async fn completed(&self, task: &Value) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let value = self.tasks.get(task["taskId"].as_str().unwrap()).unwrap();
            if value["final"] == true {
                return value;
            }
            assert!(Instant::now() < deadline, "source task did not finish");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}
fn control() -> (watch::Sender<bool>, Control) {
    let (send, cancel) = watch::channel(false);
    (
        send,
        Control {
            cancel,
            deadline: Instant::now() + TIMEOUT,
        },
    )
}

#[test]
fn public_urls_are_strict_and_canonical_without_credentials_or_redirect_targets() {
    assert_eq!(
        policy::validate_url("https://github.com/openai/example/")
            .unwrap()
            .0,
        "https://github.com/openai/example.git"
    );
    for url in [
        "http://github.com/a/b",
        "https://github.com/a/b/tree/main",
        "https://u:p@github.com/a/b",
        "https://github.com:443/a/b",
        "https://github.com/a/b?token=x",
        "https://github.com/a/b#main",
        "https://gitee.com/a/%62",
        "file:///D:/repo",
        "ssh://github.com/a/b",
        "https://evil.example/a/b",
    ] {
        assert!(policy::validate_url(url).is_err(), "{url}");
    }
}
#[test]
fn tree_parser_rejects_links_reserved_names_and_windows_unicode_collisions() {
    let row = |path: &str| format!("100644 blob {} 1\t{path}\0", "1".repeat(40));
    for path in [
        "../escape",
        ".git/config",
        "git~1/HEAD",
        "NUL.txt",
        "COM¹.log",
        "aux",
        "trailing.",
        "A:B",
        "bad?name",
        "bad\\path",
    ] {
        assert!(policy::parse_tree(row(path).as_bytes()).is_err(), "{path}");
    }
    for paths in [
        ["A/file", "a/other"],
        ["é.txt", "e\u{301}.txt"],
        ["file", "file/nested"],
    ] {
        assert!(
            policy::parse_tree(format!("{}{}", row(paths[0]), row(paths[1])).as_bytes()).is_err()
        );
    }
    for mode in ["120000", "160000"] {
        assert!(
            policy::parse_tree(format!("{mode} blob {} 1\tlink\0", "1".repeat(40)).as_bytes())
                .is_err()
        );
    }
    assert!(policy::parse_tree(
        format!(
            "100644 blob {} {}\tlarge.bin\0",
            "1".repeat(40),
            MAX_BLOB + 1
        )
        .as_bytes()
    )
    .is_err());
    assert!(policy::parse_tree(row("safe.txt").trim_end_matches('\0').as_bytes()).is_err());
}

#[tokio::test]
async fn local_capture_excludes_private_material_keeps_source_unchanged_and_copies_projection() {
    let fixture = Fixture::new();
    let source = fixture.source();
    source
        .write_new(".env", b"ARTIFICIAL_PRIVATE_SENTINEL")
        .unwrap();
    source
        .create_private_directory("data")
        .unwrap()
        .write_new("main.sqlite", b"existing data evidence")
        .unwrap();
    source
        .create_private_directory("node_modules")
        .unwrap()
        .write_new("must-not-copy", b"existing dependencies")
        .unwrap();
    source
        .create_private_directory("dist")
        .unwrap()
        .write_new("index.html", b"existing build output")
        .unwrap();
    let original = fs::read(source.path().join("project.json")).unwrap();
    let inspection = fixture.manager.inspect_local(source.path()).unwrap();
    assert_eq!(inspection["inspection"]["kind"], "project-v1-source");
    assert!(
        platform_host::windows_development::area(&fixture.workspace.root(), "sources", false)
            .unwrap()
            .is_none()
    );
    let task = fixture
        .manager
        .capture(CaptureInput {
            source_path: source.path().into(),
            name: None,
        })
        .unwrap();
    let final_task = fixture.completed(&task).await;
    assert_eq!(final_task["status"], "succeeded", "{final_task}");
    let id = task["sourceId"].as_str().unwrap();
    let candidates = fixture.manager.list().unwrap();
    assert_eq!(candidates.as_array().unwrap().len(), 1);
    assert_eq!(candidates[0]["status"], "ready");
    assert_eq!(candidates[0]["inspection"]["kind"], "project-v1-source");
    let lease = fixture.manager.candidate(id).unwrap();
    for path in [".env", "data", "node_modules", "dist", "app/must-not-run"] {
        assert!(!lease.root.path().join(path).exists(), "{path}");
    }
    assert_eq!(
        fs::read(source.path().join("project.json")).unwrap(),
        original
    );
    assert_eq!(
        fs::read(source.path().join(".env")).unwrap(),
        b"ARTIFICIAL_PRIVATE_SENTINEL"
    );
    assert_eq!(
        fs::read(source.path().join("data/main.sqlite")).unwrap(),
        b"existing data evidence"
    );
    assert!(!source.path().join("app/must-not-run").exists());
    let target = fixture
        .workspace
        .root()
        .create_directory("build-copy")
        .unwrap();
    lease.copy_subdirectory_to("app", &target).unwrap();
    assert_eq!(
        target.read_bounded("source.txt", 100).unwrap().bytes,
        b"original source bytes"
    );
    assert!(!target.path().join("project.json").exists());
    assert!(!target.path().join("data").exists());
    assert!(!target.path().join("must-not-run").exists());
    assert!(lease.copy_subdirectory_to("../outside", &target).is_err());
    let cancelled_target = fixture
        .workspace
        .root()
        .create_directory("cancelled-copy")
        .unwrap();
    let (_send, cancelled) = watch::channel(true);
    assert_eq!(
        lease
            .copy_subdirectory_to_with_cancellation("app", &cancelled_target, cancelled)
            .unwrap_err()
            .code,
        "SOURCE_ACQUISITION_CANCELLED"
    );
    assert!(cancelled_target.list().unwrap().is_empty());
    lease.verify().unwrap();
}

#[tokio::test]
async fn manifest_directory_case_must_match_actual_identity_and_capture_names_match_main() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let manifest_path = source.path().join("project.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["sourceDir"] = json!("APP");
    let original = serde_json::to_vec(&manifest).unwrap();
    fs::write(&manifest_path, &original).unwrap();
    assert_eq!(
        fixture.manager.inspect_local(source.path()).unwrap()["inspection"]["kind"],
        "needs-adaptation"
    );
    let invalid_name = fixture
        .manager
        .capture(CaptureInput {
            source_path: source.path().into(),
            name: Some("中".repeat(201)),
        })
        .unwrap_err();
    assert_eq!(invalid_name.code, "SOURCE_ACQUISITION_INVALID");
    assert!(
        platform_host::windows_development::area(&fixture.workspace.root(), "sources", false)
            .unwrap()
            .is_none()
    );
    let name = "中".repeat(200);
    let task = fixture
        .manager
        .capture(CaptureInput {
            source_path: source.path().into(),
            name: Some(name.clone()),
        })
        .unwrap();
    assert_eq!(fixture.completed(&task).await["status"], "succeeded");
    let lease = fixture
        .manager
        .candidate(task["sourceId"].as_str().unwrap())
        .unwrap();
    assert_eq!(lease.name, name);
    assert_eq!(lease.inspection["kind"], "needs-adaptation");
    let target = fixture
        .workspace
        .root()
        .create_directory("case-mismatch-copy")
        .unwrap();
    assert_eq!(
        lease.copy_subdirectory_to("APP", &target).unwrap_err().code,
        "SOURCE_MANIFEST_PATH_MISMATCH"
    );
    assert!(target.list().unwrap().is_empty());
    assert_eq!(fs::read(manifest_path).unwrap(), original);
}

#[test]
fn excluded_hardlinks_are_not_opened_and_selected_hardlinks_are_rejected() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let sentinel = fixture.parent.path().join("outside-secret-sentinel");
    fs::write(&sentinel, b"ARTIFICIAL_OUTSIDE_SECRET").unwrap();
    fs::hard_link(&sentinel, source.path().join(".env.disposable")).unwrap();
    let (_send, control) = control();
    let plan = snapshot::scan(source.clone(), &control, true).unwrap();
    assert!(plan.excluded.iter().any(|path| path == ".env.disposable"));
    assert!(!plan.files.contains_key(".env.disposable"));
    drop(plan);
    fs::hard_link(&sentinel, source.path().join("selected-secret.txt")).unwrap();
    assert!(snapshot::scan(source.clone(), &control, true).is_err());
    assert_eq!(fs::read(&sentinel).unwrap(), b"ARTIFICIAL_OUTSIDE_SECRET");
    assert_eq!(
        fs::read(source.path().join("app/source.txt")).unwrap(),
        b"original source bytes"
    );
}

#[test]
fn source_snapshot_pins_selected_files_and_detects_added_inputs() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let (_send, control) = control();
    let plan = snapshot::scan(source.clone(), &control, true).unwrap();
    assert!(fs::write(source.path().join("app/source.txt"), b"changed").is_err());
    assert!(fs::rename(
        source.path().join("app/source.txt"),
        source.path().join("app/replaced.txt")
    )
    .is_err());
    source
        .write_new("added-after-scan.txt", b"new source input")
        .unwrap();
    assert_eq!(
        plan.verify(&control, true).unwrap_err().code,
        "SOURCE_LOCAL_CHANGED"
    );
    assert_eq!(
        fs::read(source.path().join("app/source.txt")).unwrap(),
        b"original source bytes"
    );
}

#[tokio::test]
async fn npm_tilde_semver_is_allowed_while_home_relative_dependencies_remain_rejected() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let package = source.path().join("app/package.json");
    fs::write(
        &package,
        br#"{"name":"disposable","dependencies":{"range":"~1.2.12"}}"#,
    )
    .unwrap();
    let lock = source.path().join("app/package-lock.json");
    fs::write(&lock, br#"{"lockfileVersion":3,"packages":{"":{"dependencies":{"range":"~1.2.12"}},"node_modules/range":{"version":"1.2.12","dependencies":{"optional-range":"~2.3.3"}}}}"#).unwrap();
    let task = fixture
        .manager
        .capture(CaptureInput {
            source_path: source.path().into(),
            name: None,
        })
        .unwrap();
    assert_eq!(fixture.completed(&task).await["status"], "succeeded");
    fs::write(
        &package,
        br#"{"name":"disposable","dependencies":{"outside":"~/outside"}}"#,
    )
    .unwrap();
    let task = fixture
        .manager
        .capture(CaptureInput {
            source_path: source.path().into(),
            name: None,
        })
        .unwrap();
    let result = fixture.completed(&task).await;
    assert_eq!(result["status"], "failed");
    assert_eq!(result["error"]["code"], "SOURCE_ACQUISITION_INVALID");
}

#[tokio::test]
async fn out_of_scope_local_dependencies_fail_without_mutating_the_source() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let package = source.path().join("app/package.json");
    let original = br#"{"name":"disposable","dependencies":{"outside":"file:../../private"}}"#;
    fs::write(&package, original).unwrap();
    let task = fixture
        .manager
        .capture(CaptureInput {
            source_path: source.path().into(),
            name: None,
        })
        .unwrap();
    let final_task = fixture.completed(&task).await;
    assert_eq!(final_task["status"], "failed");
    assert_eq!(fs::read(package).unwrap(), original);
    assert_eq!(fixture.manager.list().unwrap()[0]["status"], "failed");
    assert!(fixture
        .manager
        .candidate(task["sourceId"].as_str().unwrap())
        .is_err());
}

#[tokio::test]
async fn cancelled_and_interrupted_captures_remain_evidence_without_automatic_retry() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let task = fixture
        .manager
        .capture(CaptureInput {
            source_path: source.path().into(),
            name: None,
        })
        .unwrap();
    fixture
        .tasks
        .cancel(task["taskId"].as_str().unwrap())
        .unwrap();
    let final_task = fixture.completed(&task).await;
    assert_eq!(final_task["status"], "failed");
    assert_eq!(fixture.manager.list().unwrap()[0]["status"], "cancelled");
    let second = fixture
        .manager
        .capture(CaptureInput {
            source_path: source.path().into(),
            name: None,
        })
        .unwrap();
    let runner = fixture.manager.runners.lock().unwrap().pop().unwrap();
    runner.abort();
    assert!(runner.await.unwrap_err().is_cancelled());
    let records = SourceManager::new(
        fixture.workspace.root(),
        DevelopmentTools::new(fixture.parent.path().join("unused")),
        fixture.tasks.clone(),
    )
    .unwrap();
    let list = records.list().unwrap();
    assert!(list
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value["sourceId"] == second["sourceId"] && value["status"] == "interrupted"));
    let slot = records.slot(second["sourceId"].as_str().unwrap()).unwrap();
    assert!(!slot.path().join("source.json").exists());
    assert!(!source.path().join("app/must-not-run").exists());
}

#[tokio::test]
async fn ready_candidate_survives_restart_and_replacement_payload_is_never_adopted() {
    let fixture = Fixture::new();
    let source = fixture.source();
    let task = fixture
        .manager
        .capture(CaptureInput {
            source_path: source.path().into(),
            name: None,
        })
        .unwrap();
    assert_eq!(fixture.completed(&task).await["status"], "succeeded");
    let id = task["sourceId"].as_str().unwrap();
    let digest = fixture.manager.candidate(id).unwrap().content_digest;
    let tasks = DevelopmentTasks::new(fixture.workspace.root()).unwrap();
    let reopened = SourceManager::new(
        fixture.workspace.root(),
        DevelopmentTools::new(fixture.parent.path().join("unused")),
        tasks,
    )
    .unwrap();
    assert_eq!(reopened.candidate(id).unwrap().content_digest, digest);
    let slot = reopened.slot(id).unwrap();
    let original = slot.path().join("repository");
    let retained = fixture.parent.path().join("retained-original-candidate");
    fs::rename(&original, &retained).unwrap();
    slot.create_directory("repository").unwrap();
    assert!(reopened.candidate(id).is_err());
    assert_eq!(reopened.list().unwrap()[0]["status"], "unavailable");
    assert_eq!(
        fs::read(retained.join("app/source.txt")).unwrap(),
        b"original source bytes"
    );
}

#[tokio::test]
async fn unknown_source_slot_is_listed_unavailable_and_preserved() {
    let fixture = Fixture::new();
    let root = platform_host::windows_development::area(&fixture.workspace.root(), "sources", true)
        .unwrap()
        .unwrap();
    let id = format!("src_{}", Uuid::new_v4().simple());
    let slot = root.create_directory(&id).unwrap();
    slot.write_new("retained-evidence.txt", b"unknown slot evidence")
        .unwrap();
    let list = fixture.manager.list().unwrap();
    assert_eq!(list[0]["status"], "unavailable");
    assert!(fixture.manager.candidate(&id).is_err());
    assert_eq!(
        slot.read_bounded("retained-evidence.txt", 100)
            .unwrap()
            .bytes,
        b"unknown slot evidence"
    );
}

fn official_git_manager(fixture: &Fixture) -> Arc<SourceManager> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let source = std::env::var_os("WINDOWS_DEVELOPMENT_GIT_TEST_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            repository
                .join(".test-artifacts/windows-development-tools-20261023/official-git-second")
        });
    let expected: Value = serde_json::from_str(include_str!(
        "../../../../scripts/windows-development-git-release.json"
    ))
    .unwrap();
    let metadata = fs::read(source.join("development-git.json"))
        .expect("prepare the independently verified official MinGit fixture");
    assert_eq!(
        serde_json::from_slice::<Value>(&metadata).unwrap(),
        expected
    );
    let resources = fixture
        .parent
        .create_private_directory("Git resources 中文")
        .unwrap();
    let target = resources
        .create_private_directory("development-git")
        .unwrap();
    for file in expected["files"].as_array().unwrap() {
        let path = file["path"].as_str().unwrap();
        let output = target.path().join(path);
        fs::create_dir_all(output.parent().unwrap()).unwrap();
        assert!(fs::symlink_metadata(source.join(path)).unwrap().is_file());
        fs::copy(source.join(path), output).unwrap();
    }
    target.write_new("development-git.json", &metadata).unwrap();
    SourceManager::new(
        fixture.workspace.root(),
        DevelopmentTools::new(resources.path().into()),
        fixture.tasks.clone(),
    )
    .unwrap()
}
fn git_record(ctx: &TaskContext) -> SourceRecord {
    SourceRecord {
        schema_version: 1,
        source_id: format!("src_{}", Uuid::new_v4().simple()),
        task_id: ctx.id().into(),
        name: "Disposable Git candidate".into(),
        status: "acquiring".into(),
        phase: "environment".into(),
        url: "https://github.com/disposable/example.git".into(),
        provider: "github".into(),
        local_source_path: None,
        capture_policy: None,
        excluded_paths: Vec::new(),
        created_at: now(),
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
    }
}

#[tokio::test]
#[ignore = "Requires the independently verified official private MinGit fixture"]
async fn official_mingit_objects_materialize_binary_unicode_files_without_checkout_or_scripts() {
    let fixture = Fixture::new();
    let manager = official_git_manager(&fixture);
    let source_id = format!("src_{}", Uuid::new_v4().simple());
    let ctx = fixture
        .tasks
        .start("sourceAcquire", Some(&source_id))
        .unwrap();
    let control = Control {
        cancel: ctx.cancellation(),
        deadline: Instant::now() + TIMEOUT,
    };
    let git = GitSession::new(&manager, &ctx).unwrap();
    git.initialize(&ctx, &control).await.unwrap();
    let binary = b"raw\0\xff\x80 binary contents\r\n";
    let binary_oid = policy::parse_commit(
        &git.execute(
            &ctx,
            &control,
            git.arguments(&["hash-object", "-w", "--stdin"]),
            binary.to_vec(),
            65536,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let script = b"require('fs').writeFileSync('must-not-run','unsafe')";
    let script_oid = policy::parse_commit(
        &git.execute(
            &ctx,
            &control,
            git.arguments(&["hash-object", "-w", "--stdin"]),
            script.to_vec(),
            65536,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let attributes = b"*.bin filter=must-not-run\n";
    let attributes_oid = policy::parse_commit(
        &git.execute(
            &ctx,
            &control,
            git.arguments(&["hash-object", "-w", "--stdin"]),
            attributes.to_vec(),
            65536,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let tree = format!("100644 blob {binary_oid}\t中文 file.bin\0")
        + &format!("100755 blob {script_oid}\tpostinstall.cjs\0")
        + &format!("100644 blob {attributes_oid}\t.gitattributes\0");
    let tree_oid = policy::parse_commit(
        &git.execute(
            &ctx,
            &control,
            git.arguments(&["mktree", "-z"]),
            tree.into_bytes(),
            65536,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let commit = policy::parse_commit(
        &git.execute(
            &ctx,
            &control,
            git.arguments(&[
                "-c",
                "user.name=Disposable",
                "-c",
                "user.email=disposable@example.invalid",
                "commit-tree",
                &tree_oid,
                "-m",
                "disposable fixture",
            ]),
            Vec::new(),
            65536,
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let sources =
        platform_host::windows_development::area(&fixture.workspace.root(), "sources", true)
            .unwrap()
            .unwrap();
    let slot = sources.create_directory(&source_id).unwrap();
    slot.write_new(
        ".windows-source-slot.json",
        &serde_json::to_vec(&SlotProof {
            schema_version: 1,
            source_id: source_id.clone(),
            task_id: ctx.id().into(),
            directory_identity: slot.identity().clone(),
        })
        .unwrap(),
    )
    .unwrap();
    let destination = slot.create_directory("repository").unwrap();
    let mut destination = Destination::new(destination);
    let mut record = git_record(&ctx);
    record.source_id = source_id.clone();
    manager
        .materialize_git(&ctx, &control, &mut record, &mut destination, &git, &commit)
        .await
        .unwrap();
    assert_eq!(
        destination
            .root
            .read_bounded("中文 file.bin", 65536)
            .unwrap()
            .bytes,
        binary
    );
    assert_eq!(
        destination
            .root
            .read_bounded("postinstall.cjs", 65536)
            .unwrap()
            .bytes,
        script
    );
    assert_eq!(destination.files.len(), 3);
    assert!(destination.files["postinstall.cjs"].executable);
    assert!(!destination.root.path().join(".git").exists());
    assert!(!destination.root.path().join("must-not-run").exists());
    assert!(!git.repository.path().join("must-not-run").exists());
    assert!(git.repository.path().join("objects").is_dir());
    assert_eq!(record.commit.as_deref(), Some(commit.as_str()));
    let digest = destination.digest();
    record.status = "ready".into();
    record.phase = "completed".into();
    record.completed_at = Some(now());
    record.file_count = Some(destination.files.len());
    record.total_bytes = Some(destination.files.values().map(|file| file.size).sum());
    record.content_digest = Some(digest.clone());
    let record_identity = slot
        .write_new("source.json", &serde_json::to_vec(&record).unwrap())
        .unwrap();
    slot.write_new(
        ".windows-source-ready.json",
        &serde_json::to_vec(&ReadyProof {
            schema_version: 1,
            source_id: source_id.clone(),
            payload_identity: destination.root.identity().clone(),
            record_identity,
            files: destination.files.into_values().collect(),
        })
        .unwrap(),
    )
    .unwrap();
    let lease = manager.candidate(&source_id).unwrap();
    assert!(lease.snapshot.files["postinstall.cjs"].record.executable);
    assert_eq!(lease.content_digest, digest);
    assert_eq!(lease.snapshot.digest(), digest);
    lease.verify().unwrap();
    let copied = ctx
        .directory()
        .create_directory("mode-preserving-copy")
        .unwrap();
    lease.copy_to(&copied).unwrap();
    assert_eq!(
        copied.read_bounded("postinstall.cjs", 65536).unwrap().bytes,
        script
    );
    assert!(!copied.path().join("must-not-run").exists());
    ctx.finish(Ok(
        json!({"sourceDigest":digest,"executionState":"not-executed"}),
    ))
    .unwrap();
}

#[tokio::test]
#[ignore = "Optional real public HTTPS fetch; requires the fixed official MinGit fixture and network"]
async fn optional_public_https_fetch_creates_only_an_independent_source_candidate() {
    let fixture = Fixture::new();
    let manager = official_git_manager(&fixture);
    let url = std::env::var("WINDOWS_SOURCE_HTTPS_TEST_URL")
        .unwrap_or_else(|_| "https://github.com/octocat/Hello-World.git".into());
    let task = manager
        .acquire(AcquireInput {
            url,
            name: Some("Disposable public HTTPS source".into()),
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    let final_task = loop {
        let task = fixture.tasks.get(task["taskId"].as_str().unwrap()).unwrap();
        if task["final"] == true {
            break task;
        }
        assert!(Instant::now() < deadline, "public fetch did not finish");
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert_eq!(final_task["status"], "succeeded", "{final_task}");
    let lease = manager
        .candidate(task["sourceId"].as_str().unwrap())
        .unwrap();
    lease.verify().unwrap();
    assert!(!lease.root.path().join(".git").exists());
    let listed = manager.list().unwrap();
    assert_eq!(listed[0]["status"], "ready");
    assert_eq!(listed[0]["executionState"], "not-executed");
    assert_eq!(listed[0]["registrationAvailable"], false);
}
