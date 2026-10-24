use super::*;
use crate::{
    data_restore_guard as guard, CreateProjectInput, RuntimePaths, SqlCaller, SqlOptions,
    SqlRequest,
};

struct Fixture {
    _temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    project: ProjectHandle,
    root: PathBuf,
    header: guard::DataRestoreJournalHeader,
    source_record: Vec<u8>,
    apply_epoch: String,
    undo_epoch: String,
}
impl Fixture {
    async fn new() -> Self {
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let info = store
            .create(CreateProjectInput {
                name: "Disposable data restore primitives".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&info.id).unwrap();
        for sql in [
            "CREATE TABLE notes(body TEXT)",
            "INSERT INTO notes VALUES('original live record')",
        ] {
            store
                .execute(
                    &info.id,
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
        let lease = store
            .reserve_transient_operation(&info.id, "prepare-data-restore-primitive-fixture")
            .unwrap();
        let freeze = Freeze::new(project.control.clone());
        let gate = project.operation_gate.clone().write_owned().await;
        store.close_database(&info.id).await;
        let staging = paths.workspace_root().join(".platform").join(format!(
            "disposable-data-restore-stage-{}",
            Uuid::new_v4().simple()
        ));
        fs::create_dir(&staging).unwrap();
        let candidate = staging.join("candidate.sqlite");
        let db = rusqlite::Connection::open(&candidate).unwrap();
        db.execute_batch(
            "CREATE TABLE notes(body TEXT);INSERT INTO notes VALUES('target version record');",
        )
        .unwrap();
        drop(db);
        let apply_epoch = Uuid::new_v4().to_string();
        let undo_epoch = Uuid::new_v4().to_string();
        let source_record = fs::read(registration_path(paths.workspace_root(), &info.id)).unwrap();
        let plan = store
            .prepare_data_restore_registration(
                &info.id,
                &lease,
                &gate,
                DataRestoreRegistrationPreparation {
                    staging: &staging,
                    candidate_database: &candidate,
                    apply_epoch: &apply_epoch,
                    undo_epoch: &undo_epoch,
                },
            )
            .unwrap();
        let header = guard::DataRestoreJournalHeader {
            schema_version: 1,
            operation: "dataRestore".into(),
            project_id: info.id,
            task_id: format!("task_{}", Uuid::new_v4().simple()),
            phase: "prepared".into(),
            plan_digest: "a".repeat(64),
            operation_identity: FileIdentity::capture(&staging).unwrap(),
            registration_plan: plan,
            transaction_plan: None,
            journal_identity: None,
        };
        let root = guard::publish_prepared(paths.workspace_root(), &staging, &header).unwrap();
        drop(gate);
        drop(freeze);
        drop(lease);
        Self {
            _temporary: temporary,
            paths,
            store,
            project,
            root,
            header,
            source_record,
            apply_epoch,
            undo_epoch,
        }
    }
    fn permit(&self) -> guard::DataRestoreRecoveryPermit {
        let state = guard::inspect(self.paths.workspace_root(), &self.project.manifest.id);
        assert!(state.issues.is_empty(), "{state:?}");
        assert_eq!(state.pending.len(), 1);
        let pending = &state.pending[0];
        guard::validate_recovery(
            self.paths.workspace_root(),
            &self.project.manifest.id,
            &pending.task_id,
            &pending.plan_digest,
            &pending.journal_digest,
        )
        .unwrap()
    }
    async fn access(&self) -> DataRestoreExclusiveAccess {
        let permit = self.permit();
        let lease = self
            .store
            .reserve_data_restore_recovery_task(
                &self.project.manifest.id,
                &format!("task_{}", Uuid::new_v4().simple()),
                &permit,
            )
            .unwrap();
        let access = self
            .store
            .data_restore_recovery_access(&self.project.manifest.id, lease, permit)
            .unwrap();
        let (_s, stop) = watch::channel(false);
        let (_c, cancel) = watch::channel(false);
        access.lock(stop, cancel).await.unwrap()
    }
    fn install_candidate_for_registration_test(&self) {
        // Test-only explicit file placement; production has no live-data apply API.
        let live = self.project.root.join(&self.project.manifest.database);
        fs::rename(&live, self.root.join("original.sqlite")).unwrap();
        fs::rename(self.root.join("candidate.sqlite"), &live).unwrap();
    }
    async fn finish(self) {
        self.store.shutdown().await;
    }
}

#[tokio::test]
async fn data_restore_guard_blocks_cached_business_diagnostics_deletion_migration_and_identity_bypasses(
) {
    let fixture = Fixture::new().await;
    let id = &fixture.project.manifest.id;
    assert_eq!(
        fixture.project.check().unwrap_err().code,
        "PROJECT_DATA_RESTORE_PENDING"
    );
    assert_eq!(
        fixture
            .project
            .validate_epoch(Some(&fixture.project.document_epoch))
            .unwrap_err()
            .code,
        "PROJECT_DATA_RESTORE_PENDING"
    );
    assert_eq!(
        fixture.store.project(id).err().unwrap().code,
        "PROJECT_DATA_RESTORE_PENDING"
    );
    assert_eq!(
        fixture.store.schema(id, None).await.unwrap_err().code,
        "PROJECT_DATA_RESTORE_PENDING"
    );
    assert!(fixture.store.check_development_available(id).is_err());
    assert!(fixture
        .store
        .reserve_delete_task(id, "task_forbidden_delete")
        .is_err());
    assert!(fixture
        .store
        .reserve_identity(id, "task_forbidden_identity", None)
        .is_err());
    assert!(fixture.store.begin_workspace_migration().is_err());
    assert!(fixture.store.begin_workspace_cleanup().is_err());
    assert!(crate::workspace::validate_idle(&fixture.paths).is_err());
    let epoch = Uuid::new_v4().to_string();
    for error in [
        fixture
            .store
            .unregister_lifecycle(id, &epoch, None)
            .unwrap_err(),
        fixture
            .store
            .register_lifecycle(id, &epoch, "task_foreign", true)
            .unwrap_err(),
        fixture.store.rebind_migrated(id, &epoch).unwrap_err(),
        fixture
            .store
            .retain_migrated_diagnostic(id, ApiError::new("TEST", "foreign migration"))
            .unwrap_err(),
        fixture
            .store
            .rebase_retired_registration_after_migration(
                id,
                &fixture.paths.workspace_root().join("old"),
            )
            .unwrap_err(),
        fixture
            .store
            .begin_deletion(
                id,
                "task_foreign",
                &epoch,
                &FileIdentity::capture(&fixture.project.root).unwrap(),
            )
            .unwrap_err(),
    ] {
        assert_eq!(error.code, "PROJECT_DATA_RESTORE_PENDING");
    }
    assert_eq!(
        fs::read(registration_path(fixture.paths.workspace_root(), id)).unwrap(),
        fixture.source_record
    );
    let live = fixture
        .project
        .root
        .join(&fixture.project.manifest.database);
    fs::rename(&live, fixture.root.join("original.sqlite")).unwrap();
    assert_eq!(
        fixture.store.schema(id, None).await.unwrap_err().code,
        "PROJECT_DATA_RESTORE_PENDING"
    );
    assert!(
        !live.exists(),
        "blocked diagnostics must never create an empty DB"
    );
    let access = fixture.access().await;
    assert!(access
        .publish_epoch(DataRestoreDirection::Continue)
        .is_err());
    assert!(!live.exists());
    drop(access);
    let reopened = ProjectStore::new(fixture.paths.workspace_root().into()).unwrap();
    assert_eq!(
        reopened.project(id).err().unwrap().code,
        "PROJECT_DATA_RESTORE_PENDING"
    );
    reopened.shutdown().await;
    fixture.finish().await;
}

#[tokio::test]
async fn data_restore_epoch_publication_is_fixed_idempotent_and_preserves_old_registration() {
    let fixture = Fixture::new().await;
    fixture.install_candidate_for_registration_test();
    let access = fixture.access().await;
    let committed = access
        .publish_epoch(DataRestoreDirection::Continue)
        .unwrap();
    assert_eq!(committed.document_epoch, fixture.apply_epoch);
    assert!(!committed.already_committed);
    let repeated = access
        .publish_epoch(DataRestoreDirection::Continue)
        .unwrap();
    assert_eq!(repeated.document_epoch, fixture.apply_epoch);
    assert!(repeated.already_committed);
    assert!(access.publish_epoch(DataRestoreDirection::Undo).is_err());
    assert_eq!(
        fs::read(fixture.root.join(APPLY_SLOT)).unwrap(),
        fixture.source_record
    );
    assert!(
        fixture.store.project(&fixture.project.manifest.id).is_err(),
        "epoch commit alone must not clear the guard"
    );
    drop(access);
    let registered: Registration = read_json(&registration_path(
        fixture.paths.workspace_root(),
        &fixture.project.manifest.id,
    ))
    .unwrap();
    assert_eq!(registered.document_epoch, fixture.apply_epoch);
    assert_ne!(registered.document_epoch, fixture.project.document_epoch);
    assert_eq!(
        fixture
            .project
            .validate_epoch(Some(&fixture.project.document_epoch))
            .unwrap_err()
            .code,
        "PROJECT_DATA_RESTORE_PENDING"
    );
    let db = rusqlite::Connection::open(
        fixture
            .project
            .root
            .join(&fixture.project.manifest.database),
    )
    .unwrap();
    assert_eq!(
        db.query_row("SELECT body FROM notes", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "target version record"
    );
    drop(db);
    fixture.finish().await;
}

#[tokio::test]
async fn data_restore_phase_relabel_never_releases_before_or_after_epoch_commit() {
    for committed in [false, true] {
        let fixture = Fixture::new().await;
        if committed {
            fixture.install_candidate_for_registration_test();
            let access = fixture.access().await;
            access
                .publish_epoch(DataRestoreDirection::Continue)
                .unwrap();
        }
        for phase in ["completed", "undone", "not-applied"] {
            let mut header = fixture.header.clone();
            header.phase = phase.into();
            crate::builds::atomic_json(&fixture.root.join("journal.json"), &header).unwrap();
            assert!(
                !guard::inspect(fixture.paths.workspace_root(), &fixture.project.manifest.id)
                    .issues
                    .is_empty(),
                "{committed} {phase}"
            );
            assert!(fixture.project.check().is_err(), "{committed} {phase}");
            assert!(
                fixture.store.project(&fixture.project.manifest.id).is_err(),
                "{committed} {phase}"
            );
            assert!(
                fixture.store.begin_workspace_migration().is_err(),
                "{committed} {phase}"
            );
            assert!(fixture.root.join("journal.json").exists());
        }
        fixture.finish().await;
    }
}

#[tokio::test]
async fn data_restore_undo_publishes_its_own_fixed_epoch_without_touching_database() {
    let fixture = Fixture::new().await;
    let original = fs::read(
        fixture
            .project
            .root
            .join(&fixture.project.manifest.database),
    )
    .unwrap();
    let access = fixture.access().await;
    let result = access.publish_epoch(DataRestoreDirection::Undo).unwrap();
    assert_eq!(result.document_epoch, fixture.undo_epoch);
    assert!(
        access
            .publish_epoch(DataRestoreDirection::Undo)
            .unwrap()
            .already_committed
    );
    assert!(access
        .publish_epoch(DataRestoreDirection::Continue)
        .is_err());
    assert_eq!(
        fs::read(
            fixture
                .project
                .root
                .join(&fixture.project.manifest.database)
        )
        .unwrap(),
        original
    );
    assert_eq!(
        fs::read(fixture.root.join(UNDO_SLOT)).unwrap(),
        fixture.source_record
    );
    drop(access);
    fixture.finish().await;
}

#[tokio::test]
async fn data_restore_registration_swap_failure_receipt_stays_committed_and_retry_finishes_same_epoch(
) {
    let fixture = Fixture::new().await;
    fixture.install_candidate_for_registration_test();
    let access = fixture.access().await;
    let error = access
        .publish_epoch_with(
            DataRestoreDirection::Continue,
            || Ok(()),
            || {
                Err(ApiError::new(
                    "INJECTED_SYNC_BOUNDARY",
                    "after swap before sync",
                ))
            },
        )
        .unwrap_err();
    assert_eq!(error.details["operationOutcome"], "committed");
    assert_eq!(error.details["pending"], true);
    let retry = access
        .publish_epoch(DataRestoreDirection::Continue)
        .unwrap();
    assert!(retry.already_committed);
    assert_eq!(retry.document_epoch, fixture.apply_epoch);
    assert_eq!(
        fs::read(fixture.root.join(APPLY_SLOT)).unwrap(),
        fixture.source_record
    );
    drop(access);
    fixture.finish().await;
}

#[tokio::test]
async fn data_restore_third_party_registration_at_syscall_boundary_is_preserved_and_blocks_recovery(
) {
    let fixture = Fixture::new().await;
    fixture.install_candidate_for_registration_test();
    let access = fixture.access().await;
    let record = registration_path(fixture.paths.workspace_root(), &fixture.project.manifest.id);
    let foreign = b"foreign registration bytes must be retained".to_vec();
    let error = access
        .publish_epoch_with(
            DataRestoreDirection::Continue,
            || {
                fs::rename(
                    &record,
                    fixture.root.join("fixture-preserved-registration.json"),
                )?;
                fs::write(&record, &foreign)?;
                Ok(())
            },
            || Ok(()),
        )
        .unwrap_err();
    assert_eq!(error.details["operationOutcome"], "committed");
    assert_eq!(fs::read(fixture.root.join(APPLY_SLOT)).unwrap(), foreign);
    assert_eq!(
        fs::read(fixture.root.join("fixture-preserved-registration.json")).unwrap(),
        fixture.source_record
    );
    assert!(
        !guard::inspect(fixture.paths.workspace_root(), &fixture.project.manifest.id)
            .issues
            .is_empty()
    );
    assert!(access
        .publish_epoch(DataRestoreDirection::Continue)
        .is_err());
    drop(access);
    fixture.finish().await;
}

#[tokio::test]
async fn data_restore_same_bytes_replaced_slot_and_stale_digest_cannot_be_adopted() {
    let fixture = Fixture::new().await;
    let permit = fixture.permit();
    let path = fixture.root.join(APPLY_SLOT);
    let bytes = fs::read(&path).unwrap();
    fs::rename(&path, fixture.root.join("fixture-preserved-slot.json")).unwrap();
    fs::write(&path, &bytes).unwrap();
    assert!(permit
        .revalidate(fixture.paths.workspace_root(), &fixture.project.manifest.id)
        .is_err());
    assert!(
        !guard::inspect(fixture.paths.workspace_root(), &fixture.project.manifest.id)
            .issues
            .is_empty()
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(
        fs::read(registration_path(
            fixture.paths.workspace_root(),
            &fixture.project.manifest.id
        ))
        .unwrap(),
        fixture.source_record
    );
    fixture.finish().await;
}

#[tokio::test]
async fn data_restore_permission_changes_are_preserved_and_cannot_be_adopted() {
    let fixture = Fixture::new().await;
    let permit = fixture.permit();
    let path = fixture.root.join(APPLY_SLOT);
    let bytes = fs::read(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(permit
        .revalidate(fixture.paths.workspace_root(), &fixture.project.manifest.id)
        .is_err());
    assert!(fixture.store.project(&fixture.project.manifest.id).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(
        fs::read(registration_path(
            fixture.paths.workspace_root(),
            &fixture.project.manifest.id
        ))
        .unwrap(),
        fixture.source_record
    );
    fixture.finish().await;
}

#[tokio::test]
async fn data_restore_legacy_operation_identity_is_not_implicitly_upgraded() {
    let mut fixture = Fixture::new().await;
    fixture.header.operation_identity.volume_uuid = None;
    crate::builds::atomic_json(&fixture.root.join("journal.json"), &fixture.header).unwrap();
    assert!(
        !guard::inspect(fixture.paths.workspace_root(), &fixture.project.manifest.id)
            .issues
            .is_empty()
    );
    assert!(fixture.store.project(&fixture.project.manifest.id).is_err());
    assert_eq!(
        fs::read(registration_path(
            fixture.paths.workspace_root(),
            &fixture.project.manifest.id
        ))
        .unwrap(),
        fixture.source_record
    );
    assert!(fixture.root.join("candidate.sqlite").exists());
    fixture.finish().await;
}

#[test]
fn data_restore_registration_unknown_fields_are_refused_without_rewriting() {
    let record = serde_json::json!({
        "schemaVersion":1,"id":"project_disposable","projectRoot":"/disposable/projects/example",
        "directoryName":"example","rootIdentity":{"device":1,"inode":2,"volumeUuid":"volume"},
        "databasePath":"data/main.sqlite","databaseIdentity":{"device":1,"inode":3,"volumeUuid":"volume"},
        "documentEpoch":Uuid::new_v4().to_string()
    });
    let mut future = record.clone();
    future["unknownRetainedField"] = serde_json::json!({"must":"be preserved"});
    let bytes = serde_json::to_vec(&future).unwrap();
    assert!(read_registration(&bytes).is_err());
    assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), future);
    future = record;
    future["rootIdentity"]["unknownVolumeEvidence"] = serde_json::json!("retained");
    assert!(read_registration(&serde_json::to_vec(&future).unwrap()).is_err());
}

#[tokio::test]
async fn data_restore_stable_identities_survive_simulated_device_renumbering() {
    let mut fixture = Fixture::new().await;
    fixture.header.operation_identity.device =
        fixture.header.operation_identity.device.wrapping_add(17);
    let plan = &mut fixture.header.registration_plan;
    for identity in [
        &mut plan.before.proof.identity,
        &mut plan.apply.proof.identity,
        &mut plan.undo.proof.identity,
        &mut plan.source_database.identity,
        &mut plan.target_database.identity,
    ] {
        assert!(!identity.is_legacy());
        identity.device = identity.device.wrapping_add(19);
    }
    crate::builds::atomic_json(&fixture.root.join("journal.json"), &fixture.header).unwrap();
    let access = fixture.access().await;
    assert_eq!(
        access
            .publish_epoch(DataRestoreDirection::Undo)
            .unwrap()
            .document_epoch,
        fixture.undo_epoch
    );
    drop(access);
    fixture.finish().await;
}

#[tokio::test]
async fn data_restore_operation_directory_identity_change_is_not_automatically_claimed() {
    let fixture = Fixture::new().await;
    let preserved = fixture.root.with_extension("fixture-original");
    fs::rename(&fixture.root, &preserved).unwrap();
    fs::create_dir(&fixture.root).unwrap();
    for name in ["journal.json", APPLY_SLOT, UNDO_SLOT] {
        fs::copy(preserved.join(name), fixture.root.join(name)).unwrap();
    }
    assert!(
        !guard::inspect(fixture.paths.workspace_root(), &fixture.project.manifest.id)
            .issues
            .is_empty()
    );
    assert!(preserved.join("candidate.sqlite").exists());
    assert!(fixture.store.project(&fixture.project.manifest.id).is_err());
    fixture.finish().await;
}

#[test]
fn data_restore_layout_rejects_companion_and_parent_aliases() {
    let mut manifest:ProjectManifest=serde_json::from_value(serde_json::json!({
        "manifestVersion":1,"id":"project_disposable","name":"Disposable","sdkApiVersion":1,
        "sourceDir":"app","outputDir":"build","entry":"build/index.html",
        "database":"data/main.sqlite","attachmentsDir":"data/attachments","build":{"script":"build"},"docs":{}
    })).unwrap();
    validate_data_restore_layout(&manifest).unwrap();
    for path in [
        "data/main.sqlite-wal",
        "data/main.sqlite-shm/inside",
        "data/main.sqlite-journal",
        "data",
        "data/MAIN.sqlite",
        "data/Main.sqlite-wal",
    ] {
        manifest.attachments_dir = path.into();
        assert!(validate_data_restore_layout(&manifest).is_err(), "{path}");
    }
}
