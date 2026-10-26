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
    async fn stop(self) {
        self.lifecycle.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
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

fn lifecycle_evidence(paths: &[PathBuf]) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn collect(path: &Path, evidence: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        if metadata.is_dir() {
            evidence.insert(path.into(), vec![]);
            for entry in fs::read_dir(path).unwrap() {
                collect(&entry.unwrap().path(), evidence);
            }
        } else {
            assert!(metadata.is_file());
            evidence.insert(path.into(), fs::read(path).unwrap());
        }
    }
    let mut evidence = std::collections::BTreeMap::new();
    for path in paths {
        collect(path, &mut evidence);
    }
    evidence
}

async fn retained_delete_journal() -> (Fixture, TaskRecord, Journal, Vec<PathBuf>) {
    let fixture = Fixture::new().await;
    let (root, expected) = fixture.store.deletion_root(&fixture.id).unwrap();
    let (task, lease) = fixture.tasks.create_delete_operation(&fixture.id).unwrap();
    let mut journal = LifecycleManager::journal(&task, "delete", &fixture.id);
    journal.directory_name = Some(root_directory_name(&root).unwrap());
    journal.directory_identity = Some(expected.clone());
    let control = fixture.store.control(&fixture.id).unwrap();
    let mut state = control.get();
    state.pending_build = true;
    state.pending_build_reason = Some("Retain later control and every payload".into());
    control.set(state.clone()).unwrap();
    journal.control = Some(state);
    fixture
        .store
        .begin_deletion(&fixture.id, &task.task_id, &journal.target_epoch, &expected)
        .unwrap();
    let evidence = vec![
        root,
        fixture.lifecycle.operation(&task.task_id),
        fixture
            .paths
            .workspace_root()
            .join(".platform/project-state"),
    ];
    fs::create_dir_all(&evidence[1]).unwrap();
    fs::write(evidence[1].join("keep.txt"), b"retained operation evidence").unwrap();
    drop(lease);
    (fixture, task, journal, evidence)
}

#[tokio::test]
async fn resolved_system_trash_journal_restarts_without_reading_markers_or_rewriting_evidence() {
    for opaque_marker in [false, true] {
        let (fixture, task, mut journal, mut evidence) = retained_delete_journal().await;
        let retained = fixture.paths.data_root().join("disposable-system-trash");
        fs::create_dir(&retained).unwrap();
        fs::write(
            retained.join("keep.txt"),
            b"operating-system trash evidence",
        )
        .unwrap();
        evidence.push(retained.clone());
        journal.system_trash = true;
        journal.system_trash_path = Some(retained.clone());
        journal.phase = "system-trash-moved".into();
        journal.committed = true;
        journal.resolved = true;
        journal.result = json!({"projectId":fixture.id,"operationOutcome":"system_trashed","systemTrashPath":retained,"restoreAvailable":true});
        let path = fixture.lifecycle.journal_path(&task.task_id);
        // Preserve arbitrary original formatting too, rather than reserializing
        // an already completed old-worktree record during startup.
        fs::write(&path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();
        if opaque_marker {
            fs::write(
                fixture
                    .paths
                    .workspace_root()
                    .join(".platform/project-state/deleting")
                    .join(format!("{}.json", fixture.id)),
                b"opaque later marker must not be read by this historical journal",
            )
            .unwrap();
        }
        let before = lifecycle_evidence(&evidence);
        let original_result = journal.result.clone();
        assert_eq!(
            fixture.lifecycle.reconcile(&mut journal).unwrap(),
            original_result
        );
        let fixture = fixture.restart_task(task.clone()).await;
        let mut recovered_result = original_result;
        recovered_result["recovered"] = true.into();
        assert_eq!(
            fixture.tasks.task(&task.task_id).unwrap().result,
            Some(recovered_result)
        );
        assert_eq!(lifecycle_evidence(&evidence), before);
        fixture.stop().await;
    }
}

#[tokio::test]
async fn unresolved_system_trash_intent_never_enters_direct_delete_recovery() {
    for (system_trash, path_present, phase_only) in [
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let (fixture, _task, mut journal, mut evidence) = retained_delete_journal().await;
        let retained = fixture.paths.data_root().join("disposable-system-trash");
        fs::create_dir(&retained).unwrap();
        fs::write(
            retained.join("keep.txt"),
            b"preserve possible trash payload",
        )
        .unwrap();
        evidence.push(retained.clone());
        journal.system_trash = system_trash;
        journal.system_trash_path = path_present.then_some(retained);
        journal.phase = if phase_only {
            "system-trash-intent"
        } else {
            "delete-intent"
        }
        .into();
        fixture.lifecycle.save(&journal).unwrap();
        let before = lifecycle_evidence(&evidence);
        assert_eq!(
            fixture.lifecycle.reconcile(&mut journal).unwrap_err().code,
            "RECOVERY_REQUIRED"
        );
        let error = LifecycleManager::new(
            fixture.paths.clone(),
            fixture.store.clone(),
            fixture.tasks.clone(),
            fixture.maintenance.clone(),
        )
        .err()
        .unwrap();
        assert_eq!(error.code, "RECOVERY_REQUIRED");
        assert_eq!(lifecycle_evidence(&evidence), before);
        assert!(fixture
            .store
            .pending_deletion(&fixture.id)
            .unwrap()
            .is_some());
        fixture.stop().await;
    }
}

#[tokio::test]
async fn ordinary_delete_recovery_keeps_false_null_compatibility_and_defaults_missing_fields() {
    for omit_fields in [false, true] {
        let (fixture, task, journal, evidence) = retained_delete_journal().await;
        let path = fixture.lifecycle.journal_path(&task.task_id);
        let mut value = json!(journal);
        if omit_fields {
            value.as_object_mut().unwrap().remove("systemTrash");
            value.as_object_mut().unwrap().remove("systemTrashPath");
        }
        builds::atomic_json(&path, &value).unwrap();
        let parsed: Journal = read_json(&path).unwrap();
        assert!(!parsed.system_trash);
        assert!(parsed.system_trash_path.is_none());
        let root_before = lifecycle_evidence(&evidence[..1]);
        let fixture = fixture.restart_task(task.clone()).await;
        assert_eq!(
            fixture.tasks.task(&task.task_id).unwrap().result.unwrap()["operationOutcome"],
            "delete_incomplete"
        );
        assert_eq!(lifecycle_evidence(&evidence[..1]), root_before);
        let saved: Value = read_json(&path).unwrap();
        assert_eq!(saved["systemTrash"], false);
        assert_eq!(saved["systemTrashPath"], Value::Null);
        assert_eq!(saved["resolved"], true);
        assert!(fixture
            .store
            .pending_deletion(&fixture.id)
            .unwrap()
            .is_some());
        fixture.stop().await;
    }
}

#[tokio::test]
async fn compatible_journal_fields_do_not_relax_schema_identity_or_type_validation() {
    for (field, value, expected_code) in [
        ("schemaVersion", json!(2), "RECOVERY_REQUIRED"),
        ("unexpectedField", json!(true), "INVALID_JSON"),
        ("systemTrash", json!("false"), "INVALID_JSON"),
        ("systemTrash", Value::Null, "INVALID_JSON"),
        ("systemTrashPath", json!(false), "INVALID_JSON"),
        (
            "systemTrashPath",
            json!({"path":"untrusted"}),
            "INVALID_JSON",
        ),
        ("projectId", json!("../unsafe"), "RECOVERY_REQUIRED"),
        ("taskId", json!("task_other"), "RECOVERY_REQUIRED"),
        ("targetEpoch", json!("invalid-epoch"), "RECOVERY_REQUIRED"),
    ] {
        let (fixture, task, mut journal, evidence) = retained_delete_journal().await;
        journal.system_trash = true;
        journal.resolved = true;
        journal.phase = "system-trash-moved".into();
        let mut record = json!(journal);
        record[field] = value;
        builds::atomic_json(&fixture.lifecycle.journal_path(&task.task_id), &record).unwrap();
        let before = lifecycle_evidence(&evidence);
        let error = LifecycleManager::new(
            fixture.paths.clone(),
            fixture.store.clone(),
            fixture.tasks.clone(),
            fixture.maintenance.clone(),
        )
        .err()
        .unwrap();
        assert_eq!(error.code, expected_code, "{field}");
        assert_eq!(lifecycle_evidence(&evidence), before, "{field}");
        fixture.stop().await;
    }
}

#[path = "direct_delete_tests.rs"]
mod direct_delete;

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
        .reserve_identity(&target, &task.task_id)
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
        .check_identity_available(&target, None)
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
