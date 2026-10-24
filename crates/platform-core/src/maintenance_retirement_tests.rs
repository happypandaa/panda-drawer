//! Historical ZIP fixtures are created explicitly in a disposable workspace.
//! This first retirement step stops automatic pruning; creation and legacy
//! maintenance admission remain unchanged until their callers are migrated.
use super::*;
use crate::{CreateProjectInput, TaskStatus};
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
async fn legacy_baseline_preparation_does_not_prune_existing_archives() {
    let fixture = LegacyFixture::new().await;
    // Admission is intentionally still live in this first, narrowly scoped step.
    fixture
        .perform(
            "maintenanceBegin",
            OperationInput {
                reason: Some("Disposable compatibility preparation".into()),
                ..OperationInput::default()
            },
        )
        .await;
    let status = fixture.manager.status(&fixture.project_id).unwrap();
    assert_eq!(status["readyForChanges"], true);
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
        14
    );
    fixture.stop().await;
}
