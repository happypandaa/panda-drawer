use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    app_id: String,
    slot: PathBuf,
    source: PathBuf,
}
impl Fixture {
    fn new(managed: bool) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-platform-data");
        let source = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-author-source");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&source).unwrap();
        fs::write(
            source.join("author-sentinel"),
            b"preserve external author files",
        )
        .unwrap();
        let app_id = Uuid::new_v4().to_string();
        let slot = root.join("node-apps").join(&app_id);
        fs::create_dir_all(&slot).unwrap();
        for name in ["configuration", "data", "cache", "runs"] {
            fs::create_dir(slot.join(name)).unwrap();
            fs::write(
                slot.join(name).join("sentinel"),
                format!("disposable {name}"),
            )
            .unwrap();
        }
        if managed {
            fs::create_dir(slot.join("generations")).unwrap();
            fs::write(
                slot.join("generations/sentinel"),
                b"disposable installed program",
            )
            .unwrap();
        }
        fs::write(
            slot.join("registration.json"),
            b"registration is a caller-owned final step",
        )
        .unwrap();
        Self {
            _temporary: temporary,
            root,
            app_id,
            slot,
            source,
        }
    }
    fn journal(&self, managed: bool) -> NodeDeletionJournal {
        NodeDeletionJournal::create(
            &self.root,
            json!({
                "appId":self.app_id,"name":"Disposable journal test",
                "ownership":if managed { "managed" } else { "external" },
                "sourcePath":self.source,"expectedRegistrationRevision":"a".repeat(64),
                "preservedPaths":[self.source,self.slot.join("runs")]
            }),
        )
        .unwrap()
    }
    fn assert_preserved(&self) {
        assert_eq!(
            fs::read(self.source.join("author-sentinel")).unwrap(),
            b"preserve external author files"
        );
        assert_eq!(
            fs::read(self.slot.join("runs/sentinel")).unwrap(),
            b"disposable runs"
        );
        assert!(self.slot.join("registration.json").is_file());
    }
}
fn interrupted() -> ApiError {
    ApiError::new(
        "DISPOSABLE_INTERRUPTION",
        "Simulated interruption in a temporary journal",
    )
}

#[test]
fn failed_stop_can_be_abandoned_before_any_namespace_move_without_touching_sources() {
    let fixture = Fixture::new(false);
    let mut journal = fixture.journal(false);
    assert!(journal
        .directory
        .starts_with(fixture.root.join("node-deletion-tasks")));
    assert!(!journal.directory.starts_with(&fixture.slot));
    journal.set_phase("stopping").unwrap();
    journal
        .mark_failed(&ApiError::new(
            "TEST_STOP_FAILED",
            "Disposable stop failure",
        ))
        .unwrap();
    assert_eq!(journal.snapshot()["canRetry"], true);
    assert!(journal.can_abandon().unwrap());
    journal.request_abandon().unwrap();
    assert!(journal.stage_next().is_err());
    journal.abandon().unwrap();
    journal.abandon().unwrap();
    let loaded = NodeDeletionJournal::load(&fixture.root, journal.task_id()).unwrap();
    assert_eq!(loaded.phase(), "abandoned");
    assert_eq!(loaded.snapshot()["canRetry"], false);
    assert_eq!(loaded.snapshot()["canAbandon"], false);
    assert_eq!(NodeDeletionJournal::list(&fixture.root).unwrap().len(), 1);
    for name in ["configuration", "data", "cache"] {
        assert!(fixture.slot.join(name).join("sentinel").exists());
    }
    fixture.assert_preserved();
}

#[test]
fn legacy_partial_permanent_deletion_does_not_gain_abandonment_from_a_new_journal() {
    let fixture = Fixture::new(false);
    let mut metadata = fixture.journal(false).metadata().clone();
    metadata["legacyIrreversible"] = json!(true);
    let mut journal = NodeDeletionJournal::create(&fixture.root, metadata).unwrap();
    journal.mark_failed(&interrupted()).unwrap();
    assert_eq!(journal.snapshot()["renamedAny"], false);
    assert_eq!(journal.snapshot()["canAbandon"], false);
    assert_eq!(journal.snapshot()["canRetry"], true);
    assert!(!journal.can_abandon().unwrap());
    assert!(journal.request_abandon().is_err());
    assert!(journal.abandon().is_err());
    fixture.assert_preserved();
}

#[test]
fn all_managed_targets_stage_and_remove_separately_and_keep_registration_and_run_evidence() {
    let fixture = Fixture::new(true);
    let mut journal = fixture.journal(true);
    let before = journal.snapshot()["journalRevision"].as_u64().unwrap();
    journal.stage_all().unwrap();
    assert_eq!(journal.snapshot()["renamedAny"], true);
    assert!(!journal.can_abandon().unwrap());
    assert!(journal.abandon().is_err());
    assert_eq!(journal.snapshot()["remaining"].as_array().unwrap().len(), 4);
    for name in ["configuration", "data", "cache", "generations"] {
        assert!(!fixture.slot.join(name).exists());
    }
    fixture.assert_preserved();
    journal.remove_staged_all().unwrap();
    assert!(journal.snapshot()["remaining"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(journal.mark_completed().is_ok());
    assert_eq!(journal.phase(), "deleted");
    assert!(journal.snapshot()["journalRevision"].as_u64().unwrap() > before);
    let loaded = NodeDeletionJournal::load(&fixture.root, journal.task_id()).unwrap();
    assert!(loaded.is_terminal());
    assert_eq!(loaded.phase(), "deleted");
    fixture.assert_preserved();
}

#[test]
fn intent_without_move_is_reconciled_before_abandon_is_allowed() {
    let fixture = Fixture::new(false);
    let mut journal = fixture.journal(false);
    assert!(journal
        .stage_next_with(|point| if matches!(point, JournalPoint::RenameIntent) {
            Err(interrupted())
        } else {
            Ok(())
        })
        .is_err());
    let mut loaded = NodeDeletionJournal::load(&fixture.root, journal.task_id()).unwrap();
    assert_eq!(
        loaded.snapshot()["canAbandon"],
        false,
        "a durable intent is initially unresolved"
    );
    assert!(loaded.can_abandon().unwrap());
    loaded.abandon().unwrap();
    assert!(fixture.slot.join("configuration/sentinel").is_file());
    fixture.assert_preserved();
}

#[test]
fn crash_after_rename_but_before_record_update_is_irreversible_and_resumes_remaining_targets() {
    let fixture = Fixture::new(false);
    let mut journal = fixture.journal(false);
    assert!(journal
        .stage_next_with(|point| if matches!(point, JournalPoint::Renamed) {
            Err(interrupted())
        } else {
            Ok(())
        })
        .is_err());
    assert!(!fixture.slot.join("configuration").exists());
    let mut loaded = NodeDeletionJournal::load(&fixture.root, journal.task_id()).unwrap();
    assert_eq!(
        loaded.snapshot()["renamedAny"],
        false,
        "the disk row still has only intent"
    );
    assert!(!loaded.can_abandon().unwrap());
    assert_eq!(loaded.snapshot()["renamedAny"], true);
    loaded.mark_failed(&interrupted()).unwrap();
    assert_eq!(loaded.snapshot()["canRetry"], true);
    assert_eq!(loaded.snapshot()["canAbandon"], false);
    loaded.stage_all().unwrap();
    loaded.remove_staged_all().unwrap();
    loaded.mark_completed().unwrap();
    fixture.assert_preserved();
}

#[test]
fn removed_directory_before_record_update_is_reconciled_and_retried_without_touching_other_apps() {
    let fixture = Fixture::new(false);
    let another = fixture
        .root
        .join("node-apps")
        .join(Uuid::new_v4().to_string());
    fs::create_dir_all(another.join("data")).unwrap();
    fs::write(
        another.join("data/other-app-sentinel"),
        b"another application remains intact",
    )
    .unwrap();
    let mut journal = fixture.journal(false);
    journal.stage_all().unwrap();
    assert!(journal
        .remove_next_with(|point| if matches!(point, JournalPoint::Removed) {
            Err(interrupted())
        } else {
            Ok(())
        })
        .is_err());
    let mut loaded = NodeDeletionJournal::load(&fixture.root, journal.task_id()).unwrap();
    loaded.reconcile().unwrap();
    assert_eq!(loaded.record.targets[0].state, TargetState::Removed);
    loaded.remove_staged_all().unwrap();
    loaded.mark_completed().unwrap();
    assert_eq!(
        fs::read(another.join("data/other-app-sentinel")).unwrap(),
        b"another application remains intact"
    );
    fixture.assert_preserved();
}

#[test]
fn changed_source_before_the_first_rename_is_preserved_and_can_still_be_abandoned() {
    let fixture = Fixture::new(false);
    let mut journal = fixture.journal(false);
    fs::rename(
        fixture.slot.join("configuration"),
        fixture.slot.join("retained-configuration"),
    )
    .unwrap();
    fs::create_dir(fixture.slot.join("configuration")).unwrap();
    fs::write(
        fixture.slot.join("configuration/foreign-sentinel"),
        b"preserve replacement",
    )
    .unwrap();
    assert!(journal.stage_next().is_err());
    assert!(journal.can_abandon().unwrap());
    journal.abandon().unwrap();
    assert_eq!(
        fs::read(fixture.slot.join("configuration/foreign-sentinel")).unwrap(),
        b"preserve replacement"
    );
    assert!(fixture
        .slot
        .join("retained-configuration/sentinel")
        .is_file());
    fixture.assert_preserved();
}

#[test]
fn changed_staged_object_or_missing_volume_never_authorizes_removal_or_abandonment_of_unknown_intent(
) {
    let fixture = Fixture::new(false);
    let mut journal = fixture.journal(false);
    assert!(journal
        .stage_next_with(|point| if matches!(point, JournalPoint::Renamed) {
            Err(interrupted())
        } else {
            Ok(())
        })
        .is_err());
    let staging = journal.record.targets[0].staged.clone();
    let retained = staging.with_file_name("retained-original-configuration");
    fs::rename(&staging, &retained).unwrap();
    fs::create_dir(&staging).unwrap();
    fs::write(
        staging.join("foreign-sentinel"),
        b"foreign staging contents must survive",
    )
    .unwrap();
    let mut loaded = NodeDeletionJournal::load(&fixture.root, journal.task_id()).unwrap();
    assert!(loaded.can_abandon().is_err());
    assert!(loaded.remove_next_staged().is_err());
    assert_eq!(
        fs::read(staging.join("foreign-sentinel")).unwrap(),
        b"foreign staging contents must survive"
    );
    assert!(retained.join("sentinel").is_file());
    fixture.assert_preserved();
}

#[test]
fn pure_unmoved_plan_can_be_abandoned_when_its_instance_is_unavailable() {
    let fixture = Fixture::new(false);
    let mut journal = fixture.journal(false);
    fs::rename(
        &fixture.slot,
        fixture.slot.with_file_name("retained-disposable-instance"),
    )
    .unwrap();
    assert!(journal.stage_next().is_err());
    assert!(journal.can_abandon().unwrap());
    journal.abandon().unwrap();
    assert_eq!(journal.phase(), "abandoned");
    assert!(fixture
        .slot
        .with_file_name("retained-disposable-instance")
        .join("data/sentinel")
        .is_file());
    assert_eq!(
        fs::read(fixture.source.join("author-sentinel")).unwrap(),
        b"preserve external author files"
    );
}

#[test]
fn staged_directory_disappearing_before_any_remove_intent_is_not_reported_as_deleted() {
    let fixture = Fixture::new(false);
    let mut journal = fixture.journal(false);
    journal.stage_all().unwrap();
    let staged = journal.record.targets[0].staged.clone();
    let retained = fixture
        .root
        .join("retained-disposable-staging-outside-task");
    fs::rename(staged, &retained).unwrap();
    let mut loaded = NodeDeletionJournal::load(&fixture.root, journal.task_id()).unwrap();
    assert!(loaded.reconcile().is_err());
    assert!(loaded.mark_completed().is_err());
    assert!(!loaded.can_abandon().unwrap());
    assert!(retained.join("sentinel").is_file());
    fixture.assert_preserved();
}

#[test]
fn staging_parent_symlink_does_not_write_to_external_source_and_plan_can_be_abandoned() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new(false);
    let mut journal = fixture.journal(false);
    symlink(&fixture.source, fixture.slot.join(STAGING_NAME)).unwrap();
    assert!(journal.stage_next().is_err());
    assert!(journal.can_abandon().unwrap());
    journal.abandon().unwrap();
    assert_eq!(fs::read_dir(&fixture.source).unwrap().count(), 1);
    fixture.assert_preserved();
}

#[test]
fn a_directory_missing_from_the_plan_cannot_silently_appear_at_completion() {
    let fixture = Fixture::new(false);
    fs::remove_dir_all(fixture.slot.join("cache")).unwrap();
    let mut journal = fixture.journal(false);
    fs::create_dir(fixture.slot.join("cache")).unwrap();
    fs::write(
        fixture.slot.join("cache/late-sentinel"),
        b"late directory stays intact",
    )
    .unwrap();
    assert!(journal.stage_all().is_err());
    assert!(journal.mark_completed().is_err());
    assert!(journal.can_abandon().unwrap());
    journal.abandon().unwrap();
    assert_eq!(
        fs::read(fixture.slot.join("cache/late-sentinel")).unwrap(),
        b"late directory stays intact"
    );
    fixture.assert_preserved();
}

#[test]
fn interruption_after_empty_staging_cleanup_before_terminal_write_can_finish() {
    let fixture = Fixture::new(false);
    let mut journal = fixture.journal(false);
    journal.stage_all().unwrap();
    journal.remove_staged_all().unwrap();
    let staging = journal.record.targets[0].staging_directory.clone();
    journal.cleanup_staging().unwrap();
    assert!(!staging.exists());
    let mut loaded = NodeDeletionJournal::load(&fixture.root, journal.task_id()).unwrap();
    loaded.reconcile().unwrap();
    loaded.mark_completed().unwrap();
    assert_eq!(loaded.phase(), "deleted");
    fixture.assert_preserved();
}

#[test]
fn tampered_ranges_and_replaced_record_are_rejected_without_deleting_any_source() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new(false);
    let mut journal = fixture.journal(false);
    let path = journal.directory.join(RECORD_NAME);
    let original = fs::read(&path).unwrap();
    let mut value: Value = serde_json::from_slice(&original).unwrap();
    value["targets"][0]["original"] = json!(fixture.source);
    fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(NodeDeletionJournal::load(&fixture.root, journal.task_id()).is_err());
    assert!(journal.set_phase("stopping").is_err());
    fs::write(&path, &original).unwrap();
    let displaced = journal.directory.join("retained-old-task.json");
    fs::rename(&path, &displaced).unwrap();
    fs::write(&path, b"foreign journal replacement").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(journal.mark_failed(&interrupted()).is_err());
    assert_eq!(fs::read(path).unwrap(), b"foreign journal replacement");
    assert_eq!(fs::read(displaced).unwrap(), original);
    fixture.assert_preserved();
}

#[test]
fn source_symlinks_are_never_followed_and_external_source_is_not_a_target() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new(false);
    fs::rename(
        fixture.slot.join("data"),
        fixture.slot.join("retained-data"),
    )
    .unwrap();
    symlink(&fixture.source, fixture.slot.join("data")).unwrap();
    let metadata = json!({"appId":fixture.app_id,"name":"Disposable symlink test","ownership":"external","sourcePath":fixture.source,"expectedRegistrationRevision":"b".repeat(64),"preservedPaths":[fixture.source]});
    assert!(NodeDeletionJournal::create(&fixture.root, metadata).is_err());
    assert!(NodeDeletionJournal::list(&fixture.root).unwrap().is_empty());
    assert!(fixture.slot.join("retained-data/sentinel").is_file());
    fixture.assert_preserved();
}

#[test]
fn unpublished_empty_partial_and_complete_creating_evidence_is_kept_without_blocking_valid_tasks() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new(false);
    let normal = fixture.journal(false);
    let metadata = normal.metadata().clone();
    for stop in [
        JournalPoint::TaskDirectoryCreated,
        JournalPoint::BeforeTaskPublication,
    ] {
        assert!(
            NodeDeletionJournal::create_with(&fixture.root, metadata.clone(), |point| {
                if point == stop {
                    Err(interrupted())
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
    }
    let tasks = fixture.root.join("node-deletion-tasks");
    let creating = fs::read_dir(&tasks)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".creating-")
        })
        .collect::<Vec<_>>();
    assert_eq!(creating.len(), 2);
    let empty = creating
        .iter()
        .find(|path| !path.join(RECORD_NAME).exists())
        .unwrap();
    let temporary = empty.join(format!(".write-{}", Uuid::new_v4()));
    fs::write(&temporary, b"{\"partial-disposable-record\":").unwrap();
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600)).unwrap();
    let visible = NodeDeletionJournal::list(&fixture.root).unwrap();
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].task_id(), normal.task_id());
    assert_eq!(
        fs::read(temporary).unwrap(),
        b"{\"partial-disposable-record\":"
    );
    assert!(creating.iter().all(|path| path.is_dir()));
    for name in ["configuration", "data", "cache"] {
        assert!(fixture.slot.join(name).join("sentinel").is_file());
    }
    fixture.assert_preserved();
}

#[test]
fn published_task_with_unconfirmed_parent_flush_returns_identity_for_adoption_not_a_new_task() {
    let fixture = Fixture::new(false);
    let normal = fixture.journal(false);
    let metadata = normal.metadata().clone();
    let failure = NodeDeletionJournal::create_with(&fixture.root, metadata.clone(), |point| {
        if point == JournalPoint::TaskPublished {
            Err(interrupted())
        } else {
            Ok(())
        }
    })
    .err()
    .unwrap();
    assert_eq!(failure.code, "NODE_DELETION_TASK_PUBLICATION_UNCONFIRMED");
    assert_eq!(failure.details["publicationUnknown"], true);
    assert_eq!(failure.details["appId"], fixture.app_id);
    let task_id = failure.details["taskId"].as_str().unwrap();
    assert_ne!(task_id, normal.task_id());
    let adopted = NodeDeletionJournal::load(&fixture.root, task_id).unwrap();
    assert_eq!(adopted.metadata(), &metadata);
    assert_eq!(adopted.task_id(), task_id);
    assert_eq!(adopted.phase(), "waiting");
    assert_eq!(NodeDeletionJournal::list(&fixture.root).unwrap().len(), 2);
    fixture.assert_preserved();
}

#[test]
fn unknown_unpublished_names_or_symlinks_are_not_silently_adopted() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new(false);
    let _normal = fixture.journal(false);
    let root = fixture.root.join("node-deletion-tasks");
    let unknown = root.join(".creating-not-a-task-id");
    private_directory(&unknown, true).unwrap();
    assert!(NodeDeletionJournal::list(&fixture.root).is_err());
    fs::remove_dir(&unknown).unwrap();
    let linked = root.join(format!(".creating-{}", Uuid::new_v4()));
    symlink(&fixture.source, &linked).unwrap();
    assert!(NodeDeletionJournal::list(&fixture.root).is_err());
    assert!(fs::symlink_metadata(&linked)
        .unwrap()
        .file_type()
        .is_symlink());
    fixture.assert_preserved();
}

#[test]
fn simulated_cross_volume_or_known_mount_target_rejects_the_entire_plan_before_publication() {
    let fixture = Fixture::new(false);
    let normal = fixture.journal(false);
    for boundary in 0..3 {
        let failure = NodeDeletionJournal::create_checked(
            &fixture.root,
            normal.metadata().clone(),
            |_| Ok(()),
            |parent, targets| {
                let mut observed = targets.to_vec();
                assert_eq!(observed[0].name, "configuration");
                assert_eq!(observed[1].name, "data");
                match boundary {
                    0 => {
                        observed[1].identity.as_mut().unwrap().device =
                            parent.device.checked_add(1).unwrap()
                    }
                    1 => {
                        observed[1].identity.as_mut().unwrap().volume_uuid =
                            Some(Uuid::new_v4().to_string())
                    }
                    _ => (),
                }
                preflight_targets(parent, &observed, |path| {
                    Ok(boundary == 2 && path.file_name().unwrap() == "data")
                })
            },
        )
        .err()
        .unwrap();
        assert_eq!(failure.code, "NODE_DELETION_TARGET_VOLUME_UNSUPPORTED");
        let visible = NodeDeletionJournal::list(&fixture.root).unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].task_id(), normal.task_id());
        assert_eq!(
            fs::read_dir(fixture.root.join("node-deletion-tasks"))
                .unwrap()
                .count(),
            1
        );
        assert!(!fixture.slot.join(STAGING_NAME).exists());
        for name in ["configuration", "data", "cache"] {
            assert!(fixture.slot.join(name).join("sentinel").is_file());
        }
        fixture.assert_preserved();
    }
}

#[test]
fn registration_restore_binding_can_be_persisted_while_abandonment_is_requested() {
    let fixture = Fixture::new(false);
    let mut journal = fixture.journal(false);
    journal.request_abandon().unwrap();
    let binding = json!({"directoryIdentity":journal.record.journal_directory_identity,"originalIdentity":journal.record.instance_identity,"sha256":"d".repeat(64)});
    journal.bind_registration_removal(binding.clone()).unwrap();
    let mut restored = NodeDeletionJournal::load(&fixture.root, journal.task_id()).unwrap();
    assert!(restored.abandon_requested());
    assert_eq!(restored.metadata()["registrationRemoval"], binding);
    restored.abandon().unwrap();
    assert!(restored
        .bind_registration_removal(json!({"invalid":"terminal write"}))
        .is_err());
    assert_eq!(restored.metadata()["registrationRemoval"], binding);
    fixture.assert_preserved();
}

#[test]
fn terminal_deleted_record_cannot_claim_success_while_targets_are_pending() {
    let fixture = Fixture::new(false);
    let journal = fixture.journal(false);
    let path = journal.directory.join(RECORD_NAME);
    let mut record = serde_json::to_value(&journal.record).unwrap();
    record["phase"] = json!("deleted");
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(NodeDeletionJournal::load(&fixture.root, journal.task_id()).is_err());
    for name in ["configuration", "data", "cache"] {
        assert!(fixture.slot.join(name).join("sentinel").is_file());
    }
    fixture.assert_preserved();
}

#[test]
fn terminal_abandoned_record_rejects_move_history_unresolved_intent_and_legacy_irreversibility() {
    for state in [
        TargetState::RenameIntent,
        TargetState::Staged,
        TargetState::Removing,
        TargetState::Removed,
    ] {
        let fixture = Fixture::new(false);
        let mut journal = fixture.journal(false);
        journal.prepare_staging(0).unwrap();
        let mut record = serde_json::to_value(&journal.record).unwrap();
        record["phase"] = json!("abandoned");
        record["abandonRequested"] = json!(true);
        record["targets"][0]["state"] = serde_json::to_value(state).unwrap();
        record["renamedAny"] = json!(state != TargetState::RenameIntent);
        fs::write(
            journal.directory.join(RECORD_NAME),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        assert!(
            NodeDeletionJournal::load(&fixture.root, journal.task_id()).is_err(),
            "invalid abandoned target state {state:?}"
        );
        fixture.assert_preserved();
    }
    for legacy in [false, true] {
        let fixture = Fixture::new(false);
        let journal = fixture.journal(false);
        let mut record = serde_json::to_value(&journal.record).unwrap();
        record["phase"] = json!("abandoned");
        record["abandonRequested"] = json!(true);
        record["renamedAny"] = json!(!legacy);
        record["metadata"]["legacyIrreversible"] = json!(legacy);
        fs::write(
            journal.directory.join(RECORD_NAME),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        assert!(NodeDeletionJournal::load(&fixture.root, journal.task_id()).is_err());
        fixture.assert_preserved();
    }
}

#[test]
fn normal_terminal_load_does_not_follow_or_remove_later_recreated_target_paths() {
    for terminal in ["deleted", "abandoned"] {
        let fixture = Fixture::new(false);
        let mut journal = fixture.journal(false);
        if terminal == "deleted" {
            journal.stage_all().unwrap();
            journal.remove_staged_all().unwrap();
            journal.mark_completed().unwrap();
        } else {
            journal.abandon().unwrap();
            fs::rename(
                fixture.slot.join("configuration"),
                fixture.slot.join("retained-old-configuration"),
            )
            .unwrap();
        }
        fs::create_dir(fixture.slot.join("configuration")).unwrap();
        let sentinel = fixture.slot.join("configuration/later-instance-sentinel");
        fs::write(&sentinel, b"later files stay outside the terminal task").unwrap();
        let loaded = NodeDeletionJournal::load(&fixture.root, journal.task_id()).unwrap();
        assert_eq!(loaded.phase(), terminal);
        assert!(loaded.is_terminal());
        assert_eq!(
            fs::read(sentinel).unwrap(),
            b"later files stay outside the terminal task"
        );
        fixture.assert_preserved();
    }
}
