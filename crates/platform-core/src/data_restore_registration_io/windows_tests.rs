//! Actual file exchange over fresh disposable roots; no project code runs.
use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    binding: Binding,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-registration-exchange-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let control = root.join("control");
        let operation = root.join("operation");
        fs::create_dir(&control).unwrap();
        fs::create_dir(&operation).unwrap();
        let live = control.join("project.json");
        let slot = operation.join(APPLY_SLOT);
        fs::write(&live, b"original registration").unwrap();
        fs::write(&slot, b"next registration").unwrap();
        let binding = Binding {
            operation_identity: FileIdentity::capture(&operation).unwrap(),
            old: read_proof(&live, MAX_RECORD, false).unwrap().0,
            next: read_proof(&slot, MAX_RECORD, false).unwrap().0,
            live,
            operation,
            direction: DataRestoreDirection::Continue,
        };
        Self {
            _temporary: temporary,
            binding,
        }
    }
    fn before(&self) {
        assert!(self
            .binding
            .old
            .matches(&read_proof(&self.binding.live, MAX_RECORD, false).unwrap().0));
        assert!(self.binding.next.matches(
            &read_proof(&self.binding.slot(), MAX_RECORD, false)
                .unwrap()
                .0
        ));
    }
    fn after(&self) {
        assert!(self
            .binding
            .next
            .matches(&read_proof(&self.binding.live, MAX_RECORD, false).unwrap().0));
        assert!(self.binding.old.matches(
            &read_proof(&self.binding.slot(), MAX_RECORD, false)
                .unwrap()
                .0
        ));
    }
    fn interrupt(&self, point: Point) {
        assert!(exchange_with(&self.binding, &mut |observed, _| {
            if observed == point {
                Err(ApiError::new(
                    "DISPOSABLE_EXCHANGE_BREAK",
                    "Injected after a real native step",
                ))
            } else {
                Ok(())
            }
        })
        .is_err());
    }
}

#[test]
fn actual_file_exchange_preserves_both_original_full_ids_without_acl_repair() {
    let fixture = Fixture::new();
    assert!(native::verify_private(&File::open(&fixture.binding.live).unwrap()).is_err());
    exchange(&fixture.binding).unwrap();
    fixture.after();
    assert!(inspect_pending(&fixture.binding).unwrap().is_none());
    assert!(exchange(&fixture.binding).is_err());
    fixture.after();
    assert!(native::verify_private(&File::open(fixture.binding.slot()).unwrap()).is_err());
}

#[test]
fn every_real_move_and_journal_retirement_reopens_without_reversing_a_committed_pair() {
    for point in [
        Point::JournalReady,
        Point::OldHeld,
        Point::NewVisible,
        Point::Exchanged,
        Point::JournalRetired,
    ] {
        let fixture = Fixture::new();
        fixture.interrupt(point);
        let reopened = fixture.binding.clone();
        if point == Point::JournalRetired {
            assert!(inspect_pending(&reopened).unwrap().is_none());
            assert!(!stabilize_pending(&reopened).unwrap());
            fixture.after();
        } else {
            let mut before = fs::read_dir(&reopened.operation)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>();
            before.sort();
            let observed = inspect_pending(&reopened).unwrap().unwrap();
            assert_eq!(
                observed.committed(),
                matches!(point, Point::NewVisible | Point::Exchanged)
            );
            let mut after = fs::read_dir(&reopened.operation)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>();
            after.sort();
            assert_eq!(before, after, "inspection must not rename any file");
            assert!(stabilize_pending(&reopened).unwrap());
            if observed.committed() {
                fixture.after();
                assert!(exchange(&reopened).is_err());
            } else {
                fixture.before();
                exchange(&reopened).unwrap();
                fixture.after();
            }
        }
        assert!(!reopened.operation.join(JOURNAL).exists());
    }
}

#[test]
fn precommit_recovery_keeps_both_direction_choices_but_visible_commit_never_accepts_the_other() {
    for point in [Point::OldHeld, Point::NewVisible] {
        let fixture = Fixture::new();
        fixture.interrupt(point);
        let undo = fixture.binding.operation.join(UNDO_SLOT);
        fs::write(&undo, b"undo registration").unwrap();
        let mut reversed = fixture.binding.clone();
        reversed.direction = DataRestoreDirection::Undo;
        reversed.next = read_proof(&undo, MAX_RECORD, false).unwrap().0;
        assert!(inspect_pending(&reversed).is_err());
        stabilize_pending(&fixture.binding).unwrap();
        if point == Point::OldHeld {
            exchange(&reversed).unwrap();
            assert!(reversed
                .next
                .matches(&read_proof(&reversed.live, MAX_RECORD, false).unwrap().0));
            assert!(reversed
                .old
                .matches(&read_proof(&undo, MAX_RECORD, false).unwrap().0));
            assert!(fixture.binding.next.matches(
                &read_proof(&fixture.binding.slot(), MAX_RECORD, false)
                    .unwrap()
                    .0
            ));
        } else {
            assert!(exchange(&reversed).is_err());
            fixture.after();
        }
    }
}

#[test]
fn occupied_original_handle_retains_all_material_then_release_allows_exact_retry() {
    let fixture = Fixture::new();
    let reader = OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(&fixture.binding.live)
        .unwrap();
    assert!(exchange(&fixture.binding).is_err());
    assert_eq!(
        inspect_pending(&fixture.binding).unwrap(),
        Some(State::Ready)
    );
    fixture.before();
    drop(reader);
    stabilize_pending(&fixture.binding).unwrap();
    exchange(&fixture.binding).unwrap();
    fixture.after();
}

#[test]
fn a_real_journal_reader_cannot_be_dropped_or_treated_as_a_completed_exchange() {
    let fixture = Fixture::new();
    fixture.interrupt(Point::NewVisible);
    let path = fixture.binding.operation.join(JOURNAL);
    let bytes = fs::read(&path).unwrap();
    let reader = OpenOptions::new()
        .read(true)
        .share_mode(7)
        .open(&path)
        .unwrap();
    assert!(inspect_pending(&fixture.binding).is_err());
    assert!(stabilize_pending(&fixture.binding).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    drop(reader);
    stabilize_pending(&fixture.binding).unwrap();
    fixture.after();
    assert!(!path.exists());
}

#[test]
fn readonly_sources_keep_their_attributes_and_original_single_link_policy() {
    let mut fixture = Fixture::new();
    for path in [&fixture.binding.live, &fixture.binding.slot()] {
        native::set_read_only(path, true).unwrap();
    }
    fixture.binding.old = read_proof(&fixture.binding.live, MAX_RECORD, false)
        .unwrap()
        .0;
    fixture.binding.next = read_proof(&fixture.binding.slot(), MAX_RECORD, false)
        .unwrap()
        .0;
    exchange(&fixture.binding).unwrap();
    fixture.after();
    assert!(fs::metadata(&fixture.binding.live)
        .unwrap()
        .permissions()
        .readonly());
    assert!(fs::metadata(fixture.binding.slot())
        .unwrap()
        .permissions()
        .readonly());
    native::set_read_only(&fixture.binding.live, false).unwrap();
    native::set_read_only(&fixture.binding.slot(), false).unwrap();
    let second = Fixture::new();
    let external = second.binding.operation.join("retained-other-name");
    fs::hard_link(&second.binding.live, &external).unwrap();
    assert!(exchange(&second.binding).is_err());
    assert_eq!(fs::read(&external).unwrap(), b"original registration");
    assert!(!second.binding.operation.join(JOURNAL).exists());
}

#[test]
fn foreign_held_names_or_replaced_high_id_evidence_are_preserved_without_a_move() {
    let fixture = Fixture::new();
    let mut foreign = None;
    assert!(exchange_with(&fixture.binding, &mut |point, held| {
        if point == Point::JournalReady {
            fs::write(held, b"foreign held name")?;
            foreign = Some(held.to_path_buf());
        }
        Ok(())
    })
    .is_err());
    fixture.before();
    assert_eq!(fs::read(foreign.unwrap()).unwrap(), b"foreign held name");
    assert!(stabilize_pending(&fixture.binding).is_err());
    let second = Fixture::new();
    second.interrupt(Point::OldHeld);
    let mut changed_binding = second.binding.clone();
    changed_binding
        .old
        .identity
        .windows_file_id
        .as_mut()
        .unwrap()[15] ^= 1;
    assert!(inspect_pending(&changed_binding).is_err());
    assert!(stabilize_pending(&changed_binding).is_err());
    assert_eq!(
        inspect_pending(&second.binding).unwrap(),
        Some(State::OldHeld)
    );
}

#[test]
fn replaced_parent_or_journal_is_unknown_and_never_recaptured_as_authority() {
    let fixture = Fixture::new();
    fixture.interrupt(Point::OldHeld);
    let original = fixture
        .binding
        .operation
        .with_file_name("retained-original-operation");
    fs::rename(&fixture.binding.operation, &original).unwrap();
    fs::create_dir(&fixture.binding.operation).unwrap();
    fs::copy(
        original.join(JOURNAL),
        fixture.binding.operation.join(JOURNAL),
    )
    .unwrap();
    fs::write(
        fixture.binding.operation.join("foreign"),
        b"retain new operation",
    )
    .unwrap();
    assert!(stabilize_pending(&fixture.binding).is_err());
    assert_eq!(
        fs::read(fixture.binding.operation.join("foreign")).unwrap(),
        b"retain new operation"
    );
    let second = Fixture::new();
    second.interrupt(Point::NewVisible);
    let path = second.binding.operation.join(JOURNAL);
    fs::write(&path, b"unknown replacement journal").unwrap();
    assert!(inspect_pending(&second.binding).is_err());
    assert!(stabilize_pending(&second.binding).is_err());
    assert_eq!(fs::read(path).unwrap(), b"unknown replacement journal");
}

#[test]
fn exact_extra_metadata_is_measured_and_unknown_held_or_preparation_material_is_rejected() {
    let fixture = Fixture::new();
    fixture.interrupt(Point::OldHeld);
    let entries = metadata(&|direction| {
        let mut binding = fixture.binding.clone();
        binding.direction = direction;
        binding
    })
    .unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries
        .iter()
        .any(|(name, bytes)| name == JOURNAL && *bytes > 0));
    assert!(entries
        .iter()
        .any(|(name, bytes)| name.starts_with(".registration-held-")
            && *bytes == fixture.binding.old.bytes));
    let unknown = fixture.binding.operation.join(".registration-held-foreign");
    fs::write(&unknown, b"unbound bytes").unwrap();
    assert!(metadata(&|_| fixture.binding.clone()).is_err());
    assert_eq!(fs::read(&unknown).unwrap(), b"unbound bytes");
    fs::remove_file(unknown).unwrap();
    let preparation = fixture
        .binding
        .operation
        .join(format!(".registration-preparing-{}.json", Uuid::new_v4()));
    fs::write(&preparation, b"partial unknown preparation").unwrap();
    assert!(metadata(&|_| fixture.binding.clone()).is_err());
    assert_eq!(
        fs::read(preparation).unwrap(),
        b"partial unknown preparation"
    );
}

mod registration_gate {
    use super::*;
    use crate::{data_restore_guard as guard, CreateProjectInput, RuntimePaths};

    struct Registered {
        _temporary: tempfile::TempDir,
        paths: RuntimePaths,
        store: Arc<ProjectStore>,
        id: String,
        operation: PathBuf,
        operation_identity: FileIdentity,
        plan: DataRestoreRegistrationPlan,
        original: Vec<u8>,
    }
    impl Registered {
        async fn new() -> Self {
            let (temporary, paths) = RuntimePaths::for_test();
            let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
            let info = store
                .create(CreateProjectInput {
                    name: "Disposable registration exchange gate".into(),
                    description: None,
                })
                .await
                .unwrap();
            let project = store.project(&info.id).unwrap();
            let lease = store
                .reserve_transient_operation(&info.id, "prepare-disposable-registration")
                .unwrap();
            let freeze = Freeze::new(project.control.clone());
            let gate = project.operation_gate.clone().write_owned().await;
            store.close_database(&info.id).await.unwrap();
            let staging = paths
                .workspace_root()
                .join(".platform")
                .join(format!("disposable-registration-stage-{}", Uuid::new_v4()));
            fs::create_dir(&staging).unwrap();
            let candidate = staging.join("candidate.sqlite");
            let database = rusqlite::Connection::open(&candidate).unwrap();
            database.execute_batch("CREATE TABLE disposable_target(value TEXT); INSERT INTO disposable_target VALUES('target');").unwrap();
            drop(database);
            let original = fs::read(registration_path(paths.workspace_root(), &info.id)).unwrap();
            let plan = store
                .prepare_data_restore_registration(
                    &info.id,
                    &lease,
                    &gate,
                    DataRestoreRegistrationPreparation {
                        staging: &staging,
                        candidate_database: &candidate,
                        apply_epoch: &Uuid::new_v4().to_string(),
                        undo_epoch: &Uuid::new_v4().to_string(),
                    },
                )
                .unwrap();
            let task = format!("task_{}", Uuid::new_v4().simple());
            let operation = paths
                .workspace_root()
                .join(".platform/project-state/data-restore")
                .join(&info.id)
                .join(&task);
            let operation_identity = FileIdentity::capture(&staging).unwrap();
            let header = guard::DataRestoreJournalHeader {
                schema_version: 1,
                operation: "dataRestore".into(),
                project_id: info.id.clone(),
                task_id: task,
                phase: "prepared".into(),
                plan_digest: "a".repeat(64),
                operation_identity: operation_identity.clone(),
                registration_plan: plan.clone(),
                transaction_plan: None,
                journal_identity: None,
            };
            assert_eq!(
                guard::publish_prepared(paths.workspace_root(), &staging, &header).unwrap(),
                operation
            );
            let live = project.root.join(&project.manifest.database);
            fs::rename(&live, operation.join("retained-original-database.sqlite")).unwrap();
            fs::rename(operation.join("candidate.sqlite"), &live).unwrap();
            drop(gate);
            drop(freeze);
            drop(lease);
            Self {
                _temporary: temporary,
                paths,
                store,
                id: info.id,
                operation,
                operation_identity,
                plan,
                original,
            }
        }
        fn binding(&self) -> Binding {
            Binding {
                live: registration_path(self.paths.workspace_root(), &self.id),
                operation: self.operation.clone(),
                operation_identity: self.operation_identity.clone(),
                direction: DataRestoreDirection::Continue,
                old: self.plan.before.proof.clone(),
                next: self.plan.apply.proof.clone(),
            }
        }
        async fn access(&self) -> DataRestoreExclusiveAccess {
            let state = guard::inspect(self.paths.workspace_root(), &self.id);
            assert!(state.issues.is_empty(), "{state:?}");
            assert_eq!(state.pending.len(), 1);
            let pending = &state.pending[0];
            let permit = guard::validate_recovery(
                self.paths.workspace_root(),
                &self.id,
                &pending.task_id,
                &pending.plan_digest,
                &pending.journal_digest,
            )
            .unwrap();
            let lease = self
                .store
                .reserve_data_restore_recovery_task(
                    &self.id,
                    "task_disposable_exchange_retry",
                    &permit,
                )
                .unwrap();
            let access = self
                .store
                .data_restore_recovery_access(&self.id, lease, permit)
                .unwrap();
            let (_stop, stop) = watch::channel(false);
            let (_cancel, cancel) = watch::channel(false);
            access.lock(stop, cancel).await.unwrap()
        }
    }

    #[tokio::test]
    async fn current_header_permit_and_real_gate_recover_both_native_sides_of_commit() {
        for point in [Point::OldHeld, Point::NewVisible] {
            let fixture = Registered::new().await;
            let binding = fixture.binding();
            assert!(exchange_with(&binding, &mut |observed, _| {
                if observed == point {
                    Err(ApiError::new(
                        "DISPOSABLE_REGISTRATION_PAUSE",
                        "After actual native rename",
                    ))
                } else {
                    Ok(())
                }
            })
            .is_err());
            let expected = if point == Point::OldHeld {
                DataRestoreEpochState::Before
            } else {
                DataRestoreEpochState::Applied
            };
            assert_eq!(
                fixture
                    .plan
                    .visible_state_at(
                        fixture.paths.workspace_root(),
                        &fixture.id,
                        &fixture.operation
                    )
                    .unwrap(),
                expected
            );
            assert_eq!(
                inspect_pending(&binding).unwrap(),
                Some(if point == Point::OldHeld {
                    State::OldHeld
                } else {
                    State::NewVisible
                })
            );
            // The original guard/permit reads the pending state without moving
            // it. Only the later original freeze/write-gate owner stabilizes it.
            let state = guard::inspect(fixture.paths.workspace_root(), &fixture.id);
            assert!(state.issues.is_empty(), "{state:?}");
            assert_eq!(state.pending.len(), 1);
            assert_eq!(
                inspect_pending(&binding).unwrap(),
                Some(if point == Point::OldHeld {
                    State::OldHeld
                } else {
                    State::NewVisible
                })
            );
            let access = fixture.access().await;
            assert_eq!(access.epoch_state().unwrap(), expected);
            assert!(!fixture.operation.join(JOURNAL).exists());
            let committed = access
                .publish_epoch(DataRestoreDirection::Continue)
                .unwrap();
            assert_eq!(committed.already_committed, point == Point::NewVisible);
            assert_eq!(
                access.epoch_state().unwrap(),
                DataRestoreEpochState::Applied
            );
            let repeated = access
                .publish_epoch(DataRestoreDirection::Continue)
                .unwrap();
            assert!(repeated.already_committed);
            assert!(access.publish_epoch(DataRestoreDirection::Undo).is_err());
            assert_eq!(
                fs::read(fixture.operation.join(APPLY_SLOT)).unwrap(),
                fixture.original
            );
            assert!(!fixture.operation.join(JOURNAL).exists());
            drop(access);
            fixture.store.shutdown().await;
        }
    }

    #[tokio::test]
    async fn a_fresh_manager_recovers_the_exact_missing_live_registration_without_opening_normal_access(
    ) {
        let mut fixture = Registered::new().await;
        let binding = fixture.binding();
        assert!(exchange_with(&binding, &mut |point, _| {
            if point == Point::OldHeld {
                Err(ApiError::new(
                    "DISPOSABLE_REGISTRATION_PAUSE",
                    "After actual original registration capture",
                ))
            } else {
                Ok(())
            }
        })
        .is_err());
        fixture.store.shutdown().await;
        fixture.store = Arc::new(ProjectStore::new(fixture.paths.workspace_root().into()).unwrap());
        assert!(!binding.live.exists());
        assert_eq!(
            fixture.store.control(&fixture.id).err().unwrap().code,
            "PROJECT_REMOVED"
        );
        assert!(fixture.store.project(&fixture.id).is_err());
        assert_eq!(inspect_pending(&binding).unwrap(), Some(State::OldHeld));
        let access = fixture.access().await;
        assert_eq!(access.epoch_state().unwrap(), DataRestoreEpochState::Before);
        assert_eq!(fs::read(&binding.live).unwrap(), fixture.original);
        assert!(!fixture.operation.join(JOURNAL).exists());
        assert!(Arc::ptr_eq(
            &access.access.control,
            &fixture.store.control(&fixture.id).unwrap()
        ));
        let committed = access
            .publish_epoch(DataRestoreDirection::Continue)
            .unwrap();
        assert!(!committed.already_committed);
        assert_eq!(
            access.epoch_state().unwrap(),
            DataRestoreEpochState::Applied
        );
        assert!(
            access
                .publish_epoch(DataRestoreDirection::Continue)
                .unwrap()
                .already_committed
        );
        drop(access);
        fixture.store.shutdown().await;
    }

    #[tokio::test]
    async fn capacity_reserves_original_limits_without_creating_exchange_evidence() {
        let fixture = Registered::new().await;
        let before = fs::read(fixture.operation.join("journal.json")).unwrap();
        assert!(guard::reserve_registration_exchange(
            fixture.paths.workspace_root(),
            &fixture.id,
            &fixture.operation,
            u64::MAX,
            2
        )
        .is_err());
        assert!(guard::reserve_registration_exchange(
            fixture.paths.workspace_root(),
            &fixture.id,
            &fixture.operation,
            1,
            usize::MAX
        )
        .is_err());
        let permit = guard::reserve_registration_exchange(
            fixture.paths.workspace_root(),
            &fixture.id,
            &fixture.operation,
            MAX_INNER + fixture.plan.before.proof.bytes,
            2,
        )
        .unwrap();
        drop(permit);
        assert!(!fixture.operation.join(JOURNAL).exists());
        assert_eq!(
            fs::read(fixture.operation.join("journal.json")).unwrap(),
            before
        );
        fixture.store.shutdown().await;
    }
}
