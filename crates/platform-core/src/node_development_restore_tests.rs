//! Real private Git and real atomic file moves, disposable source/data only.
use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    manager: NodeGit,
    scope: NodeRestoreScope,
    target: String,
    original: BTreeMap<String, Vec<u8>>,
}
impl Fixture {
    async fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let base = temporary.path().canonicalize().unwrap();
        let root = base.join("disposable-node-restore-source");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir(root.join("data")).unwrap();
        fs::create_dir(root.join("configuration")).unwrap();
        fs::write(
            root.join("local-app.json"),
            b"{\"id\":\"disposable.node.restore\"}\n",
        )
        .unwrap();
        fs::write(
            root.join("package.json"),
            b"{\"name\":\"disposable-node-restore\",\"version\":\"1.0.0\"}\n",
        )
        .unwrap();
        fs::write(root.join("src/a.mjs"), b"old-a\n").unwrap();
        fs::write(root.join("src/old-only.mjs"), b"old-only\n").unwrap();
        fs::write(
            root.join("data/disposable.sqlite"),
            b"retained artificial database",
        )
        .unwrap();
        fs::write(
            root.join("configuration/disposable.json"),
            b"retained artificial configuration",
        )
        .unwrap();
        let resources = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/desktop/src-tauri/resources/development-git")
            .canonicalize()
            .unwrap();
        let environment = GitEnvironmentManager::new(base.join("tool-settings"), Some(resources));
        let manager = NodeGit::new(environment);
        let status = manager.status(&root).await.unwrap();
        assert!(status.can_initialize, "{status:?}");
        manager
            .initialize(
                &root,
                GitInitInput {
                    expected_snapshot: status.snapshot_digest,
                    initial_branch: "main".into(),
                },
            )
            .await
            .unwrap();
        let target = save(&manager, &root, "original").await;
        fs::write(root.join("src/a.mjs"), b"new-a\n").unwrap();
        fs::remove_file(root.join("src/old-only.mjs")).unwrap();
        fs::write(root.join("src/new-only.mjs"), b"new-only\n").unwrap();
        save(&manager, &root, "current").await;
        let branch = ".git/refs/heads/main";
        let original = [
            ".git/HEAD",
            ".git/index",
            branch,
            "data/disposable.sqlite",
            "configuration/disposable.json",
            "local-app.json",
        ]
        .into_iter()
        .map(|path| (path.into(), fs::read(root.join(path)).unwrap()))
        .collect();
        let control = base.join("node-control/11111111-1111-4111-8111-111111111111");
        fs::create_dir_all(&control).unwrap();
        let scope = NodeRestoreScope {
            app_id: "11111111-1111-4111-8111-111111111111".into(),
            registration_digest: "a".repeat(64),
            control_path: control.join("source-restore.json"),
        };
        Self {
            _temporary: temporary,
            root,
            manager,
            scope,
            target,
            original,
        }
    }
    async fn input(&self) -> NodeRestoreApplyInput {
        let preview = self
            .manager
            .restore_preview(
                &self.root,
                &self.scope,
                NodeRestorePreviewInput {
                    target_commit: self.target.clone(),
                },
            )
            .await
            .unwrap();
        assert_eq!(preview["changes"].as_array().unwrap().len(), 3);
        NodeRestoreApplyInput {
            target_commit: self.target.clone(),
            expected_snapshot: preview["snapshotDigest"].as_str().unwrap().into(),
            expected_plan_digest: preview["planDigest"].as_str().unwrap().into(),
        }
    }
    fn recovery(&self, action: RestoreAction) -> NodeRestoreRecoverInput {
        let status = self
            .manager
            .restore_status(&self.root, &self.scope)
            .unwrap();
        assert_eq!(status["issues"], json!([]), "{status}");
        let record = &status["operations"][0];
        assert_eq!(record["canContinue"], true, "{status}");
        assert_eq!(record["canUndo"], true);
        NodeRestoreRecoverInput {
            operation_id: record["operationId"].as_str().unwrap().into(),
            expected_plan_digest: record["planDigest"].as_str().unwrap().into(),
            expected_journal_digest: record["journalDigest"].as_str().unwrap().into(),
            action,
        }
    }
    fn unchanged(&self) {
        for (path, bytes) in &self.original {
            assert_eq!(fs::read(self.root.join(path)).unwrap(), *bytes, "{path}");
        }
    }
    fn old(&self) {
        assert_eq!(fs::read(self.root.join("src/a.mjs")).unwrap(), b"old-a\n");
        assert!(self.root.join("src/old-only.mjs").is_file());
        assert!(!self.root.join("src/new-only.mjs").exists());
        self.unchanged();
    }
    fn current(&self) {
        assert_eq!(fs::read(self.root.join("src/a.mjs")).unwrap(), b"new-a\n");
        assert!(!self.root.join("src/old-only.mjs").exists());
        assert!(self.root.join("src/new-only.mjs").is_file());
        self.unchanged();
    }
}
async fn save(manager: &NodeGit, root: &Path, message: &str) -> String {
    let status = manager.status(root).await.unwrap();
    assert!(status.can_save, "{status:?}");
    let value = manager
        .save(
            root,
            GitSaveInput {
                expected_head: head_oid(&status).map(str::to_owned),
                expected_snapshot: status.snapshot_digest,
                message: message.into(),
                author: GitAuthor {
                    name: "Disposable Author".into(),
                    email: "disposable@example.invalid".into(),
                },
            },
        )
        .await
        .unwrap();
    value["commit"].as_str().unwrap().into()
}

#[tokio::test]
#[ignore = "explicit private Git, disposable source, real macOS atomic moves"]
async fn real_git_restore_changes_only_source_and_leaves_a_new_unsaved_worktree() {
    let fixture = Fixture::new().await;
    let input = fixture.input().await;
    let result = fixture
        .manager
        .restore_apply(&fixture.root, &fixture.scope, input)
        .await
        .unwrap();
    assert_eq!(result["state"], "completed");
    fixture.old();
    require_clear(&fixture.root).unwrap();
    let status = fixture.manager.status(&fixture.root).await.unwrap();
    assert!(status.can_save, "{status:?}");
    assert_eq!(status.changes.len(), 3);
    let mut marker = read_control(&fixture.scope.control_path).unwrap().unwrap();
    marker.phase = "pending".into();
    atomic_json(&fixture.scope.control_path, &marker).unwrap();
    require_control_clear(&fixture.scope.control_path).unwrap();
    assert_eq!(
        fixture
            .manager
            .restore_status(&fixture.root, &fixture.scope)
            .unwrap()["state"],
        "clear"
    );
    fs::remove_dir_all(fixture.scope.control_path.parent().unwrap()).unwrap();
    require_clear(&fixture.root).unwrap();
}

#[tokio::test]
#[ignore = "explicit private Git, exhaustive disposable per-file fault injection"]
async fn real_git_every_file_boundary_can_continue_or_undo_without_touching_git_or_data() {
    for index in 0..3 {
        for after in [false, true] {
            for action in [RestoreAction::Continue, RestoreAction::Undo] {
                let fixture = Fixture::new().await;
                let input = fixture.input().await;
                let result = fixture
                    .manager
                    .restore_apply_with(&fixture.root, &fixture.scope, input, |step, applied| {
                        if step == index && applied == after {
                            Err(issue(
                                "DISPOSABLE_RESTORE_INTERRUPT",
                                "artificial interruption",
                            ))
                        } else {
                            Ok(())
                        }
                    })
                    .await;
                assert_eq!(result.unwrap_err().code, "DISPOSABLE_RESTORE_INTERRUPT");
                assert!(require_clear(&fixture.root).is_err());
                fixture.unchanged();
                let recovery = fixture.recovery(action);
                fixture
                    .manager
                    .restore_recover(&fixture.root, &fixture.scope, recovery)
                    .await
                    .unwrap();
                if action == RestoreAction::Continue {
                    fixture.old();
                } else {
                    fixture.current();
                }
                require_clear(&fixture.root).unwrap();
            }
        }
    }
}

#[tokio::test]
#[ignore = "explicit private Git, disposable source and unknown-change evidence"]
async fn real_git_stale_preview_or_external_changes_stop_and_preserve_all_evidence() {
    let fixture = Fixture::new().await;
    let input = fixture.input().await;
    fs::write(fixture.root.join("src/a.mjs"), b"third party before apply").unwrap();
    assert!(fixture
        .manager
        .restore_apply(&fixture.root, &fixture.scope, input)
        .await
        .is_err());
    assert_eq!(
        fs::read(fixture.root.join("src/a.mjs")).unwrap(),
        b"third party before apply"
    );
    fixture.unchanged();
    let fixture = Fixture::new().await;
    let input = fixture.input().await;
    fixture
        .manager
        .restore_apply_with(&fixture.root, &fixture.scope, input, |_, after| {
            if after {
                Err(issue(
                    "DISPOSABLE_RESTORE_INTERRUPT",
                    "artificial interruption",
                ))
            } else {
                Ok(())
            }
        })
        .await
        .unwrap_err();
    let recovery = fixture.recovery(RestoreAction::Continue);
    fs::write(
        fixture.root.join("src/a.mjs"),
        b"unknown third-party source",
    )
    .unwrap();
    let error = fixture
        .manager
        .restore_recover(&fixture.root, &fixture.scope, recovery)
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_SOURCE_RESTORE_CHANGED");
    assert_eq!(
        fs::read(fixture.root.join("src/a.mjs")).unwrap(),
        b"unknown third-party source"
    );
    assert!(require_clear(&fixture.root).is_err());
    fixture.unchanged();
}

#[tokio::test]
#[ignore = "explicit private Git, disposable journals and stale recovery tokens"]
async fn real_git_recovery_checks_registration_identity_and_fresh_journal_digest() {
    let fixture = Fixture::new().await;
    let input = fixture.input().await;
    fixture
        .manager
        .restore_apply_with(&fixture.root, &fixture.scope, input, |_, after| {
            if after {
                Err(issue(
                    "DISPOSABLE_RESTORE_INTERRUPT",
                    "artificial interruption",
                ))
            } else {
                Ok(())
            }
        })
        .await
        .unwrap_err();
    let mut stale = fixture.recovery(RestoreAction::Undo);
    stale.expected_journal_digest = "b".repeat(64);
    assert!(fixture
        .manager
        .restore_recover(&fixture.root, &fixture.scope, stale)
        .await
        .is_err());
    let changed_scope = NodeRestoreScope {
        registration_digest: "c".repeat(64),
        ..fixture.scope.clone()
    };
    let recovery = fixture.recovery(RestoreAction::Continue);
    assert!(fixture
        .manager
        .restore_recover(&fixture.root, &changed_scope, recovery)
        .await
        .is_err());
    let recovery = fixture.recovery(RestoreAction::Undo);
    let retained = operation(&fixture.root, &recovery.operation_id).unwrap();
    let path = retained.join("journal.json");
    let original = fs::read(&path).unwrap();
    let (mut altered, _) = read_journal(&retained).unwrap();
    altered.entries[0].after.as_mut().unwrap().inode += 1;
    write_journal(&retained, &altered).unwrap();
    let status = fixture
        .manager
        .restore_status(&fixture.root, &fixture.scope)
        .unwrap();
    assert_eq!(status["operations"][0]["canContinue"], false);
    assert!(fixture
        .manager
        .restore_recover(&fixture.root, &fixture.scope, recovery)
        .await
        .is_err());
    fs::write(&path, &original).unwrap();
    let (mut altered, _) = read_journal(&retained).unwrap();
    altered.phase = "completed".into();
    write_journal(&retained, &altered).unwrap();
    assert!(require_control_clear(&fixture.scope.control_path).is_err());
    assert_eq!(
        fixture
            .manager
            .restore_status(&fixture.root, &fixture.scope)
            .unwrap()["state"],
        "needs-attention"
    );
    fs::write(&path, &original).unwrap();
    let recovery = fixture.recovery(RestoreAction::Undo);
    fixture
        .manager
        .restore_recover(&fixture.root, &fixture.scope, recovery)
        .await
        .unwrap();
    fixture.current();
}

#[tokio::test]
#[ignore = "explicit private Git, disposable removed repository and persistent instance control"]
async fn real_git_missing_repository_keeps_instance_blocked_until_original_evidence_returns() {
    let fixture = Fixture::new().await;
    let input = fixture.input().await;
    fixture
        .manager
        .restore_apply_with(&fixture.root, &fixture.scope, input, |_, after| {
            if after {
                Err(issue(
                    "DISPOSABLE_RESTORE_INTERRUPT",
                    "artificial interruption",
                ))
            } else {
                Ok(())
            }
        })
        .await
        .unwrap_err();
    let recovery = fixture.recovery(RestoreAction::Undo);
    let marker = fs::read(&fixture.scope.control_path).unwrap();
    let held_git = fixture
        .root
        .parent()
        .unwrap()
        .join("retained-disposable-git-evidence");
    fs::rename(fixture.root.join(".git"), &held_git).unwrap();
    assert!(require_control_clear(&fixture.scope.control_path).is_err());
    let status = fixture
        .manager
        .restore_status(&fixture.root, &fixture.scope)
        .unwrap();
    assert_eq!(status["state"], "needs-attention");
    assert!(!status["issues"].as_array().unwrap().is_empty());
    assert!(fixture
        .manager
        .restore_recover(&fixture.root, &fixture.scope, recovery)
        .await
        .is_err());
    assert_eq!(fs::read(&fixture.scope.control_path).unwrap(), marker);
    // No recovery path creates a replacement repository or discards evidence.
    assert!(held_git.join(RESTORE_ROOT).is_dir());
    assert!(!fixture.root.join(".git").exists());
    fs::rename(held_git, fixture.root.join(".git")).unwrap();
    let recovery = fixture.recovery(RestoreAction::Undo);
    fixture
        .manager
        .restore_recover(&fixture.root, &fixture.scope, recovery)
        .await
        .unwrap();
    fixture.current();
    require_control_clear(&fixture.scope.control_path).unwrap();
}

#[tokio::test]
#[ignore = "explicit private Git, disposable ignored file and manifest constraints"]
async fn real_git_ignored_local_files_and_manifest_differences_are_never_restored() {
    let fixture = Fixture::new().await;
    for invalid in ["HEAD", &fixture.target[..12], &"a".repeat(64)] {
        assert!(fixture
            .manager
            .restore_preview(
                &fixture.root,
                &fixture.scope,
                NodeRestorePreviewInput {
                    target_commit: invalid.into()
                }
            )
            .await
            .is_err());
    }
    fs::write(fixture.root.join(".gitignore"), b"src/old-only.mjs\n").unwrap();
    save(&fixture.manager, &fixture.root, "ignore a local file").await;
    fs::write(
        fixture.root.join("src/old-only.mjs"),
        b"retained ignored local source",
    )
    .unwrap();
    let error = fixture
        .manager
        .restore_preview(
            &fixture.root,
            &fixture.scope,
            NodeRestorePreviewInput {
                target_commit: fixture.target.clone(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_SOURCE_RESTORE_SCOPE_UNSUPPORTED");
    assert_eq!(
        fs::read(fixture.root.join("src/old-only.mjs")).unwrap(),
        b"retained ignored local source"
    );
    fs::remove_file(fixture.root.join("src/old-only.mjs")).unwrap();
    fs::create_dir(fixture.root.join("src/old-only.mjs")).unwrap();
    let error = fixture
        .manager
        .restore_preview(
            &fixture.root,
            &fixture.scope,
            NodeRestorePreviewInput {
                target_commit: fixture.target.clone(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_SOURCE_RESTORE_SCOPE_UNSUPPORTED");
    fs::remove_dir(fixture.root.join("src/old-only.mjs")).unwrap();
    fs::write(
        fixture.root.join("local-app.json"),
        b"{\"id\":\"disposable.node.restore\"} \n",
    )
    .unwrap();
    save(&fixture.manager, &fixture.root, "manifest byte change").await;
    let error = fixture
        .manager
        .restore_preview(
            &fixture.root,
            &fixture.scope,
            NodeRestorePreviewInput {
                target_commit: fixture.target.clone(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_SOURCE_RESTORE_MANIFEST_CHANGED");
    require_clear(&fixture.root).unwrap();
    assert!(!fixture.scope.control_path.exists());
}

#[test]
fn new_operation_reserves_terminal_journal_capacity_before_any_source_move() {
    assert!(check_capacity(MAX_OPERATIONS - 1, MAX_JOURNAL_TOTAL - 10, 10).is_ok());
    assert!(check_capacity(MAX_OPERATIONS, 0, 1).is_err());
    assert!(check_capacity(0, MAX_JOURNAL_TOTAL - 10, 11).is_err());
    assert!(check_capacity(0, u64::MAX, 1).is_err());
}

#[test]
fn every_mixed_tree_reserves_bytes_existing_entries_target_directories_and_depth() {
    let before: BTreeMap<_, _> = (0..4)
        .map(|index| (format!("src/before-{index}.mjs"), MAX_BLOB))
        .collect();
    let target: BTreeMap<_, _> = (0..4)
        .map(|index| (format!("src/target-{index}.mjs"), MAX_BLOB))
        .collect();
    let existing = before
        .keys()
        .cloned()
        .chain([
            "src".into(),
            ".git".into(),
            "data".into(),
            "src/ignored-empty".into(),
        ])
        .collect();
    assert!(source_union_budget(&existing, &before, &BTreeMap::new()).is_ok());
    assert!(source_union_budget(&BTreeSet::new(), &BTreeMap::new(), &target).is_ok());
    assert!(source_union_budget(&existing, &before, &target).is_err());
    assert!(source_union_budget(&existing, &before, &before).is_ok());
    let crowded: BTreeSet<_> = (0..MAX_FILES - 2)
        .map(|index| format!("src/existing-{index}"))
        .collect();
    assert!(source_union_budget(
        &crowded,
        &BTreeMap::new(),
        &BTreeMap::from([("src/new-parent/file.mjs".into(), 1)])
    )
    .is_err());
    let allowed = format!("{}/file.mjs", vec!["src"; 64].join("/"));
    let deep = format!("{}/file.mjs", vec!["src"; 65].join("/"));
    assert!(source_union_budget(
        &BTreeSet::new(),
        &BTreeMap::new(),
        &BTreeMap::from([(allowed, 1)])
    )
    .is_ok());
    assert!(source_union_budget(
        &BTreeSet::new(),
        &BTreeMap::new(),
        &BTreeMap::from([(deep, 1)])
    )
    .is_err());
}

#[tokio::test]
#[ignore = "explicit private Git, target paths exceeding live scanner depth"]
async fn real_git_overdeep_target_is_rejected_before_journal_or_source_publication() {
    let fixture = Fixture::new().await;
    let (mut context, _senders) = fixture.manager.context().unwrap();
    let tools = context.resolve(&fixture.manager.environment).await.unwrap();
    let mut command = node_command(&tools, context.root(), Some(&fixture.root));
    command.args(["hash-object", "-w", "--stdin"]);
    let blob = one_oid(
        &context
            .run(command, b"disposable deep target".to_vec())
            .await
            .unwrap()
            .success()
            .unwrap(),
    )
    .unwrap();
    let index = context.root().join("disposable-deep-index");
    let mut command = node_command(&tools, context.root(), Some(&fixture.root));
    command
        .env("GIT_INDEX_FILE", &index)
        .args(["read-tree", "HEAD"]);
    context
        .run(command, vec![])
        .await
        .unwrap()
        .success()
        .unwrap();
    let deep = format!("src/{}/deep.mjs", vec!["dir"; 64].join("/"));
    let mut command = node_command(&tools, context.root(), Some(&fixture.root));
    command.env("GIT_INDEX_FILE", &index).args([
        "update-index",
        "--add",
        "--cacheinfo",
        "100644",
        &blob,
        &deep,
    ]);
    context
        .run(command, vec![])
        .await
        .unwrap()
        .success()
        .unwrap();
    let mut command = node_command(&tools, context.root(), Some(&fixture.root));
    command.env("GIT_INDEX_FILE", &index).arg("write-tree");
    let tree = one_oid(
        &context
            .run(command, vec![])
            .await
            .unwrap()
            .success()
            .unwrap(),
    )
    .unwrap();
    let mut command = node_command(&tools, context.root(), Some(&fixture.root));
    command.args(["commit-tree", &tree, "--no-gpg-sign"]);
    author_environment(
        &mut command,
        &GitAuthor {
            name: "Disposable Author".into(),
            email: "disposable@example.invalid".into(),
        },
    );
    let target = one_oid(
        &context
            .run(command, b"disposable deep commit".to_vec())
            .await
            .unwrap()
            .success()
            .unwrap(),
    )
    .unwrap();
    let error = fixture
        .manager
        .restore_preview(
            &fixture.root,
            &fixture.scope,
            NodeRestorePreviewInput {
                target_commit: target,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, limit().code);
    fixture.current();
    assert!(!fixture.root.join("src/dir").exists());
    assert!(!fixture.scope.control_path.exists());
    assert!(!operations(&fixture.root).exists());
}

#[tokio::test]
#[ignore = "explicit private Git, interrupted recovery and recreated manager"]
async fn real_git_recovery_itself_can_be_interrupted_and_explicitly_reopened() {
    let fixture = Fixture::new().await;
    let input = fixture.input().await;
    fixture
        .manager
        .restore_apply_with(&fixture.root, &fixture.scope, input, |step, after| {
            if step == 2 && after {
                Err(issue(
                    "DISPOSABLE_RESTORE_INTERRUPT",
                    "artificial interruption",
                ))
            } else {
                Ok(())
            }
        })
        .await
        .unwrap_err();
    let recovery = fixture.recovery(RestoreAction::Undo);
    fixture
        .manager
        .restore_recover_with(&fixture.root, &fixture.scope, recovery, |step, after| {
            if step == 1 && after {
                Err(issue(
                    "DISPOSABLE_RECOVERY_INTERRUPT",
                    "artificial recovery interruption",
                ))
            } else {
                Ok(())
            }
        })
        .await
        .unwrap_err();
    fixture.unchanged();
    let reopened = NodeGit::new(fixture.manager.environment.clone());
    let recovery = fixture.recovery(RestoreAction::Continue);
    reopened
        .restore_recover(&fixture.root, &fixture.scope, recovery)
        .await
        .unwrap();
    fixture.old();
    require_clear(&fixture.root).unwrap();
    require_control_clear(&fixture.scope.control_path).unwrap();
}
