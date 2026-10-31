use super::*;
use std::os::windows::fs::OpenOptionsExt;

const LIVE: &str = "app/disposable-source.txt";
struct Fixture {
    _temporary: TempDir,
    store: Arc<ProjectStore>,
    project: ProjectHandle,
    root: PathBuf,
    journal: RestoreJournal,
}
impl Fixture {
    async fn new(read_only: bool) -> Self {
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let created = store
            .create(crate::CreateProjectInput {
                name: "Disposable source exchange Windows".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&created.id).unwrap();
        fs::write(project.root.join(LIVE), b"original source").unwrap();
        if read_only {
            native::set_read_only(&project.root.join(LIVE), true).unwrap();
        }
        let root = paths
            .workspace_root()
            .join(".platform/project-state/source-restore")
            .join(&created.id)
            .join("task_disposable_source_io");
        private_dir(&root.join("slots")).unwrap();
        let scan = scan_source(
            &project.root,
            &project.manifest,
            &included_roots(&project.manifest),
        )
        .unwrap();
        assert!(scan.issues.is_empty() && scan.unclassified.is_empty());
        let before = scan
            .files
            .iter()
            .map(|(path, file)| (path.clone(), descriptor(file)))
            .collect::<BTreeMap<_, _>>();
        let mut target = before
            .iter()
            .map(|(path, descriptor)| {
                (
                    path.clone(),
                    PlannedFile {
                        oid: "a".repeat(40),
                        descriptor: descriptor.clone(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let bytes = b"target source";
        let wanted = RestoreDescriptor {
            sha256: sha256(bytes),
            size: bytes.len() as u64,
            executable: false,
        };
        target.get_mut(LIVE).unwrap().descriptor = wanted.clone();
        let change = RestoreChange {
            path: LIVE.into(),
            kind: "modify".into(),
            before: Some(before[LIVE].clone()),
            after: Some(wanted),
        };
        let binding = Binding {
            root_identity: identity(&project.root).unwrap(),
            manifest_sha256: before["project.json"].sha256.clone(),
            repository_digest: "b".repeat(64),
            index: None,
            head: "c".repeat(40),
            branch: "refs/heads/main".into(),
            branch_ref: None,
            metadata_digest: "d".repeat(64),
        };
        let plan = RestorePlan {
            policy: RESTORE_POLICY.into(),
            project_id: created.id,
            snapshot_digest: "e".repeat(64),
            target_commit: "f".repeat(40),
            binding,
            before,
            target,
            changes: vec![change],
        };
        let before = stamp(&project.root.join(LIVE)).unwrap();
        let candidate = super::super::slot(&root, 0);
        let mut file = create_file(&candidate, false).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let entry = RestoreEntry {
            path: LIVE.into(),
            before,
            after: stamp(&candidate).unwrap(),
        };
        let journal = new_journal(
            &root,
            "task_disposable_source_io",
            plan_digest(&plan).unwrap(),
            plan,
            vec![entry],
            ExecutableInfo {
                path: root.join("fixture-tool-never-created.exe"),
                version: "native-file-fixture; never execute".into(),
            },
        )
        .unwrap();
        write_journal(&root, &journal).unwrap();
        Self {
            _temporary: temporary,
            store,
            project,
            root,
            journal,
        }
    }
    fn live(&self) -> PathBuf {
        self.project.root.join(LIVE)
    }
    fn slot(&self) -> PathBuf {
        super::super::slot(&self.root, 0)
    }
    fn run(&mut self, direction: RestoreAction, stop: Option<Point>) -> Result<(), ApiError> {
        self.journal.phase = if direction == RestoreAction::Undo {
            "undoing"
        } else {
            "applying"
        }
        .into();
        write_journal(&self.root, &self.journal)?;
        move_entry_with(
            Move {
                project: &self.project,
                root: &self.root,
                index: 0,
                direction,
                live: &self.live(),
                held: &self.slot(),
            },
            &mut self.journal,
            |point| {
                if Some(point) == stop {
                    Err(ApiError::new(
                        "DISPOSABLE_INTERRUPTION",
                        format!("{point:?}"),
                    ))
                } else {
                    Ok(())
                }
            },
        )
    }
    fn reopen(&mut self) {
        self.journal =
            load_journal(&self.root, &self.project.manifest.id, &self.journal.task_id).unwrap();
    }
    async fn finish(self) {
        for path in [self.live(), self.slot(), self.root.join(CAPTURE)] {
            if let Ok(metadata) = fs::symlink_metadata(&path) {
                if metadata.is_file() && !platform_native::filesystem::is_reparse(&metadata) {
                    native::set_read_only(&path, false).unwrap();
                }
            }
        }
        self.store.shutdown().await;
    }
}

#[tokio::test]
async fn real_exchange_and_reverse_preserve_original_full_ids_and_read_only_state() {
    let mut fixture = Fixture::new(true).await;
    let before = fixture.journal.entries[0].before.clone();
    let after = fixture.journal.entries[0].after.clone();
    fixture.run(RestoreAction::Continue, None).unwrap();
    assert_eq!(stamp(&fixture.live()).unwrap(), after);
    assert_eq!(stamp(&fixture.slot()).unwrap(), before);
    assert!(!fixture.live().metadata().unwrap().permissions().readonly());
    fixture.run(RestoreAction::Undo, None).unwrap();
    assert_eq!(stamp(&fixture.live()).unwrap(), before);
    assert_eq!(stamp(&fixture.slot()).unwrap(), after);
    assert!(fixture.live().metadata().unwrap().permissions().readonly());
    assert!(absent(&fixture.root.join(CAPTURE)).unwrap());
    fixture.finish().await;
}

#[tokio::test]
async fn every_real_three_rename_boundary_reopens_and_keeps_both_recovery_choices() {
    for direction in [RestoreAction::Continue, RestoreAction::Undo] {
        for point in [
            Point::Prepared,
            Point::OldHeld,
            Point::NewVisible,
            Point::Exchanged,
        ] {
            for wanted in [RestoreAction::Continue, RestoreAction::Undo] {
                let mut fixture = Fixture::new(false).await;
                if direction == RestoreAction::Undo {
                    fixture.run(RestoreAction::Continue, None).unwrap();
                }
                let error = fixture.run(direction, Some(point)).unwrap_err();
                assert_eq!(error.code, "DISPOSABLE_INTERRUPTION");
                fixture.reopen();
                let bytes = fs::read(fixture.root.join("journal.json")).unwrap();
                let changes =
                    classify_all(&fixture.project, &fixture.root, &fixture.journal).unwrap();
                verify_mixed_source_at(&fixture.project, &fixture.root, &fixture.journal, &changes)
                    .unwrap();
                let original = matches!(point, Point::Prepared | Point::OldHeld);
                assert_eq!(
                    changes[0].state,
                    if original == (direction == RestoreAction::Continue) {
                        "before"
                    } else {
                        "after"
                    }
                );
                assert_eq!(
                    fs::read(fixture.root.join("journal.json")).unwrap(),
                    bytes,
                    "pure inspection must never stabilize"
                );
                assert!(require_terminal_ready(&fixture.root, &fixture.journal).is_err());
                if point == Point::OldHeld {
                    assert!(!fixture.live().exists());
                }
                stabilize_pending(&fixture.project, &fixture.root, &mut fixture.journal).unwrap();
                let expected = if wanted == RestoreAction::Continue {
                    "after"
                } else {
                    "before"
                };
                if classify_entry(
                    &fixture.project,
                    &fixture.root,
                    0,
                    &fixture.journal.entries[0],
                )
                .unwrap()
                    != expected
                {
                    fixture.run(wanted, None).unwrap();
                }
                let expected = if wanted == RestoreAction::Continue {
                    &fixture.journal.entries[0].after
                } else {
                    &fixture.journal.entries[0].before
                };
                assert_eq!(&stamp(&fixture.live()).unwrap(), expected);
                let stable = fs::read(fixture.live()).unwrap();
                stabilize_pending(&fixture.project, &fixture.root, &mut fixture.journal).unwrap();
                assert_eq!(
                    fs::read(fixture.live()).unwrap(),
                    stable,
                    "idle receipt must not exchange twice"
                );
                require_terminal_ready(&fixture.root, &fixture.journal).unwrap();
                fixture.finish().await;
            }
        }
    }
}

#[tokio::test]
async fn foreign_live_or_unrelated_source_change_is_preserved_before_stabilization() {
    for foreign_live in [false, true] {
        let mut fixture = Fixture::new(false).await;
        fixture
            .run(RestoreAction::Continue, Some(Point::OldHeld))
            .unwrap_err();
        fixture.reopen();
        let foreign = if foreign_live {
            fixture.live()
        } else {
            fixture
                .project
                .root
                .join("app/concurrent-foreign-source.txt")
        };
        fs::write(&foreign, b"foreign source must remain").unwrap();
        let before = fs::read(fixture.root.join("journal.json")).unwrap();
        assert!(stabilize_pending(&fixture.project, &fixture.root, &mut fixture.journal).is_err());
        assert_eq!(fs::read(&foreign).unwrap(), b"foreign source must remain");
        assert_eq!(fs::read(fixture.root.join("journal.json")).unwrap(), before);
        assert_eq!(
            stamp(&fixture.root.join(CAPTURE)).unwrap(),
            fixture.journal.entries[0].before
        );
        if !foreign_live {
            assert!(!fixture.live().exists());
        }
        fixture.finish().await;
    }
}

#[tokio::test]
async fn actual_reader_denies_move_until_release_and_retry_uses_original_identity() {
    let mut fixture = Fixture::new(false).await;
    let reader = fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(fixture.live())
        .unwrap();
    assert!(fixture.run(RestoreAction::Continue, None).is_err());
    assert_eq!(
        stamp(&fixture.live()).unwrap(),
        fixture.journal.entries[0].before
    );
    assert!(absent(&fixture.root.join(CAPTURE)).unwrap());
    fixture.reopen();
    drop(reader);
    stabilize_pending(&fixture.project, &fixture.root, &mut fixture.journal).unwrap();
    fixture.run(RestoreAction::Continue, None).unwrap();
    assert_eq!(
        stamp(&fixture.live()).unwrap(),
        fixture.journal.entries[0].after
    );
    fixture.finish().await;
}

#[tokio::test]
async fn replacement_operation_root_and_unbound_capture_do_not_gain_authority() {
    let fixture = Fixture::new(false).await;
    fs::write(fixture.root.join(CAPTURE), b"unbound foreign capture").unwrap();
    assert!(verify_journal_root(&fixture.root, &fixture.journal).is_err());
    assert!(require_terminal_ready(&fixture.root, &fixture.journal).is_err());
    assert_eq!(
        fs::read(fixture.root.join(CAPTURE)).unwrap(),
        b"unbound foreign capture"
    );
    fs::remove_file(fixture.root.join(CAPTURE)).unwrap();
    let old = fixture.root.with_extension("original-disposable");
    fs::rename(&fixture.root, &old).unwrap();
    native::create_private_directory(&fixture.root).unwrap();
    fs::write(fixture.root.join("foreign"), b"replacement root remains").unwrap();
    assert!(verify_journal_root(&fixture.root, &fixture.journal).is_err());
    assert_eq!(
        fs::read(fixture.root.join("foreign")).unwrap(),
        b"replacement root remains"
    );
    assert!(old.join("journal.json").is_file());
    fixture.finish().await;
}

#[tokio::test]
async fn fixed_width_pending_control_keeps_every_phase_budget_exact() {
    let fixture = Fixture::new(false).await;
    for phase in [
        "prepared",
        "applying",
        "applied",
        "undoing",
        "completed",
        "undone",
        "not-applied",
    ] {
        let mut journal = fixture.journal.clone();
        journal.phase = phase.into();
        let size = serde_json::to_vec_pretty(&journal).unwrap().len();
        for kind in [0, 1, 2] {
            for index in [0, 9999, MAX_FILES * 2 - 1] {
                journal.pending_move = PendingMove {
                    kind,
                    entry: format!("{index:05}"),
                };
                assert_eq!(serde_json::to_vec_pretty(&journal).unwrap().len(), size);
            }
        }
    }
    let mut wrong = fixture.journal.clone();
    wrong.pending_move = PendingMove {
        kind: 0,
        entry: "00001".into(),
    };
    assert!(verify_journal_root(&fixture.root, &wrong).is_err());
    wrong = fixture.journal.clone();
    wrong.operation_identity.file_id[15] ^= 0x80;
    assert!(verify_journal_root(&fixture.root, &wrong).is_err());
    wrong = fixture.journal.clone();
    wrong.operation_identity.volume_serial ^= 1;
    assert!(verify_journal_root(&fixture.root, &wrong).is_err());
    fixture.finish().await;
}
