//! Disposable projects only. Filesystem fault tests use real macOS atomic moves;
//! no real user's repositories, data or tool preferences are read or changed.
use super::*;
use crate::tasks::TaskStatus;
use std::os::unix::fs::symlink;

struct Fixture {
    _temp: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    manager: Arc<ProjectGitManager>,
    project: ProjectHandle,
    root: PathBuf,
    journal: RestoreJournal,
    stop: watch::Sender<bool>,
    cancel: watch::Sender<bool>,
    context: Context,
    protected: BTreeMap<String, Vec<u8>>,
}
impl Fixture {
    async fn new() -> Self {
        let (temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().to_owned()).unwrap());
        let info = store
            .create(crate::CreateProjectInput {
                name: "Disposable source restore failure evidence".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&info.id).unwrap();
        let git = project.root.join(".git");
        private_dir(&git.join("refs/heads")).unwrap();
        for (path, bytes) in [
            ("HEAD", "ref: refs/heads/main\n".to_owned()),
            (
                "config",
                "[core]\nrepositoryformatversion = 0\nbare = false\n".into(),
            ),
            ("refs/heads/main", format!("{}\n", "a".repeat(40))),
            (
                "index",
                "synthetic unchanged index; never executed by Git".into(),
            ),
        ] {
            fs::write(git.join(path), bytes).unwrap();
        }
        fs::write(
            project.root.join("app/src/a-modify.txt"),
            b"before replacement\n",
        )
        .unwrap();
        fs::write(
            project.root.join("app/src/b-delete.txt"),
            b"before deletion\n",
        )
        .unwrap();
        fs::write(project.root.join("app/src/keep.txt"), b"untouched source\n").unwrap();
        // Existing business files and built output are outside the source contract.
        private_dir(&project.root.join("data")).unwrap();
        private_dir(&project.root.join("dist")).unwrap();
        fs::write(
            project.root.join("data/restore-test.sqlite"),
            b"business record\n",
        )
        .unwrap();
        fs::write(
            project.root.join("dist/restore-test.html"),
            b"running old output\n",
        )
        .unwrap();
        let protected = [
            ".git/HEAD",
            ".git/index",
            ".git/config",
            ".git/refs/heads/main",
            "data/restore-test.sqlite",
            "dist/restore-test.html",
            "project.json",
        ]
        .into_iter()
        .map(|path| (path.to_owned(), fs::read(project.root.join(path)).unwrap()))
        .collect();
        let scan = scan_source(
            &project.root,
            &project.manifest,
            &included_roots(&project.manifest),
        )
        .unwrap();
        assert!(scan.issues.is_empty() && scan.unclassified.is_empty() && scan.markers.is_empty());
        let before: BTreeMap<_, _> = scan
            .files
            .iter()
            .map(|(path, file)| (path.clone(), descriptor(file)))
            .collect();
        let mut target: BTreeMap<_, _> = before
            .iter()
            .map(|(path, descriptor)| {
                (
                    path.clone(),
                    PlannedFile {
                        oid: "b".repeat(40),
                        descriptor: descriptor.clone(),
                    },
                )
            })
            .collect();
        let replacements = BTreeMap::from([
            ("app/src/a-modify.txt", b"after replacement\n".as_slice()),
            (
                "app/src/new/c-added.txt",
                b"new historical source\n".as_slice(),
            ),
        ]);
        target.remove("app/src/b-delete.txt");
        for (path, bytes) in &replacements {
            target.insert(
                (*path).into(),
                PlannedFile {
                    oid: "c".repeat(40),
                    descriptor: RestoreDescriptor {
                        sha256: sha256(bytes),
                        size: bytes.len() as u64,
                        executable: false,
                    },
                },
            );
        }
        let mut changes = Vec::new();
        for path in before
            .keys()
            .chain(target.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
        {
            let old = before.get(&path).cloned();
            let new = target.get(&path).map(|value| value.descriptor.clone());
            if old != new {
                changes.push(RestoreChange {
                    path,
                    kind: if old.is_none() {
                        "add"
                    } else if new.is_none() {
                        "delete"
                    } else {
                        "modify"
                    }
                    .into(),
                    before: old,
                    after: new,
                });
            }
        }
        let plan = RestorePlan {
            policy: RESTORE_POLICY.into(),
            project_id: info.id.clone(),
            target_commit: "b".repeat(40),
            snapshot_digest: "c".repeat(64),
            binding: Binding {
                root_identity: identity(&project.root).unwrap(),
                manifest_sha256: sha256(&fs::read(project.root.join("project.json")).unwrap()),
                repository_digest: repository_digest(&git).unwrap(),
                index: index_fingerprint(&git.join("index")).unwrap(),
                head: "a".repeat(40),
                branch: "refs/heads/main".into(),
                branch_ref: index_fingerprint(&git.join("refs/heads/main")).unwrap(),
                metadata_digest: binding_metadata(&project, "refs/heads/main").unwrap(),
            },
            before,
            target,
            changes,
        };
        let tasks = BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into())
            .unwrap();
        let manager = ProjectGitManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            GitEnvironmentManager::new(paths.data_root().to_owned(), None),
        )
        .unwrap();
        let root = manager.restore_root(&info.id, "task_disposable_restore");
        private_dir(&root.join("slots")).unwrap();
        let mut entries = Vec::new();
        for (index, change) in plan.changes.iter().enumerate() {
            let before = stamp(&project.root.join(&change.path)).unwrap();
            let after = if change.after.is_some() {
                fs::write(slot(&root, index), replacements[change.path.as_str()]).unwrap();
                fs::set_permissions(slot(&root, index), fs::Permissions::from_mode(0o644)).unwrap();
                stamp(&slot(&root, index)).unwrap()
            } else {
                None
            };
            entries.push(RestoreEntry {
                path: change.path.clone(),
                before,
                after,
            });
        }
        let journal = RestoreJournal {
            schema_version: 1,
            operation: "sourceRestore".into(),
            project_id: info.id,
            task_id: "task_disposable_restore".into(),
            phase: "prepared".into(),
            plan_digest: plan_digest(&plan).unwrap(),
            plan,
            entries,
            git: ExecutableInfo {
                path: paths.data_root().join("never-executed-git"),
                version: "2.56.0".into(),
            },
        };
        write_journal(&root, &journal).unwrap();
        load_journal(&root, &journal.project_id, &journal.task_id).unwrap();
        let (stop, receiver) = watch::channel(false);
        let (cancel, cancelled) = watch::channel(false);
        let context = Context::new(receiver, cancelled).unwrap();
        Self {
            _temp: temp,
            paths,
            store,
            tasks,
            manager,
            project,
            root,
            journal,
            stop,
            cancel,
            context,
            protected,
        }
    }
    fn protected_unchanged(&self) {
        for (path, bytes) in &self.protected {
            assert_eq!(
                &fs::read(self.project.root.join(path)).unwrap(),
                bytes,
                "{path}"
            );
        }
    }
    async fn partial(&mut self) {
        let error = apply_entries(
            &self.project,
            &self.root,
            &mut self.journal,
            RestoreAction::Continue,
            &mut self.context,
            |index, after| {
                if index == 0 && after {
                    Err(interrupted())
                } else {
                    Ok(())
                }
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "PROJECT_GIT_INTERRUPTED");
        let states = classify_all(&self.project, &self.root, &self.journal).unwrap();
        assert_eq!(
            states
                .iter()
                .map(|value| value.state.as_str())
                .collect::<Vec<_>>(),
            ["after", "before", "before"]
        );
        assert_eq!(
            load_journal(&self.root, &self.journal.project_id, &self.journal.task_id)
                .unwrap()
                .phase,
            "applying"
        );
        self.protected_unchanged();
    }
    async fn recover(&self, action: RestoreAction) -> TaskRecord {
        let state = self
            .manager
            .source_restore_status(&self.journal.project_id, self.stop.subscribe())
            .await
            .unwrap();
        assert_eq!(state.state, "pending", "{state:?}");
        let op = &state.operations[0];
        assert!(op.can_continue && op.can_undo);
        let task = self
            .manager
            .source_restore_recover(
                &self.journal.project_id,
                GitSourceRestoreRecoverInput {
                    restore_task_id: op.restore_task_id.clone(),
                    expected_plan_digest: op.plan_digest.clone(),
                    expected_journal_digest: op.journal_digest.clone(),
                    action,
                },
                self.stop.subscribe(),
            )
            .await
            .unwrap();
        wait_task(&self.tasks, &task.task_id).await
    }
    async fn close(self) {
        self.manager.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
}
async fn wait_task(tasks: &BuildManager, id: &str) -> TaskRecord {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let task = tasks.task(id).unwrap();
            if task.status.is_final() {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn interrupted_real_swap_can_continue_without_git_or_changing_running_state() {
    let mut f = Fixture::new().await;
    f.partial().await;
    let done = f.recover(RestoreAction::Continue).await;
    assert_eq!(done.status, TaskStatus::Succeeded, "{done:?}");
    let result = done.result.unwrap();
    assert_eq!(result["state"], "completed");
    assert_eq!(result["pending"], false);
    assert_eq!(result["operationOutcome"], "committed");
    assert_eq!(
        fs::read(f.project.root.join("app/src/a-modify.txt")).unwrap(),
        b"after replacement\n"
    );
    assert!(!f.project.root.join("app/src/b-delete.txt").exists());
    assert_eq!(
        fs::read(f.project.root.join("app/src/new/c-added.txt")).unwrap(),
        b"new historical source\n"
    );
    assert_eq!(fs::read(slot(&f.root, 0)).unwrap(), b"before replacement\n");
    assert_eq!(fs::read(slot(&f.root, 1)).unwrap(), b"before deletion\n");
    assert!(
        guard::inspect(f.paths.workspace_root(), &f.journal.project_id)
            .check()
            .is_ok()
    );
    f.protected_unchanged();
    f.close().await;
}
#[tokio::test]
async fn interrupted_real_swap_can_undo_and_retains_target_evidence() {
    let mut f = Fixture::new().await;
    f.partial().await;
    let done = f.recover(RestoreAction::Undo).await;
    assert_eq!(done.status, TaskStatus::Succeeded, "{done:?}");
    assert_eq!(done.result.unwrap()["state"], "undone");
    assert_eq!(
        fs::read(f.project.root.join("app/src/a-modify.txt")).unwrap(),
        b"before replacement\n"
    );
    assert_eq!(
        fs::read(f.project.root.join("app/src/b-delete.txt")).unwrap(),
        b"before deletion\n"
    );
    assert!(!f.project.root.join("app/src/new/c-added.txt").exists());
    assert_eq!(fs::read(slot(&f.root, 0)).unwrap(), b"after replacement\n");
    assert_eq!(
        fs::read(slot(&f.root, 2)).unwrap(),
        b"new historical source\n"
    );
    f.protected_unchanged();
    f.close().await;
}
#[tokio::test]
async fn newly_created_git_lock_stops_before_first_live_move_and_is_never_removed() {
    let mut f = Fixture::new().await;
    let lock = f.project.root.join(".git/index.lock");
    let error = apply_entries(
        &f.project,
        &f.root,
        &mut f.journal,
        RestoreAction::Continue,
        &mut f.context,
        |index, after| {
            if index == 0 && !after {
                fs::write(&lock, b"external writer").unwrap();
            }
            Ok(())
        },
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "PROJECT_SOURCE_RESTORE_GIT_BUSY");
    assert_eq!(fs::read(&lock).unwrap(), b"external writer");
    assert!(classify_all(&f.project, &f.root, &f.journal)
        .unwrap()
        .iter()
        .all(|v| v.state == "before"));
    let status = f
        .manager
        .source_restore_status(&f.journal.project_id, f.stop.subscribe())
        .await
        .unwrap();
    assert_eq!(status.state, "blocked");
    assert!(!status.operations[0].can_continue);
    f.protected_unchanged();
    f.close().await;
}
#[tokio::test]
async fn external_head_or_index_change_after_swap_blocks_recovery_without_losing_either_content() {
    for relative in [".git/HEAD", ".git/index"] {
        let mut f = Fixture::new().await;
        let path = f.project.root.join(relative);
        let error = apply_entries(
            &f.project,
            &f.root,
            &mut f.journal,
            RestoreAction::Continue,
            &mut f.context,
            |index, after| {
                if index == 0 && after {
                    fs::write(&path, b"external git operation\n").unwrap();
                }
                Ok(())
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "PROJECT_SOURCE_RESTORE_CHANGED");
        let status = f
            .manager
            .source_restore_status(&f.journal.project_id, f.stop.subscribe())
            .await
            .unwrap();
        assert_eq!(status.state, "blocked");
        assert!(!status.operations[0].can_continue && !status.operations[0].can_undo);
        assert_eq!(fs::read(&path).unwrap(), b"external git operation\n");
        assert_eq!(fs::read(slot(&f.root, 0)).unwrap(), b"before replacement\n");
        assert_eq!(
            fs::read(f.project.root.join("app/src/a-modify.txt")).unwrap(),
            b"after replacement\n"
        );
        assert_eq!(
            fs::read(f.project.root.join("app/src/b-delete.txt")).unwrap(),
            b"before deletion\n"
        );
        f.close().await;
    }
}
#[tokio::test]
async fn external_replacement_at_swap_boundary_is_preserved_as_third_state() {
    let f = Fixture::new().await;
    let live = f.project.root.join(&f.journal.entries[0].path);
    assert_eq!(
        classify_entry(&f.project, &f.root, 0, &f.journal.entries[0]).unwrap(),
        "before"
    );
    // A third-party writer changes the pathname after inspection and before rename.
    let original = f.root.join("external-displaced-original");
    fs::rename(&live, &original).unwrap();
    fs::write(&live, b"third party bytes\n").unwrap();
    move_preserving(&slot(&f.root, 0), &live, true).unwrap();
    assert_eq!(
        classify_entry(&f.project, &f.root, 0, &f.journal.entries[0]).unwrap(),
        "unknown"
    );
    assert_eq!(fs::read(slot(&f.root, 0)).unwrap(), b"third party bytes\n");
    assert_eq!(fs::read(original).unwrap(), b"before replacement\n");
    let status = f
        .manager
        .source_restore_status(&f.journal.project_id, f.stop.subscribe())
        .await
        .unwrap();
    assert_eq!(status.state, "blocked");
    assert_eq!(status.operations[0].changes[0].state, "unknown");
    f.protected_unchanged();
    f.close().await;
}
#[tokio::test]
async fn additions_never_replace_a_collision_and_unsafe_slots_are_not_followed() {
    let f = Fixture::new().await;
    let live = f.project.root.join(&f.journal.entries[2].path);
    ensure_live_parent(&f.project.root, &f.journal.entries[2].path).unwrap();
    fs::write(&live, b"user created collision").unwrap();
    assert!(move_preserving(&slot(&f.root, 2), &live, false).is_err());
    assert_eq!(fs::read(&live).unwrap(), b"user created collision");
    assert_eq!(
        fs::read(slot(&f.root, 2)).unwrap(),
        b"new historical source\n"
    );
    let held = slot(&f.root, 0);
    fs::rename(&held, f.root.join("candidate-kept")).unwrap();
    symlink(f.project.root.join("data/restore-test.sqlite"), &held).unwrap();
    assert!(classify_all(&f.project, &f.root, &f.journal).is_err());
    f.protected_unchanged();
    f.close().await;
}
#[tokio::test]
async fn terminal_rename_then_sync_failure_is_acknowledged_without_phantom_pending() {
    for phase in ["completed", "undone", "not-applied"] {
        let mut f = Fixture::new().await;
        if phase == "completed" {
            apply_entries(
                &f.project,
                &f.root,
                &mut f.journal,
                RestoreAction::Continue,
                &mut f.context,
                |_, _| Ok(()),
            )
            .await
            .unwrap();
        }
        let warning = persist_terminal(&f.root, &mut f.journal, phase, |root, journal| {
            write_journal(root, journal)?;
            Err(restore_error(
                "TEST_FSYNC_FAILED",
                "injected final directory sync failure",
            ))
        })
        .unwrap()
        .unwrap();
        assert_eq!(
            warning.code,
            "PROJECT_SOURCE_RESTORE_DURABILITY_UNCONFIRMED"
        );
        assert!(
            guard::inspect(f.paths.workspace_root(), &f.journal.project_id)
                .check()
                .is_ok()
        );
        assert_eq!(
            load_journal(&f.root, &f.journal.project_id, &f.journal.task_id)
                .unwrap()
                .phase,
            phase
        );
        f.protected_unchanged();
        f.close().await;
    }
}
#[tokio::test]
async fn terminal_failure_before_rename_retains_recoverable_applied_evidence() {
    let mut f = Fixture::new().await;
    apply_entries(
        &f.project,
        &f.root,
        &mut f.journal,
        RestoreAction::Continue,
        &mut f.context,
        |_, _| Ok(()),
    )
    .await
    .unwrap();
    assert!(
        persist_terminal(&f.root, &mut f.journal, "completed", |_, _| Err(
            restore_error("TEST_WRITE_FAILED", "injected before rename")
        ))
        .is_err()
    );
    assert_eq!(
        load_journal(&f.root, &f.journal.project_id, &f.journal.task_id)
            .unwrap()
            .phase,
        "applied"
    );
    let state = f
        .manager
        .source_restore_status(&f.journal.project_id, f.stop.subscribe())
        .await
        .unwrap();
    assert_eq!(state.state, "pending");
    assert!(state.operations[0].can_continue && state.operations[0].can_undo);
    let done = f.recover(RestoreAction::Undo).await;
    assert_eq!(done.status, TaskStatus::Succeeded, "{done:?}");
    f.protected_unchanged();
    f.close().await;
}
#[tokio::test]
async fn phase_record_failure_after_prepared_publication_reports_retained_pending() {
    let mut f = Fixture::new().await;
    let pending = guard::inspect(f.paths.workspace_root(), &f.journal.project_id)
        .pending
        .remove(0);
    let permit = guard::validate_recovery(
        f.paths.workspace_root(),
        &f.journal.project_id,
        &pending.task_id,
        &pending.plan_digest,
        &pending.journal_digest,
    )
    .unwrap();
    let (task, lease) = f
        .tasks
        .create_source_restore_recovery_operation(
            &f.journal.project_id,
            "gitSourceRestoreContinue",
            &permit,
        )
        .unwrap();
    let record = f
        .paths
        .workspace_root()
        .join(".platform/tasks")
        .join(&task.task_id)
        .join("task.json");
    fs::remove_file(&record).unwrap();
    fs::create_dir(&record).unwrap();
    let error = f
        .manager
        .drive_restore(
            &f.project,
            &task.task_id,
            &mut f.journal,
            RestoreAction::Continue,
            "source-restore",
            &mut f.context,
        )
        .await
        .unwrap_err();
    assert_eq!(error.details["operationOutcome"], "unknown");
    assert_eq!(error.details["pending"], true);
    assert_eq!(error.details["restoreTaskId"], f.journal.task_id);
    assert_eq!(
        load_journal(&f.root, &f.journal.project_id, &f.journal.task_id)
            .unwrap()
            .phase,
        "prepared"
    );
    assert!(classify_all(&f.project, &f.root, &f.journal)
        .unwrap()
        .iter()
        .all(|v| v.state == "before"));
    fs::remove_dir(&record).unwrap();
    f.tasks.finish(&task.task_id, Err(error)).unwrap();
    drop(lease);
    f.protected_unchanged();
    f.close().await;
}
#[tokio::test]
async fn stop_and_user_cancel_preserve_partial_journal_for_explicit_recovery() {
    for user_cancel in [false, true] {
        let mut f = Fixture::new().await;
        let signal = if user_cancel {
            f.cancel.clone()
        } else {
            f.stop.clone()
        };
        let error = apply_entries(
            &f.project,
            &f.root,
            &mut f.journal,
            RestoreAction::Continue,
            &mut f.context,
            |index, after| {
                if index == 0 && after {
                    signal.send(true).unwrap();
                }
                Ok(())
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "PROJECT_GIT_INTERRUPTED");
        assert_eq!(
            classify_all(&f.project, &f.root, &f.journal).unwrap()[0].state,
            "after"
        );
        assert_eq!(
            load_journal(&f.root, &f.journal.project_id, &f.journal.task_id)
                .unwrap()
                .phase,
            "applying"
        );
        f.protected_unchanged();
        f.close().await;
    }
}
#[test]
fn exact_commit_inputs_and_cross_version_path_collisions_are_rejected() {
    assert!(validate_target(&"a".repeat(40)).is_ok());
    for value in ["HEAD", "main", "abc", &"A".repeat(40), &"a".repeat(64)] {
        assert!(validate_target(value).is_err());
    }
    for paths in [
        vec!["app/File.ts", "app/file.ts"],
        vec!["app/a", "app/a/b"],
        vec!["app/é", "app/e\u{301}"],
        vec!["app/a", "app/../secret"],
    ] {
        assert!(validate_collisions(paths.into_iter()).is_err());
    }
    assert!(serde_json::from_value::<GitSourceRestorePreviewInput>(
        json!({"targetCommit":"a".repeat(40),"checkout":true})
    )
    .is_err());
    assert!(serde_json::from_value::<GitSourceRestoreRecoverInput>(
        json!({"restoreTaskId":"task_x","action":"continue","expectedPlanDigest":"a".repeat(64)})
    )
    .is_err());
}
#[tokio::test]
async fn changed_journal_plan_and_unknown_source_block_recovery_before_moving() {
    let mut f = Fixture::new().await;
    f.journal
        .plan
        .target
        .get_mut("project.json")
        .unwrap()
        .descriptor
        .sha256 = "f".repeat(64);
    f.journal.plan_digest = plan_digest(&f.journal.plan).unwrap();
    write_journal(&f.root, &f.journal).unwrap();
    assert!(load_journal(&f.root, &f.journal.project_id, &f.journal.task_id).is_err());
    let status = f
        .manager
        .source_restore_status(&f.journal.project_id, f.stop.subscribe())
        .await
        .unwrap();
    assert_eq!(status.state, "blocked");
    assert_eq!(
        fs::read(f.project.root.join("app/src/a-modify.txt")).unwrap(),
        b"before replacement\n"
    );
    f.protected_unchanged();
    f.close().await;
    let f = Fixture::new().await;
    private_dir(&f.project.root.join("unknown-root")).unwrap();
    fs::write(
        f.project.root.join("unknown-root/file"),
        "new unknown scope",
    )
    .unwrap();
    let status = f
        .manager
        .source_restore_status(&f.journal.project_id, f.stop.subscribe())
        .await
        .unwrap();
    assert_eq!(status.state, "blocked");
    assert!(!status.operations[0].can_continue);
    f.protected_unchanged();
    f.close().await;
}

#[tokio::test]
async fn recovery_cancel_while_waiting_for_gate_keeps_original_pending_identity() {
    let f = Fixture::new().await;
    let pending = guard::inspect(f.paths.workspace_root(), &f.journal.project_id)
        .pending
        .remove(0);
    let gate = f.project.operation_gate.write().await;
    let task = f
        .manager
        .source_restore_recover(
            &f.journal.project_id,
            GitSourceRestoreRecoverInput {
                restore_task_id: pending.task_id.clone(),
                expected_plan_digest: pending.plan_digest,
                expected_journal_digest: pending.journal_digest,
                action: RestoreAction::Continue,
            },
            f.stop.subscribe(),
        )
        .await
        .unwrap();
    f.manager.cancel(&task.task_id).unwrap();
    let done = wait_task(&f.tasks, &task.task_id).await;
    assert_eq!(done.status, TaskStatus::Interrupted, "{done:?}");
    let error = done.error.unwrap();
    assert_eq!(error.details["operationOutcome"], "not_committed");
    assert_eq!(error.details["restoreTaskId"], pending.task_id);
    assert_eq!(error.details["pending"], true);
    assert!(!f.store.has_active_tasks());
    drop(gate);
    f.protected_unchanged();
    f.close().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to an explicit disposable official Git resource; no network"]
async fn official_git_source_materialization_streams_large_blob_and_more_than_one_pipe_of_requests()
{
    let resource = PathBuf::from(std::env::var_os("LOCAL_PROJECTS_TEST_GIT_ROOT").unwrap())
        .canonicalize()
        .unwrap();
    let evidence = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../.test-artifacts")
        .canonicalize()
        .unwrap();
    assert!(resource.starts_with(evidence));
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
    let created = store
        .create(crate::projects::CreateProjectInput {
            name: "Disposable raw source materialization".into(),
            description: None,
        })
        .await
        .unwrap();
    let project = store.project(&created.id).unwrap();
    let before_database = fs::read(project.root.join(&project.manifest.database)).unwrap();
    let operation = Arc::new(
        store
            .reserve_transient_operation(&created.id, "raw-source-fixture")
            .unwrap(),
    );
    let phase = Arc::new(project.operation_gate.clone().read_owned().await);
    let (_stop, receiver) = watch::channel(false);
    let (_cancel, cancellation) = watch::channel(false);
    let mut context = Context::new(receiver, cancellation).unwrap();
    context.bind(store.processes(), operation);
    context.phase(phase);
    let environment = GitEnvironmentManager::with_processes(
        paths.data_root().to_path_buf(),
        Some(resource),
        store.processes(),
    );
    let tools = context.resolve(&environment).await.unwrap();
    let template = context.root().join("empty-template");
    private_dir(&template).unwrap();
    let mut command = git_command(&tools, context.root(), None);
    command
        .args(["init", "--quiet", "--initial-branch=main", "--template"])
        .arg(&template)
        .arg(&project.root);
    context
        .run(command, vec![])
        .await
        .unwrap()
        .success()
        .unwrap();
    let payload = vec![b'p'; 9 * 1024 * 1024];
    let original = context.root().join("original-large-payload");
    fs::write(&original, &payload).unwrap();
    let mut command = git_command(&tools, context.root(), Some(&project));
    command
        .args(["hash-object", "--no-filters", "-w", "--"])
        .arg(&original);
    let large = one_oid(
        &context
            .run(command, vec![])
            .await
            .unwrap()
            .success()
            .unwrap(),
    )
    .unwrap();
    let mut command = git_command(&tools, context.root(), Some(&project));
    command.args(["hash-object", "--no-filters", "-w", "--stdin"]);
    let empty = one_oid(
        &context
            .run(command, vec![])
            .await
            .unwrap()
            .success()
            .unwrap(),
    )
    .unwrap();
    let mut blobs = vec![Blob {
        path: "app/src/large.bin".into(),
        oid: large,
        size: payload.len() as u64,
        executable: false,
    }];
    for index in 0..1700 {
        blobs.push(Blob {
            path: format!("app/src/small-{index:04}.txt"),
            oid: empty.clone(),
            size: 0,
            executable: index == 0,
        });
    }
    assert!(blobs.len() * 41 > 64 * 1024);
    let target = context.root().join("restore-target");
    private_dir(&target).unwrap();
    let materialized = materialize_blobs(&project, &tools, &mut context, &blobs, &target)
        .await
        .unwrap();
    assert_eq!(materialized.len(), 1701);
    assert_eq!(fs::read(target.join("app/src/large.bin")).unwrap(), payload);
    assert_eq!(
        materialized["app/src/large.bin"].descriptor.sha256,
        sha256(&payload)
    );
    assert!(
        fs::metadata(target.join("app/src/small-0000.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111
            != 0
    );
    assert_eq!(
        fs::read(project.root.join(&project.manifest.database)).unwrap(),
        before_database
    );
    assert!(!project.root.join("app/src/large.bin").exists());
    assert!(!project.root.join(".git/index").exists());
    drop(context);
    assert!(store.processes().shutdown().await.is_empty());
    assert!(!store.has_active_tasks());
    assert!(store.shutdown().await.is_empty());
}
