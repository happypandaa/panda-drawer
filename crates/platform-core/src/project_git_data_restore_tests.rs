use super::*;
use crate::{CreateProjectInput, SqlCaller, SqlOptions, SqlRequest, TaskStatus};

struct Fixture {
    _temp: TempDir,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    manager: Arc<ProjectGitManager>,
    project: ProjectHandle,
    _sender: watch::Sender<bool>,
    stop: watch::Receiver<bool>,
}
impl Fixture {
    async fn new() -> Self {
        let tool = PathBuf::from(
            std::env::var("LOCAL_PROJECTS_TEST_GIT_ROOT")
                .expect("explicit disposable Git required"),
        )
        .canonicalize()
        .unwrap();
        assert!(tool.starts_with(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.test-artifacts")
                .canonicalize()
                .unwrap()
        ));
        let (temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let project = store
            .create(CreateProjectInput {
                name: "Disposable data apply recovery".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&project.id).unwrap();
        let tasks = BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into())
            .unwrap();
        let git = GitEnvironmentManager::new(paths.data_root().into(), Some(tool));
        let manager = ProjectGitManager::new(paths, store.clone(), tasks.clone(), git).unwrap();
        let (sender, stop) = watch::channel(false);
        let view = manager
            .status(&project.manifest.id, stop.clone())
            .await
            .unwrap();
        let task = manager
            .initialize(
                &project.manifest.id,
                GitInitInput {
                    expected_snapshot: view.snapshot_digest,
                    initial_branch: "main".into(),
                },
                stop.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            wait(&tasks, &task.task_id).await.status,
            TaskStatus::Succeeded
        );
        Self {
            _temp: temp,
            store,
            tasks,
            manager,
            project,
            _sender: sender,
            stop,
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
            .status(&self.project.manifest.id, self.stop.clone())
            .await
            .unwrap();
        let task = self
            .manager
            .save_data(
                &self.project.manifest.id,
                GitDataSaveInput {
                    expected_head: head_oid(&status).map(str::to_string),
                    expected_snapshot: status.snapshot_digest,
                    message: "disposable restore target".into(),
                    author: GitAuthor {
                        name: "Data restore fixture".into(),
                        email: "restore@example.invalid".into(),
                    },
                },
                self.stop.clone(),
            )
            .await
            .unwrap();
        let task = wait(&self.tasks, &task.task_id).await;
        assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
        task.result.unwrap()["commit"].as_str().unwrap().into()
    }
    async fn input(&self, target: &str) -> GitDataRestoreApplyInput {
        let preview = Box::pin(self.manager.data_restore_preview(
            &self.project.manifest.id,
            GitDataRestorePreviewInput {
                target_commit: target.into(),
            },
            self.stop.clone(),
        ))
        .await
        .unwrap();
        GitDataRestoreApplyInput {
            target_commit: target.into(),
            expected_head: preview.head.oid.unwrap(),
            expected_branch: preview.head.branch.unwrap(),
            expected_snapshot: preview.snapshot_digest,
            expected_document_epoch: preview.document_epoch,
            expected_live_data_digest: preview.live_data_digest,
            expected_target_snapshot_digest: preview.target_snapshot_digest,
            expected_plan_digest: preview.plan_digest,
        }
    }
    async fn apply(&self, input: GitDataRestoreApplyInput) -> TaskRecord {
        let task = Box::pin(self.manager.data_restore_apply(
            &self.project.manifest.id,
            input,
            self.stop.clone(),
        ))
        .await
        .unwrap();
        wait(&self.tasks, &task.task_id).await
    }
    async fn status(&self) -> GitDataRestoreRecoveryStatus {
        self.manager
            .data_restore_status(&self.project.manifest.id, self.stop.clone())
            .await
            .unwrap()
    }
    async fn recover(&self, action: RestoreAction) -> TaskRecord {
        let state = self.status().await;
        assert_eq!(state.operations.len(), 1, "{state:?}");
        let op = &state.operations[0];
        assert!(
            if action == RestoreAction::Continue {
                op.can_continue
            } else {
                op.can_undo
            },
            "{state:?}"
        );
        let task = self
            .manager
            .data_restore_recover(
                &self.project.manifest.id,
                GitDataRestoreRecoverInput {
                    restore_task_id: op.restore_task_id.clone(),
                    expected_plan_digest: op.plan_digest.clone(),
                    expected_journal_digest: op.journal_digest.clone(),
                    action,
                },
                self.stop.clone(),
            )
            .await
            .unwrap();
        wait(&self.tasks, &task.task_id).await
    }
    async fn finish(self) {
        assert!(self.manager.shutdown().await.is_empty());
        assert!(self.tasks.shutdown().await.is_empty());
        assert!(self.store.shutdown().await.is_empty());
    }
    async fn restart(&mut self) {
        let paths = self.manager.paths.clone();
        let git = self.manager.git.clone();
        assert!(self.manager.shutdown().await.is_empty());
        assert!(self.tasks.shutdown().await.is_empty());
        assert!(self.store.shutdown().await.is_empty());
        self.store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        self.tasks = BuildManager::new(
            paths.clone(),
            self.store.clone(),
            "http://127.0.0.1:1234".into(),
        )
        .unwrap();
        self.manager =
            ProjectGitManager::new(paths, self.store.clone(), self.tasks.clone(), git).unwrap();
    }
}
async fn wait(tasks: &BuildManager, id: &str) -> TaskRecord {
    tokio::time::timeout(Duration::from_secs(120), async {
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
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable Git payload"]
async fn official_git_data_restore_applies_real_sqlite_attachments_without_changing_source_or_git()
{
    let fixture = Fixture::new().await;
    fixture.sql("CREATE TABLE notes(body TEXT)").await;
    fixture
        .sql("INSERT INTO notes VALUES('target record')")
        .await;
    let attachments = fixture
        .project
        .root
        .join(&fixture.project.manifest.attachments_dir);
    fs::write(attachments.join("target.txt"), b"target attachment").unwrap();
    fs::create_dir(attachments.join("target-empty")).unwrap();
    let target = fixture.save().await;
    fixture
        .sql("UPDATE notes SET body='live before restore'")
        .await;
    fs::write(attachments.join("target.txt"), b"current live attachment").unwrap();
    fs::write(attachments.join("extra.txt"), b"keep original").unwrap();
    fs::create_dir(attachments.join("live-empty")).unwrap();
    let input = fixture.input(&target).await;
    let old_epoch = input.expected_document_epoch.clone();
    let git = fs::read(fixture.project.root.join(".git/HEAD")).unwrap();
    let index = fs::read(fixture.project.root.join(".git/index")).unwrap();
    let manifest = fs::read(fixture.project.root.join("project.json")).unwrap();
    let applied = fixture.apply(input).await;
    assert_eq!(applied.status, TaskStatus::Succeeded, "{applied:?}");
    assert_eq!(applied.result.as_ref().unwrap()["dataState"], "applied");
    assert_eq!(fixture.status().await.state, "clear");
    assert_eq!(
        fs::read(fixture.project.root.join(".git/HEAD")).unwrap(),
        git
    );
    assert_eq!(
        fs::read(fixture.project.root.join(".git/index")).unwrap(),
        index
    );
    assert_eq!(
        fs::read(fixture.project.root.join("project.json")).unwrap(),
        manifest
    );
    assert_eq!(
        fs::read(attachments.join("target.txt")).unwrap(),
        b"target attachment"
    );
    assert!(!attachments.join("extra.txt").exists());
    assert!(!attachments.join("live-empty").exists());
    assert!(attachments.join("target-empty").is_dir());
    let fresh = fixture.store.project(&fixture.project.manifest.id).unwrap();
    assert_ne!(fresh.document_epoch, old_epoch);
    assert_eq!(
        fresh.validate_epoch(Some(&old_epoch)).unwrap_err().code,
        "PROJECT_RELOAD_REQUIRED"
    );
    let db = rusqlite::Connection::open(fresh.root.join(&fresh.manifest.database)).unwrap();
    assert_eq!(
        db.query_row("SELECT body FROM notes", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "target record"
    );
    drop(db);
    let retained = PathBuf::from(
        applied.result.as_ref().unwrap()["retainedPath"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(
        fs::read(retained.join("slots/attachments/before/extra.txt")).unwrap(),
        b"keep original"
    );
    fixture
        .sql("INSERT INTO notes VALUES('ordinary write after completed restore')")
        .await;
    assert_eq!(fixture.status().await.state, "clear");
    fixture.save().await;
    assert_eq!(fixture.status().await.state, "clear");
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable Git payload"]
async fn official_git_data_restore_pending_allows_explicit_continue_and_undo() {
    for action in [RestoreAction::Continue, RestoreAction::Undo] {
        let fixture = Fixture::new().await;
        fixture.sql("CREATE TABLE notes(body TEXT)").await;
        fixture.sql("INSERT INTO notes VALUES('target')").await;
        let target = fixture.save().await;
        fixture.sql("UPDATE notes SET body='before'").await;
        let input = fixture.input(&target).await;
        fixture
            .manager
            .data_restore_fault
            .store(4, Ordering::Release);
        let task = fixture.apply(input).await;
        assert_eq!(task.status, TaskStatus::Failed, "{task:?}");
        let state = fixture.status().await;
        assert_eq!(state.state, "pending", "{state:?}");
        assert!(state.business_blocked);
        assert_eq!(state.operations[0].data_state, "before");
        assert!(fixture
            .store
            .schema(&fixture.project.manifest.id, None)
            .await
            .is_err());
        let info = fixture
            .store
            .info(&fixture.project.manifest.id)
            .await
            .unwrap();
        assert!(info.manifest.is_some());
        assert_eq!(info.project_root, fixture.project.root);
        let recovered = fixture.recover(action).await;
        assert_eq!(recovered.status, TaskStatus::Succeeded, "{recovered:?}");
        assert_eq!(fixture.status().await.state, "clear");
        let fresh = fixture.store.project(&fixture.project.manifest.id).unwrap();
        let db = rusqlite::Connection::open(fresh.root.join(&fresh.manifest.database)).unwrap();
        assert_eq!(
            db.query_row("SELECT body FROM notes", [], |row| row.get::<_, String>(0))
                .unwrap(),
            if action == RestoreAction::Continue {
                "target"
            } else {
                "before"
            }
        );
        drop(db);
        fixture.finish().await;
    }
}

async fn seed(fixture: &Fixture) -> String {
    fixture.sql("CREATE TABLE notes(body TEXT)").await;
    fixture.sql("INSERT INTO notes VALUES('target')").await;
    let attachments = fixture
        .project
        .root
        .join(&fixture.project.manifest.attachments_dir);
    fs::write(attachments.join("value.txt"), b"target attachment").unwrap();
    fs::create_dir(attachments.join("empty-target")).unwrap();
    fixture.save().await
}
async fn assert_body(fixture: &Fixture, expected: &str) {
    let project = fixture.store.project(&fixture.project.manifest.id).unwrap();
    let connection =
        rusqlite::Connection::open(project.root.join(project.manifest.database)).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT body FROM notes", [], |row| row.get::<_, String>(0))
            .unwrap(),
        expected
    );
    drop(connection);
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable Git payload"]
async fn official_git_data_restore_forward_every_boundary_continue_or_undo_matrix() {
    let fixture = Fixture::new().await;
    let target = seed(&fixture).await;
    let mut covered_all = false;
    for (failures, point) in (1..=64).enumerate() {
        let original = format!("before-forward-{point}");
        fixture
            .sql(&format!("UPDATE notes SET body='{original}'"))
            .await;
        let attachments = fixture
            .project
            .root
            .join(&fixture.project.manifest.attachments_dir);
        fs::write(attachments.join("value.txt"), original.as_bytes()).unwrap();
        let input = fixture.input(&target).await;
        fixture.manager.test_data_restore_fault(point);
        let task = fixture.apply(input).await;
        if task.status == TaskStatus::Succeeded {
            assert!(failures >= 30, "{failures}");
            eprintln!("forward boundaries verified: {failures}");
            covered_all = true;
            break;
        }
        let error = task.error.as_ref().unwrap();
        assert_eq!(
            error.code, "PROJECT_DATA_RESTORE_TEST_FAULT",
            "point {point}: {task:?}"
        );
        let status = fixture.status().await;
        if status.state == "clear" {
            if point == 1 {
                assert_eq!(error.details["operationOutcome"], "not_started");
                assert_body(&fixture, &original).await;
            } else {
                assert_eq!(error.details["dataState"], "applied");
                assert_eq!(error.details["pending"], false);
                assert_body(&fixture, "target").await;
            }
            continue;
        }
        assert_eq!(status.state, "pending", "point {point}: {status:?}");
        let op = &status.operations[0];
        assert!(op.original_retained, "{status:?}");
        let action = if op.can_undo && point % 2 == 1 {
            RestoreAction::Undo
        } else {
            RestoreAction::Continue
        };
        let finished = fixture.recover(action).await;
        assert_eq!(
            finished.status,
            TaskStatus::Succeeded,
            "point {point}: {finished:?}"
        );
        assert_eq!(fixture.status().await.state, "clear");
        assert_body(
            &fixture,
            if action == RestoreAction::Undo {
                &original
            } else {
                "target"
            },
        )
        .await;
    }
    assert!(covered_all, "all forward fault boundaries must be reached");
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable Git payload"]
async fn official_git_data_restore_undo_every_boundary_keeps_fixed_epoch_and_journal() {
    let fixture = Fixture::new().await;
    let target = seed(&fixture).await;
    let mut covered_all = false;
    for (failures, point) in (1..=64).enumerate() {
        let original = format!("before-undo-{point}");
        fixture
            .sql(&format!("UPDATE notes SET body='{original}'"))
            .await;
        let input = fixture.input(&target).await;
        fixture.manager.test_data_restore_fault(u32::MAX);
        let prepared = fixture.apply(input).await;
        assert_eq!(prepared.status, TaskStatus::Failed);
        let status = fixture.status().await;
        assert_eq!(status.state, "pending", "{status:?}");
        assert!(status.operations[0].can_undo);
        let digest = status.operations[0].journal_digest.clone();
        fixture.manager.test_data_restore_fault(point);
        let task = fixture.recover(RestoreAction::Undo).await;
        if task.status == TaskStatus::Succeeded {
            assert!(failures >= 28, "{failures}");
            eprintln!("undo boundaries verified: {failures}");
            covered_all = true;
            assert_body(&fixture, &original).await;
            break;
        }
        assert_eq!(
            task.error.as_ref().unwrap().code,
            "PROJECT_DATA_RESTORE_TEST_FAULT",
            "point {point}: {task:?}"
        );
        let status = fixture.status().await;
        if status.state == "clear" {
            assert_eq!(task.error.as_ref().unwrap().details["dataState"], "undone");
            assert_body(&fixture, &original).await;
            continue;
        }
        assert_eq!(status.state, "pending", "point {point}: {status:?}");
        assert_eq!(
            status.operations[0].journal_digest, digest,
            "immutable journal does not summarize changing slots"
        );
        if status.operations[0].data_state == "undone" {
            assert!(!status.operations[0].can_continue);
        }
        let finished = fixture.recover(RestoreAction::Undo).await;
        assert_eq!(
            finished.status,
            TaskStatus::Succeeded,
            "point {point}: {finished:?}"
        );
        assert_eq!(fixture.status().await.state, "clear");
        assert_body(&fixture, &original).await;
    }
    assert!(covered_all, "all undo fault boundaries must be reached");
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable Git payload"]
async fn official_git_data_restore_restart_retains_failed_candidates_and_uses_reserved_finalization(
) {
    let mut fixture = Fixture::new().await;
    let target = seed(&fixture).await;
    fixture.sql("UPDATE notes SET body='original'").await;
    let input = fixture.input(&target).await;
    fixture.manager.test_data_restore_fault(u32::MAX - 1);
    let failed = fixture.apply(input).await;
    assert_eq!(failed.status, TaskStatus::Failed, "{failed:?}");
    let status = fixture.status().await;
    assert_eq!(status.state, "pending", "{status:?}");
    let root = status.operations[0].retained_path.clone();
    let digest = status.operations[0].journal_digest.clone();
    let attempts = root.join("metadata-attempts");
    let initial = fs::read_dir(&attempts)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(initial.len(), 1);
    let original_candidate = fs::read(&initial[0]).unwrap();
    let descriptor = files::root(&attempts).unwrap();
    for _ in initial.len()..128 {
        let name = format!("completion-{}.json", uuid::Uuid::new_v4().simple());
        files::create(&descriptor, &name)
            .unwrap()
            .sync_all()
            .unwrap();
    }
    let (_, plan, _) = transaction::load(
        &root,
        &fixture.project.manifest.id,
        &status.operations[0].restore_task_id,
    )
    .unwrap();
    fixture.restart().await;
    let fresh = fixture.status().await;
    assert_eq!(fresh.state, "pending", "{fresh:?}");
    assert_eq!(fresh.operations[0].journal_digest, digest);
    assert!(!fresh.operations[0].can_undo);
    let recovered = fixture.recover(RestoreAction::Continue).await;
    assert_eq!(recovered.status, TaskStatus::Succeeded, "{recovered:?}");
    assert_eq!(fixture.status().await.state, "clear");
    assert_eq!(fs::read_dir(&attempts).unwrap().count(), 128);
    assert_eq!(fs::read(&initial[0]).unwrap(), original_candidate);
    for name in ["completion", "terminal"] {
        assert!(plan.metadata_reserve[name]
            .identity
            .matches_path(&root.join(format!("{name}.json")))
            .unwrap());
    }
    assert_body(&fixture, "target").await;
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable Git payload"]
async fn official_git_data_restore_stale_live_data_and_foreign_companions_are_preserved() {
    let fixture = Fixture::new().await;
    let target = seed(&fixture).await;
    fixture.sql("UPDATE notes SET body='before'").await;
    let input = fixture.input(&target).await;
    fixture
        .sql("UPDATE notes SET body='external change after preview'")
        .await;
    let rejected = fixture.apply(input).await;
    assert_eq!(rejected.status, TaskStatus::Failed);
    assert_eq!(
        rejected.error.as_ref().unwrap().code,
        "PROJECT_DATA_RESTORE_STALE"
    );
    assert_eq!(fixture.status().await.state, "clear");
    assert_body(&fixture, "external change after preview").await;
    let input = fixture.input(&target).await;
    fixture.manager.test_data_restore_fault(4);
    let failed = fixture.apply(input).await;
    assert_eq!(failed.status, TaskStatus::Failed);
    let status = fixture.status().await;
    assert_eq!(status.state, "pending");
    let root = status.operations[0].retained_path.clone();
    let live = fixture
        .project
        .root
        .join(&fixture.project.manifest.database);
    assert!(!live.exists());
    let foreign = b"foreign DB occupying the live destination";
    fs::write(&live, foreign).unwrap();
    let blocked = fixture.status().await;
    assert_eq!(blocked.state, "blocked", "{blocked:?}");
    assert!(!blocked.operations[0].can_continue);
    assert!(!blocked.operations[0].can_undo);
    assert_eq!(fs::read(&live).unwrap(), foreign);
    let info = fixture
        .store
        .info(&fixture.project.manifest.id)
        .await
        .unwrap();
    assert!(info.manifest.is_some());
    assert_eq!(info.build_info["dataRecoveryRequired"], true);
    assert!(root.join("slots/database/before").exists());
    let wal = PathBuf::from(format!("{}-wal", live.display()));
    fs::write(&wal, b"foreign companion").unwrap();
    assert_eq!(fixture.status().await.state, "blocked");
    assert_eq!(fs::read(&wal).unwrap(), b"foreign companion");
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable Git payload"]
async fn official_git_data_restore_owned_runner_drains_after_request_has_returned() {
    let fixture = Fixture::new().await;
    let target = seed(&fixture).await;
    fixture.sql("UPDATE notes SET body='before shutdown'").await;
    let input = fixture.input(&target).await;
    let (entered, receive) = std::sync::mpsc::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    *fixture.manager.data_restore_pause.lock().unwrap() = Some((entered, blocked));
    let task = fixture
        .manager
        .data_restore_apply(&fixture.project.manifest.id, input, fixture.stop.clone())
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(40),
        tokio::task::spawn_blocking(move || receive.recv().unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(fixture
        .store
        .reserve_delete_task(
            &fixture.project.manifest.id,
            "task_forbidden_shutdown_delete"
        )
        .is_err());
    fixture.manager.cancel(&task.task_id).unwrap();
    let manager = fixture.manager.clone();
    let shutdown = tokio::spawn(async move {
        manager.shutdown().await;
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!shutdown.is_finished());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), shutdown)
        .await
        .unwrap()
        .unwrap();
    let task = fixture.tasks.task(&task.task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Interrupted);
    assert_eq!(task.error.as_ref().unwrap().details["dataState"], "before");
    assert_body(&fixture, "before shutdown").await;
    fixture.finish().await;
}

#[test]
fn data_restore_failed_metadata_candidates_have_a_strict_read_only_budget() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    private_dir(&root.join("metadata-attempts")).unwrap();
    let directory = files::root(&root.join("metadata-attempts")).unwrap();
    for _ in 0..128 {
        files::create(
            &directory,
            &format!("completion-{}.json", uuid::Uuid::new_v4().simple()),
        )
        .unwrap();
    }
    assert_eq!(transaction::metadata_usage(&root).unwrap().0, 128);
    files::create(
        &directory,
        &format!("terminal-{}.json", uuid::Uuid::new_v4().simple()),
    )
    .unwrap();
    assert!(transaction::metadata_usage(&root).is_err());
    assert_eq!(
        fs::read_dir(root.join("metadata-attempts"))
            .unwrap()
            .count(),
        129
    );
}

#[test]
fn data_restore_evidence_rejects_unknown_nested_identity_fields() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let path = root.join("proof");
    fs::write(&path, b"proof").unwrap();
    let (proof, _) = read_proof(&path, 1024, false).unwrap();
    let mut value = serde_json::to_value(&proof).unwrap();
    value["identity"]["unrecognized"] = json!(true);
    assert!(
        serde_json::from_value::<crate::projects::data_restore_registration::FileProof>(value)
            .is_err()
    );
    let directory = slot_files::DirectoryProof::capture(&root).unwrap();
    let mut value = serde_json::to_value(&directory).unwrap();
    value["identity"]["unrecognized"] = json!(true);
    assert!(serde_json::from_value::<slot_files::DirectoryProof>(value).is_err());
}

async fn assert_evidence_blocked(fixture: &Fixture) {
    let state = fixture.status().await;
    assert_eq!(state.state, "blocked", "{state:?}");
    assert!(state.business_blocked);
    assert!(state
        .operations
        .iter()
        .all(|operation| !operation.can_continue && !operation.can_undo));
    assert!(fixture.store.project(&fixture.project.manifest.id).is_err());
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable Git payload"]
async fn official_git_data_restore_pending_evidence_cannot_be_relabelled_or_replaced() {
    let mut fixture = Fixture::new().await;
    let target = seed(&fixture).await;
    fixture
        .sql("UPDATE notes SET body='original evidence'")
        .await;
    let input = fixture.input(&target).await;
    fixture.manager.test_data_restore_fault(u32::MAX);
    assert_eq!(fixture.apply(input).await.status, TaskStatus::Failed);
    let status = fixture.status().await;
    assert_eq!(status.state, "pending");
    let operation = status.operations[0].retained_path.clone();
    let journal_path = operation.join("journal.json");
    let journal_bytes = fs::read(&journal_path).unwrap();
    for phase in ["completed", "undone", "not-applied"] {
        let mut journal: Value = serde_json::from_slice(&journal_bytes).unwrap();
        journal["phase"] = json!(phase);
        fs::write(&journal_path, serde_json::to_vec(&journal).unwrap()).unwrap();
        assert_evidence_blocked(&fixture).await;
        fs::write(&journal_path, &journal_bytes).unwrap();
        assert_eq!(fixture.status().await.state, "pending");
    }
    for name in ["journal.json", "plan.json"] {
        let source = operation.join(name);
        let saved = fixture._temp.path().join(format!("retained-{name}"));
        let bytes = fs::read(&source).unwrap();
        fs::rename(&source, &saved).unwrap();
        assert_evidence_blocked(&fixture).await;
        fs::write(&source, &bytes).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        assert_evidence_blocked(&fixture).await;
        assert_eq!(fs::read(&source).unwrap(), bytes);
        fs::remove_file(&source).unwrap(); // Test-created foreign evidence only.
        fs::rename(&saved, &source).unwrap();
        assert_eq!(fixture.status().await.state, "pending");
        fs::write(&source, b"{broken}").unwrap();
        assert_evidence_blocked(&fixture).await;
        fs::write(&source, bytes).unwrap();
        assert_eq!(fixture.status().await.state, "pending");
    }
    let foreign = operation.join("completion-extra.json");
    fs::write(&foreign, b"foreign evidence").unwrap();
    assert_evidence_blocked(&fixture).await;
    assert_eq!(fs::read(&foreign).unwrap(), b"foreign evidence");
    fs::rename(&foreign, fixture._temp.path().join("foreign-evidence-kept")).unwrap();
    fixture.restart().await;
    assert_eq!(fixture.status().await.state, "pending");
    let task = fixture.recover(RestoreAction::Undo).await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
    assert_body(&fixture, "original evidence").await;
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable Git payload"]
async fn official_git_data_restore_completion_requires_identity_and_terminal_then_archives() {
    let mut fixture = Fixture::new().await;
    let target = seed(&fixture).await;
    fixture
        .sql("UPDATE notes SET body='receipt original'")
        .await;
    let input = fixture.input(&target).await;
    fixture.manager.test_data_restore_fault(u32::MAX - 2);
    let failed = fixture.apply(input).await;
    assert_eq!(failed.status, TaskStatus::Failed, "{failed:?}");
    assert_eq!(
        failed.error.as_ref().unwrap().details["operationOutcome"],
        "committed"
    );
    let state = fixture.status().await;
    assert_eq!(state.state, "pending", "{state:?}");
    let operation = state.operations[0].retained_path.clone();
    assert!(state.operations[0].completion_published);
    assert!(state.operations[0].can_continue);
    assert!(!state.operations[0].can_undo);
    let receipt = operation.join("completion.json");
    let bytes = fs::read(&receipt).unwrap();
    let saved = fixture._temp.path().join("original-completion.json");
    fs::rename(&receipt, &saved).unwrap();
    fs::write(&receipt, &bytes).unwrap();
    fs::set_permissions(&receipt, fs::Permissions::from_mode(0o600)).unwrap();
    assert_evidence_blocked(&fixture).await;
    fs::remove_file(&receipt).unwrap(); // Test-created same-bytes foreign receipt only.
    fs::rename(&saved, &receipt).unwrap();
    for nested in [false, true] {
        let mut changed: Value = serde_json::from_slice(&bytes).unwrap();
        if nested {
            changed["recordIdentity"]["unknown"] = json!(true);
        } else {
            changed["unknown"] = json!(true);
        }
        fs::write(&receipt, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert_evidence_blocked(&fixture).await;
        fs::write(&receipt, &bytes).unwrap();
    }
    fixture.restart().await;
    let finished = fixture.recover(RestoreAction::Continue).await;
    assert_eq!(finished.status, TaskStatus::Succeeded, "{finished:?}");
    assert_eq!(fixture.status().await.state, "clear");
    fixture
        .sql("UPDATE notes SET body='ordinary data after completed'")
        .await;
    fixture.save().await;
    assert_eq!(fixture.status().await.state, "clear");
    // A public copy/migration creates new inodes. Completed evidence is checked
    // by bound immutable bytes, and never rechecks the historical live database.
    for name in [
        "plan.json",
        "journal.json",
        "completion.json",
        "terminal.json",
    ] {
        let source = operation.join(name);
        let saved = fixture._temp.path().join(format!("archived-{name}"));
        let bytes = fs::read(&source).unwrap();
        let permissions = fs::metadata(&source).unwrap().permissions();
        fs::rename(&source, &saved).unwrap();
        assert_evidence_blocked(&fixture).await;
        fs::write(&source, &bytes).unwrap();
        fs::set_permissions(&source, permissions).unwrap();
        assert_eq!(fixture.status().await.state, "clear", "{name}");
    }
    assert_body(&fixture, "ordinary data after completed").await;
    let next = fixture.input(&target).await;
    let task = fixture.apply(next).await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
    assert_body(&fixture, "target").await;
    fixture.finish().await;
}

fn fill_metadata_attempts(operation: &Path) {
    let root = operation.join("metadata-attempts");
    let directory = files::root(&root).unwrap();
    let existing = fs::read_dir(&root).unwrap().count();
    for _ in existing..128 {
        files::create(
            &directory,
            &format!("completion-{}.json", uuid::Uuid::new_v4().simple()),
        )
        .unwrap()
        .sync_all()
        .unwrap();
    }
    directory.sync_all().unwrap();
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable Git payload"]
async fn official_git_data_restore_full_reserve_every_publication_boundary_restarts() {
    let mut covered = false;
    for (failures, point) in (1..=32).enumerate() {
        // Each retained 128-candidate history legitimately consumes the finite
        // project budget. Separate disposable projects test each crash boundary.
        let mut fixture = Fixture::new().await;
        let target = seed(&fixture).await;
        fixture
            .sql(&format!("UPDATE notes SET body='reserve-original-{point}'"))
            .await;
        let input = fixture.input(&target).await;
        fixture.manager.test_data_restore_fault(u32::MAX - 1);
        let prepared = fixture.apply(input).await;
        assert_eq!(
            prepared.status,
            TaskStatus::Failed,
            "point {point}: {prepared:?}"
        );
        assert_eq!(
            prepared.error.as_ref().unwrap().code,
            "PROJECT_DATA_RESTORE_TEST_FAULT",
            "point {point}: {prepared:?}"
        );
        let status = fixture.status().await;
        assert_eq!(status.state, "pending", "{status:?}");
        let root = status.operations[0].retained_path.clone();
        fill_metadata_attempts(&root);
        fixture.restart().await;
        fixture.manager.test_data_restore_fault(point);
        let task = fixture.recover(RestoreAction::Continue).await;
        if task.status == TaskStatus::Succeeded {
            assert!(failures >= 19, "{failures}");
            eprintln!("full reserve publication boundaries verified: {failures}");
            covered = true;
            fixture.finish().await;
            break;
        }
        let error = task.error.as_ref().unwrap();
        assert_eq!(
            error.code, "PROJECT_DATA_RESTORE_TEST_FAULT",
            "point {point}: {task:?}"
        );
        assert_eq!(error.details["operationOutcome"], "committed");
        let before_restart = fixture.status().await;
        if error.message == "after-metadata-partial-write" {
            let partial = ["completion", "terminal"]
                .into_iter()
                .filter_map(|name| {
                    let path = root.join("metadata-reserve").join(name);
                    fs::metadata(path)
                        .ok()
                        .filter(|metadata| metadata.len() > 0)
                        .map(|metadata| metadata.len())
                })
                .collect::<Vec<_>>();
            assert_eq!(
                partial.len(),
                1,
                "partial reserve bytes must remain preserved"
            );
        }
        if root.join("terminal.json").exists()
            && fs::metadata(root.join("terminal.json")).unwrap().mode() & 0o777 == 0o600
        {
            assert_eq!(
                before_restart.state, "pending",
                "unsealed namespace never releases"
            );
        }
        fixture.restart().await;
        let status = fixture.status().await;
        assert_eq!(status.state, before_restart.state);
        if status.state != "clear" {
            assert_eq!(status.state, "pending", "point {point}: {status:?}");
            assert!(!status.operations[0].can_undo);
            let finished = fixture.recover(RestoreAction::Continue).await;
            assert_eq!(
                finished.status,
                TaskStatus::Succeeded,
                "point {point}: {finished:?}"
            );
        }
        assert_eq!(fixture.status().await.state, "clear");
        assert_eq!(
            fs::read_dir(root.join("metadata-attempts"))
                .unwrap()
                .count(),
            128
        );
        assert_eq!(
            fs::metadata(root.join("terminal.json")).unwrap().mode() & 0o777,
            0o400
        );
        assert_body(&fixture, "target").await;
        fixture.finish().await;
    }
    assert!(
        covered,
        "all reserved completion and terminal boundaries must be exercised"
    );
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable Git payload"]
async fn official_git_data_restore_reserve_unknown_prefix_or_identity_stays_preserved() {
    let fixture = Fixture::new().await;
    let target = seed(&fixture).await;
    let input = fixture.input(&target).await;
    fixture.manager.test_data_restore_fault(u32::MAX);
    assert_eq!(fixture.apply(input).await.status, TaskStatus::Failed);
    let status = fixture.status().await;
    let operation = status.operations[0].retained_path.clone();
    let reserve = operation.join("metadata-reserve/completion");
    fs::write(&reserve, b"{}").unwrap();
    assert_evidence_blocked(&fixture).await;
    fs::write(&reserve, b"").unwrap(); // Test-created bytes only.
    fixture.manager.test_data_restore_fault(u32::MAX - 1);
    assert_eq!(
        fixture.recover(RestoreAction::Continue).await.status,
        TaskStatus::Failed
    );
    fill_metadata_attempts(&operation);
    fs::write(&reserve, b"foreign incomplete bytes").unwrap();
    assert_evidence_blocked(&fixture).await;
    assert_eq!(fs::read(&reserve).unwrap(), b"foreign incomplete bytes");
    fs::write(&reserve, b"").unwrap();
    let kept = fixture._temp.path().join("owned-original-reserve");
    fs::rename(&reserve, &kept).unwrap();
    fs::write(&reserve, b"").unwrap();
    fs::set_permissions(&reserve, fs::Permissions::from_mode(0o600)).unwrap();
    assert_evidence_blocked(&fixture).await;
    fs::remove_file(&reserve).unwrap(); // Test-created foreign empty inode only.
    fs::rename(&kept, &reserve).unwrap();
    let task = fixture.recover(RestoreAction::Continue).await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to a verified disposable Git payload"]
async fn official_git_data_restore_recovery_close_failure_reports_actual_retained_state() {
    for (fault, data_state, outcome) in [
        (4, "before", "not_committed"),
        (u32::MAX - 1, "applied", "committed"),
    ] {
        let fixture = Fixture::new().await;
        let target = seed(&fixture).await;
        fixture
            .sql("UPDATE notes SET body='before database join fault'")
            .await;
        let input = fixture.input(&target).await;
        fixture.manager.test_data_restore_fault(fault);
        let task = fixture.apply(input).await;
        assert_eq!(task.status, TaskStatus::Failed, "{task:?}");
        let before = fixture.status().await;
        assert_eq!(before.state, "pending", "{before:?}");
        let operation = &before.operations[0];
        assert_eq!(operation.data_state, data_state);
        let root = operation.retained_path.clone();
        let plan = fs::read(root.join("plan.json")).unwrap();
        let journal = fs::read(root.join("journal.json")).unwrap();
        let slots = serde_json::to_value(&operation.slots).unwrap();
        let live = fixture
            .project
            .root
            .join(&fixture.project.manifest.database);
        let live_bytes = fs::read(&live).ok();
        assert_eq!(live_bytes.is_some(), data_state == "applied");

        // The fault is a real SQLite-owning thread panic and failed OS join,
        // but its connection uses a separate owned database so it cannot alter
        // any original/target bytes whose identity the restore plan protects.
        let disposable = fixture
            ._temp
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-join-fault.sqlite");
        let connection = rusqlite::Connection::open(&disposable).unwrap();
        connection
            .execute_batch("CREATE TABLE probe(value TEXT)")
            .unwrap();
        drop(connection);
        fixture
            .store
            .fixture_panicked_recovery_database_worker(&fixture.project.manifest.id, disposable);
        let failed = fixture.recover(RestoreAction::Continue).await;
        assert_eq!(failed.status, TaskStatus::Failed, "{failed:?}");
        let error = failed.error.as_ref().unwrap();
        assert_eq!(error.code, "DATABASE_CLOSE_FAILED", "{failed:?}");
        assert_eq!(error.details["operationOutcome"], outcome);
        assert_eq!(error.details["dataState"], data_state);
        assert_eq!(error.details["restoreTaskId"], operation.restore_task_id);
        assert_eq!(error.details["retainedPath"], root.to_str().unwrap());
        assert_eq!(error.details["pending"], true);
        assert_eq!(error.details["businessBlocked"], true);
        assert_eq!(error.details["liveDataChanged"], true);
        assert_eq!(error.details["originalRetained"], true);
        assert_eq!(fs::read(root.join("plan.json")).unwrap(), plan);
        assert_eq!(fs::read(root.join("journal.json")).unwrap(), journal);
        assert_eq!(fs::read(&live).ok(), live_bytes);
        let after = fixture.status().await;
        assert_eq!(after.state, "pending", "{after:?}");
        assert_eq!(
            serde_json::to_value(&after.operations[0].slots).unwrap(),
            slots
        );
        assert!(fixture.store.project(&fixture.project.manifest.id).is_err());
        let finished = fixture.recover(RestoreAction::Continue).await;
        assert_eq!(finished.status, TaskStatus::Succeeded, "{finished:?}");
        assert_eq!(fixture.status().await.state, "clear");
        assert_body(&fixture, "target").await;
        assert!(fixture.manager.shutdown().await.is_empty());
        assert!(fixture.tasks.shutdown().await.is_empty());
        let issues = fixture.store.shutdown().await;
        assert_eq!(issues.len(), 1, "{issues:?}");
        assert_eq!(issues[0].component, "database");
        assert_eq!(issues[0].code, "DATABASE_THREAD_JOIN_FAILED");
    }
}
