use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    data: PathBuf,
    workspace: PathBuf,
    id: String,
    slot: PathBuf,
    source: PathBuf,
    outside: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let data = root.join("disposable-project-delete-data");
        let workspace = data.join("workspace");
        let id = Uuid::new_v4().to_string();
        let slot = data.join("node-apps").join(&id);
        let source = workspace.join("project-sources/disposable-source");
        let outside = root.join("external-author-source");
        for dir in [&slot, &source, &outside] {
            fs::create_dir_all(dir).unwrap();
        }
        for name in [
            "configuration/credentials",
            "data",
            "cache",
            "logs",
            "runs",
            "generations",
            "future-private-state",
        ] {
            fs::create_dir_all(slot.join(name)).unwrap();
            fs::write(
                slot.join(name).join("sentinel"),
                format!("disposable {name}"),
            )
            .unwrap();
        }
        fs::create_dir(source.join(".git")).unwrap();
        fs::write(source.join(".git/HEAD"), b"uncommitted source version").unwrap();
        fs::write(source.join("edited-by-ai"), b"later source edits").unwrap();
        fs::write(outside.join("sentinel"), b"external source stays").unwrap();
        fs::write(
            slot.join("registration.json"),
            b"registration belongs to whole slot",
        )
        .unwrap();
        Self {
            _temporary: temporary,
            data,
            workspace,
            id,
            slot,
            source,
            outside,
        }
    }
    fn metadata(&self) -> Value {
        json!({"appId":self.id,"name":"Disposable complete deletion","ownership":"managed","sourcePath":self.source,
            "expectedRegistrationRevision":"a".repeat(64),"workspaceRoot":self.workspace,"ownedPaths":[self.slot,self.source],"preservedPaths":[],"externalOwnedPaths":[]})
    }
    fn journal(&self) -> NodeDeletionJournal {
        NodeDeletionJournal::create(&self.data, self.metadata()).unwrap()
    }
    fn assert_outside(&self) {
        assert_eq!(
            fs::read(self.outside.join("sentinel")).unwrap(),
            b"external source stays"
        );
    }
    fn finish(&self, mut journal: NodeDeletionJournal) {
        journal.stage_all().unwrap();
        journal.remove_staged_all().unwrap();
        journal.mark_completed().unwrap();
        journal.remove_terminal().unwrap();
        assert!(!self.slot.exists());
        assert!(!self.source.exists());
        assert!(NodeDeletionJournal::list(&self.data).unwrap().is_empty());
        self.assert_outside();
    }
}
fn interrupted() -> ApiError {
    ApiError::new("DISPOSABLE_INTERRUPTION", "Artificial interruption")
}

#[test]
fn complete_deletion_removes_entire_slot_source_git_edits_and_its_own_task() {
    let fixture = Fixture::new();
    let journal = fixture.journal();
    assert!(journal
        .record
        .targets
        .iter()
        .all(|target| !target.staging_directory.starts_with(&fixture.slot)));
    fixture.finish(journal);
}

#[test]
fn source_edits_after_admission_are_included_without_requiring_download_digest() {
    let fixture = Fixture::new();
    let journal = fixture.journal();
    fs::write(fixture.source.join("edited-by-ai"), b"new uncommitted edit").unwrap();
    fs::create_dir(fixture.source.join("node_modules")).unwrap();
    fs::write(
        fixture.source.join("node_modules/later-dependency"),
        b"new dependency",
    )
    .unwrap();
    fixture.finish(journal);
}

#[test]
fn every_rename_and_remove_crash_boundary_reconciles_the_exact_captured_directories() {
    for point in [
        JournalPoint::RenameIntent,
        JournalPoint::Renamed,
        JournalPoint::Staged,
        JournalPoint::RemoveIntent,
        JournalPoint::Removed,
    ] {
        for index in 0..2 {
            let fixture = Fixture::new();
            let mut journal = fixture.journal();
            let task = journal.task_id().to_owned();
            if matches!(point, JournalPoint::RemoveIntent | JournalPoint::Removed) {
                journal.stage_all().unwrap();
                for _ in 0..index {
                    journal.remove_next_staged().unwrap();
                }
                assert!(journal
                    .remove_next_with(|seen| if seen == point {
                        Err(interrupted())
                    } else {
                        Ok(())
                    })
                    .is_err());
            } else {
                for _ in 0..index {
                    journal.stage_next().unwrap();
                }
                assert!(journal
                    .stage_next_with(|seen| if seen == point {
                        Err(interrupted())
                    } else {
                        Ok(())
                    })
                    .is_err());
            }
            drop(journal);
            let loaded = NodeDeletionJournal::load(&fixture.data, &task).unwrap();
            fixture.finish(loaded);
        }
    }
}

#[test]
fn abandoning_before_namespace_moves_preserves_project_and_removes_the_plan() {
    let fixture = Fixture::new();
    let mut journal = fixture.journal();
    journal.mark_failed(&interrupted()).unwrap();
    assert!(journal.can_abandon().unwrap());
    journal.request_abandon().unwrap();
    assert!(journal.stage_next().is_err());
    journal.abandon().unwrap();
    journal.remove_terminal().unwrap();
    assert!(fixture.slot.exists());
    assert!(fixture.source.exists());
    assert!(NodeDeletionJournal::list(&fixture.data).unwrap().is_empty());
}

#[test]
fn cannot_abandon_after_whole_instance_has_been_captured() {
    let fixture = Fixture::new();
    let mut journal = fixture.journal();
    journal.stage_next().unwrap();
    assert!(!journal.can_abandon().unwrap());
    assert!(journal.abandon().is_err());
    fixture.finish(journal);
}

#[test]
fn a_replacement_at_a_removed_original_path_is_never_deleted_or_reported_complete() {
    let fixture = Fixture::new();
    let mut journal = fixture.journal();
    journal.stage_all().unwrap();
    fs::create_dir(&fixture.slot).unwrap();
    fs::write(fixture.slot.join("foreign"), b"foreign replacement").unwrap();
    assert!(journal.remove_next_staged().is_err());
    assert_eq!(
        fs::read(fixture.slot.join("foreign")).unwrap(),
        b"foreign replacement"
    );
    fixture.assert_outside();
}

#[test]
fn a_parent_symlink_or_outside_target_is_rejected_before_any_move() {
    let fixture = Fixture::new();
    let mut metadata = fixture.metadata();
    metadata["ownedPaths"] = json!([fixture.slot, fixture.outside]);
    assert!(NodeDeletionJournal::create(&fixture.data, metadata).is_err());
    assert!(fixture.slot.exists());
    fixture.assert_outside();
    let parent = fixture.source.parent().unwrap();
    let moved = parent.with_file_name("retained-original-parent");
    fs::rename(parent, &moved).unwrap();
    std::os::unix::fs::symlink(&fixture.outside, parent).unwrap();
    assert!(NodeDeletionJournal::create(&fixture.data, fixture.metadata()).is_err());
    fixture.assert_outside();
}

#[test]
fn nested_targets_and_old_formats_are_rejected_without_compatibility_replay() {
    let fixture = Fixture::new();
    let mut metadata = fixture.metadata();
    metadata["ownedPaths"] = json!([fixture.slot, fixture.slot.join("data")]);
    assert!(NodeDeletionJournal::create(&fixture.data, metadata).is_err());
    let journal = fixture.journal();
    let path = journal.directory.join("task.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["version"] = json!(2);
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(NodeDeletionJournal::load(&fixture.data, journal.task_id()).is_err());
    assert!(fixture.slot.exists());
}

#[test]
fn complete_plan_preflight_rejects_a_mounted_target_before_publication() {
    let fixture = Fixture::new();
    let result = NodeDeletionJournal::create_checked(
        &fixture.data,
        fixture.metadata(),
        |_| Ok(()),
        |parent, targets| preflight_targets(parent, targets, |path| Ok(path == fixture.source)),
    );
    assert_eq!(
        result.err().unwrap().code,
        "NODE_DELETION_TARGET_VOLUME_UNSUPPORTED"
    );
    assert!(!fixture.data.join("node-deletion-tasks").exists());
    assert!(fixture.slot.exists());
    fixture.assert_outside();
}

#[test]
fn symlinks_inside_owned_source_do_not_follow_external_author_files() {
    let fixture = Fixture::new();
    std::os::unix::fs::symlink(&fixture.outside, fixture.source.join("external-link")).unwrap();
    fixture.finish(fixture.journal());
}

#[test]
fn missing_directory_appearing_after_admission_requires_a_bound_ledger_refresh() {
    let fixture = Fixture::new();
    let ledger = fixture
        .data
        .join("node-project-ownership")
        .join(&fixture.id);
    fs::create_dir_all(&ledger).unwrap();
    let candidate = fixture
        .data
        .join("node-apps/.installations")
        .join(format!("install_{}", Uuid::new_v4().simple()));
    fs::create_dir_all(candidate.parent().unwrap()).unwrap();
    let mut metadata = fixture.metadata();
    metadata["ownedPaths"] = json!([fixture.slot, fixture.source, candidate, ledger]);
    metadata["ownedTaskIds"] = json!([]);
    let mut journal = NodeDeletionJournal::create(&fixture.data, metadata).unwrap();
    fs::create_dir(&candidate).unwrap();
    fs::write(
        candidate.join("partial-build"),
        b"failed or cancelled writer material",
    )
    .unwrap();
    assert!(journal.reconcile().is_err());
    journal
        .refresh_owned_paths(
            vec![
                fixture.slot.clone(),
                fixture.source.clone(),
                candidate.clone(),
                ledger.clone(),
            ],
            vec![],
            None,
            vec![],
        )
        .unwrap();
    fixture.finish(journal);
    assert!(!candidate.exists());
    assert!(!ledger.exists());
}

#[test]
fn writer_created_targets_are_added_only_under_the_original_ledger_identity() {
    let fixture = Fixture::new();
    let ledger = fixture
        .data
        .join("node-project-ownership")
        .join(&fixture.id);
    fs::create_dir_all(&ledger).unwrap();
    let mut metadata = fixture.metadata();
    metadata["ownedPaths"] = json!([fixture.slot, fixture.source, ledger]);
    metadata["ownedTaskIds"] = json!([]);
    let mut journal = NodeDeletionJournal::create(&fixture.data, metadata).unwrap();
    let extra = fixture
        .data
        .join("node-development-builds")
        .join(format!("task_{}", Uuid::new_v4().simple()));
    fs::create_dir_all(&extra).unwrap();
    fs::rename(&ledger, ledger.with_extension("retained-original")).unwrap();
    fs::create_dir(&ledger).unwrap();
    assert!(journal
        .refresh_owned_paths(
            vec![
                fixture.slot.clone(),
                fixture.source.clone(),
                extra.clone(),
                ledger
            ],
            vec![],
            None,
            vec![],
        )
        .is_err());
    assert!(fixture.slot.exists());
    assert!(extra.exists());
    fixture.assert_outside();
}

#[test]
fn staged_object_or_parent_replacement_never_authorizes_removal() {
    for case in ["object", "symlink", "parent"] {
        let fixture = Fixture::new();
        let mut journal = fixture.journal();
        journal.stage_all().unwrap();
        let target = &journal.record.targets[0];
        let staged = target.staged.clone();
        if case == "parent" {
            let parent = target.staging_directory.clone();
            fs::rename(&parent, parent.with_extension("retained-original")).unwrap();
            fs::create_dir(&parent).unwrap();
        } else {
            fs::rename(&staged, staged.with_extension("retained-original")).unwrap();
            if case == "symlink" {
                std::os::unix::fs::symlink(&fixture.outside, &staged).unwrap();
            } else {
                fs::create_dir(&staged).unwrap();
                fs::write(staged.join("replacement"), b"keep").unwrap();
            }
        }
        assert!(journal.remove_next_staged().is_err());
        assert!(journal.mark_completed().is_err());
        fixture.assert_outside();
    }
}

#[test]
fn staged_object_disappearance_without_remove_intent_never_claims_success() {
    let fixture = Fixture::new();
    let mut journal = fixture.journal();
    journal.stage_all().unwrap();
    let staged = journal.record.targets[0].staged.clone();
    fs::rename(&staged, staged.with_extension("retained-original")).unwrap();
    assert!(journal.reconcile().is_err());
    assert!(journal.mark_completed().is_err());
    fixture.assert_outside();
}

#[test]
fn stale_record_writer_and_terminal_state_tampering_are_rejected() {
    let fixture = Fixture::new();
    let mut first = fixture.journal();
    let mut stale = NodeDeletionJournal::load(&fixture.data, first.task_id()).unwrap();
    first.set_phase("stopping").unwrap();
    assert!(stale.set_phase("staging").is_err());
    assert!(fixture.slot.exists());
    let record = first.directory.join(RECORD_NAME);
    let mut value: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    value["phase"] = json!("deleted");
    fs::write(&record, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(NodeDeletionJournal::load(&fixture.data, first.task_id()).is_err());
    fixture.assert_outside();
}

#[test]
fn interrupted_empty_staging_cleanup_can_finish_and_unpublished_scaffolds_do_not_survive() {
    let fixture = Fixture::new();
    let mut journal = fixture.journal();
    let id = journal.task_id().to_owned();
    journal.stage_all().unwrap();
    journal.remove_staged_all().unwrap();
    journal.cleanup_staging().unwrap();
    drop(journal);
    fixture.finish(NodeDeletionJournal::load(&fixture.data, &id).unwrap());
    for point in [
        JournalPoint::TaskDirectoryCreated,
        JournalPoint::BeforeTaskPublication,
    ] {
        let fixture = Fixture::new();
        assert!(
            NodeDeletionJournal::create_with(&fixture.data, fixture.metadata(), |seen| {
                if seen == point {
                    Err(interrupted())
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        assert_eq!(
            fs::read_dir(fixture.data.join("node-deletion-tasks"))
                .unwrap()
                .count(),
            0
        );
        assert!(fixture.slot.exists());
    }
}

#[test]
fn unknown_unpublished_names_and_symlinks_are_rejected_without_following_them() {
    for link in [false, true] {
        let fixture = Fixture::new();
        let root = fixture.data.join("node-deletion-tasks");
        private_directory(&root, false).unwrap();
        let creating = root.join(format!(".creating-{}", Uuid::new_v4()));
        private_directory(&creating, true).unwrap();
        if link {
            std::os::unix::fs::symlink(
                fixture.outside.join("sentinel"),
                creating.join(RECORD_NAME),
            )
            .unwrap();
        } else {
            fs::write(creating.join("foreign"), b"keep unknown file").unwrap();
        }
        assert!(NodeDeletionJournal::list(&fixture.data).is_err());
        assert!(creating.exists());
        fixture.assert_outside();
    }
}

#[test]
fn published_but_unconfirmed_task_returns_the_original_identity_for_adoption() {
    let fixture = Fixture::new();
    let error = NodeDeletionJournal::create_with(&fixture.data, fixture.metadata(), |seen| {
        if seen == JournalPoint::TaskPublished {
            Err(interrupted())
        } else {
            Ok(())
        }
    })
    .err()
    .unwrap();
    assert_eq!(error.code, "NODE_DELETION_TASK_PUBLICATION_UNCONFIRMED");
    let task = error.details["taskId"].as_str().unwrap();
    let journal = NodeDeletionJournal::load(&fixture.data, task).unwrap();
    assert_eq!(journal.app_id(), fixture.id);
    fixture.finish(journal);
}

#[test]
fn unmoved_rename_intent_can_be_abandoned_without_leaving_task_scaffolding() {
    let fixture = Fixture::new();
    let mut journal = fixture.journal();
    assert!(journal
        .stage_next_with(|seen| if seen == JournalPoint::RenameIntent {
            Err(interrupted())
        } else {
            Ok(())
        })
        .is_err());
    let staging = journal.record.targets[0].staging_directory.clone();
    assert!(staging.exists());
    assert!(journal.can_abandon().unwrap());
    journal.request_abandon().unwrap();
    journal.abandon().unwrap();
    journal.remove_terminal().unwrap();
    assert!(!staging.exists());
    assert!(fixture.slot.exists());
    assert!(fixture.source.exists());
    fixture.assert_outside();
}

#[test]
fn a_symlink_at_the_staging_container_never_writes_outside_the_project() {
    let fixture = Fixture::new();
    let mut journal = fixture.journal();
    let staging = fixture.slot.parent().unwrap().join(STAGING_NAME);
    std::os::unix::fs::symlink(&fixture.outside, &staging).unwrap();
    assert!(journal.stage_next().is_err());
    assert!(fixture.slot.exists());
    assert_eq!(fs::read_dir(&fixture.outside).unwrap().count(), 1);
    fixture.assert_outside();
}

#[test]
fn preflight_checks_each_targets_own_parent_volume() {
    for mismatch in [false, true] {
        let fixture = Fixture::new();
        let result = NodeDeletionJournal::create_checked(
            &fixture.data,
            fixture.metadata(),
            |_| Ok(()),
            |parent, targets| {
                let mut simulated = targets.to_vec();
                simulated[1].identity.as_mut().unwrap().device += 1;
                if !mismatch {
                    simulated[1].parent_identity.as_mut().unwrap().device += 1;
                }
                preflight_targets(parent, &simulated, |_| Ok(false))
            },
        );
        if mismatch {
            assert_eq!(
                result.err().unwrap().code,
                "NODE_DELETION_TARGET_VOLUME_UNSUPPORTED"
            );
            assert!(fixture.slot.exists());
        } else {
            fixture.finish(result.unwrap());
        }
    }
}

#[test]
fn exact_owned_material_in_a_previous_workspace_is_deleted_while_other_sources_remain() {
    let fixture = Fixture::new();
    let previous = fixture
        .outside
        .with_file_name("disposable-prior-workspace")
        .join("project-sources/disposable-migrated-project");
    fs::create_dir_all(previous.join(".git")).unwrap();
    fs::write(previous.join(".git/HEAD"), b"disposable pre-migration copy").unwrap();
    let mut metadata = fixture.metadata();
    metadata["ownedPaths"] = json!([fixture.slot, fixture.source, previous]);
    metadata["externalOwnedPaths"] = json!([previous]);
    fixture.finish(NodeDeletionJournal::create(&fixture.data, metadata).unwrap());
    assert!(!previous.exists());
    fixture.assert_outside();
}

#[test]
fn only_task_created_empty_staging_containers_are_removed_and_cleanup_is_idempotent() {
    for preexisting in [false, true] {
        let fixture = Fixture::new();
        let container = fixture.source.parent().unwrap().join(STAGING_NAME);
        if preexisting {
            private_directory(&container, true).unwrap();
            fs::write(container.join("foreign-content"), b"preexisting owner").unwrap();
        }
        let mut journal = fixture.journal();
        journal.stage_all().unwrap();
        journal.remove_staged_all().unwrap();
        journal.mark_completed().unwrap();
        journal.cleanup_staging().unwrap();
        journal.remove_terminal().unwrap();
        journal.remove_terminal().unwrap();
        if preexisting {
            assert_eq!(
                fs::read(container.join("foreign-content")).unwrap(),
                b"preexisting owner"
            );
        } else {
            assert!(!container.exists());
        }
        fixture.assert_outside();
    }
}

#[test]
fn task_created_staging_container_replacement_is_not_removed() {
    let fixture = Fixture::new();
    let mut journal = fixture.journal();
    journal.stage_all().unwrap();
    journal.remove_staged_all().unwrap();
    let container = fixture.source.parent().unwrap().join(STAGING_NAME);
    fs::rename(&container, container.with_extension("retained-original")).unwrap();
    private_directory(&container, true).unwrap();
    assert!(journal.mark_completed().is_err());
    assert!(container.exists());
    fixture.assert_outside();
}

#[test]
fn uninstall_scope_tracks_current_identity_bound_payload_after_owner_namespace_moves() {
    let fixture = Fixture::new();
    let mut journal = fixture.journal();
    let record = journal.directory.join(RECORD_NAME);
    let bytes = fs::read(&record).unwrap();
    let before = local_cleanup_owned_paths(&fixture.data).unwrap();
    assert!(before.contains(&fixture.slot));
    assert!(before.contains(&fixture.source));
    assert!(before.contains(&record));
    assert_eq!(fs::read(&record).unwrap(), bytes);
    journal.stage_all().unwrap();
    let staged: Vec<_> = journal
        .record
        .targets
        .iter()
        .map(|target| target.staged.clone())
        .collect();
    let scopes = local_cleanup_owned_paths(&fixture.data).unwrap();
    assert!(!scopes.contains(&fixture.slot));
    for path in &staged {
        assert!(scopes.contains(path));
        assert!(path.exists());
    }
    assert!(scopes.contains(&journal.directory));
    fixture.finish(journal);
}

#[test]
fn uninstall_scope_rejects_replaced_payload_and_does_not_claim_foreign_staging_content() {
    let fixture = Fixture::new();
    let mut journal = fixture.journal();
    journal.stage_all().unwrap();
    let parent = journal.record.targets[1]
        .staging_directory
        .parent()
        .unwrap()
        .to_path_buf();
    fs::write(parent.join("foreign-sentinel"), b"keep unrelated content").unwrap();
    let paths = local_cleanup_owned_paths(&fixture.data).unwrap();
    assert!(!paths.contains(&parent));
    let payload = journal.record.targets[1].staged.clone();
    fs::rename(&payload, payload.with_extension("retained-original")).unwrap();
    fs::create_dir(&payload).unwrap();
    assert!(local_cleanup_owned_paths(&fixture.data).is_err());
    assert_eq!(
        fs::read(parent.join("foreign-sentinel")).unwrap(),
        b"keep unrelated content"
    );
    fixture.assert_outside();
}
