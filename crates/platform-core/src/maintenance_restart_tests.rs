//! Restart evidence uses only RuntimePaths::for_test and real temporary ZIPs.
//! Persisted task files model a process ending between durable checkpoints; no
//! production/development application data or Node installation is involved.
use super::*;
use crate::{CreateProjectInput, TaskStatus};

struct InterruptedFixture {
    temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    project_id: String,
    task_id: String,
    backup_id: String,
}

impl InterruptedFixture {
    async fn new() -> Self {
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let project = store
            .create(CreateProjectInput {
                name: "Discardable M3 restart archive evidence".into(),
                description: Some("Isolated temporary fixture; never real workspace data".into()),
            })
            .await
            .unwrap();
        let root = store.project(&project.id).unwrap().root;
        let connection = rusqlite::Connection::open(root.join("data/main.sqlite")).unwrap();
        connection
            .execute_batch("CREATE TABLE restart_evidence(value TEXT NOT NULL); INSERT INTO restart_evidence VALUES('original survives');")
            .unwrap();
        drop(connection);
        fs::write(
            root.join("data/attachments/original.txt"),
            b"Original attachment",
        )
        .unwrap();
        Self {
            temporary,
            paths,
            store,
            project_id: project.id,
            task_id: format!("task_{}", Uuid::new_v4().simple()),
            backup_id: format!("backup_{}", Uuid::new_v4().simple()),
        }
    }

    fn operation_dir(&self) -> PathBuf {
        self.paths
            .workspace_root()
            .join(".platform/operations")
            .join(&self.task_id)
    }

    fn archive_path(&self) -> PathBuf {
        self.paths
            .workspace_root()
            .join("backups")
            .join(&self.project_id)
            .join(format!("{}.zip", self.backup_id))
    }

    fn checkpoint(&self, kind: &str, phase: &str, status: TaskStatus, error: Option<ApiError>) {
        builds::atomic_json(
            &self.operation_dir().join("snapshot.json"),
            &json!({"taskId":self.task_id,"projectId":self.project_id,"backupId":self.backup_id,"maintenanceId":self.store.control(&self.project_id).unwrap().get().maintenance_id}),
        )
        .unwrap();
        builds::atomic_json(
            &self
                .paths
                .workspace_root()
                .join(".platform/tasks")
                .join(&self.task_id)
                .join("task.json"),
            &TaskRecord {
                task_id: self.task_id.clone(),
                project_id: Some(self.project_id.clone()),
                task_type: kind.into(),
                status,
                phase: phase.into(),
                result: None,
                error,
            },
        )
        .unwrap();
        let control = self.store.control(&self.project_id).unwrap();
        let mut state = control.get();
        state.last_task_id = Some(self.task_id.clone());
        control.set(state).unwrap();
        // Freeze is deliberately never serialized. A new process must not inherit
        // this true bit even though no graceful unfreeze occurred in the fixture.
        control.frozen.store(true, Ordering::Release);
    }

    fn prepare(&self, maintenance: bool) -> backups::PreparedSnapshot {
        let project = self.store.project(&self.project_id).unwrap();
        let state = self.store.control(&self.project_id).unwrap().get();
        backups::prepare_snapshot(
            &project.root,
            &project.manifest,
            &self
                .operation_dir()
                .join(format!("snapshot-{}", self.backup_id)),
            SnapshotOptions {
                backup_id: self.backup_id.clone(),
                kind: if maintenance {
                    "maintenanceBaseline"
                } else {
                    "manual"
                }
                .into(),
                note: Some("Persisted restart checkpoint".into()),
                snapshot_state: if maintenance {
                    SnapshotState::Maintenance
                } else {
                    SnapshotState::Ready
                },
                pending_build: PendingBuild {
                    required: state.pending_build,
                    reason: state.pending_build_reason,
                },
                build_provenance: backups::capture_provenance(&project).unwrap(),
            },
        )
        .unwrap()
    }

    fn begin_maintenance(&self) -> String {
        let project = self.store.project(&self.project_id).unwrap();
        let control = self.store.control(&self.project_id).unwrap();
        let mut state = control
            .begin(
                "Restart evidence: maintenance remains explicit".into(),
                Some(builds::input_digest(&project).unwrap()),
                false,
            )
            .unwrap();
        state.pending_build = true;
        state.pending_build_reason =
            Some("Known unfinished source requirement survives restart".into());
        let id = state.maintenance_id.clone().unwrap();
        control.set(state).unwrap();
        id
    }

    fn publish(&self, snapshot: &backups::PreparedSnapshot) {
        let info = backups::publish_snapshot(snapshot, &self.archive_path()).unwrap();
        assert_eq!(info.manifest.backup_id, self.backup_id);
        assert_eq!(info.manifest.project_id, self.project_id);
        assert!(info.database.integrity_ok);
    }

    async fn restart(self) -> Restarted {
        let old_control = self.store.control(&self.project_id).unwrap();
        assert!(old_control.frozen.load(Ordering::Acquire));
        self.store.shutdown().await;
        drop(self.store);
        let store = Arc::new(ProjectStore::new(self.paths.workspace_root().into()).unwrap());
        let tasks = BuildManager::new(
            self.paths.clone(),
            store.clone(),
            "http://127.0.0.1:9".into(),
        )
        .unwrap();
        let manager =
            MaintenanceManager::new(self.paths.clone(), store.clone(), tasks.clone()).unwrap();
        assert!(!store
            .control(&self.project_id)
            .unwrap()
            .frozen
            .load(Ordering::Acquire));
        assert!(
            old_control.frozen.load(Ordering::Acquire),
            "old memory was not graciously unfrozen"
        );
        assert_eq!(manager.status(&self.project_id).unwrap()["busy"], false);
        assert!(store.active_task(&self.project_id).is_none());
        let restarted = Restarted {
            _temporary: self.temporary,
            paths: self.paths,
            store,
            tasks,
            manager,
            project_id: self.project_id,
            task_id: self.task_id,
            backup_id: self.backup_id,
        };
        restarted.assert_original_intact();
        restarted
    }
}

struct Restarted {
    _temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    manager: Arc<MaintenanceManager>,
    project_id: String,
    task_id: String,
    backup_id: String,
}

impl Restarted {
    fn assert_original_intact(&self) {
        let root = self.store.project(&self.project_id).unwrap().root;
        assert_eq!(
            fs::read(root.join("data/attachments/original.txt")).unwrap(),
            b"Original attachment"
        );
        let connection = rusqlite::Connection::open(root.join("data/main.sqlite")).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT value FROM restart_evidence", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            "original survives"
        );
    }

    fn assert_interrupted(&self, phase: &str, outcome: &str) {
        let task = self.tasks.task(&self.task_id).unwrap();
        assert_eq!(task.status, TaskStatus::Interrupted);
        assert_eq!(task.phase, phase);
        assert_eq!(task.result.as_ref().unwrap()["operationOutcome"], outcome);
        assert_eq!(task.result.as_ref().unwrap()["recovered"], true);
        assert_eq!(task.error.as_ref().unwrap().code, "TASK_INTERRUPTED");
        assert_eq!(serde_json::to_value(&task).unwrap()["final"], true);
        let saved: TaskRecord = serde_json::from_slice(
            &fs::read(
                self.paths
                    .workspace_root()
                    .join(".platform/tasks")
                    .join(&self.task_id)
                    .join("task.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(saved.status, TaskStatus::Interrupted);
        assert_eq!(saved.result.unwrap()["operationOutcome"], outcome);
    }

    fn assert_maintenance(&self, maintenance_id: &str, ready: bool) {
        let control = self.store.control(&self.project_id).unwrap();
        let state = control.get();
        assert_eq!(state.mode, "maintenance");
        assert_eq!(state.maintenance_id.as_deref(), Some(maintenance_id));
        assert_eq!(state.ready_for_changes, ready);
        assert!(
            state.pending_build,
            "new baseline or restart must not clear unfinished build requirements"
        );
        assert_eq!(
            state.pending_build_reason.as_deref(),
            Some("Known unfinished source requirement survives restart")
        );
        assert!(!state.recovery_required);
        assert_eq!(
            control.check(true, None, false).unwrap_err().code,
            "PROJECT_MAINTENANCE"
        );
        if ready {
            control.check(false, Some(maintenance_id), false).unwrap();
            assert_eq!(
                state.baseline_backup_id.as_deref(),
                Some(self.backup_id.as_str())
            );
            assert!(state.protected_backup_ids.contains(&self.backup_id));
        } else {
            assert_eq!(
                control
                    .check(false, Some(maintenance_id), false)
                    .unwrap_err()
                    .code,
                "PROJECT_MAINTENANCE"
            );
            assert!(state.baseline_backup_id.is_none());
        }
    }

    async fn stop(self) {
        self.manager.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
}

#[tokio::test]
async fn ordinary_backup_staged_before_publication_restarts_as_not_published() {
    let fixture = InterruptedFixture::new().await;
    let snapshot = fixture.prepare(false);
    fixture.checkpoint("backup", "archive-publish", TaskStatus::Running, None);
    assert!(snapshot.root.join("project/data/main.sqlite").is_file());
    assert!(!fixture.archive_path().exists());
    let restarted = fixture.restart().await;
    restarted.assert_interrupted("archive-publish", "not_published");
    assert_eq!(
        restarted
            .store
            .control(&restarted.project_id)
            .unwrap()
            .get()
            .mode,
        "ready"
    );
    restarted
        .store
        .control(&restarted.project_id)
        .unwrap()
        .check(true, None, false)
        .unwrap();
    assert!(restarted
        .manager
        .list(&restarted.project_id)
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    assert!(
        snapshot.root.exists(),
        "unpublished snapshot remains explicitly distinguishable from a valid backup"
    );
    restarted.stop().await;
}

#[tokio::test]
async fn ordinary_backup_published_with_lost_response_restarts_as_committed_not_succeeded() {
    let fixture = InterruptedFixture::new().await;
    let snapshot = fixture.prepare(false);
    fixture.publish(&snapshot);
    fixture.checkpoint("backup", "archive-publish", TaskStatus::Running, None);
    let restarted = fixture.restart().await;
    restarted.assert_interrupted("archive-publish", "committed");
    assert_eq!(
        restarted
            .store
            .control(&restarted.project_id)
            .unwrap()
            .get()
            .mode,
        "ready"
    );
    let listed = restarted.manager.list(&restarted.project_id).await.unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["backupId"], restarted.backup_id);
    assert_eq!(listed[0]["verified"], true);
    restarted
        .store
        .control(&restarted.project_id)
        .unwrap()
        .check(true, None, false)
        .unwrap();
    restarted.stop().await;
}

#[tokio::test]
async fn maintenance_baseline_preparation_interrupted_keeps_same_session_unready() {
    let fixture = InterruptedFixture::new().await;
    let maintenance_id = fixture.begin_maintenance();
    fixture.checkpoint(
        "maintenanceBegin",
        "snapshot-freeze",
        TaskStatus::Running,
        None,
    );
    fs::create_dir_all(
        fixture
            .operation_dir()
            .join(format!("snapshot-{}", fixture.backup_id)),
    )
    .unwrap();
    fs::write(
        fixture
            .operation_dir()
            .join(format!("snapshot-{}/partial-copy", fixture.backup_id)),
        b"incomplete",
    )
    .unwrap();
    let restarted = fixture.restart().await;
    restarted.assert_interrupted("snapshot-freeze", "maintenance_not_ready");
    restarted.assert_maintenance(&maintenance_id, false);
    assert!(restarted
        .manager
        .list(&restarted.project_id)
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    restarted.stop().await;
}

#[tokio::test]
async fn maintenance_baseline_published_before_readiness_does_not_unlock_on_restart() {
    let fixture = InterruptedFixture::new().await;
    let maintenance_id = fixture.begin_maintenance();
    let snapshot = fixture.prepare(true);
    fixture.publish(&snapshot);
    fixture.checkpoint(
        "maintenanceBegin",
        "archive-publish",
        TaskStatus::Running,
        None,
    );
    let restarted = fixture.restart().await;
    restarted.assert_interrupted("archive-publish", "maintenance_not_ready");
    restarted.assert_maintenance(&maintenance_id, false);
    let listed = restarted.manager.list(&restarted.project_id).await.unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["backupId"], restarted.backup_id);
    assert_eq!(listed[0]["snapshotState"], "maintenance");
    assert_eq!(listed[0]["pendingBuild"]["required"], true);
    restarted.stop().await;
}

#[tokio::test]
async fn maintenance_ready_baseline_with_lost_response_remains_ready_but_task_interrupted() {
    let fixture = InterruptedFixture::new().await;
    let maintenance_id = fixture.begin_maintenance();
    let snapshot = fixture.prepare(true);
    fixture.publish(&snapshot);
    let control = fixture.store.control(&fixture.project_id).unwrap();
    let mut state = control.get();
    state.ready_for_changes = true;
    state.baseline_backup_id = Some(fixture.backup_id.clone());
    state.protected_backup_ids.push(fixture.backup_id.clone());
    control.set(state).unwrap();
    fixture.checkpoint(
        "maintenanceBegin",
        "archive-publish",
        TaskStatus::Running,
        None,
    );
    let restarted = fixture.restart().await;
    restarted.assert_interrupted("archive-publish", "baseline_ready");
    restarted.assert_maintenance(&maintenance_id, true);
    let listed = restarted.manager.list(&restarted.project_id).await.unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert!(!listed[0]["protectionReasons"]
        .as_array()
        .unwrap()
        .is_empty());
    restarted.stop().await;
}

#[tokio::test]
async fn recorded_baseline_preparation_failure_remains_failed_and_maintenance_persists() {
    let fixture = InterruptedFixture::new().await;
    let maintenance_id = fixture.begin_maintenance();
    fs::create_dir_all(fixture.operation_dir()).unwrap();
    fs::write(
        fixture.operation_dir().join("blocked-parent"),
        b"existing file prevents directory creation",
    )
    .unwrap();
    let project = fixture.store.project(&fixture.project_id).unwrap();
    let failure = backups::prepare_snapshot(
        &project.root,
        &project.manifest,
        &fixture.operation_dir().join("blocked-parent/snapshot"),
        SnapshotOptions {
            backup_id: fixture.backup_id.clone(),
            kind: "maintenanceBaseline".into(),
            note: None,
            snapshot_state: SnapshotState::Maintenance,
            pending_build: PendingBuild::default(),
            build_provenance: None,
        },
    )
    .unwrap_err();
    assert_eq!(failure.code, "FILESYSTEM_ERROR");
    let control = fixture.store.control(&fixture.project_id).unwrap();
    let mut state = control.get();
    state.last_error = Some(failure.clone());
    control.set(state).unwrap();
    fixture.checkpoint(
        "maintenanceBegin",
        "snapshot-freeze",
        TaskStatus::Failed,
        Some(failure),
    );
    let restarted = fixture.restart().await;
    restarted.assert_maintenance(&maintenance_id, false);
    let task = restarted.tasks.task(&restarted.task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
    assert!(task.result.is_none());
    assert_eq!(task.error.unwrap().code, "FILESYSTEM_ERROR");
    assert_eq!(
        restarted
            .store
            .control(&restarted.project_id)
            .unwrap()
            .get()
            .last_error
            .unwrap()
            .code,
        "FILESYSTEM_ERROR"
    );
    assert!(restarted
        .manager
        .list(&restarted.project_id)
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    restarted.stop().await;
}

#[tokio::test]
async fn missing_published_baseline_is_not_reported_ready_and_rejects_management_sql() {
    let fixture = InterruptedFixture::new().await;
    let maintenance_id = fixture.begin_maintenance();
    let snapshot = fixture.prepare(true);
    fixture.publish(&snapshot);
    let control = fixture.store.control(&fixture.project_id).unwrap();
    let mut state = control.get();
    state.ready_for_changes = true;
    state.baseline_backup_id = Some(fixture.backup_id.clone());
    state.protected_backup_ids.push(fixture.backup_id.clone());
    control.set(state).unwrap();
    fixture.checkpoint(
        "maintenanceBegin",
        "archive-publish",
        TaskStatus::Running,
        None,
    );
    fs::remove_file(fixture.archive_path()).unwrap();
    let restarted = fixture.restart().await;
    restarted.assert_interrupted("archive-publish", "maintenance_not_ready");
    let state = restarted
        .store
        .control(&restarted.project_id)
        .unwrap()
        .get();
    assert_eq!(state.mode, "maintenance");
    assert_eq!(
        state.maintenance_id.as_deref(),
        Some(maintenance_id.as_str())
    );
    let status = restarted.manager.status(&restarted.project_id).unwrap();
    assert_eq!(status["baselineAvailable"], false);
    assert_eq!(status["readyForChanges"], false);
    assert!(restarted
        .manager
        .list(&restarted.project_id)
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    let request: crate::SqlRequest = serde_json::from_value(
        json!({"sql":"INSERT INTO restart_evidence VALUES ('must not execute')"}),
    )
    .unwrap();
    let error = restarted
        .store
        .execute_managed(
            &restarted.project_id,
            request,
            None,
            crate::SqlCaller::Management,
            Some(&maintenance_id),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "PROJECT_MAINTENANCE");
    assert_eq!(error.details["operationOutcome"], "not_started");
    let project = restarted.store.project(&restarted.project_id).unwrap();
    let connection = rusqlite::Connection::open(project.root.join("data/main.sqlite")).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM restart_evidence", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    drop(connection);
    let repair = restarted
        .manager
        .submit(
            &restarted.project_id,
            "maintenanceBegin",
            OperationInput {
                maintenance_id: Some(maintenance_id.clone()),
                reason: Some("Recreate missing baseline without ending maintenance".into()),
                ..OperationInput::default()
            },
        )
        .await
        .unwrap();
    let mut repaired = None;
    for _ in 0..1000 {
        let current = restarted.tasks.task(&repair.task_id).unwrap();
        if current.status.is_final() {
            repaired = Some(current);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let repaired = repaired.expect("baseline retry did not complete");
    assert_eq!(repaired.status, TaskStatus::Succeeded, "{repaired:?}");
    let repaired_state = restarted
        .store
        .control(&restarted.project_id)
        .unwrap()
        .get();
    assert_eq!(
        repaired_state.maintenance_id.as_deref(),
        Some(maintenance_id.as_str())
    );
    assert!(repaired_state.ready_for_changes);
    assert!(repaired_state.pending_build);
    assert_ne!(
        repaired_state.baseline_backup_id.as_deref(),
        Some(restarted.backup_id.as_str())
    );
    assert_eq!(
        restarted.tasks.task(&restarted.task_id).unwrap().status,
        TaskStatus::Interrupted
    );
    restarted.stop().await;
}

#[tokio::test]
async fn previous_maintenance_journal_cannot_claim_current_sessions_ready_baseline() {
    let fixture = InterruptedFixture::new().await;
    let previous_id = fixture.begin_maintenance();
    let snapshot = fixture.prepare(true);
    fixture.publish(&snapshot);
    fixture.checkpoint(
        "maintenanceBegin",
        "archive-publish",
        TaskStatus::Running,
        None,
    );
    let control = fixture.store.control(&fixture.project_id).unwrap();
    let mut state = control.get();
    let current_id = format!("maintenance_{}", Uuid::new_v4().simple());
    assert_ne!(current_id, previous_id);
    state.maintenance_id = Some(current_id.clone());
    state.ready_for_changes = true;
    state.baseline_backup_id = Some(fixture.backup_id.clone());
    state.protected_backup_ids.push(fixture.backup_id.clone());
    control.set(state).unwrap();
    let restarted = fixture.restart().await;
    restarted.assert_interrupted("archive-publish", "maintenance_not_ready");
    restarted.assert_maintenance(&current_id, true);
    assert_eq!(
        restarted
            .manager
            .list(&restarted.project_id)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        1
    );
    restarted.stop().await;
}

async fn wait_for_operation(tasks: &BuildManager, id: &str) -> TaskRecord {
    for _ in 0..1000 {
        let task = tasks.task(id).unwrap();
        if task.status.is_final() {
            return task;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("operation did not finish");
}

#[tokio::test]
async fn resolved_restore_lost_response_keeps_restored_outcome_without_replaying_state() {
    let mut fixture = InterruptedFixture::new().await;
    fixture.publish(&fixture.prepare(false));
    let root = fixture.store.project(&fixture.project_id).unwrap().root;
    rusqlite::Connection::open(root.join("data/main.sqlite"))
        .unwrap()
        .execute(
            "INSERT INTO restart_evidence VALUES ('pre-restore change')",
            [],
        )
        .unwrap();
    let tasks = BuildManager::new(
        fixture.paths.clone(),
        fixture.store.clone(),
        "http://127.0.0.1:9".into(),
    )
    .unwrap();
    let manager =
        MaintenanceManager::new(fixture.paths.clone(), fixture.store.clone(), tasks.clone())
            .unwrap();
    let submitted = manager
        .submit(
            &fixture.project_id,
            "restore",
            OperationInput {
                backup_id: Some(fixture.backup_id.clone()),
                ..OperationInput::default()
            },
        )
        .await
        .unwrap();
    let mut task = wait_for_operation(&tasks, &submitted.task_id).await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
    fixture.task_id = task.task_id.clone();
    let operation = fixture.operation_dir();
    let journal: RestoreJournal =
        serde_json::from_slice(&fs::read(operation.join("journal.json")).unwrap()).unwrap();
    assert!(journal.committed && journal.resolved);
    let snapshot: Value =
        serde_json::from_slice(&fs::read(operation.join("snapshot.json")).unwrap()).unwrap();
    assert_ne!(snapshot["backupId"], fixture.backup_id);
    let original = operation.join("original");
    assert_eq!(
        rusqlite::Connection::open(original.join("data/main.sqlite"))
            .unwrap()
            .query_row("SELECT count(*) FROM restart_evidence", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
    let identities = (identity(&root).unwrap(), identity(&original).unwrap());

    // A completed journal is historical: later legitimate session state and
    // document registrations must remain current even if its task response was lost.
    let control = fixture.store.control(&fixture.project_id).unwrap();
    let mut later_state = control.get();
    later_state.maintenance_id = Some(format!("maintenance_{}", Uuid::new_v4().simple()));
    later_state.reason = Some("Later maintenance session must survive journal replay".into());
    later_state.pending_build = true;
    later_state.pending_build_reason = Some("Later source changes".into());
    control.set(later_state.clone()).unwrap();
    let epoch = fixture.store.rebind(&fixture.project_id, true).unwrap();
    manager.shutdown().await;
    tasks.shutdown().await;
    drop(manager);
    drop(tasks);
    task.status = TaskStatus::Running;
    task.phase = "restore-commit".into();
    task.result = None;
    task.error = None;
    builds::atomic_json(
        &fixture
            .paths
            .workspace_root()
            .join(".platform/tasks")
            .join(&task.task_id)
            .join("task.json"),
        &task,
    )
    .unwrap();
    control.frozen.store(true, Ordering::Release);
    let restarted = fixture.restart().await;
    restarted.assert_interrupted("restore-commit", "restored");
    let result = restarted
        .tasks
        .task(&restarted.task_id)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(result["originalPath"], json!(original));
    assert_eq!(result["originalRetained"], true);
    assert_eq!(identity(&root).unwrap(), identities.0);
    assert_eq!(identity(&original).unwrap(), identities.1);
    assert!(!operation.join("candidate").exists());
    assert_eq!(
        restarted
            .store
            .project(&restarted.project_id)
            .unwrap()
            .document_epoch,
        epoch
    );
    assert_eq!(
        serde_json::to_value(
            restarted
                .store
                .control(&restarted.project_id)
                .unwrap()
                .get()
        )
        .unwrap(),
        serde_json::to_value(later_state).unwrap()
    );
    assert_eq!(
        MaintenanceManager::reconcile_journal(
            &restarted.paths,
            &restarted.store,
            &operation.join("journal.json")
        )
        .unwrap(),
        "restored"
    );
    assert_eq!(
        restarted
            .store
            .project(&restarted.project_id)
            .unwrap()
            .document_epoch,
        epoch
    );
    restarted.stop().await;
}

#[tokio::test]
async fn resolved_rollback_lost_response_is_not_overwritten_by_pre_restore_snapshot() {
    let fixture = InterruptedFixture::new().await;
    fixture.begin_maintenance();
    fixture.publish(&fixture.prepare(true));
    fixture.checkpoint("restore", "preserve-original", TaskStatus::Running, None);
    let root = fixture.store.project(&fixture.project_id).unwrap().root;
    let candidate = fixture.operation_dir().join("candidate");
    fs::create_dir_all(&candidate).unwrap();
    fs::write(
        candidate.join("retained.txt"),
        b"Candidate remains retained",
    )
    .unwrap();
    let journal_path = fixture.operation_dir().join("journal.json");
    builds::atomic_json(
        &journal_path,
        &RestoreJournal {
            task_id: fixture.task_id.clone(),
            project_id: fixture.project_id.clone(),
            backup_id: fixture.backup_id.clone(),
            original_identity: persistent_identity(&root).unwrap(),
            candidate_identity: persistent_identity(&candidate).unwrap(),
            committed: false,
            resolved: false,
            final_control: None,
            previous_control: fixture.store.control(&fixture.project_id).unwrap().get(),
            previous_provenance: None,
        },
    )
    .unwrap();
    assert_eq!(
        MaintenanceManager::reconcile_journal(&fixture.paths, &fixture.store, &journal_path)
            .unwrap(),
        "rolled_back"
    );
    let journal: RestoreJournal =
        serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
    assert!(journal.resolved && !journal.committed);
    let epoch = fixture
        .store
        .project(&fixture.project_id)
        .unwrap()
        .document_epoch;
    let identities = (identity(&root).unwrap(), identity(&candidate).unwrap());
    let restarted = fixture.restart().await;
    restarted.assert_interrupted("preserve-original", "rolled_back");
    let result = restarted
        .tasks
        .task(&restarted.task_id)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(result["originalRetained"], false);
    assert!(result.get("originalPath").is_none());
    assert_eq!(identity(&root).unwrap(), identities.0);
    assert_eq!(identity(&candidate).unwrap(), identities.1);
    assert_eq!(
        fs::read(candidate.join("retained.txt")).unwrap(),
        b"Candidate remains retained"
    );
    assert_eq!(
        restarted
            .store
            .project(&restarted.project_id)
            .unwrap()
            .document_epoch,
        epoch
    );
    restarted.stop().await;
}

// Immutable owned files provide a real, deterministic rename failure on the
// supported macOS target; permissions alone are repaired by atomic_json.
#[cfg(target_os = "macos")]
struct ImmutableFile(std::ffi::CString);
#[cfg(target_os = "macos")]
impl ImmutableFile {
    fn new(path: &Path) -> Self {
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(
            unsafe { libc::chflags(name.as_ptr(), libc::UF_IMMUTABLE) },
            0
        );
        Self(name)
    }
}
#[cfg(target_os = "macos")]
impl Drop for ImmutableFile {
    fn drop(&mut self) {
        assert_eq!(unsafe { libc::chflags(self.0.as_ptr(), 0) }, 0);
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn reconciliation_journal_write_failure_blocks_even_if_control_cannot_persist() {
    for block_control in [false, true] {
        let fixture = InterruptedFixture::new().await;
        fixture.begin_maintenance();
        fixture.publish(&fixture.prepare(true));
        let control = fixture.store.control(&fixture.project_id).unwrap();
        let mut ready = control.get();
        ready.ready_for_changes = true;
        ready.recovery_required = false;
        ready.baseline_backup_id = Some(fixture.backup_id.clone());
        control.set(ready.clone()).unwrap();
        control
            .check(false, ready.maintenance_id.as_deref(), false)
            .unwrap();
        let root = fixture.store.project(&fixture.project_id).unwrap().root;
        let journal_path = fixture.operation_dir().join("journal.json");
        let mut journal = RestoreJournal {
            task_id: fixture.task_id.clone(),
            project_id: fixture.project_id.clone(),
            backup_id: fixture.backup_id.clone(),
            original_identity: serde_json::from_value(json!([0, 0])).unwrap(),
            candidate_identity: persistent_identity(&root).unwrap(),
            committed: true,
            resolved: false,
            final_control: Some(ready.clone()),
            previous_control: ready.clone(),
            previous_provenance: None,
        };
        builds::atomic_json(&journal_path, &journal).unwrap();
        let control_path = fixture
            .paths
            .workspace_root()
            .join(".platform/project-state/maintenance")
            .join(format!("{}.json", fixture.project_id));
        let journal_guard = ImmutableFile::new(&journal_path);
        let control_guard = block_control.then(|| ImmutableFile::new(&control_path));
        let error = if block_control {
            // Model control.set(final_state) completing immediately before the
            // final journal write, then both durable locations becoming unwritable.
            MaintenanceManager::persist_reconciliation(
                &journal_path,
                &mut journal,
                &control,
                "restored",
            )
            .unwrap_err()
        } else {
            MaintenanceManager::reconcile_journal(&fixture.paths, &fixture.store, &journal_path)
                .unwrap_err()
        };
        assert_eq!(error.code, "FILESYSTEM_ERROR");
        assert_eq!(error.details["operationOutcome"], "needs_attention");
        assert_eq!(error.details["recoveryStage"], "resolve-journal");
        assert_eq!(
            error.details.get("controlPersistenceError").is_some(),
            block_control
        );
        let state = control.get();
        assert_eq!(state.mode, "maintenance");
        assert!(state.recovery_required);
        assert!(!state.ready_for_changes);
        assert_eq!(
            serde_json::to_value(state.last_error).unwrap(),
            json!(error)
        );
        for browser in [false, true] {
            assert_eq!(
                control
                    .check(browser, ready.maintenance_id.as_deref(), false)
                    .unwrap_err()
                    .code,
                "RECOVERY_REQUIRED"
            );
        }
        let saved: ControlState =
            serde_json::from_slice(&fs::read(&control_path).unwrap()).unwrap();
        assert_eq!(saved.recovery_required, !block_control);
        assert_eq!(saved.ready_for_changes, block_control);
        let saved: RestoreJournal =
            serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
        assert!(
            !saved.resolved,
            "failed final checkpoint must leave journal unresolved"
        );
        drop(control_guard);
        drop(journal_guard);
        fixture.store.shutdown().await;
    }
}
