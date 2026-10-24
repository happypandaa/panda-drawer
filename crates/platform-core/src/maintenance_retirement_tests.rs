//! Historical ZIP fixtures are created explicitly in a disposable workspace.
//! Production creation is retired even in tests; old evidence is materialized
//! only by the explicitly named disposable legacy fixture builder.
use super::*;
use crate::backups::{PendingBuild, SnapshotOptions};
use crate::{CreateProjectInput, SqlCaller, SqlOptions, SqlRequest, TaskStatus};
use std::collections::BTreeMap;

struct LegacyFixture {
    temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    manager: Arc<MaintenanceManager>,
    project_id: String,
    archives: BTreeMap<PathBuf, Vec<u8>>,
}

impl LegacyFixture {
    async fn new() -> Self {
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let project = store
            .create(CreateProjectInput {
                name: "Disposable legacy ZIP preservation fixture".into(),
                description: Some("Temporary automated retirement verification only".into()),
            })
            .await
            .unwrap();
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let manager = MaintenanceManager::new(paths.clone(), store.clone(), tasks.clone()).unwrap();
        let handle = store.project(&project.id).unwrap();
        let directory = paths.workspace_root().join("backups").join(&project.id);
        let fixture_root = temporary.path().canonicalize().unwrap();
        let mut archives = BTreeMap::new();
        // Both kinds were subject to the old ten-archive policy. These are
        // existing evidence fixtures, not calls through production admission.
        for index in 0..12 {
            let backup_id = format!("backup_{}", Uuid::new_v4().simple());
            let prepared = backups::prepare_snapshot(
                &handle.root,
                &handle.manifest,
                &fixture_root.join(format!("legacy-snapshot-{index}")),
                SnapshotOptions {
                    backup_id: backup_id.clone(),
                    kind: if index % 2 == 0 {
                        "maintenanceBaseline"
                    } else {
                        "preRestore"
                    }
                    .into(),
                    note: Some("Pre-existing legacy archive fixture".into()),
                    snapshot_state: SnapshotState::Ready,
                    pending_build: PendingBuild::default(),
                    build_provenance: backups::capture_provenance(&handle).unwrap(),
                },
            )
            .unwrap();
            let path = directory.join(format!("{backup_id}.zip"));
            backups::publish_snapshot(&prepared, &path).unwrap();
            archives.insert(path.clone(), fs::read(path).unwrap());
        }
        let invalid = directory.join("backup_retained_invalid.zip");
        fs::write(&invalid, b"Invalid legacy archive evidence must survive").unwrap();
        archives.insert(invalid.clone(), fs::read(invalid).unwrap());
        Self {
            temporary,
            paths,
            store,
            tasks,
            manager,
            project_id: project.id,
            archives,
        }
    }

    fn assert_archives_unchanged(&self) {
        for (path, expected) in &self.archives {
            assert_eq!(&fs::read(path).unwrap(), expected, "{}", path.display());
        }
    }

    async fn perform(&self, kind: &str, input: OperationInput) -> TaskRecord {
        let accepted = self
            .manager
            .submit(&self.project_id, kind, input)
            .await
            .unwrap();
        for _ in 0..1000 {
            let current = self.tasks.task(&accepted.task_id).unwrap();
            if current.status.is_final() {
                assert_eq!(current.status, TaskStatus::Succeeded, "{current:?}");
                return current;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("legacy maintenance task did not finish");
    }

    async fn stop(&self) {
        self.manager.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
}

#[tokio::test]
async fn legacy_maintenance_end_and_restart_preserve_all_historical_archives() {
    let fixture = LegacyFixture::new().await;
    let project = fixture.store.project(&fixture.project_id).unwrap();
    let control = fixture.store.control(&fixture.project_id).unwrap();
    let mut state = control
        .begin(
            "Explicit pre-existing maintenance fixture".into(),
            Some(builds::input_digest(&project).unwrap()),
            false,
        )
        .unwrap();
    let baseline = fixture.archives.keys().next().unwrap();
    let backup_id = baseline.file_stem().unwrap().to_str().unwrap().to_owned();
    state.baseline_backup_id = Some(backup_id.clone());
    state.protected_backup_ids.push(backup_id);
    state.ready_for_changes = true;
    let maintenance_id = state.maintenance_id.clone();
    control.set(state).unwrap();
    fixture
        .perform(
            "maintenanceEnd",
            OperationInput {
                maintenance_id,
                ..OperationInput::default()
            },
        )
        .await;
    assert_eq!(control.get().mode, "ready");
    fixture.assert_archives_unchanged();
    fixture.stop().await;

    let store = Arc::new(ProjectStore::new(fixture.paths.workspace_root().into()).unwrap());
    let tasks = BuildManager::new(
        fixture.paths.clone(),
        store.clone(),
        "http://127.0.0.1:9".into(),
    )
    .unwrap();
    let manager =
        MaintenanceManager::new(fixture.paths.clone(), store.clone(), tasks.clone()).unwrap();
    let list = manager.list(&fixture.project_id).await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 13);
    assert_eq!(
        manager.status(&fixture.project_id).unwrap()["mode"],
        "ready"
    );
    fixture.assert_archives_unchanged();
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
    assert!(fixture.temporary.path().exists());
}

#[tokio::test]
async fn retired_backup_and_baseline_admission_never_write_or_reserve_anything() {
    let fixture = LegacyFixture::new().await;
    fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut result = BTreeMap::new();
        let mut pending = vec![root.to_owned()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    result.insert(path.clone(), Vec::new());
                    pending.push(path);
                } else {
                    result.insert(path, fs::read(entry.path()).unwrap());
                }
            }
        }
        result
    }
    let control = fixture.store.control(&fixture.project_id).unwrap();
    for recovery_required in [false, true] {
        if recovery_required {
            control
                .begin("Retained unfinished legacy recovery".into(), None, false)
                .unwrap();
            let mut state = control.get();
            state.recovery_required = true;
            state.pending_build = true;
            control.set(state).unwrap();
        }
        let before = tree(fixture.paths.workspace_root());
        let old_control = serde_json::to_value(control.get()).unwrap();
        for kind in ["backup", "maintenanceBegin"] {
            let error = fixture
                .manager
                .submit(
                    &fixture.project_id,
                    kind,
                    OperationInput {
                        reason: Some("Never create a new baseline".into()),
                        ..OperationInput::default()
                    },
                )
                .await
                .unwrap_err();
            assert_eq!(error.code, "BACKUP_WORKFLOW_RETIRED");
            assert_eq!(error.details["operationOutcome"], "not_started");
            assert_eq!(tree(fixture.paths.workspace_root()), before);
            assert_eq!(serde_json::to_value(control.get()).unwrap(), old_control);
            assert!(!fixture.store.has_active_tasks());
        }
    }
    fixture.assert_archives_unchanged();
    fixture.stop().await;
}

#[tokio::test]
async fn legacy_restore_keeps_original_and_uses_existing_archive_without_new_zip() {
    let fixture = LegacyFixture::new().await;
    let root = fixture.store.project(&fixture.project_id).unwrap().root;
    let selected = fixture
        .archives
        .keys()
        .find(|path| path.file_name().unwrap() != "backup_retained_invalid.zip")
        .unwrap();
    let backup_id = selected.file_stem().unwrap().to_str().unwrap().to_owned();
    for sql in [
        "CREATE TABLE new_live_material(value TEXT)",
        "INSERT INTO new_live_material VALUES('preserve in original')",
    ] {
        fixture
            .store
            .execute(
                &fixture.project_id,
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
    fs::write(
        root.join("data/attachments/new-live.txt"),
        b"preserve current attachment",
    )
    .unwrap();
    let restored = fixture
        .perform(
            "restore",
            OperationInput {
                backup_id: Some(backup_id.clone()),
                ..OperationInput::default()
            },
        )
        .await;
    let result = restored.result.unwrap();
    let original = PathBuf::from(result["originalPath"].as_str().unwrap());
    assert_eq!(
        fs::read(original.join("data/attachments/new-live.txt")).unwrap(),
        b"preserve current attachment"
    );
    let original_db = rusqlite::Connection::open_with_flags(
        original.join("data/main.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        original_db
            .query_row("SELECT value FROM new_live_material", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "preserve in original"
    );
    drop(original_db);
    fixture.assert_archives_unchanged();
    assert_eq!(
        fixture
            .manager
            .list(&fixture.project_id)
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        13
    );
    assert!(!original.parent().unwrap().join("snapshot.json").exists());
    let status = fixture.manager.status(&fixture.project_id).unwrap();
    assert_eq!(status["baselineBackupId"], backup_id);
    assert_eq!(status["baselineAvailable"], true);
    assert_eq!(status["readyForChanges"], true);
    fixture
        .perform(
            "maintenanceEnd",
            OperationInput {
                maintenance_id: status["maintenanceId"].as_str().map(str::to_owned),
                ..OperationInput::default()
            },
        )
        .await;
    assert_eq!(
        fixture.manager.status(&fixture.project_id).unwrap()["mode"],
        "ready"
    );
    fixture.assert_archives_unchanged();
    fixture.stop().await;
}

#[tokio::test]
async fn normal_sql_needs_no_backup_or_maintenance_baseline() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = store
        .create(CreateProjectInput {
            name: "Disposable SQL after backup retirement".into(),
            description: None,
        })
        .await
        .unwrap();
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
    let manager = MaintenanceManager::new(paths.clone(), store.clone(), tasks.clone()).unwrap();
    for sql in [
        "CREATE TABLE evidence(value TEXT)",
        "INSERT INTO evidence VALUES('ordinary data')",
    ] {
        store
            .execute(
                &project.id,
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
    let status = manager.status(&project.id).unwrap();
    assert_eq!(status["mode"], "ready");
    assert!(status["baselineBackupId"].is_null());
    assert_eq!(
        fs::read_dir(paths.workspace_root().join("backups"))
            .unwrap()
            .count(),
        0
    );
    manager.shutdown().await;
    tasks.shutdown().await;
    store.shutdown().await;
}
