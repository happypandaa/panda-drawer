use super::*;
use crate::{CreateProjectInput, SqlCaller, SqlOptions, SqlRequest, TaskStatus};

#[test]
fn preview_manifest_rejects_implicit_deep_directories_and_collisions_before_materialization() {
    let project: ProjectManifest = serde_json::from_value(json!({
        "manifestVersion":1,"id":"disposable-preview","name":"Disposable preview",
        "sdkApiVersion":1,"sourceDir":"app","outputDir":"app/dist","entry":"app/dist/index.html",
        "database":"data/project.sqlite","attachmentsDir":"data/attachments","build":{},"docs":{}
    }))
    .unwrap();
    let mut manifest = version_data::DataSnapshotManifest {
        format_version: 1,
        project_id: project.id.clone(),
        sdk_api_version: 1,
        database_path: project.database.clone(),
        attachments_dir: project.attachments_dir.clone(),
        database: version_data::DataFile {
            bytes: 4096,
            sha256: "a".repeat(64),
        },
        attachments: BTreeMap::new(),
        directories: vec!["a/b".into()],
    };
    let limits = version_data::SnapshotLimits::default();
    assert!(validate_manifest_entries(&manifest, &project, limits, || Ok(())).is_err());
    manifest.directories = vec!["a".into(), "a/b".into()];
    assert!(validate_manifest_entries(&manifest, &project, limits, || Ok(())).is_ok());
    manifest.directories = vec!["a".into(), "a/b".into(), "a/c".into()];
    let strict = version_data::SnapshotLimits {
        max_entries: 2,
        ..limits
    };
    assert!(validate_manifest_entries(&manifest, &project, strict, || Ok(())).is_err());
    manifest.directories = vec!["a".into(), "A".into()];
    assert!(validate_manifest_entries(&manifest, &project, limits, || Ok(())).is_err());
    manifest.directories = vec![std::iter::repeat_n("deep", 300)
        .collect::<Vec<_>>()
        .join("/")];
    assert!(validate_manifest_entries(&manifest, &project, limits, || Ok(())).is_err());
    manifest.directories = vec!["a".into()];
    manifest.attachments.insert(
        "a/b/value.txt".into(),
        version_data::DataFile {
            bytes: 3,
            sha256: "b".repeat(64),
        },
    );
    assert!(validate_manifest_entries(&manifest, &project, limits, || Ok(())).is_err());
    manifest.directories.push("a/b".into());
    assert!(validate_manifest_entries(&manifest, &project, limits, || Ok(())).is_ok());
    assert_eq!(
        validate_manifest_entries(&manifest, &project, limits, || Err(interrupted()))
            .unwrap_err()
            .code,
        interrupted().code
    );
}

struct Fixture {
    _temporary: TempDir,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    manager: Arc<ProjectGitManager>,
    project: ProjectHandle,
    tools: GitToolchain,
    _stop: watch::Sender<bool>,
    receiver: watch::Receiver<bool>,
}
impl Fixture {
    async fn new() -> Self {
        let tool = PathBuf::from(
            std::env::var("LOCAL_PROJECTS_TEST_GIT_ROOT")
                .expect("explicit disposable official Git required"),
        )
        .canonicalize()
        .unwrap();
        assert!(tool.starts_with(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.test-artifacts")
                .canonicalize()
                .unwrap()
        ));
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let info = store
            .create(CreateProjectInput {
                name: "Disposable data version preview".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&info.id).unwrap();
        let tasks = BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into())
            .unwrap();
        let git = GitEnvironmentManager::new(paths.data_root().into(), Some(tool));
        let tools = git.resolve().await.unwrap();
        let manager = ProjectGitManager::new(paths, store.clone(), tasks.clone(), git).unwrap();
        let (sender, receiver) = watch::channel(false);
        let status = manager.status(&info.id, receiver.clone()).await.unwrap();
        let task = manager
            .initialize(
                &info.id,
                GitInitInput {
                    expected_snapshot: status.snapshot_digest,
                    initial_branch: "main".into(),
                },
                receiver.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            wait(&tasks, &task.task_id).await.status,
            TaskStatus::Succeeded
        );
        Self {
            _temporary: temporary,
            store,
            tasks,
            manager,
            project,
            tools,
            _stop: sender,
            receiver,
        }
    }
    async fn sql(&self, sql: &str) {
        self.store
            .execute(
                &self.project.manifest.id,
                SqlRequest {
                    sql: sql.into(),
                    params: vec![],
                    options: SqlOptions::default(),
                },
                None,
                SqlCaller::Management,
            )
            .await
            .unwrap();
    }
    async fn save(&self) -> String {
        let status = self
            .manager
            .status(&self.project.manifest.id, self.receiver.clone())
            .await
            .unwrap();
        let task = self
            .manager
            .save_data(
                &self.project.manifest.id,
                GitDataSaveInput {
                    expected_head: head_oid(&status).map(str::to_string),
                    expected_snapshot: status.snapshot_digest,
                    message: "disposable data preview version".into(),
                    author: GitAuthor {
                        name: "Preview fixture".into(),
                        email: "preview@example.invalid".into(),
                    },
                },
                self.receiver.clone(),
            )
            .await
            .unwrap();
        let result = wait(&self.tasks, &task.task_id).await;
        assert_eq!(result.status, TaskStatus::Succeeded, "{result:?}");
        result.result.unwrap()["commit"]
            .as_str()
            .unwrap()
            .to_string()
    }
    async fn preview(&self, target: &str) -> Result<GitDataRestorePreview, ApiError> {
        Box::pin(self.manager.data_restore_preview(
            &self.project.manifest.id,
            GitDataRestorePreviewInput {
                target_commit: target.into(),
            },
            self.receiver.clone(),
        ))
        .await
    }
    async fn git(&self, args: &[&str], input: Vec<u8>) -> Vec<u8> {
        let (_sender, cancel) = watch::channel(false);
        let mut context = Context::new(self.receiver.clone(), cancel).unwrap();
        let mut command = git_command(&self.tools, context.root(), Some(&self.project));
        command.args(args);
        // For fixture-created commits only. No hooks or identity discovery.
        author_environment(
            &mut command,
            &GitAuthor {
                name: "Preview fixture".into(),
                email: "preview@example.invalid".into(),
            },
        );
        Box::pin(context.run(command, input))
            .await
            .unwrap()
            .success()
            .unwrap()
    }
    async fn finish(self) {
        self.manager.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
}
async fn wait(tasks: &BuildManager, id: &str) -> TaskRecord {
    tokio::time::timeout(Duration::from_secs(90), async {
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

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to the explicit disposable official payload"]
async fn official_git_data_preview_streams_large_database_and_attachment_without_touching_live_data(
) {
    let fixture = Fixture::new().await;
    fixture
        .sql("CREATE TABLE blobs(id INTEGER PRIMARY KEY, body BLOB)")
        .await;
    let external = rusqlite::Connection::open(
        fixture
            .project
            .root
            .join(&fixture.project.manifest.database),
    )
    .unwrap();
    external
        .execute("INSERT INTO blobs VALUES(1, zeroblob(12582912))", [])
        .unwrap();
    drop(external);
    let attachment = fixture.project.root.join("data/attachments/large.bin");
    let mut payload = vec![0u8; 12 * 1024 * 1024 + 19];
    // Non-uniform bytes and a tail beyond Context's 8 MiB collector catch both
    // accidental truncation and newline/batch framing bugs.
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte = (i % 251) as u8;
    }
    fs::write(&attachment, &payload).unwrap();
    fs::create_dir(fixture.project.root.join("data/attachments/empty")).unwrap();
    let target = fixture.save().await;
    let external = rusqlite::Connection::open(
        fixture
            .project
            .root
            .join(&fixture.project.manifest.database),
    )
    .unwrap();
    external
        .execute("UPDATE blobs SET body = zeroblob(12582913) WHERE id=1", [])
        .unwrap();
    drop(external);
    fs::write(&attachment, b"fresh unsaved attachment").unwrap();
    fs::write(
        fixture
            .project
            .root
            .join("data/attachments/current-only.txt"),
        "keep current",
    )
    .unwrap();
    let head = fs::read(fixture.project.root.join(".git/refs/heads/main")).unwrap();
    let index = fs::read(fixture.project.root.join(".git/index")).unwrap();
    let source = fs::read(fixture.project.root.join("app/src/main.ts")).ok();
    let saved_manifest = fs::read(fixture.project.root.join("version-data/snapshot.json")).unwrap();
    let before: version_data::DataSnapshotManifest =
        serde_json::from_slice(&saved_manifest).unwrap();
    assert!(before.database.bytes > MAX_OUTPUT as u64);
    assert!(before.attachments["large.bin"].bytes > MAX_OUTPUT as u64);
    let preview = fixture.preview(&target).await.unwrap();
    assert_eq!(preview.target_snapshot_digest, sha256(&saved_manifest));
    assert_eq!(preview.target.database_bytes, before.database.bytes);
    assert_eq!(preview.target.attachment_bytes, payload.len() as u64);
    assert_eq!(preview.target.directory_count, 1);
    assert_eq!(preview.current.attachment_count, 2);
    assert!(preview.database_changed);
    assert!(preview.attachment_changes.contains(&DataAttachmentChange {
        path: "large.bin".into(),
        kind: "modify".into()
    }));
    assert!(preview.attachment_changes.contains(&DataAttachmentChange {
        path: "current-only.txt".into(),
        kind: "delete".into()
    }));
    assert!(!preview.can_apply && !preview.apply_implemented);
    assert!(preview.issues.is_empty());
    assert_ne!(preview.live_data_digest, preview.target_snapshot_digest);
    assert_eq!(fs::read(&attachment).unwrap(), b"fresh unsaved attachment");
    assert_eq!(
        fs::read(fixture.project.root.join(".git/refs/heads/main")).unwrap(),
        head
    );
    assert_eq!(
        fs::read(fixture.project.root.join(".git/index")).unwrap(),
        index
    );
    assert_eq!(
        fs::read(fixture.project.root.join("app/src/main.ts")).ok(),
        source
    );
    assert_eq!(
        fs::read(fixture.project.root.join("version-data/snapshot.json")).unwrap(),
        saved_manifest
    );
    let connection = rusqlite::Connection::open(
        fixture
            .project
            .root
            .join(&fixture.project.manifest.database),
    )
    .unwrap();
    let length: i64 = connection
        .query_row("SELECT length(body) FROM blobs WHERE id=1", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(length, 12_582_913);
    drop(connection);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to the explicit disposable official payload"]
async fn official_git_data_preview_refreshes_after_external_sqlite_and_attachment_writes() {
    let fixture = Fixture::new().await;
    fixture.sql("CREATE TABLE notes(body TEXT)").await;
    fixture.sql("INSERT INTO notes VALUES('before')").await;
    let attachment = fixture.project.root.join("data/attachments/note.txt");
    fs::write(&attachment, "before").unwrap();
    fs::create_dir(fixture.project.root.join("data/attachments/empty-before")).unwrap();
    let target = fixture.save().await;
    let first = fixture.preview(&target).await.unwrap();
    assert!(!first.database_changed);
    assert!(first.attachment_changes.is_empty());
    assert!(first.directory_changes.is_empty());
    // A real second SQLite connection and direct filesystem writer bypass the
    // SDK. A second preview must read new bytes even if lengths are unchanged.
    let external = rusqlite::Connection::open(
        fixture
            .project
            .root
            .join(&fixture.project.manifest.database),
    )
    .unwrap();
    external
        .execute("UPDATE notes SET body='after!'", [])
        .unwrap();
    fs::write(&attachment, "after!").unwrap();
    fs::rename(
        fixture.project.root.join("data/attachments/empty-before"),
        fixture.project.root.join("data/attachments/empty-after!"),
    )
    .unwrap();
    drop(external);
    let second = fixture.preview(&target).await.unwrap();
    assert_ne!(first.live_data_digest, second.live_data_digest);
    assert_ne!(first.plan_digest, second.plan_digest);
    assert_eq!(first.snapshot_digest, second.snapshot_digest);
    assert_eq!(first.target_snapshot_digest, second.target_snapshot_digest);
    assert!(second.database_changed);
    assert_eq!(
        first.current.directory_count,
        second.current.directory_count
    );
    assert_eq!(
        second.directory_changes,
        vec![
            DataAttachmentChange {
                path: "empty-after!".into(),
                kind: "delete".into()
            },
            DataAttachmentChange {
                path: "empty-before".into(),
                kind: "add".into()
            },
        ]
    );
    assert_eq!(
        second.attachment_changes,
        vec![DataAttachmentChange {
            path: "note.txt".into(),
            kind: "modify".into()
        }]
    );
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to the explicit disposable official payload"]
async fn official_git_data_preview_rejects_absent_wrong_project_and_corrupt_target_without_adopting_it(
) {
    let fixture = Fixture::new().await;
    fixture.sql("CREATE TABLE notes(body TEXT)").await;
    let target = fixture.save().await;
    let manifest_path = fixture.project.root.join("version-data/snapshot.json");
    let manifest_bytes = fs::read(&manifest_path).unwrap();
    let mut manifest: version_data::DataSnapshotManifest =
        serde_json::from_slice(&manifest_bytes).unwrap();
    manifest.project_id = "another-project".into();
    let blob = one_oid(
        &fixture
            .git(
                &["hash-object", "--stdin", "-w"],
                serde_json::to_vec_pretty(&manifest).unwrap(),
            )
            .await,
    )
    .unwrap();
    let tree = one_oid(
        &fixture
            .git(&["rev-parse", &format!("{target}:version-data")], vec![])
            .await,
    )
    .unwrap();
    let entries = fixture.git(&["ls-tree", &tree], vec![]).await;
    let old_blob = one_oid(
        &fixture
            .git(
                &["rev-parse", &format!("{target}:version-data/snapshot.json")],
                vec![],
            )
            .await,
    )
    .unwrap();
    let changed_entries = String::from_utf8(entries)
        .unwrap()
        .replace(&old_blob, &blob);
    let bad_tree = one_oid(&fixture.git(&["mktree"], changed_entries.into_bytes()).await).unwrap();
    let bad_root = one_oid(
        &fixture
            .git(
                &["mktree"],
                format!("040000 tree {bad_tree}\tversion-data\n").into_bytes(),
            )
            .await,
    )
    .unwrap();
    let bad_commit = one_oid(
        &fixture
            .git(
                &["commit-tree", &bad_root],
                b"wrong project target\n".to_vec(),
            )
            .await,
    )
    .unwrap();
    let failure = fixture.preview(&bad_commit).await.unwrap_err();
    assert_eq!(
        failure.code, "PROJECT_DATA_RESTORE_TARGET_INVALID",
        "{failure:?}"
    );
    let missing = fixture
        .preview("1111111111111111111111111111111111111111")
        .await
        .unwrap_err();
    assert_eq!(missing.code, "PROJECT_DATA_RESTORE_TARGET_INVALID");
    let data_blob = one_oid(
        &fixture
            .git(
                &[
                    "rev-parse",
                    &format!("{target}:version-data/database.sqlite"),
                ],
                vec![],
            )
            .await,
    )
    .unwrap();
    assert_eq!(
        fixture.preview(&data_blob).await.unwrap_err().code,
        "PROJECT_DATA_RESTORE_TARGET_INVALID"
    );
    assert!(fixture.preview("HEAD").await.is_err());
    let extra_blob = one_oid(
        &fixture
            .git(
                &["hash-object", "--stdin", "-w"],
                b"unlisted attachment".to_vec(),
            )
            .await,
    )
    .unwrap();
    let extra_tree = one_oid(
        &fixture
            .git(
                &["mktree"],
                format!("100644 blob {extra_blob}\tunlisted.txt\n").into_bytes(),
            )
            .await,
    )
    .unwrap();
    let clean_entries = fixture.git(&["ls-tree", &tree], vec![]).await;
    let with_extra = format!(
        "{}040000 tree {extra_tree}\tattachments\n",
        String::from_utf8(clean_entries).unwrap()
    );
    let extra_data_tree =
        one_oid(&fixture.git(&["mktree"], with_extra.into_bytes()).await).unwrap();
    let extra_root = one_oid(
        &fixture
            .git(
                &["mktree"],
                format!("040000 tree {extra_data_tree}\tversion-data\n").into_bytes(),
            )
            .await,
    )
    .unwrap();
    let extra_commit = one_oid(
        &fixture
            .git(
                &["commit-tree", &extra_root],
                b"extra attachment target\n".to_vec(),
            )
            .await,
    )
    .unwrap();
    assert_eq!(
        fixture.preview(&extra_commit).await.unwrap_err().code,
        "PROJECT_DATA_RESTORE_TARGET_INVALID"
    );
    let mut corrupt_manifest: version_data::DataSnapshotManifest =
        serde_json::from_slice(&manifest_bytes).unwrap();
    corrupt_manifest.database.sha256 = "0".repeat(64);
    let corrupt_blob = one_oid(
        &fixture
            .git(
                &["hash-object", "--stdin", "-w"],
                serde_json::to_vec_pretty(&corrupt_manifest).unwrap(),
            )
            .await,
    )
    .unwrap();
    let corrupt_entries = String::from_utf8(fixture.git(&["ls-tree", &tree], vec![]).await)
        .unwrap()
        .replace(&old_blob, &corrupt_blob);
    let corrupt_tree =
        one_oid(&fixture.git(&["mktree"], corrupt_entries.into_bytes()).await).unwrap();
    let corrupt_root = one_oid(
        &fixture
            .git(
                &["mktree"],
                format!("040000 tree {corrupt_tree}\tversion-data\n").into_bytes(),
            )
            .await,
    )
    .unwrap();
    let corrupt_commit = one_oid(
        &fixture
            .git(
                &["commit-tree", &corrupt_root],
                b"bad manifest digest\n".to_vec(),
            )
            .await,
    )
    .unwrap();
    assert!(fixture.preview(&corrupt_commit).await.is_err());
    assert_eq!(fs::read(&manifest_path).unwrap(), manifest_bytes);
    assert_eq!(
        fixture.git(&["rev-parse", "HEAD"], vec![]).await,
        format!("{target}\n").as_bytes()
    );
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to the explicit disposable official payload"]
async fn official_git_data_preview_raw_objects_do_not_run_target_content_filters_and_candidates_are_temporary(
) {
    let fixture = Fixture::new().await;
    fs::write(
        fixture.project.root.join("data/attachments/note.txt"),
        "unchanged original",
    )
    .unwrap();
    let target = fixture.save().await;
    let sentinel = fixture._temporary.path().join("filter-must-not-run");
    let probe = fixture._temporary.path().join("disposable-filter-probe");
    fs::write(
        &probe,
        format!(
            "#!/bin/sh\n/usr/bin/touch '{}'\n/bin/cat\n",
            sentinel.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&probe, fs::Permissions::from_mode(0o700)).unwrap();
    for option in ["filter.preview.clean", "filter.preview.smudge"] {
        fixture
            .git(
                &["config", "--local", option, probe.to_str().unwrap()],
                vec![],
            )
            .await;
    }
    let attributes = one_oid(
        &fixture
            .git(
                &["hash-object", "--stdin", "-w"],
                b"version-data/** filter=preview\n".to_vec(),
            )
            .await,
    )
    .unwrap();
    let data_tree = one_oid(
        &fixture
            .git(&["rev-parse", &format!("{target}:version-data")], vec![])
            .await,
    )
    .unwrap();
    let root = one_oid(&fixture.git(&["mktree"], format!("100644 blob {attributes}\t.gitattributes\n040000 tree {data_tree}\tversion-data\n").into_bytes()).await).unwrap();
    let filtered_target = one_oid(
        &fixture
            .git(
                &["commit-tree", &root],
                b"target attributes must not run\n".to_vec(),
            )
            .await,
    )
    .unwrap();
    let preview = fixture.preview(&filtered_target).await.unwrap();
    assert!(preview.issues.is_empty());
    assert!(!sentinel.exists());
    assert_eq!(
        fs::read_to_string(fixture.project.root.join("data/attachments/note.txt")).unwrap(),
        "unchanged original"
    );
    let (_sender, cancel) = watch::channel(false);
    let mut context = Context::new(fixture.receiver.clone(), cancel).unwrap();
    let tree = objects::target_tree(
        &fixture.project,
        &filtered_target,
        &fixture.tools,
        &mut context,
    )
    .await
    .unwrap();
    let (temporary, snapshot) = target_snapshot(
        &fixture.manager,
        &fixture.project,
        &fixture.tools,
        &mut context,
        tree,
    )
    .await
    .unwrap();
    let root = snapshot.root.clone();
    assert!(root.join("database.sqlite").exists());
    drop(snapshot);
    drop(temporary);
    assert!(
        !root.exists(),
        "successful preview candidates must not accumulate data history"
    );
    assert!(!sentinel.exists());
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to the explicit disposable official payload"]
async fn official_git_data_preview_cancel_and_external_source_change_release_freeze_and_project_lease(
) {
    let fixture = Fixture::new().await;
    let target = fixture.save().await;
    for cancel_requested in [true, false] {
        let gate = fixture.project.operation_gate.read().await;
        let manager = fixture.manager.clone();
        let id = fixture.project.manifest.id.clone();
        let target_commit = target.clone();
        let (sender, receiver) = watch::channel(false);
        let preview = tokio::spawn(async move {
            Box::pin(manager.data_restore_preview(
                &id,
                GitDataRestorePreviewInput { target_commit },
                receiver,
            ))
            .await
        });
        tokio::time::timeout(Duration::from_secs(15), async {
            while !fixture.project.control.frozen.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(fixture
            .store
            .reserve_transient_operation(&fixture.project.manifest.id, "must-be-busy")
            .is_err());
        if cancel_requested {
            sender.send(true).unwrap();
        } else {
            fs::write(
                fixture
                    .project
                    .root
                    .join("app/src/external-preview-change.txt"),
                "external edit while snapshot waits",
            )
            .unwrap();
        }
        drop(gate);
        let failure = tokio::time::timeout(Duration::from_secs(15), preview)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        if !cancel_requested {
            assert_eq!(failure.code, "PROJECT_DATA_RESTORE_STALE", "{failure:?}");
        }
        assert!(!fixture.project.control.frozen.load(Ordering::Acquire));
        let available = fixture
            .store
            .reserve_transient_operation(&fixture.project.manifest.id, "available-after-preview")
            .unwrap();
        drop(available);
        fixture
            .sql("CREATE TABLE IF NOT EXISTS after_preview(body TEXT)")
            .await;
    }
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to the explicit disposable official payload"]
async fn official_git_data_preview_shutdown_drains_worker_even_after_request_receiver_is_dropped() {
    let fixture = Fixture::new().await;
    fixture.sql("CREATE TABLE notes(body TEXT)").await;
    fixture
        .sql("INSERT INTO notes VALUES('finish the active read before shutdown')")
        .await;
    let operation = fixture
        .store
        .reserve_transient_operation(&fixture.project.manifest.id, "data-preview-drain-fixture")
        .unwrap();
    let (_stop_sender, stop) = watch::channel(false);
    let (_cancel_sender, cancel) = watch::channel(false);
    let lease =
        version_data::SnapshotLease::acquire(fixture.project.clone(), stop.clone(), cancel.clone())
            .await
            .unwrap();
    let temporary = tempfile::Builder::new()
        .prefix("disposable-preview-drain-")
        .tempdir()
        .unwrap();
    let temporary_root = temporary.path().canonicalize().unwrap();
    let destination = temporary_root.join("current");
    let (entered_sender, entered) = tokio::sync::oneshot::channel();
    let (release, receiver) = std::sync::mpsc::channel();
    let worker_result = fixture
        .manager
        .preview_worker(move || {
            entered_sender.send(()).unwrap();
            // Deterministically model a SQLite/filesystem read that is already in
            // progress and cannot observe the request's cancellation immediately.
            receiver.recv().unwrap();
            let snapshot = version_data::prepare_snapshot(
                lease,
                &destination,
                version_data::SnapshotLimits::default(),
                &stop,
                &cancel,
            )?;
            assert!(snapshot.root.join("database.sqlite").exists());
            Ok((operation, temporary, snapshot))
        })
        .unwrap();
    entered.await.unwrap();
    drop(worker_result); // The request future has been aborted/dropped.
    let manager = fixture.manager.clone();
    let draining = tokio::spawn(async move { manager.shutdown().await });
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(
        !draining.is_finished(),
        "shutdown returned while a snapshot worker was still active"
    );
    assert!(fixture.project.control.frozen.load(Ordering::Acquire));
    assert!(fixture
        .store
        .reserve_transient_operation(&fixture.project.manifest.id, "must-stay-busy")
        .is_err());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), draining)
        .await
        .unwrap()
        .unwrap();
    assert!(!fixture.project.control.frozen.load(Ordering::Acquire));
    assert!(
        !temporary_root.exists(),
        "abandoned preview candidate must be cleaned after its last reader finishes"
    );
    assert!(
        fixture.manager.preview_worker(|| Ok(())).is_err(),
        "shutdown must refuse later worker registration"
    );
    fixture.finish().await;
}
