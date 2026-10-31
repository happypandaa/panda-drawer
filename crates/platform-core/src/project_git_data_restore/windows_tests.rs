//! Real native transaction fixtures; every project and retained path is disposable.
use super::*;
use platform_native::windows::files as native;
use std::os::windows::fs::OpenOptionsExt;

#[path = "fault_windows_tests.rs"]
mod faults;

struct Prepared {
    _temporary: TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    project: ProjectHandle,
    operation: PathBuf,
    journal: transaction::Journal,
    plan: transaction::Plan,
    _stop: watch::Sender<bool>,
    stop: watch::Receiver<bool>,
}
impl Prepared {
    async fn new() -> Self {
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let info = store
            .create(crate::CreateProjectInput {
                name: "Disposable Windows data transaction 中文".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&info.id).unwrap();
        store.close_database(&info.id).await.unwrap();
        let live_database = project.root.join(&project.manifest.database);
        let database = rusqlite::Connection::open(&live_database).unwrap();
        database.execute_batch("CREATE TABLE disposable_notes(value TEXT); INSERT INTO disposable_notes VALUES('before');").unwrap();
        drop(database);
        fs::write(
            project
                .root
                .join(&project.manifest.attachments_dir)
                .join("before.txt"),
            b"original attachment",
        )
        .unwrap();
        fs::create_dir(
            project
                .root
                .join(&project.manifest.attachments_dir)
                .join("empty-before"),
        )
        .unwrap();
        private_dir(&project.root.join(".git")).unwrap();
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let (task, lease) = tasks
            .create_operation(&info.id, "gitDataRestoreApply")
            .unwrap();
        let (sender, stop) = watch::channel(false);
        let (_cancel_sender, cancel) = watch::channel(false);
        let snapshot_lease =
            version_data::SnapshotLease::acquire(project.clone(), stop.clone(), cancel.clone())
                .await
                .unwrap();
        let temporary_snapshots = tempfile::Builder::new()
            .prefix("panda-disposable-data-slot-snapshots-")
            .tempdir()
            .unwrap();
        let current = version_data::prepare_snapshot_locked(
            &snapshot_lease,
            &temporary_snapshots.path().join("current"),
            version_data::SnapshotLimits::default(),
            &stop,
            &cancel,
        )
        .unwrap();
        let target_root = temporary_snapshots.path().join("target");
        private_dir(&target_root).unwrap();
        private_dir(&target_root.join("attachments")).unwrap();
        private_dir(&target_root.join("attachments/empty-after")).unwrap();
        fs::write(
            target_root.join("attachments/after.txt"),
            b"target attachment",
        )
        .unwrap();
        let target_database =
            rusqlite::Connection::open(target_root.join("database.sqlite")).unwrap();
        target_database.execute_batch("CREATE TABLE disposable_notes(value TEXT); INSERT INTO disposable_notes VALUES('after');").unwrap();
        drop(target_database);
        let data_file = |path: &Path| {
            let bytes = fs::read(path).unwrap();
            version_data::DataFile {
                bytes: bytes.len() as u64,
                sha256: sha256(&bytes),
            }
        };
        let target_manifest = version_data::DataSnapshotManifest {
            format_version: 1,
            project_id: info.id.clone(),
            sdk_api_version: project.manifest.sdk_api_version,
            database_path: project.manifest.database.clone(),
            attachments_dir: project.manifest.attachments_dir.clone(),
            database: data_file(&target_root.join("database.sqlite")),
            attachments: BTreeMap::from([(
                "after.txt".into(),
                data_file(&target_root.join("attachments/after.txt")),
            )]),
            directories: vec!["empty-after".into()],
        };
        fs::write(
            target_root.join("snapshot.json"),
            serde_json::to_vec_pretty(&target_manifest).unwrap(),
        )
        .unwrap();
        let target = version_data::validate_snapshot(
            &target_root,
            &project.manifest,
            version_data::SnapshotLimits::default(),
            &stop,
            &cancel,
        )
        .unwrap();
        let stage = paths
            .workspace_root()
            .join(".platform")
            .join(format!("data-restore-preparing-{}", task.task_id));
        private_dir(&stage).unwrap();
        slot_files::copy_target(&target, &stage, &mut || Ok(())).unwrap();
        let slots = transaction::prepare_slots(
            &project,
            &stage,
            &current.manifest,
            &target.manifest,
            &mut || Ok(()),
        )
        .unwrap();
        let registration_plan = store
            .prepare_data_restore_registration(
                &info.id,
                &lease,
                snapshot_lease.gate(),
                DataRestoreRegistrationPreparation {
                    staging: &stage,
                    candidate_database: &stage.join("slots/database/after"),
                    apply_epoch: &uuid::Uuid::new_v4().to_string(),
                    undo_epoch: &uuid::Uuid::new_v4().to_string(),
                },
            )
            .unwrap();
        let stage_fd = files::root(&stage).unwrap();
        let mut journal_file = files::create(&stage_fd, "journal.json").unwrap();
        let journal_identity = PersistedIdentity::from_file(&journal_file).unwrap();
        let git = project.root.join(".git");
        let metadata = [
            "HEAD",
            "config",
            "index",
            "packed-refs",
            "shallow",
            "info/attributes",
            "info/exclude",
            "refs",
        ]
        .into_iter()
        .map(|name| {
            Ok((
                name.into(),
                slot_files::EntryProof::capture(&git.join(name), name == "refs", &mut || Ok(()))?,
            ))
        })
        .collect::<Result<_, ApiError>>()
        .unwrap();
        let roots = included_roots(&project.manifest);
        let plan = transaction::Plan {
            schema_version: 1,
            policy: transaction::POLICY.into(),
            project_id: info.id.clone(),
            task_id: task.task_id.clone(),
            target_commit: "a".repeat(40),
            preview_plan_digest: "b".repeat(64),
            operation_identity: PersistedIdentity::capture(&stage).unwrap(),
            registration_plan: registration_plan.clone(),
            manifest: project.manifest.clone(),
            parents: slot_files::capture_parents(
                &project.root,
                slots.iter().map(|slot| slot.path.clone()),
            )
            .unwrap(),
            operation_parents: slot_files::capture_parents(
                &stage,
                slots
                    .iter()
                    .flat_map(|slot| {
                        [
                            format!("slots/{}/before", slot.kind),
                            format!("slots/{}/after", slot.kind),
                        ]
                    })
                    .chain([
                        "metadata-attempts/placeholder".into(),
                        "metadata-reserve/placeholder".into(),
                    ]),
            )
            .unwrap(),
            metadata_reserve: ["completion", "terminal"]
                .into_iter()
                .map(|name| {
                    Ok((
                        name.into(),
                        read_proof(&stage.join("metadata-reserve").join(name), 16 * 1024, false)?.0,
                    ))
                })
                .collect::<Result<_, ApiError>>()
                .unwrap(),
            journal_identity: journal_identity.clone(),
            binding: transaction::Binding {
                git: slot_files::DirectoryProof::capture(&git).unwrap(),
                metadata,
                source_digest: source_fingerprint(
                    &scan_source(&project.root, &project.manifest, &roots).unwrap(),
                )
                .unwrap(),
                roots,
            },
            current: current.manifest,
            target: target.manifest,
            live_data_digest: current.manifest_digest,
            target_snapshot_digest: target.manifest_digest,
            slots,
        };
        let mut plan_file = files::create(&stage_fd, "plan.json").unwrap();
        plan_file
            .write_all(&serde_json::to_vec_pretty(&plan).unwrap())
            .unwrap();
        plan_file.sync_all().unwrap();
        let journal = transaction::Journal {
            schema_version: 1,
            operation: "dataRestore".into(),
            project_id: info.id,
            task_id: task.task_id,
            phase: "prepared".into(),
            plan_digest: plan.preview_plan_digest.clone(),
            operation_identity: plan.operation_identity.clone(),
            registration_plan,
            transaction_plan: Some(
                read_proof(&stage.join("plan.json"), MAX_METADATA, false)
                    .unwrap()
                    .0,
            ),
            journal_identity,
        };
        journal_file
            .write_all(&serde_json::to_vec_pretty(&journal).unwrap())
            .unwrap();
        journal_file.sync_all().unwrap();
        drop((stage_fd, journal_file, plan_file));
        let operation = guard::publish_prepared(paths.workspace_root(), &stage, &journal).unwrap();
        drop(snapshot_lease);
        drop(lease);
        Self {
            _temporary: temporary,
            paths,
            store,
            tasks,
            project,
            operation,
            journal,
            plan,
            _stop: sender,
            stop,
        }
    }
    async fn execute(
        &mut self,
        direction: DataRestoreDirection,
        fault: u32,
    ) -> Result<Value, ApiError> {
        let state = guard::inspect(self.paths.workspace_root(), &self.plan.project_id);
        assert!(state.issues.is_empty(), "{state:?}");
        let pending = &state.pending[0];
        let permit = guard::validate_recovery(
            self.paths.workspace_root(),
            &self.plan.project_id,
            &pending.task_id,
            &pending.plan_digest,
            &pending.journal_digest,
        )
        .unwrap();
        let (task, lease) = self
            .tasks
            .create_data_restore_recovery_operation(
                &self.plan.project_id,
                "gitDataRestoreContinue",
                &permit,
            )
            .unwrap();
        let (_cancel_sender, cancel) = watch::channel(false);
        let mut access = self
            .store
            .data_restore_recovery_access(&self.plan.project_id, lease, permit)
            .unwrap()
            .lock(self.stop.clone(), cancel)
            .await
            .unwrap();
        let result = transaction::execute(
            self.paths.workspace_root(),
            &mut access,
            &mut self.journal,
            &self.plan,
            direction,
            &mut || Ok(()),
            &mut Faults::new(fault),
            &self.tasks,
            &task.task_id,
        );
        drop(access);
        self.tasks.finish(&task.task_id, result.clone()).unwrap();
        result
    }
    async fn reopen(&mut self) {
        self.tasks.shutdown().await;
        self.store.shutdown().await;
        self.store = Arc::new(ProjectStore::new(self.paths.workspace_root().into()).unwrap());
        self.tasks = BuildManager::new(
            self.paths.clone(),
            self.store.clone(),
            "http://127.0.0.1:9".into(),
        )
        .unwrap();
    }
    async fn finish(self) {
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
    fn live_value(&self) -> String {
        let connection =
            rusqlite::Connection::open(self.project.root.join(&self.project.manifest.database))
                .unwrap();
        connection
            .query_row("SELECT value FROM disposable_notes", [], |row| row.get(0))
            .unwrap()
    }
}

#[tokio::test]
async fn real_five_slot_transaction_publishes_exact_epoch_and_sealed_terminal_without_touching_another_project(
) {
    let mut fixture = Prepared::new().await;
    let other = fixture
        .store
        .create(crate::CreateProjectInput {
            name: "Disposable independent data B".into(),
            description: None,
        })
        .await
        .unwrap();
    let other_root = fixture.store.project(&other.id).unwrap().root;
    let other_identity = PersistedIdentity::capture(&other_root).unwrap();
    let other_database = fs::read(other_root.join("data/main.sqlite")).unwrap();
    let result = fixture
        .execute(DataRestoreDirection::Continue, 0)
        .await
        .unwrap();
    assert_eq!(result["operationOutcome"], "committed");
    assert_eq!(result["pending"], false);
    assert_eq!(fixture.live_value(), "after");
    assert_eq!(
        fs::read(fixture.project.root.join("data/attachments/after.txt")).unwrap(),
        b"target attachment"
    );
    assert!(fixture.operation.join("slots/database/before").exists());
    assert!(fs::metadata(fixture.operation.join("terminal.json"))
        .unwrap()
        .permissions()
        .readonly());
    let state = guard::inspect(fixture.paths.workspace_root(), &fixture.plan.project_id);
    assert!(state.issues.is_empty(), "{state:?}");
    assert!(state.pending.is_empty());
    assert_eq!(state.terminal, vec![fixture.plan.task_id.clone()]);
    assert!(other_identity.matches_path(&other_root).unwrap());
    assert_eq!(
        fs::read(other_root.join("data/main.sqlite")).unwrap(),
        other_database
    );
    fixture.finish().await;
}

#[tokio::test]
async fn interrupted_slots_survive_a_new_manager_and_preserve_the_original_undo_choice() {
    let mut fixture = Prepared::new().await;
    let error = fixture
        .execute(DataRestoreDirection::Continue, u32::MAX)
        .await
        .unwrap_err();
    assert_eq!(error.message, "before-epoch");
    assert_eq!(fixture.live_value(), "after");
    let before = transaction::status(
        fixture.paths.workspace_root(),
        &fixture.operation,
        &fixture.plan.project_id,
        &fixture.plan.task_id,
        &fixture.stop,
    );
    assert!(before.issues.is_empty(), "{before:?}");
    assert!(before.can_continue && before.can_undo);
    fixture.reopen().await;
    let result = fixture
        .execute(DataRestoreDirection::Undo, 0)
        .await
        .unwrap();
    assert_eq!(result["dataState"], "undone");
    assert_eq!(fixture.live_value(), "before");
    assert_eq!(
        fs::read(fixture.project.root.join("data/attachments/before.txt")).unwrap(),
        b"original attachment"
    );
    fixture.finish().await;
}

#[tokio::test]
async fn an_actual_live_reader_blocks_the_move_without_discarding_evidence_and_release_allows_retry(
) {
    let mut fixture = Prepared::new().await;
    let live = fixture
        .project
        .root
        .join(&fixture.project.manifest.database);
    let reader = OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(&live)
        .unwrap();
    let original = PersistedIdentity::capture(&live).unwrap();
    let original_bytes = fs::read(&live).unwrap();
    assert!(fixture
        .execute(DataRestoreDirection::Continue, 0)
        .await
        .is_err());
    assert!(original.matches_path(&live).unwrap());
    // A new RW SQLite read could create WAL/SHM while recovery owns the exact
    // five-slot set. Compare all original bytes here without opening SQLite.
    assert_eq!(fs::read(&live).unwrap(), original_bytes);
    assert!(fixture.operation.join("slots/database/after").exists());
    drop(reader);
    fixture.reopen().await;
    fixture
        .execute(DataRestoreDirection::Continue, 0)
        .await
        .unwrap();
    assert_eq!(fixture.live_value(), "after");
    fixture.finish().await;
}

#[test]
fn native_directory_proofs_use_full_identity_readonly_and_preserve_original_hardlink_admission() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-data-directory-proof-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let proof = slot_files::DirectoryProof::capture(&root).unwrap();
    let value = serde_json::to_value(&proof).unwrap();
    assert!(value.get("mode").is_none());
    assert_eq!(value["readOnly"], false);
    assert!(value["identity"]["windowsFileId"].is_array());
    let mut foreign = proof.clone();
    foreign.identity.windows_file_id.as_mut().unwrap()[15] ^= 1;
    assert!(!proof.matches(&foreign));
    let file = root.join("original");
    fs::write(&file, b"retained hardlink bytes").unwrap();
    fs::hard_link(&file, root.join("external-link")).unwrap();
    assert!(slot_files::EntryProof::capture(&file, false, &mut || Ok(())).is_err());
    assert_eq!(fs::read(&file).unwrap(), b"retained hardlink bytes");
}

#[test]
fn metadata_attempts_enforce_original_128_and_sixteen_kib_limits_without_deleting_any_candidate() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-data-metadata-budget-")
        .tempdir()
        .unwrap();
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
    assert_eq!(
        transaction::metadata_usage(&root).unwrap(),
        (128, 130 * 16 * 1024)
    );
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

#[tokio::test]
async fn an_unexpected_occupied_slot_is_retained_and_never_relabelled_as_a_completed_restore() {
    let mut fixture = Prepared::new().await;
    let foreign = fixture.operation.join("slots/database/before");
    fs::write(&foreign, b"foreign occupied original slot").unwrap();
    let identity = PersistedIdentity::capture(&foreign).unwrap();
    assert!(fixture
        .execute(DataRestoreDirection::Continue, 0)
        .await
        .is_err());
    assert!(identity.matches_path(&foreign).unwrap());
    assert_eq!(
        fs::read(&foreign).unwrap(),
        b"foreign occupied original slot"
    );
    assert!(!fixture.operation.join("terminal.json").exists());
    assert_eq!(fixture.live_value(), "before");
    let status = transaction::status(
        fixture.paths.workspace_root(),
        &fixture.operation,
        &fixture.plan.project_id,
        &fixture.plan.task_id,
        &fixture.stop,
    );
    assert!(!status.issues.is_empty());
    assert!(!status.can_continue && !status.can_undo);
    fixture.finish().await;
}

#[test]
fn exclusive_native_moves_preserve_readonly_full_ids_and_never_overwrite_a_destination() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-data-exclusive-move-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let source = root.join("original");
    let destination = root.join("destination");
    fs::write(&source, b"original readonly data").unwrap();
    native::set_read_only(&source, true).unwrap();
    let original = PersistedIdentity::capture(&source).unwrap();
    fs::write(&destination, b"foreign destination").unwrap();
    assert!(slot_files::move_exclusive_with(&source, &destination, || Ok(())).is_err());
    assert!(original.matches_path(&source).unwrap());
    assert_eq!(fs::read(&destination).unwrap(), b"foreign destination");
    fs::remove_file(&destination).unwrap();
    slot_files::move_exclusive_with(&source, &destination, || Ok(())).unwrap();
    assert!(original.matches_path(&destination).unwrap());
    assert!(fs::metadata(&destination).unwrap().permissions().readonly());
    native::set_read_only(&destination, false).unwrap();
}
