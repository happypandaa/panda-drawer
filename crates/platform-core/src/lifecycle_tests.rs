//! Disposable lifecycle evidence; all workspaces and exports belong to TempDir.
use super::*;
use crate::{CreateProjectInput, TaskStatus};
use std::time::Duration;

struct Fixture {
    _temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    lifecycle: Arc<LifecycleManager>,
    id: String,
}
impl Fixture {
    async fn new() -> Self {
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let project = store
            .create(CreateProjectInput {
                name: "M4 disposable source".into(),
                description: Some("Business content is independent of platform identity".into()),
            })
            .await
            .unwrap();
        let root = store.project(&project.id).unwrap().root;
        rusqlite::Connection::open(root.join("data/main.sqlite")).unwrap().execute_batch("CREATE TABLE evidence(id TEXT PRIMARY KEY,value TEXT); INSERT INTO evidence VALUES('business-uuid','original');").unwrap();
        fs::write(
            root.join("data/attachments/original.txt"),
            b"independent attachment",
        )
        .unwrap();
        fs::write(
            root.join("meta/icon.png"),
            b"optional invalid icon remains a warning",
        )
        .unwrap();
        let mut manifest: ProjectManifest = read_json(&root.join("project.json")).unwrap();
        manifest.icon = Some("meta/icon.png".into());
        builds::atomic_json(&root.join("project.json"), &manifest).unwrap();
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let maintenance = MaintenanceManager::new(store.clone(), tasks.clone());
        let lifecycle = LifecycleManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            maintenance.clone(),
        )
        .unwrap();
        Self {
            _temporary: temporary,
            paths,
            store,
            tasks,
            lifecycle,
            id: project.id,
        }
    }
    fn export_path(&self, name: &str) -> PathBuf {
        self.paths.data_root().parent().unwrap().join(name)
    }
    async fn wait(&self, task: &TaskRecord) -> TaskRecord {
        for _ in 0..2000 {
            let record = if task.task_type == "delete" {
                self.lifecycle
                    .task_for_project(&task.task_id, task.project_id.as_deref().unwrap(), "delete")
                    .unwrap()
            } else {
                self.tasks.task(&task.task_id).unwrap()
            };
            if record.status.is_final() {
                // The runner drops source/target leases after saving the final record.
                tokio::task::yield_now().await;
                return record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!("lifecycle task timed out");
    }
    async fn project(&self, kind: &str, mut input: Value) -> TaskRecord {
        if kind == "delete" && input.get("expectedDocumentEpoch").is_none() {
            if let Some(epoch) = self.store.registered_epoch(&self.id).unwrap().or_else(|| {
                self.store
                    .pending_deletion(&self.id)
                    .unwrap()
                    .map(|marker| marker.target_epoch)
            }) {
                input["expectedDocumentEpoch"] = json!(epoch);
            }
        }
        let task = self
            .lifecycle
            .submit_project(&self.id, kind, input)
            .await
            .unwrap();
        self.wait(&task).await
    }
    async fn stop(self) {
        self.lifecycle.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
    async fn restart_saved(self) -> Self {
        self.lifecycle.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
        let store = Arc::new(ProjectStore::new(self.paths.workspace_root().into()).unwrap());
        let tasks = BuildManager::new(
            self.paths.clone(),
            store.clone(),
            "http://127.0.0.1:9".into(),
        )
        .unwrap();
        let maintenance = MaintenanceManager::new(store.clone(), tasks.clone());
        let lifecycle = LifecycleManager::new(
            self.paths.clone(),
            store.clone(),
            tasks.clone(),
            maintenance,
        )
        .unwrap();
        Self {
            _temporary: self._temporary,
            paths: self.paths,
            store,
            tasks,
            lifecycle,
            id: self.id,
        }
    }
    async fn restart_task(self, mut task: TaskRecord) -> Self {
        self.lifecycle.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
        task.status = TaskStatus::Running;
        task.result = None;
        task.error = None;
        builds::atomic_json(
            &self
                .paths
                .workspace_root()
                .join(".platform/tasks")
                .join(&task.task_id)
                .join("task.json"),
            &task,
        )
        .unwrap();
        let store = Arc::new(ProjectStore::new(self.paths.workspace_root().into()).unwrap());
        let tasks = BuildManager::new(
            self.paths.clone(),
            store.clone(),
            "http://127.0.0.1:9".into(),
        )
        .unwrap();
        let maintenance = MaintenanceManager::new(store.clone(), tasks.clone());
        let lifecycle = LifecycleManager::new(
            self.paths.clone(),
            store.clone(),
            tasks.clone(),
            maintenance.clone(),
        )
        .unwrap();
        Self {
            _temporary: self._temporary,
            paths: self.paths,
            store,
            tasks,
            lifecycle,
            id: self.id,
        }
    }
}
fn good(task: &TaskRecord) -> &Value {
    assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
    task.result.as_ref().unwrap()
}

#[tokio::test]
async fn export_copy_import_preserve_data_metadata_and_independent_files_without_node() {
    let fixture = Fixture::new().await;
    let archive = fixture.export_path("export.zip");
    let task = fixture
        .project("export", json!({"destinationPath":archive}))
        .await;
    good(&task);
    let before = sha256(&archive).unwrap();
    let conflict = fixture
        .lifecycle
        .submit_import(json!({"archivePath":archive,"mode":"original"}))
        .await
        .unwrap_err();
    assert_eq!(conflict.code, "INVALID_INPUT");
    let submitted = fixture
        .lifecycle
        .submit_import(json!({"archivePath":archive,"name":"Imported copy"}))
        .await
        .unwrap();
    let new_id = submitted.result.as_ref().unwrap()["targetProjectId"]
        .as_str()
        .unwrap()
        .to_owned();
    let task = fixture.wait(&submitted).await;
    assert_eq!(good(&task)["mode"], "ready");
    assert_ne!(new_id, fixture.id);
    let original = fixture.store.project(&fixture.id).unwrap();
    let imported = fixture.store.project(&new_id).unwrap();
    assert_eq!(imported.manifest.name, "Imported copy");
    assert_eq!(imported.manifest.description, original.manifest.description);
    assert_eq!(imported.manifest.icon, original.manifest.icon);
    for relative in [
        "data/main.sqlite",
        "data/attachments/original.txt",
        "meta/icon.png",
    ] {
        let old = fs::metadata(original.root.join(relative)).unwrap();
        let copied = fs::metadata(imported.root.join(relative)).unwrap();
        assert_ne!((old.dev(), old.ino()), (copied.dev(), copied.ino()));
        assert_eq!(copied.nlink(), 1);
    }
    rusqlite::Connection::open(imported.root.join("data/main.sqlite"))
        .unwrap()
        .execute("UPDATE evidence SET value='copy-only'", [])
        .unwrap();
    let original_value: String = rusqlite::Connection::open(original.root.join("data/main.sqlite"))
        .unwrap()
        .query_row(
            "SELECT value FROM evidence WHERE id='business-uuid'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(original_value, "original");
    assert_eq!(sha256(&archive).unwrap(), before);
    let copied = fixture.project("copy", json!({})).await;
    let copied_id = good(&copied)["projectId"].as_str().unwrap();
    assert_eq!(
        fixture.store.project(copied_id).unwrap().manifest.name,
        "M4 disposable source · 副本"
    );
    assert_eq!(
        builds::build_info(&fixture.paths, &fixture.store.project(copied_id).unwrap()).unwrap()
            ["sourceState"],
        "unbuilt"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn repeated_imports_without_mode_create_independent_copies_and_preserve_business_ids() {
    let fixture = Fixture::new().await;
    let original = fixture.store.project(&fixture.id).unwrap();
    let source_relative = "app/src/disposable-import-evidence.txt";
    let attachment_relative = "data/attachments/original.txt";
    fs::write(original.root.join(source_relative), b"original source").unwrap();
    let original_manifest = fs::read(original.root.join("project.json")).unwrap();
    let original_database = sha256(&original.root.join("data/main.sqlite")).unwrap();
    let archive = fixture.export_path("disposable-repeated-import.zip");
    good(
        &fixture
            .project("export", json!({"destinationPath":archive}))
            .await,
    );
    let archive_digest = sha256(&archive).unwrap();

    let mut imported_ids = Vec::new();
    for _ in 0..2 {
        let submitted = fixture
            .lifecycle
            .submit_import(json!({"archivePath":archive}))
            .await
            .unwrap();
        let imported = fixture.wait(&submitted).await;
        let id = good(&imported)["projectId"].as_str().unwrap().to_owned();
        assert_ne!(id, fixture.id);
        assert!(!imported_ids.contains(&id));
        imported_ids.push(id);
    }
    let first = fixture.store.project(&imported_ids[0]).unwrap();
    let second = fixture.store.project(&imported_ids[1]).unwrap();
    let evidence = |root: &Path| {
        let database = rusqlite::Connection::open(root.join("data/main.sqlite")).unwrap();
        database
            .query_row("SELECT id,value FROM evidence", [], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
    };
    for project in [&original, &first, &second] {
        assert_eq!(
            evidence(&project.root),
            ("business-uuid".into(), "original".into())
        );
        assert_eq!(
            fs::read(project.root.join(source_relative)).unwrap(),
            b"original source"
        );
        assert_eq!(
            fs::read(project.root.join(attachment_relative)).unwrap(),
            b"independent attachment"
        );
    }
    for relative in ["data/main.sqlite", attachment_relative, source_relative] {
        let mut identities = Vec::new();
        for project in [&original, &first, &second] {
            let metadata = fs::metadata(project.root.join(relative)).unwrap();
            assert_eq!(metadata.nlink(), 1);
            let identity = (metadata.dev(), metadata.ino());
            assert!(!identities.contains(&identity));
            identities.push(identity);
        }
    }

    rusqlite::Connection::open(first.root.join("data/main.sqlite"))
        .unwrap()
        .execute(
            "UPDATE evidence SET value='first-copy-only' WHERE id='business-uuid'",
            [],
        )
        .unwrap();
    fs::write(first.root.join(source_relative), b"first copy source").unwrap();
    fs::write(
        first.root.join(attachment_relative),
        b"first copy attachment",
    )
    .unwrap();
    assert_eq!(
        evidence(&first.root),
        ("business-uuid".into(), "first-copy-only".into())
    );
    for project in [&original, &second] {
        assert_eq!(
            evidence(&project.root),
            ("business-uuid".into(), "original".into())
        );
        assert_eq!(
            fs::read(project.root.join(source_relative)).unwrap(),
            b"original source"
        );
        assert_eq!(
            fs::read(project.root.join(attachment_relative)).unwrap(),
            b"independent attachment"
        );
    }
    assert_eq!(
        fs::read(original.root.join("project.json")).unwrap(),
        original_manifest
    );
    assert_eq!(
        sha256(&original.root.join("data/main.sqlite")).unwrap(),
        original_database
    );
    assert_eq!(sha256(&archive).unwrap(), archive_digest);
    fixture.stop().await;
}

#[tokio::test]
async fn export_collision_and_project_destination_leave_original_files_unchanged() {
    let fixture = Fixture::new().await;
    let archive = fixture.export_path("existing.zip");
    fs::write(&archive, b"previous export").unwrap();
    assert_eq!(
        fixture
            .lifecycle
            .submit_project(&fixture.id, "export", json!({"destinationPath":archive}))
            .await
            .unwrap_err()
            .code,
        "EXPORT_DESTINATION_EXISTS"
    );
    let root = fixture.store.project(&fixture.id).unwrap().root;
    assert_eq!(
        fixture
            .lifecycle
            .submit_project(
                &fixture.id,
                "export",
                json!({"destinationPath":root.join("nested.zip")})
            )
            .await
            .unwrap_err()
            .code,
        "INVALID_EXPORT_PATH"
    );
    assert_eq!(fs::read(&archive).unwrap(), b"previous export");
    assert_eq!(
        fixture
            .lifecycle
            .submit_project(
                &fixture.id,
                "export",
                json!({"destinationPath":archive,"overwrite":true}),
            )
            .await
            .unwrap_err()
            .code,
        "EXPORT_DESTINATION_EXISTS",
    );
    assert_eq!(fs::read(&archive).unwrap(), b"previous export");
    let independent = fixture.export_path("new-exclusive-export.zip");
    good(
        &fixture
            .project("export", json!({"destinationPath":independent}))
            .await,
    );
    assert_eq!(
        backups::read_info(&independent)
            .unwrap()
            .manifest
            .project_id,
        fixture.id
    );
    fixture.stop().await;
}

#[tokio::test]
async fn export_stage_rechecks_selected_parent_after_capture_before_its_first_payload_byte() {
    use std::cell::Cell;
    let fixture = Fixture::new().await;
    let folder = fixture
        .paths
        .data_root()
        .parent()
        .unwrap()
        .join("disposable-selected-export-parent");
    fs::create_dir(&folder).unwrap();
    let original_parent = persistent_identity(&folder).unwrap();
    let moved = folder.with_extension("user-renamed");
    let archive = fixture.export_path("disposable-private-snapshot-bytes.zip");
    fs::write(
        &archive,
        b"private project payload must not reach the replacement folder",
    )
    .unwrap();
    let task = fixture
        .tasks
        .create_readonly_operation(&fixture.id, "export")
        .unwrap();
    let owner = crate::web_project_ownership::WebProjectOwnership::new(
        fixture.paths.data_root().into(),
        fixture.paths.workspace_root().into(),
    );
    let stage = folder.join(format!(".export-{}.partial", task.task_id));
    let calls = Cell::new(0);
    let error =
        write_owned_export_stage(&owner, &fixture.id, &task.task_id, &archive, &stage, || {
            if !original_parent.matches_path(&folder).unwrap_or(false) {
                return Err(ApiError::new(
                    "EXPORT_DESTINATION_CHANGED",
                    "disposable selected parent was replaced",
                ));
            }
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                fs::rename(&folder, &moved).unwrap();
                fs::create_dir(&folder).unwrap();
                fs::write(
                    folder.join("independent-B.txt"),
                    b"foreign B folder stays unchanged",
                )
                .unwrap();
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, "EXPORT_DESTINATION_CHANGED");
    assert_eq!(fs::metadata(&stage).unwrap().len(), 0);
    assert_eq!(
        fs::read(folder.join("independent-B.txt")).unwrap(),
        b"foreign B folder stays unchanged"
    );
    fixture.tasks.finish(&task.task_id, Err(error)).unwrap();
    let deleted = fixture.project("delete", json!({})).await;
    assert_eq!(good(&deleted)["operationOutcome"], "deleted");
    assert!(!stage.exists());
    assert_eq!(
        fs::read(folder.join("independent-B.txt")).unwrap(),
        b"foreign B folder stays unchanged"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn copied_bundled_artifact_keeps_portable_provenance_and_matching_inputs() {
    let fixture = Fixture::new().await;
    let source = fixture.store.project(&fixture.id).unwrap();
    fs::create_dir_all(source.root.join("dist")).unwrap();
    fs::write(
        source.root.join("dist/index.html"),
        b"<!doctype html><html><body>Portable initial application</body></html>",
    )
    .unwrap();
    let provenance = backups::PortableBuildProvenance {
        origin: "bundledTemplate".into(),
        completed_at: 123,
        digest_rule_version: 1,
        input_digest: builds::input_digest(&source).unwrap(),
        artifact_digest: builds::tree_digest(&source.root.join("dist"), false).unwrap(),
        node_version: "24.14.1".into(),
        npm_version: "11.11.0".into(),
        sdk_version: Some("0.1.0".into()),
        entry_relative: "index.html".into(),
        template_version: Some("fixture-1".into()),
    };
    backups::restore_provenance(
        &source.root,
        fixture.paths.workspace_root(),
        &source.manifest,
        Some(&provenance),
    )
    .unwrap();
    let before = builds::build_info(&fixture.paths, &source).unwrap();
    assert_eq!(before["sourceState"], "matched");
    let task = fixture
        .project("copy", json!({"name":"Renamed independent initial app"}))
        .await;
    let id = good(&task)["projectId"].as_str().unwrap();
    let copied = fixture.store.project(id).unwrap();
    let info = builds::build_info(&fixture.paths, &copied).unwrap();
    assert_eq!(info["sourceState"], "matched");
    let restored = backups::capture_provenance(&copied).unwrap().unwrap();
    assert_eq!(restored.origin, "bundledTemplate");
    assert_eq!(restored.completed_at, 123);
    assert_eq!(restored.input_digest, provenance.input_digest);
    assert_eq!(
        fs::read(copied.root.join("README.md")).unwrap(),
        fs::read(source.root.join("README.md")).unwrap()
    );
    assert!(copied.root.join(".platform/handoff.md").is_file());
    fixture.stop().await;
}

#[tokio::test]
async fn unknown_and_foreign_key_archives_are_retained_without_registering_new_instances() {
    for foreign_key in [false, true] {
        let fixture = Fixture::new().await;
        let source = fixture.store.project(&fixture.id).unwrap();
        if foreign_key {
            rusqlite::Connection::open(source.root.join("data/main.sqlite")).unwrap().execute_batch("PRAGMA foreign_keys=OFF; CREATE TABLE parent(id PRIMARY KEY); CREATE TABLE child(id REFERENCES parent(id)); INSERT INTO child VALUES(99);").unwrap();
        }
        let snapshot = backups::prepare_snapshot(
            &source.root,
            &source.manifest,
            &fixture.paths.data_root().join("external-snapshot"),
            SnapshotOptions {
                backup_id: format!("backup_{}", Uuid::new_v4().simple()),
                kind: "manual".into(),
                note: None,
                snapshot_state: if foreign_key {
                    SnapshotState::Ready
                } else {
                    SnapshotState::Unknown
                },
                pending_build: PendingBuild::default(),
                build_provenance: None,
            },
        )
        .unwrap();
        let archive = fixture.export_path("unknown-or-fk.zip");
        backups::publish_snapshot(&snapshot, &archive).unwrap();
        let before = fs::read(&archive).unwrap();
        let submitted = fixture
            .lifecycle
            .submit_import(json!({"archivePath":archive,"mode":"copy"}))
            .await
            .unwrap();
        let target = submitted.result.as_ref().unwrap()["targetProjectId"]
            .as_str()
            .unwrap()
            .to_owned();
        let failed = fixture.wait(&submitted).await;
        let error = failed.error.unwrap();
        assert_eq!(error.code, "LEGACY_ARCHIVE_REQUIRES_REVIEW");
        assert_eq!(error.details["operationOutcome"], "not_registered");
        assert_eq!(fs::read(&archive).unwrap(), before);
        let candidate = PathBuf::from(error.details["candidatePath"].as_str().unwrap());
        assert!(candidate.join("data/main.sqlite").is_file());
        assert!(candidate.parent().unwrap().join("source.zip").is_file());
        assert!(fixture.store.project(&target).is_err());
        assert!(!fixture.paths.workspace_root().join("backups").exists());
        assert_eq!(
            !error.details["diagnostics"]["foreignKeyViolations"]
                .as_array()
                .unwrap()
                .is_empty(),
            foreign_key
        );
        fixture.stop().await;
    }
}

#[tokio::test]
async fn ready_snapshot_with_pending_build_is_rejected_without_clearing_requirement() {
    let fixture = Fixture::new().await;
    let control = fixture.store.control(&fixture.id).unwrap();
    let mut state = control.get();
    state.pending_build = true;
    state.pending_build_reason = Some("Still waiting for a successful build".into());
    control.set(state.clone()).unwrap();
    let copied = fixture.project("copy", json!({})).await;
    let error = copied.error.unwrap();
    assert_eq!(error.code, "LEGACY_ARCHIVE_REQUIRES_REVIEW");
    assert_eq!(error.details["snapshotState"], "ready");
    assert_eq!(error.details["pendingBuild"]["required"], true);
    assert_eq!(
        error.details["pendingBuild"]["reason"],
        "Still waiting for a successful build"
    );
    assert_eq!(error.details["operationOutcome"], "not_registered");
    assert!(
        PathBuf::from(error.details["candidatePath"].as_str().unwrap())
            .join("data/main.sqlite")
            .is_file()
    );
    assert!(!fixture.paths.workspace_root().join("backups").exists());
    assert_eq!(
        serde_json::to_value(control.get()).unwrap(),
        serde_json::to_value(state).unwrap()
    );
    assert_eq!(fixture.store.list().await.unwrap().len(), 1);
    fixture.stop().await;
}

#[tokio::test]
async fn opt_in_import_reports_cleanup_before_success_and_preserves_original_archive() {
    let fixture = Fixture::new().await;
    let archive = fixture.export_path("disposable-progress.zip");
    good(
        &fixture
            .project("export", json!({"destinationPath":archive}))
            .await,
    );
    let before = sha256(&archive).unwrap();
    let (entered, arrived) = tokio::sync::oneshot::channel();
    let (release, resume) = tokio::sync::oneshot::channel();
    *fixture.lifecycle.import_cleanup_pause.lock().unwrap() = Some((entered, resume));
    let submitted = fixture
        .lifecycle
        .submit_import(json!({"archivePath":archive,"cleanupTemporaryFiles":true}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), arrived)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "Import did not reach cleanup: {:?}",
                fixture.tasks.task(&submitted.task_id)
            )
        })
        .unwrap();
    let active = fixture.tasks.task(&submitted.task_id).unwrap();
    assert_eq!(active.phase, "cleanup");
    assert!(!active.status.is_final());
    let id = submitted.result.as_ref().unwrap()["targetProjectId"]
        .as_str()
        .unwrap();
    assert!(fixture
        .store
        .project(id)
        .unwrap()
        .root
        .join("data/main.sqlite")
        .is_file());
    let temporary = fixture
        .lifecycle
        .operation(&submitted.task_id)
        .join("import-temporary");
    assert!(temporary.join("source.zip").is_file());
    assert!(!temporary.join("candidate").exists());
    release.send(()).unwrap();
    let completed = fixture.wait(&submitted).await;
    assert_eq!(good(&completed)["operationOutcome"], "imported");
    assert_eq!(good(&completed)["cleanupPending"], false);
    assert_eq!(good(&completed)["cleanup"]["completed"], true);
    assert!(!temporary.exists());
    assert!(!fixture
        .lifecycle
        .operation(&submitted.task_id)
        .join("lifecycle.json")
        .exists());
    assert_eq!(sha256(&archive).unwrap(), before);
    let retried = fixture
        .lifecycle
        .cleanup_import(submitted.task_id.clone())
        .await
        .unwrap();
    assert_eq!(retried["completed"], true);
    assert_eq!(retried["projectId"], id);
    assert_eq!(retried["operationOutcome"], "imported");
    fixture.stop().await;
}

#[tokio::test]
async fn import_cleanup_failure_keeps_committed_project_and_retries_only_bound_material() {
    let fixture = Fixture::new().await;
    let archive = fixture.export_path("disposable-cleanup-retry.zip");
    good(
        &fixture
            .project("export", json!({"destinationPath":archive}))
            .await,
    );
    let before = sha256(&archive).unwrap();
    let (entered, arrived) = tokio::sync::oneshot::channel();
    let (release, resume) = tokio::sync::oneshot::channel();
    *fixture.lifecycle.import_cleanup_pause.lock().unwrap() = Some((entered, resume));
    let submitted = fixture
        .lifecycle
        .submit_import(json!({"archivePath":archive,"cleanupTemporaryFiles":true}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), arrived)
        .await
        .unwrap_or_else(|_| {
            panic!(
                "Import did not reach cleanup: {:?}",
                fixture.tasks.task(&submitted.task_id)
            )
        })
        .unwrap();
    let operation = fixture.lifecycle.operation(&submitted.task_id);
    let temporary = operation.join("import-temporary");
    let bound = operation.join("disposable-held-temporary");
    fs::rename(&temporary, &bound).unwrap();
    fs::create_dir(&temporary).unwrap();
    fs::write(temporary.join("unrelated.txt"), "replacement directory").unwrap();
    release.send(()).unwrap();
    let completed = fixture.wait(&submitted).await;
    let result = good(&completed);
    assert_eq!(result["cleanupPending"], true);
    assert_eq!(result["cleanup"]["completed"], false);
    assert_eq!(result["operationOutcome"], "imported");
    let id = result["projectId"].as_str().unwrap().to_owned();
    let installed = fixture.store.project(&id).unwrap().root;
    assert!(installed.join("data/main.sqlite").is_file());
    assert!(temporary.join("unrelated.txt").is_file());
    fs::remove_file(temporary.join("unrelated.txt")).unwrap();
    fs::remove_dir(&temporary).unwrap();
    fs::rename(&bound, &temporary).unwrap();
    for _ in 0..2 {
        let cleaned = fixture
            .lifecycle
            .cleanup_import(submitted.task_id.clone())
            .await
            .unwrap();
        assert_eq!(cleaned["completed"], true);
        assert_eq!(cleaned["operationOutcome"], "imported");
        assert_eq!(cleaned["projectId"], id);
    }
    assert!(!temporary.exists());
    assert!(installed.join("data/main.sqlite").is_file());
    assert_eq!(sha256(&archive).unwrap(), before);
    fixture.stop().await;
}

#[tokio::test]
async fn failed_opt_in_import_cleanup_refuses_unknown_result_then_removes_only_temporary_copy() {
    let fixture = Fixture::new().await;
    let source = fixture.store.project(&fixture.id).unwrap();
    let snapshot = backups::prepare_snapshot(
        &source.root,
        &source.manifest,
        &fixture
            .paths
            .data_root()
            .join("disposable-unknown-snapshot"),
        SnapshotOptions {
            backup_id: format!("backup_{}", Uuid::new_v4().simple()),
            kind: "manual".into(),
            note: None,
            snapshot_state: SnapshotState::Unknown,
            pending_build: PendingBuild::default(),
            build_provenance: None,
        },
    )
    .unwrap();
    let archive = fixture.export_path("disposable-unsupported.zip");
    backups::publish_snapshot(&snapshot, &archive).unwrap();
    let before = sha256(&archive).unwrap();
    let submitted = fixture
        .lifecycle
        .submit_import(json!({"archivePath":archive,"cleanupTemporaryFiles":true}))
        .await
        .unwrap();
    let failed = fixture.wait(&submitted).await;
    assert_eq!(failed.status, TaskStatus::Failed);
    assert_eq!(
        failed.error.as_ref().unwrap().details["operationOutcome"],
        "not_registered"
    );
    let mut journal = fixture
        .lifecycle
        .operations
        .lock()
        .unwrap()
        .get(&submitted.task_id)
        .cloned()
        .unwrap();
    let saved = journal.result.clone();
    let temporary = fixture.lifecycle.import_temporary(&journal);
    assert!(temporary.join("source.zip").is_file());
    assert!(temporary.join("candidate/data/main.sqlite").is_file());
    journal.result["operationOutcome"] = json!("needs_attention");
    fixture.lifecycle.remember(&journal).unwrap();
    let blocked = fixture
        .lifecycle
        .cleanup_import(submitted.task_id.clone())
        .await
        .unwrap_err();
    assert_eq!(blocked.code, "IMPORT_CLEANUP_BLOCKED");
    assert!(temporary.join("candidate/data/main.sqlite").is_file());
    journal.result = saved;
    fixture.lifecycle.remember(&journal).unwrap();
    let cleaned = fixture
        .lifecycle
        .cleanup_import(submitted.task_id.clone())
        .await
        .unwrap();
    assert_eq!(cleaned["completed"], true);
    assert_eq!(cleaned["operationOutcome"], "not_registered");
    assert!(!temporary.exists());
    assert!(source.root.join("data/main.sqlite").is_file());
    assert_eq!(sha256(&archive).unwrap(), before);
    assert!(!fixture
        .lifecycle
        .operation(&submitted.task_id)
        .join("lifecycle.json")
        .exists());
    fixture.stop().await;
}

#[tokio::test]
async fn import_cleanup_does_not_adopt_default_or_legacy_import_materials() {
    let fixture = Fixture::new().await;
    let archive = fixture.export_path("disposable-legacy-import.zip");
    good(
        &fixture
            .project("export", json!({"destinationPath":archive}))
            .await,
    );
    let submitted = fixture
        .lifecycle
        .submit_import(json!({"archivePath":archive}))
        .await
        .unwrap();
    good(&fixture.wait(&submitted).await);
    let retained = fixture
        .lifecycle
        .operation(&submitted.task_id)
        .join("source.zip");
    let before = sha256(&retained).unwrap();
    let blocked = fixture
        .lifecycle
        .cleanup_import(submitted.task_id)
        .await
        .unwrap_err();
    assert_eq!(blocked.code, "IMPORT_CLEANUP_BLOCKED");
    assert_eq!(sha256(&retained).unwrap(), before);
    fixture.stop().await;
}

#[tokio::test]
async fn completed_operations_do_not_write_lifecycle_or_cleanup_history() {
    let fixture = Fixture::new().await;
    let archive = fixture.export_path("disposable-no-history.zip");
    let exported = fixture
        .project("export", json!({"destinationPath":archive}))
        .await;
    good(&exported);
    let imported = fixture
        .lifecycle
        .submit_import(json!({"archivePath":archive,"cleanupTemporaryFiles":true}))
        .await
        .unwrap();
    good(&fixture.wait(&imported).await);
    let operation = fixture.lifecycle.operation(&imported.task_id);
    assert!(!operation.join("lifecycle.json").exists());
    assert!(!operation.join("import-cleanup.json").exists());
    assert!(!operation.exists());
    assert!(fixture
        .lifecycle
        .operations
        .lock()
        .unwrap()
        .values()
        .all(|op| op.cleanup_temporary_files));
    let legacy = fixture
        .paths
        .workspace_root()
        .join(".platform/operations/task_legacy/lifecycle.json");
    fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    fs::write(&legacy, b"opaque retired operation history").unwrap();
    fixture.lifecycle.shutdown().await;
    let restarted = LifecycleManager::new(
        fixture.paths.clone(),
        fixture.store.clone(),
        fixture.tasks.clone(),
        MaintenanceManager::new(fixture.store.clone(), fixture.tasks.clone()),
    )
    .unwrap();
    assert!(restarted.operations.lock().unwrap().is_empty());
    assert_eq!(
        fs::read(&legacy).unwrap(),
        b"opaque retired operation history"
    );
    let error = restarted
        .cleanup_import(imported.task_id.clone())
        .await
        .unwrap_err();
    assert_eq!(error.code, "IMPORT_CLEANUP_BLOCKED");
    assert!(archive.is_file());
    restarted.shutdown().await;
    fixture.stop().await;
}

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

#[path = "direct_delete_tests.rs"]
mod direct_delete;
