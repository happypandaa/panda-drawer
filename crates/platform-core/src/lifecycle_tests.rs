//! Disposable lifecycle evidence; all workspaces and exports belong to TempDir.
use super::*;
use crate::{CreateProjectInput, TaskStatus};

struct Fixture {
    _temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    maintenance: Arc<MaintenanceManager>,
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
        let maintenance =
            MaintenanceManager::new(paths.clone(), store.clone(), tasks.clone()).unwrap();
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
            maintenance,
            lifecycle,
            id: project.id,
        }
    }
    fn export_path(&self, name: &str) -> PathBuf {
        self.paths.data_root().join(name)
    }
    async fn wait(&self, task: &TaskRecord) -> TaskRecord {
        for _ in 0..2000 {
            let record = self.tasks.task(&task.task_id).unwrap();
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
            input["expectedDocumentEpoch"] =
                json!(self.store.registered_epoch(&self.id).unwrap().unwrap());
        }
        let task = self
            .lifecycle
            .submit_project(&self.id, kind, input)
            .await
            .unwrap();
        self.wait(&task).await
    }
    async fn legacy_delete(&self, input: Value) -> TaskRecord {
        let task = self
            .lifecycle
            .submit_legacy_trash_for_test(&self.id, input)
            .await
            .unwrap();
        self.wait(&task).await
    }
    async fn stop(self) {
        self.lifecycle.shutdown().await;
        self.maintenance.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
    async fn restart_task(self, mut task: TaskRecord) -> Self {
        self.lifecycle.shutdown().await;
        self.maintenance.shutdown().await;
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
        let maintenance =
            MaintenanceManager::new(self.paths.clone(), store.clone(), tasks.clone()).unwrap();
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
            maintenance,
            lifecycle,
            id: self.id,
        }
    }
}
fn good(task: &TaskRecord) -> &Value {
    assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
    task.result.as_ref().unwrap()
}

#[path = "direct_delete_tests.rs"]
mod direct_delete;

#[cfg(target_os = "macos")]
#[tokio::test]
async fn trash_stable_identity_accepts_renumbering_and_legacy_device_changes_fail_closed() {
    for legacy in [false, true] {
        let fixture = Fixture::new().await;
        let deleted = fixture.legacy_delete(json!({})).await;
        let trash = good(&deleted)["trashId"].as_str().unwrap().to_owned();
        let directory = fixture.lifecycle.trash_root(&trash).unwrap();
        let root = directory.join("project");
        let attachment = fs::read(root.join("data/attachments/original.txt")).unwrap();
        let mut record = fixture.lifecycle.read_trash(&trash).unwrap();
        assert!(!record.root_identity.is_legacy());
        if legacy {
            record.root_identity.volume_uuid = None;
        }
        record.root_identity.device = record.root_identity.device.wrapping_add(1);
        fixture.lifecycle.save_trash(&record).unwrap();
        let request = fixture
            .lifecycle
            .submit_trash(&trash, "restore")
            .await
            .unwrap();
        let restored = fixture.wait(&request).await;
        if legacy {
            assert_eq!(restored.status, TaskStatus::Failed, "{restored:?}");
            assert_eq!(restored.error.unwrap().code, "TRASH_IDENTITY_CHANGED");
            assert_eq!(
                fs::read(root.join("data/attachments/original.txt")).unwrap(),
                attachment
            );
            assert!(!fixture.store.project_root(&fixture.id).unwrap().exists());
        } else {
            good(&restored);
            assert_eq!(
                fs::read(
                    fixture
                        .store
                        .project(&fixture.id)
                        .unwrap()
                        .root
                        .join("data/attachments/original.txt")
                )
                .unwrap(),
                attachment,
            );
        }
        fixture.stop().await;
    }
}

#[tokio::test]
async fn legacy_tuple_trash_record_remains_restorable_on_its_verified_device() {
    let fixture = Fixture::new().await;
    let deleted = fixture.legacy_delete(json!({})).await;
    let trash = good(&deleted)["trashId"].as_str().unwrap().to_owned();
    let directory = fixture.lifecycle.trash_root(&trash).unwrap();
    let mut record: Value = read_json(&directory.join("record.json")).unwrap();
    record["rootIdentity"] = json!(identity(&directory.join("project")).unwrap());
    builds::atomic_json(&directory.join("record.json"), &record).unwrap();
    let request = fixture
        .lifecycle
        .submit_trash(&trash, "restore")
        .await
        .unwrap();
    good(&fixture.wait(&request).await);
    fixture.stop().await;
}

async fn migrate_fixture(fixture: Fixture) -> (Fixture, PathBuf, String) {
    fixture.lifecycle.shutdown().await;
    fixture.maintenance.shutdown().await;
    fixture.tasks.shutdown().await;
    let source = fixture.paths.workspace_root().to_path_buf();
    let destination = fixture
        ._temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("migrated-workspace");
    fs::create_dir(&destination).unwrap();
    fixture.store.begin_workspace_migration().unwrap();
    let task = crate::workspace::create(&fixture.paths, &destination).unwrap();
    fixture.store.pause_workspace().await;
    let (paths, mut journal) =
        crate::workspace::copy_and_commit(&fixture.paths, &fixture.store, &task.task_id).unwrap();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    crate::workspace::initialize_selected(&paths, &store, &mut journal).unwrap();
    crate::workspace::mark_old_retired(&paths, &journal).unwrap();
    assert_eq!(
        paths.reinitialize_for_test().unwrap().workspace_root(),
        destination
    );
    fixture.store.shutdown().await;
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
    let maintenance = MaintenanceManager::new(paths.clone(), store.clone(), tasks.clone()).unwrap();
    let lifecycle = LifecycleManager::new(
        paths.clone(),
        store.clone(),
        tasks.clone(),
        maintenance.clone(),
    )
    .unwrap();
    (
        Fixture {
            _temporary: fixture._temporary,
            paths,
            store,
            tasks,
            maintenance,
            lifecycle,
            id: fixture.id,
        },
        source,
        task.task_id,
    )
}

#[tokio::test]
async fn migration_rebinds_verified_trash_and_real_restore_keeps_source_unchanged() {
    let fixture = Fixture::new().await;
    let old_epoch = fixture.store.project(&fixture.id).unwrap().document_epoch;
    let deleted = fixture.legacy_delete(json!({})).await;
    let trash = good(&deleted)["trashId"].as_str().unwrap().to_owned();
    let trash_root = fixture.lifecycle.trash_root(&trash).unwrap();
    let record_bytes = fs::read(trash_root.join("record.json")).unwrap();
    let old_record: Value = serde_json::from_slice(&record_bytes).unwrap();
    let database = sha256(&trash_root.join("project/data/main.sqlite")).unwrap();
    let links = trash_root.join("project/app/node_modules/.bin");
    fs::create_dir_all(&links).unwrap();
    std::os::unix::fs::symlink("../package/command", links.join("command")).unwrap();
    let (fixture, source, migration) = migrate_fixture(fixture).await;
    let copied_root = fixture.lifecycle.trash_root(&trash).unwrap();
    let copied_record: Value = read_json(&copied_root.join("record.json")).unwrap();
    let new_identity = persistent_identity(&copied_root.join("project")).unwrap();
    assert_ne!(old_record["rootIdentity"], json!(new_identity));
    let mut expected = old_record.clone();
    expected["rootIdentity"] = json!(new_identity);
    assert_eq!(copied_record, expected);
    assert_eq!(
        fs::read(
            source
                .join(".platform/trash")
                .join(&trash)
                .join("record.json")
        )
        .unwrap(),
        record_bytes
    );
    let evidence: Value = read_json(
        &fixture
            .paths
            .migrations_dir()
            .join(migration)
            .join("trash-rebindings.json"),
    )
    .unwrap();
    assert!(evidence["entries"].as_array().unwrap().iter().any(|entry| {
        entry["trashId"] == trash && entry["targetIdentity"] == json!(new_identity)
    }));
    let request = fixture
        .lifecycle
        .submit_trash(&trash, "restore")
        .await
        .unwrap();
    let restored = fixture.wait(&request).await;
    assert_eq!(good(&restored)["operationOutcome"], "restored");
    let root = fixture.store.project(&fixture.id).unwrap();
    assert_ne!(root.document_epoch, old_epoch);
    assert_eq!(
        sha256(&root.root.join("data/main.sqlite")).unwrap(),
        database
    );
    assert_eq!(
        fs::read_link(root.root.join("app/node_modules/.bin/command")).unwrap(),
        PathBuf::from("../package/command")
    );
    assert_eq!(
        sha256(
            &source
                .join(".platform/trash")
                .join(&trash)
                .join("project/data/main.sqlite")
        )
        .unwrap(),
        database
    );
    fixture.stop().await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn migration_rebinds_partial_purge_residue_and_only_explicit_retry_deletes_copy() {
    let fixture = Fixture::new().await;
    let deleted = fixture.legacy_delete(json!({})).await;
    let trash = good(&deleted)["trashId"].as_str().unwrap().to_owned();
    let trash_root = fixture.lifecycle.trash_root(&trash).unwrap();
    let database_path = trash_root.join("project/data/main.sqlite");
    let database = sha256(&database_path).unwrap();
    let guard = ImmutableFile::new(&database_path);
    let request = fixture
        .lifecycle
        .submit_trash(&trash, "purge")
        .await
        .unwrap();
    let failed = fixture.wait(&request).await;
    assert_eq!(failed.status, TaskStatus::Failed);
    assert_eq!(
        failed.error.unwrap().details["operationOutcome"],
        "purge_incomplete"
    );
    drop(guard);
    let record_bytes = fs::read(trash_root.join("record.json")).unwrap();
    let (fixture, source, _) = migrate_fixture(fixture).await;
    let copied_root = fixture
        .lifecycle
        .trash_root(&trash)
        .unwrap()
        .join("project");
    assert_eq!(
        sha256(&copied_root.join("data/main.sqlite")).unwrap(),
        database
    );
    let info = fixture.lifecycle.trash_info(&trash).await.unwrap();
    assert_eq!(info["state"], "purgeIncomplete");
    assert_eq!(info["restoreAvailable"], false);
    let request = fixture
        .lifecycle
        .submit_trash(&trash, "purge")
        .await
        .unwrap();
    assert_eq!(
        good(&fixture.wait(&request).await)["operationOutcome"],
        "purged"
    );
    assert!(!copied_root.exists());
    let retained = source.join(".platform/trash").join(&trash);
    assert_eq!(
        fs::read(retained.join("record.json")).unwrap(),
        record_bytes
    );
    assert_eq!(
        sha256(&retained.join("project/data/main.sqlite")).unwrap(),
        database
    );
    fixture.stop().await;
}

#[tokio::test]
async fn migration_preserves_unverified_trash_identity_and_malformed_metadata() {
    let fixture = Fixture::new().await;
    let deleted = fixture.legacy_delete(json!({})).await;
    let trash = good(&deleted)["trashId"].as_str().unwrap().to_owned();
    let record_path = fixture
        .lifecycle
        .trash_root(&trash)
        .unwrap()
        .join("record.json");
    let mut record: Value = read_json(&record_path).unwrap();
    record["rootIdentity"] = json!([0, 0]);
    builds::atomic_json(&record_path, &record).unwrap();
    let original_bytes = fs::read(&record_path).unwrap();
    let malformed = fixture
        .paths
        .workspace_root()
        .join(".platform/trash/trash_invalid");
    fs::create_dir_all(malformed.join("project")).unwrap();
    fs::write(malformed.join("record.json"), b"preserve invalid metadata").unwrap();
    let (fixture, source, migration) = migrate_fixture(fixture).await;
    let copied = fixture.lifecycle.trash_root(&trash).unwrap();
    assert_eq!(
        fs::read(copied.join("record.json")).unwrap(),
        original_bytes
    );
    assert_eq!(
        fs::read(
            fixture
                .paths
                .workspace_root()
                .join(".platform/trash/trash_invalid/record.json")
        )
        .unwrap(),
        b"preserve invalid metadata"
    );
    let evidence: Value = read_json(
        &fixture
            .paths
            .migrations_dir()
            .join(migration)
            .join("trash-rebindings.json"),
    )
    .unwrap();
    assert!(evidence["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["trashId"] == trash && entry["reason"] == "source_identity_unverified"));
    assert!(
        evidence["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["trashId"] == "trash_invalid"
                && entry["reason"] == "unreadable_record")
    );
    // An unreadable record could claim any project ID, so registration correctly
    // refuses it. Quarantine only this disposable target fixture to independently
    // exercise the other entry's preexisting identity rejection.
    assert_eq!(
        fixture
            .lifecycle
            .submit_trash(&trash, "restore")
            .await
            .unwrap_err()
            .code,
        "INVALID_JSON"
    );
    fs::rename(
        fixture
            .paths
            .workspace_root()
            .join(".platform/trash/trash_invalid"),
        fixture.paths.data_root().join("retained-malformed-record"),
    )
    .unwrap();
    let request = fixture
        .lifecycle
        .submit_trash(&trash, "restore")
        .await
        .unwrap();
    let task = fixture.wait(&request).await;
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.error.unwrap().code, "TRASH_IDENTITY_CHANGED");
    assert!(copied.join("project/data/main.sqlite").exists());
    assert_eq!(
        fs::read(
            source
                .join(".platform/trash")
                .join(&trash)
                .join("record.json")
        )
        .unwrap(),
        original_bytes
    );
    fixture.stop().await;
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
    assert_eq!(conflict.code, "PROJECT_ID_CONFLICT");
    let submitted = fixture
        .lifecycle
        .submit_import(json!({"archivePath":archive,"mode":"copy","name":"Imported copy"}))
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
    good(
        &fixture
            .project(
                "export",
                json!({"destinationPath":archive,"overwrite":true}),
            )
            .await,
    );
    assert_eq!(
        backups::read_info(&archive).unwrap().manifest.project_id,
        fixture.id
    );
    fixture.stop().await;
}

#[tokio::test]
async fn delete_restore_and_purge_preserve_epochs_backups_and_corrupt_database_bytes() {
    let fixture = Fixture::new().await;
    let original = fixture.store.project(&fixture.id).unwrap();
    let epoch = original.document_epoch.clone();
    let archive = fixture.export_path("healthy.zip");
    good(
        &fixture
            .project("export", json!({"destinationPath":archive}))
            .await,
    );
    let backup_dir = fixture
        .paths
        .workspace_root()
        .join("backups")
        .join(&fixture.id);
    fs::create_dir_all(&backup_dir).unwrap();
    let history = backup_dir.join("backup_history.zip");
    fs::copy(&archive, &history).unwrap();
    let digest = sha256(&history).unwrap();
    let broken = b"intentionally corrupt sqlite bytes retained by lifecycle";
    fs::write(original.root.join("data/main.sqlite"), broken).unwrap();
    fs::write(
        original.root.join("data/main.sqlite-wal"),
        b"retained external WAL bytes",
    )
    .unwrap();
    fs::write(
        original.root.join("data/main.sqlite-shm"),
        b"retained external SHM bytes",
    )
    .unwrap();
    let deleted = fixture.legacy_delete(json!({})).await;
    let trash = good(&deleted)["trashId"].as_str().unwrap().to_owned();
    assert_eq!(
        fixture.store.project(&fixture.id).err().unwrap().code,
        "PROJECT_TRASHED"
    );
    assert_eq!(
        fixture
            .lifecycle
            .submit_import(json!({"archivePath":archive,"mode":"original"}))
            .await
            .unwrap_err()
            .code,
        "PROJECT_ID_CONFLICT"
    );
    let retained = fixture
        .lifecycle
        .trash_root(&trash)
        .unwrap()
        .join("project");
    assert_eq!(fs::read(retained.join("data/main.sqlite")).unwrap(), broken);
    assert!(retained.join("data/main.sqlite-wal").is_file());
    let request = fixture
        .lifecycle
        .submit_trash(&trash, "restore")
        .await
        .unwrap();
    let restored = fixture.wait(&request).await;
    assert_eq!(good(&restored)["mode"], "maintenance");
    assert_eq!(good(&restored)["readyForChanges"], false);
    let restored = fixture.store.project(&fixture.id).unwrap();
    assert_ne!(restored.document_epoch, epoch);
    assert_eq!(
        fs::read(restored.root.join("data/main.sqlite")).unwrap(),
        broken
    );
    let state = fixture.store.control(&fixture.id).unwrap().get();
    assert!(fixture
        .store
        .control(&fixture.id)
        .unwrap()
        .check(false, state.maintenance_id.as_deref(), false)
        .is_err());
    assert!(original.check().is_err());
    let deleted = fixture
        .legacy_delete(json!({"maintenanceId":state.maintenance_id}))
        .await;
    let trash = good(&deleted)["trashId"].as_str().unwrap();
    let request = fixture
        .lifecycle
        .submit_trash(trash, "purge")
        .await
        .unwrap();
    let purged = fixture.wait(&request).await;
    assert_eq!(good(&purged)["operationOutcome"], "purged");
    assert!(!fixture
        .lifecycle
        .trash_root(trash)
        .unwrap()
        .join("project")
        .exists());
    assert_eq!(sha256(&history).unwrap(), digest);
    assert_eq!(
        fixture.lifecycle.trash_info(trash).await.unwrap()["restoreAvailable"],
        false
    );
    let request = fixture
        .lifecycle
        .submit_import(json!({"archivePath":archive,"mode":"original"}))
        .await
        .unwrap();
    let imported = fixture.wait(&request).await;
    assert_eq!(good(&imported)["mode"], "ready");
    assert_ne!(
        fixture.store.project(&fixture.id).unwrap().document_epoch,
        epoch
    );
    assert!(original.check().is_err());
    fixture.stop().await;
}

#[tokio::test]
async fn maintenance_copy_establishes_new_identity_baseline_without_clearing_pending_build() {
    let fixture = Fixture::new().await;
    let project = fixture.store.project(&fixture.id).unwrap();
    let control = fixture.store.control(&fixture.id).unwrap();
    let mut state = control
        .begin(
            "Unfinished source work".into(),
            Some(builds::input_digest(&project).unwrap()),
            false,
        )
        .unwrap();
    state.pending_build = true;
    state.pending_build_reason = Some("Known source work".into());
    let maintenance_id = state.maintenance_id.clone().unwrap();
    control.set(state).unwrap();
    let copied = fixture
        .project("copy", json!({"maintenanceId":maintenance_id}))
        .await;
    let result = good(&copied);
    assert_eq!(result["mode"], "maintenance");
    assert_eq!(result["readyForChanges"], true);
    assert_eq!(result["pendingBuild"]["required"], true);
    assert_ne!(result["maintenanceId"], maintenance_id);
    let id = result["projectId"].as_str().unwrap();
    let baseline = result["baselineBackupId"].as_str().unwrap();
    let archive = fixture
        .paths
        .workspace_root()
        .join("backups")
        .join(id)
        .join(format!("{baseline}.zip"));
    assert_eq!(
        backups::read_info(&archive).unwrap().manifest.project_id,
        id
    );
    let candidate = fixture.paths.data_root().join("baseline-restore-candidate");
    backups::extract_validated(&archive, &candidate, Some(id)).unwrap();
    assert!(control.get().pending_build);
    assert_eq!(
        control.get().maintenance_id.as_deref(),
        Some(maintenance_id.as_str())
    );
    fixture.stop().await;
}

#[tokio::test]
async fn installed_copy_restart_recovers_response_without_replaying_registration_or_control() {
    for resolved in [false, true] {
        let fixture = Fixture::new().await;
        let task = fixture.project("copy", json!({})).await;
        let id = good(&task)["projectId"].as_str().unwrap().to_owned();
        let project = fixture.store.project(&id).unwrap();
        let epoch = project.document_epoch.clone();
        let directory_identity = identity(&project.root).unwrap();
        let control = fixture.store.control(&id).unwrap();
        let mut later = control.get();
        later.pending_build = true;
        later.pending_build_reason =
            Some("Later legitimate changes must survive historical recovery".into());
        control.set(later.clone()).unwrap();
        let path = fixture.lifecycle.journal_path(&task.task_id);
        let mut journal: Journal = read_json(&path).unwrap();
        journal.resolved = resolved;
        journal.committed = resolved;
        builds::atomic_json(&path, &journal).unwrap();
        let task_id = task.task_id.clone();
        let restarted = fixture.restart_task(task).await;
        let recovered = restarted.tasks.task(&task_id).unwrap();
        assert_eq!(recovered.status, TaskStatus::Interrupted);
        assert_eq!(
            recovered.result.as_ref().unwrap()["operationOutcome"],
            "copied"
        );
        assert_eq!(recovered.result.as_ref().unwrap()["recovered"], true);
        let current = restarted.store.project(&id).unwrap();
        assert_eq!(current.document_epoch, epoch);
        assert_eq!(identity(&current.root).unwrap(), directory_identity);
        assert_eq!(
            serde_json::to_value(restarted.store.control(&id).unwrap().get()).unwrap(),
            json!(later)
        );
        restarted.stop().await;
    }
}

#[tokio::test]
async fn delete_and_restore_restart_reconcile_completed_moves_with_lost_task_response() {
    let fixture = Fixture::new().await;
    let old = fixture.store.project(&fixture.id).unwrap();
    let deleted = fixture.legacy_delete(json!({})).await;
    let trash = good(&deleted)["trashId"].as_str().unwrap().to_owned();
    let path = fixture.lifecycle.journal_path(&deleted.task_id);
    let mut journal: Journal = read_json(&path).unwrap();
    journal.committed = false;
    journal.resolved = false;
    builds::atomic_json(&path, &journal).unwrap();
    let task_id = deleted.task_id.clone();
    let fixture = fixture.restart_task(deleted).await;
    let task = fixture.tasks.task(&task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Interrupted);
    assert_eq!(task.result.unwrap()["operationOutcome"], "trashed");
    assert!(fixture.store.project(&fixture.id).is_err());
    let submitted = fixture
        .lifecycle
        .submit_trash(&trash, "restore")
        .await
        .unwrap();
    let restored = fixture.wait(&submitted).await;
    let epoch = good(&restored)["documentEpoch"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(epoch, old.document_epoch);
    let path = fixture.lifecycle.journal_path(&restored.task_id);
    let mut journal: Journal = read_json(&path).unwrap();
    journal.committed = false;
    journal.resolved = false;
    builds::atomic_json(&path, &journal).unwrap();
    let task_id = restored.task_id.clone();
    let fixture = fixture.restart_task(restored).await;
    let task = fixture.tasks.task(&task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Interrupted);
    assert_eq!(task.result.unwrap()["operationOutcome"], "restored");
    assert_eq!(
        fixture.store.project(&fixture.id).unwrap().document_epoch,
        epoch
    );
    assert_eq!(fixture.lifecycle.trash_list().await.unwrap(), json!([]));
    assert!(old.check().is_err());
    fixture.stop().await;
}

#[tokio::test]
async fn install_recovery_preserves_both_copies_when_registration_redirects_the_journal() {
    let fixture = Fixture::new().await;
    let archive = fixture.export_path("same-id-recovery-material.zip");
    good(
        &fixture
            .project("export", json!({"destinationPath":archive}))
            .await,
    );
    let copied = fixture
        .project("copy", json!({"name":"Authorized recovery target"}))
        .await;
    let id = good(&copied)["projectId"].as_str().unwrap().to_owned();
    let root = fixture.store.project(&id).unwrap().root;
    let alternate = fixture
        .paths
        .workspace_root()
        .join("projects/unreviewed-same-id-copy");
    backups::extract_validated(&archive, &alternate, Some(&fixture.id)).unwrap();
    let mut manifest: ProjectManifest = read_json(&alternate.join("project.json")).unwrap();
    manifest.id = id.clone();
    builds::atomic_json(&alternate.join("project.json"), &manifest).unwrap();
    let identity_before = identity(&root).unwrap();
    let alternate_before = identity(&alternate).unwrap();
    let registration = fixture
        .paths
        .workspace_root()
        .join(".platform/project-state")
        .join(format!("{id}.json"));
    let mut state: Value = read_json(&registration).unwrap();
    state["directoryName"] = json!("unreviewed-same-id-copy");
    state["projectRoot"] = json!(alternate);
    state["documentEpoch"] = json!(Uuid::new_v4().to_string());
    builds::atomic_json(&registration, &state).unwrap();
    let reservation = fixture
        .paths
        .workspace_root()
        .join(".platform/project-state/reservations")
        .join(format!("{id}.json"));
    builds::atomic_json(
        &reservation,
        &json!({"projectId":id,"taskId":copied.task_id,"directoryName":"unreviewed-same-id-copy"}),
    )
    .unwrap();
    let registration_before = fs::read(&registration).unwrap();
    let mut journal: Journal = read_json(&fixture.lifecycle.journal_path(&copied.task_id)).unwrap();
    journal.resolved = false;
    journal.committed = false;
    fixture.lifecycle.save(&journal).unwrap();
    let fixture = fixture.restart_task(copied.clone()).await;
    let recovered = fixture.tasks.task(&copied.task_id).unwrap();
    assert_eq!(
        recovered.result.as_ref().unwrap()["operationOutcome"],
        "needs_attention"
    );
    assert_eq!(identity(&root).unwrap(), identity_before);
    assert_eq!(identity(&alternate).unwrap(), alternate_before);
    assert_eq!(fs::read(&registration).unwrap(), registration_before);
    assert!(reservation.is_file());
    assert!(fixture.store.control(&id).unwrap().get().recovery_required);
    for project in [&root, &alternate] {
        assert_eq!(
            fs::read(project.join("data/attachments/original.txt")).unwrap(),
            b"independent attachment"
        );
    }
    fixture.stop().await;
}

#[tokio::test]
async fn readable_copy_recovery_uses_saved_destination_without_a_candidate_or_registration() {
    for legacy in [false, true] {
        let fixture = Fixture::new().await;
        let copied = fixture
            .project("copy", json!({"name":"恢复目录测试"}))
            .await;
        let id = good(&copied)["projectId"].as_str().unwrap().to_owned();
        let mut root = fixture.store.project(&id).unwrap().root;
        assert_ne!(root.file_name().unwrap().to_str().unwrap(), id);
        let journal_path = fixture.lifecycle.journal_path(&copied.task_id);
        let mut journal: Journal = read_json(&journal_path).unwrap();
        assert_eq!(
            journal.directory_name.as_deref(),
            root.file_name().and_then(|name| name.to_str())
        );
        assert!(!fixture
            .lifecycle
            .operation(&copied.task_id)
            .join("candidate")
            .exists());
        let registration = fixture
            .paths
            .workspace_root()
            .join(".platform/project-state")
            .join(format!("{id}.json"));
        if legacy {
            // Represent an actual pre-naming project and journal. No existing
            // development project or historical record is modified by this fixture.
            let old_root = fixture.paths.workspace_root().join("projects").join(&id);
            fs::rename(&root, &old_root).unwrap();
            root = old_root;
            let mut state: Value = read_json(&registration).unwrap();
            state["projectRoot"] = json!(root);
            state.as_object_mut().unwrap().remove("directoryName");
            builds::atomic_json(&registration, &state).unwrap();
            journal.directory_name = None;
        } else {
            let handoff = fs::read_to_string(root.join(".platform/handoff.md")).unwrap();
            assert!(handoff.contains(&format!("Project root: `{}`", root.display())));
            // A host can stop after the candidate rename but before the final
            // registration. The reservation and journal remain authoritative.
            fs::remove_file(&registration).unwrap();
            let reservation = fixture
                .paths
                .workspace_root()
                .join(".platform/project-state/reservations")
                .join(format!("{id}.json"));
            builds::atomic_json(&reservation, &json!({"projectId":id,"taskId":copied.task_id,"directoryName":journal.directory_name})).unwrap();
        }
        journal.committed = false;
        journal.resolved = false;
        builds::atomic_json(&journal_path, &journal).unwrap();
        let fixture = fixture.restart_task(copied.clone()).await;
        let recovered = fixture.tasks.task(&copied.task_id).unwrap();
        assert_eq!(
            recovered.result.as_ref().unwrap()["operationOutcome"],
            "copied"
        );
        let current = fixture.store.project(&id).unwrap();
        assert_eq!(current.root, root);
        assert_eq!(current.document_epoch, journal.target_epoch);
        let db = rusqlite::Connection::open(current.root.join("data/main.sqlite")).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT value FROM evidence WHERE id='business-uuid'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "original"
        );
        drop(db);
        assert_eq!(
            fs::read(current.root.join("data/attachments/original.txt")).unwrap(),
            b"independent attachment"
        );
        assert!(fixture.store.project(&fixture.id).unwrap().root.exists());
        fixture.stop().await;
    }
}

#[test]
fn lifecycle_journal_directory_binding_rejects_unsafe_names_and_keeps_legacy_fallback() {
    let temporary = tempfile::tempdir().unwrap();
    let workspace = temporary.path().canonicalize().unwrap();
    fs::create_dir(workspace.join("projects")).unwrap();
    let id = "prj_0123456789abcdef0123456789abcdef";
    assert_eq!(
        journal_project_root(&workspace, id, None).unwrap(),
        workspace.join("projects").join(id)
    );
    assert_eq!(
        journal_project_root(&workspace, id, Some("ji-lu-01234567")).unwrap(),
        workspace.join("projects/ji-lu-01234567")
    );
    for name in [
        "",
        ".",
        "..",
        "../outside",
        "/outside",
        "a/b",
        "a\\b",
        "a\nb",
        "Upper",
        "名称",
    ] {
        assert!(
            journal_project_root(&workspace, id, Some(name)).is_err(),
            "{name:?}"
        );
    }
    assert!(journal_project_root(&workspace, id, Some(&"a".repeat(129))).is_err());
}

#[tokio::test]
async fn pending_import_reservation_is_released_when_no_directory_was_committed() {
    let fixture = Fixture::new().await;
    let target = format!("prj_{}", Uuid::new_v4().simple());
    let (task, lease) = fixture.tasks.create_operation(&target, "import").unwrap();
    let journal = LifecycleManager::journal(&task, "import", &target);
    fixture.lifecycle.save(&journal).unwrap();
    fixture
        .store
        .reserve_identity(&target, &task.task_id, None)
        .unwrap();
    let candidate = fixture.lifecycle.operation(&task.task_id).join("candidate");
    fs::create_dir_all(&candidate).unwrap();
    fs::write(
        candidate.join("retained.txt"),
        b"unvalidated temporary content",
    )
    .unwrap();
    drop(lease);
    let task_id = task.task_id.clone();
    let fixture = fixture.restart_task(task).await;
    assert_eq!(
        fixture.tasks.task(&task_id).unwrap().result.unwrap()["operationOutcome"],
        "not_registered"
    );
    fixture
        .store
        .check_identity_available(&target, None, None)
        .unwrap();
    assert!(!fixture
        .paths
        .workspace_root()
        .join("projects")
        .join(&target)
        .exists());
    assert_eq!(
        fs::read(candidate.join("retained.txt")).unwrap(),
        b"unvalidated temporary content"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn interrupted_purge_keeps_retryable_residual_record_and_never_replays_deletion() {
    let fixture = Fixture::new().await;
    let deleted = fixture.legacy_delete(json!({})).await;
    let trash = good(&deleted)["trashId"].as_str().unwrap().to_owned();
    let root = fixture
        .lifecycle
        .trash_root(&trash)
        .unwrap()
        .join("project");
    let (task, lease) = fixture
        .tasks
        .create_operation(&fixture.id, "trashPurge")
        .unwrap();
    let mut journal = LifecycleManager::journal(&task, "trashPurge", &fixture.id);
    journal.trash_id = Some(trash.clone());
    journal.directory_identity = Some(persistent_identity(&root).unwrap());
    journal.phase = "purge-intent".into();
    fixture.lifecycle.save(&journal).unwrap();
    let mut record = fixture.lifecycle.read_trash(&trash).unwrap();
    record.state = "purging".into();
    record.last_task_id = task.task_id.clone();
    fixture.lifecycle.save_trash(&record).unwrap();
    fs::remove_file(root.join("data/attachments/original.txt")).unwrap();
    drop(lease);
    let task_id = task.task_id.clone();
    let fixture = fixture.restart_task(task).await;
    assert_eq!(
        fixture.tasks.task(&task_id).unwrap().result.unwrap()["operationOutcome"],
        "purge_incomplete"
    );
    assert!(
        root.join("data/main.sqlite").is_file(),
        "restart must not resume permanent deletion"
    );
    assert_eq!(
        fixture.lifecycle.trash_info(&trash).await.unwrap()["state"],
        "purgeIncomplete"
    );
    assert_eq!(
        fixture
            .lifecycle
            .submit_trash(&trash, "restore")
            .await
            .unwrap_err()
            .code,
        "TRASH_INCOMPLETE"
    );
    let request = fixture
        .lifecycle
        .submit_trash(&trash, "purge")
        .await
        .unwrap();
    assert_eq!(
        good(&fixture.wait(&request).await)["operationOutcome"],
        "purged"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn unrelated_unresolved_recovery_reference_blocks_permanent_purge() {
    let fixture = Fixture::new().await;
    let deleted = fixture.legacy_delete(json!({})).await;
    let trash = good(&deleted)["trashId"].as_str().unwrap().to_owned();
    let other = format!("task_{}", Uuid::new_v4().simple());
    builds::atomic_json(
        &fixture.lifecycle.operation(&other).join("journal.json"),
        &json!({"taskId":other,"projectId":fixture.id,"resolved":false}),
    )
    .unwrap();
    let error = fixture
        .lifecycle
        .submit_trash(&trash, "purge")
        .await
        .unwrap_err();
    assert_eq!(error.code, "TRASH_PROTECTED");
    assert!(fixture
        .lifecycle
        .trash_root(&trash)
        .unwrap()
        .join("project/data/main.sqlite")
        .is_file());
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
async fn unknown_and_foreign_key_archives_import_into_new_local_maintenance() {
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
        let task = fixture
            .lifecycle
            .submit_import(json!({"archivePath":archive,"mode":"copy"}))
            .await
            .unwrap();
        let task = fixture.wait(&task).await;
        let result = good(&task);
        assert_eq!(result["mode"], "maintenance");
        assert_eq!(result["readyForChanges"], true);
        assert!(result["maintenanceId"]
            .as_str()
            .unwrap()
            .starts_with("maintenance_"));
        assert_eq!(
            !result["diagnostics"]["foreignKeyViolations"]
                .as_array()
                .unwrap()
                .is_empty(),
            foreign_key
        );
        fixture.stop().await;
    }
}

#[tokio::test]
async fn baseline_publication_failure_registers_blocked_maintenance_and_can_retry() {
    let fixture = Fixture::new().await;
    let control = fixture.store.control(&fixture.id).unwrap();
    let mut state = control
        .begin("Unfinished maintenance".into(), None, false)
        .unwrap();
    state.pending_build = true;
    let maintenance_id = state.maintenance_id.clone().unwrap();
    control.set(state).unwrap();
    let submitted = fixture
        .lifecycle
        .submit_project(&fixture.id, "copy", json!({"maintenanceId":maintenance_id}))
        .await
        .unwrap();
    let id = submitted.result.as_ref().unwrap()["targetProjectId"]
        .as_str()
        .unwrap()
        .to_owned();
    let obstruction = fixture.paths.workspace_root().join("backups").join(&id);
    fs::create_dir_all(obstruction.parent().unwrap()).unwrap();
    fs::write(&obstruction, b"owned test file blocks baseline directory").unwrap();
    let task = fixture.wait(&submitted).await;
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(
        task.error.unwrap().details["operationOutcome"],
        "registered_maintenance_not_ready"
    );
    assert!(fixture.store.project(&id).is_ok());
    let state = fixture.store.control(&id).unwrap().get();
    assert_eq!(state.mode, "maintenance");
    assert!(!state.ready_for_changes);
    assert!(state.pending_build);
    assert!(fixture
        .store
        .control(&id)
        .unwrap()
        .check(false, state.maintenance_id.as_deref(), false)
        .is_err());
    fs::remove_file(obstruction).unwrap();
    let request = fixture
        .maintenance
        .submit(
            &id,
            "maintenanceBegin",
            crate::maintenance::OperationInput {
                maintenance_id: state.maintenance_id.clone(),
                reason: Some("Retry baseline preparation".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let task = fixture.wait(&request).await;
    good(&task);
    let final_state = fixture.store.control(&id).unwrap().get();
    assert!(final_state.ready_for_changes && final_state.pending_build);
    assert_eq!(final_state.maintenance_id, state.maintenance_id);
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

#[cfg(target_os = "macos")]
#[tokio::test]
async fn real_purge_failure_reports_residue_and_explicit_retry_finishes() {
    let fixture = Fixture::new().await;
    let deleted = fixture.legacy_delete(json!({})).await;
    let trash = good(&deleted)["trashId"].as_str().unwrap().to_owned();
    let root = fixture
        .lifecycle
        .trash_root(&trash)
        .unwrap()
        .join("project");
    let guard = ImmutableFile::new(&root.join("data/main.sqlite"));
    let submitted = fixture
        .lifecycle
        .submit_trash(&trash, "purge")
        .await
        .unwrap();
    let task = fixture.wait(&submitted).await;
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(
        task.error.unwrap().details["operationOutcome"],
        "purge_incomplete"
    );
    let info = fixture.lifecycle.trash_info(&trash).await.unwrap();
    assert_eq!(info["state"], "purgeIncomplete");
    assert_eq!(info["restoreAvailable"], false);
    assert!(root.join("data/main.sqlite").exists());
    drop(guard);
    let submitted = fixture
        .lifecycle
        .submit_trash(&trash, "purge")
        .await
        .unwrap();
    assert_eq!(
        good(&fixture.wait(&submitted).await)["operationOutcome"],
        "purged"
    );
    assert!(!root.exists());
    fixture.stop().await;
}

#[tokio::test]
async fn committed_purge_response_lost_cannot_retire_a_later_same_id_import() {
    let fixture = Fixture::new().await;
    let archive = fixture.export_path("reimport-after-purge.zip");
    good(
        &fixture
            .project("export", json!({"destinationPath":archive}))
            .await,
    );
    let deleted = fixture.legacy_delete(json!({})).await;
    let trash = good(&deleted)["trashId"].as_str().unwrap().to_owned();
    let submitted = fixture
        .lifecycle
        .submit_trash(&trash, "purge")
        .await
        .unwrap();
    let purged = fixture.wait(&submitted).await;
    good(&purged);
    let path = fixture.lifecycle.journal_path(&purged.task_id);
    let mut journal: Journal = read_json(&path).unwrap();
    journal.resolved = false;
    journal.committed = false;
    builds::atomic_json(&path, &journal).unwrap();
    let submitted = fixture
        .lifecycle
        .submit_import(json!({"archivePath":archive,"mode":"original"}))
        .await
        .unwrap();
    good(&fixture.wait(&submitted).await);
    let project = fixture.store.project(&fixture.id).unwrap();
    let epoch = project.document_epoch.clone();
    let root_identity = identity(&project.root).unwrap();
    let control = fixture.store.control(&fixture.id).unwrap();
    let mut later = control.get();
    later.pending_build = true;
    later.pending_build_reason = Some("Newly imported project owns this state".into());
    control.set(later.clone()).unwrap();
    let task_id = purged.task_id.clone();
    let fixture = fixture.restart_task(purged).await;
    assert_eq!(
        fixture.tasks.task(&task_id).unwrap().result.unwrap()["operationOutcome"],
        "purged"
    );
    let current = fixture.store.project(&fixture.id).unwrap();
    assert_eq!(current.document_epoch, epoch);
    assert_eq!(identity(&current.root).unwrap(), root_identity);
    assert_eq!(
        json!(fixture.store.control(&fixture.id).unwrap().get()),
        json!(later)
    );
    fixture.stop().await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn final_lifecycle_journal_failure_blocks_in_memory_and_reconciles_its_own_gate() {
    let fixture = Fixture::new().await;
    let copied = fixture.project("copy", json!({})).await;
    let id = good(&copied)["projectId"].as_str().unwrap().to_owned();
    let path = fixture.lifecycle.journal_path(&copied.task_id);
    let mut journal: Journal = read_json(&path).unwrap();
    journal.resolved = false;
    builds::atomic_json(&path, &journal).unwrap();
    let control = fixture.store.control(&id).unwrap();
    let mut later = control.get();
    later.pending_build = true;
    later.pending_build_reason = Some("Preserve newer metadata during gate recovery".into());
    control.set(later).unwrap();
    let control_path = fixture
        .paths
        .workspace_root()
        .join(".platform/project-state/maintenance")
        .join(format!("{id}.json"));
    let journal_guard = ImmutableFile::new(&path);
    let control_guard = ImmutableFile::new(&control_path);
    let error = fixture.lifecycle.finish_journal(&mut journal).unwrap_err();
    assert_eq!(error.details["operationOutcome"], "needs_attention");
    assert!(error.details.get("controlPersistenceError").is_some());
    assert!(!journal.resolved);
    assert!(control.get().recovery_required);
    assert_eq!(
        control.check(true, None, false).unwrap_err().code,
        "RECOVERY_REQUIRED"
    );
    drop(control_guard);
    drop(journal_guard);
    assert_eq!(
        fixture.lifecycle.reconcile(&mut journal).unwrap()["operationOutcome"],
        "copied"
    );
    let state = control.get();
    assert!(!state.recovery_required);
    assert_eq!(state.mode, "ready");
    assert!(state.pending_build);
    assert_eq!(
        state.pending_build_reason.as_deref(),
        Some("Preserve newer metadata during gate recovery")
    );
    control.check(true, None, false).unwrap();
    fixture.stop().await;
}
