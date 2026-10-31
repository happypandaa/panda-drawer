//! Opt-ins use only the fixed verified MinGit and disposable projects. The
//! interruption seam returns after real renames; it is not a power-loss test.
use super::*;
use crate::file_identity::PersistedIdentity;

struct Fixture {
    _temporary: TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    manager: Arc<ProjectGitManager>,
    project: ProjectHandle,
    environment: GitEnvironmentManager,
    tools: GitToolchain,
    context: Context,
    stop: watch::Sender<bool>,
    _cancel: watch::Sender<bool>,
    target: String,
    current: String,
    main: String,
}
impl Fixture {
    async fn new() -> Self {
        let sample = PathBuf::from(
            std::env::var_os("PANDA_W4_GIT_SAMPLE_ROOT")
                .expect("fixed disposable official MinGit sample"),
        );
        crate::managed_git::verify(sample.clone()).await.unwrap();
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let created = store
            .create(crate::CreateProjectInput {
                name: "Disposable Windows source restore official 中文".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&created.id).unwrap();
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let environment = GitEnvironmentManager::with_processes(
            paths.data_root().into(),
            Some(sample),
            store.processes().clone(),
        );
        let manager = ProjectGitManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            environment.clone(),
        )
        .unwrap();
        let (stop, receiver) = watch::channel(false);
        let (cancel, cancelled) = watch::channel(false);
        let mut context = Context::new(receiver, cancelled).unwrap();
        context.bind_keep_alive(store.processes().clone(), Arc::new(()));
        let tools = context.resolve(&environment).await.unwrap();
        let main = format!("app/{}/{}.txt", "中".repeat(86), "文".repeat(86));
        let mut fixture = Self {
            _temporary: temporary,
            paths,
            store,
            tasks,
            manager,
            project,
            environment,
            tools,
            context,
            stop,
            _cancel: cancel,
            target: String::new(),
            current: String::new(),
            main,
        };
        fixture
            .git(&["init", "--initial-branch=main"], vec![])
            .await;
        fs::create_dir_all(fixture.project.root.join(&fixture.main).parent().unwrap()).unwrap();
        fs::write(
            fixture.project.root.join(&fixture.main),
            b"source version one",
        )
        .unwrap();
        fs::write(
            fixture.project.root.join("app/only-in-first.txt"),
            b"first-only source",
        )
        .unwrap();
        fixture.commit("disposable source one").await;
        fixture.target = one_oid(&fixture.git(&["rev-parse", "HEAD"], vec![]).await).unwrap();
        fs::write(
            fixture.project.root.join(&fixture.main),
            b"source version two",
        )
        .unwrap();
        fs::remove_file(fixture.project.root.join("app/only-in-first.txt")).unwrap();
        fs::write(
            fixture.project.root.join("app/only-in-second.txt"),
            b"second-only source",
        )
        .unwrap();
        fixture.commit("disposable source two").await;
        fixture.current = one_oid(&fixture.git(&["rev-parse", "HEAD"], vec![]).await).unwrap();
        fs::create_dir_all(
            fixture
                .project
                .root
                .join(&fixture.project.manifest.output_dir),
        )
        .unwrap();
        fs::write(fixture.output(), b"published output remains").unwrap();
        fixture
    }
    fn output(&self) -> PathBuf {
        self.project
            .root
            .join(&self.project.manifest.output_dir)
            .join("disposable-unchanged-output.txt")
    }
    async fn git(&mut self, args: &[&str], input: Vec<u8>) -> Vec<u8> {
        let mut command = git_command(&self.tools, self.context.root(), Some(&self.project));
        command.args(args);
        self.context
            .run(command, input)
            .await
            .unwrap()
            .success()
            .unwrap()
    }
    async fn commit(&mut self, message: &str) {
        let roots = included_roots(&self.project.manifest)
            .into_iter()
            .filter(|path| self.project.root.join(path).exists())
            .collect::<Vec<_>>();
        let mut command = git_command(&self.tools, self.context.root(), Some(&self.project));
        command.args(["add", "-A", "--"]).args(roots);
        self.context
            .run(command, vec![])
            .await
            .unwrap()
            .success()
            .unwrap();
        self.git(
            &[
                "-c",
                "user.name=Disposable",
                "-c",
                "user.email=disposable@example.invalid",
                "commit",
                "-m",
                message,
            ],
            vec![],
        )
        .await;
    }
    async fn wait(&self, task: &TaskRecord) -> TaskRecord {
        tokio::time::timeout(Duration::from_secs(180), async {
            loop {
                let current = self.tasks.task(&task.task_id).unwrap();
                if current.status.is_final() {
                    return current;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual source task must finish")
    }
    fn preserved(&self) -> Vec<(PathBuf, PersistedIdentity, Vec<u8>)> {
        [
            self.project.root.join(".git/HEAD"),
            self.project.root.join(".git/index"),
            self.project.root.join(".git/config"),
            self.project.root.join("project.json"),
            self.project.root.join(&self.project.manifest.database),
            self.output(),
        ]
        .into_iter()
        .map(|path| {
            let id = PersistedIdentity::capture(&path).unwrap();
            let bytes = fs::read(&path).unwrap();
            (path, id, bytes)
        })
        .collect()
    }
    fn unchanged(preserved: &[(PathBuf, PersistedIdentity, Vec<u8>)]) {
        for (path, id, bytes) in preserved {
            assert!(id.matches_path(path).unwrap(), "{}", path.display());
            assert_eq!(&fs::read(path).unwrap(), bytes, "{}", path.display());
        }
    }
    fn body(&self, after: bool) {
        assert_eq!(
            fs::read(self.project.root.join(&self.main)).unwrap(),
            if after {
                b"source version one"
            } else {
                b"source version two"
            }
        );
        assert_eq!(
            self.project.root.join("app/only-in-first.txt").exists(),
            after
        );
        assert_eq!(
            self.project.root.join("app/only-in-second.txt").exists(),
            !after
        );
    }
    async fn finish(self) {
        assert!(self.manager.shutdown().await.is_empty());
        self.tasks.shutdown().await;
        self.store.shutdown().await;
        assert!(self.store.processes().shutdown().await.is_empty());
    }

    // Prepare the exact ordinary immutable plan with the real Git object reader,
    // then interrupt only the native exchange of its modified long-name file.
    async fn interrupt(
        &mut self,
        direction: RestoreAction,
        point: native_restore::Point,
    ) -> String {
        let tools = self.tools.clone();
        let target = self.target.clone();
        let task = format!("task_{}", uuid::Uuid::new_v4().simple());
        let lease = Arc::new(
            self.store
                .reserve_development_task(&self.project.manifest.id, &task)
                .unwrap(),
        );
        self.context
            .bind(self.store.processes().clone(), lease.clone());
        let gate = read_gate(&self.project, &mut self.context).await.unwrap();
        let plan = self
            .manager
            .restore_plan(&self.project, &target, &tools, &mut self.context)
            .await
            .unwrap();
        let root = self.manager.restore_root(&self.project.manifest.id, &task);
        let staging = self
            .paths
            .workspace_root()
            .join(".platform/project-state/source-restore-staging")
            .join(&self.project.manifest.id)
            .join(&task);
        private_dir(staging.parent().unwrap()).unwrap();
        native_restore::create_staging(&staging).unwrap();
        private_dir(&staging.join("slots")).unwrap();
        let mut entries = vec![];
        for (index, change) in plan.changes.iter().enumerate() {
            let before = stamp(&self.project.root.join(&change.path)).unwrap();
            let after = if let Some(wanted) = &change.after {
                let bytes = read_file(
                    &self
                        .context
                        .root()
                        .join("restore-target")
                        .join(&change.path),
                    MAX_BLOB,
                )
                .unwrap();
                assert_eq!(sha256(&bytes), wanted.sha256);
                let mut file =
                    native_restore::create_file(&slot(&staging, index), wanted.executable).unwrap();
                file.write_all(&bytes).unwrap();
                file.sync_all().unwrap();
                drop(file);
                stamp(&slot(&staging, index)).unwrap()
            } else {
                None
            };
            entries.push(RestoreEntry {
                path: change.path.clone(),
                before,
                after,
            });
        }
        let mut journal = native_restore::new_journal(
            &staging,
            &task,
            plan_digest(&plan).unwrap(),
            plan,
            entries,
            self.tools.git.clone(),
        )
        .unwrap();
        write_journal(&staging, &journal).unwrap();
        {
            let _publication = guard::reserve_publication(
                self.paths.workspace_root(),
                &self.project.manifest.id,
                &task,
                maximum_journal_bytes(&journal).unwrap(),
            )
            .unwrap();
            private_dir(root.parent().unwrap()).unwrap();
            native_restore::move_prepared(&staging, &root, &journal).unwrap();
        }
        // For an interrupted undo, first produce the real full after-state.
        if direction == RestoreAction::Undo {
            apply_entries(
                &self.project,
                &root,
                &mut journal,
                RestoreAction::Continue,
                &mut self.context,
                |_, _| Ok(()),
            )
            .await
            .unwrap();
        }
        let index = journal
            .entries
            .iter()
            .position(|entry| entry.path == self.main)
            .unwrap();
        assert!(journal.entries[index].before.is_some() && journal.entries[index].after.is_some());
        let error = native_restore::fixture_interrupt(
            &self.project,
            &root,
            &mut journal,
            index,
            direction,
            point,
        )
        .unwrap_err();
        assert_eq!(error.code, "DISPOSABLE_SOURCE_NATIVE_INTERRUPTION");
        self.context.clear_phase();
        self.context.clear_operation();
        drop(gate);
        tokio::time::timeout(Duration::from_secs(10), async {
            while Arc::strong_count(&lease) > 1 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        drop(lease);
        task
    }
}

#[tokio::test]
#[ignore = "Opt-in fixed PANDA_W4_GIT_SAMPLE_ROOT; public source apply with real raw objects and long Unicode"]
async fn official_windows_source_preview_apply_preserves_git_identity_data_and_output() {
    let fixture = Fixture::new().await;
    let preserved = fixture.preserved();
    let preview = fixture
        .manager
        .source_restore_preview(
            &fixture.project.manifest.id,
            GitSourceRestorePreviewInput {
                target_commit: fixture.target.clone(),
            },
            fixture.stop.subscribe(),
        )
        .await
        .unwrap();
    assert!(preview.can_apply);
    assert_eq!(preview.head.oid.as_deref(), Some(fixture.current.as_str()));
    assert_eq!(preview.changes.len(), 3);
    let task = fixture
        .manager
        .source_restore_apply(
            &fixture.project.manifest.id,
            GitSourceRestoreApplyInput {
                target_commit: fixture.target.clone(),
                expected_head: preview.head.oid.unwrap(),
                expected_branch: preview.head.branch.unwrap(),
                expected_snapshot: preview.snapshot_digest,
                expected_plan_digest: preview.plan_digest,
            },
            fixture.stop.subscribe(),
        )
        .await
        .unwrap();
    let done = fixture.wait(&task).await;
    assert_eq!(
        done.status,
        crate::TaskStatus::Succeeded,
        "{:?}",
        done.error
    );
    fixture.body(true);
    Fixture::unchanged(&preserved);
    let status = fixture
        .manager
        .source_restore_status(&fixture.project.manifest.id, fixture.stop.subscribe())
        .await
        .unwrap();
    assert_eq!(status.state, "clear");
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "Opt-in fixed MinGit; real rename interruption, new manager pure status, public continue/undo"]
async fn official_windows_source_new_manager_recovers_both_directions_after_real_native_moves() {
    for pending_direction in [RestoreAction::Continue, RestoreAction::Undo] {
        for point in [
            native_restore::Point::OldHeld,
            native_restore::Point::NewVisible,
        ] {
            for action in [RestoreAction::Continue, RestoreAction::Undo] {
                let mut fixture = Fixture::new().await;
                let preserved = fixture.preserved();
                let original = fixture.interrupt(pending_direction, point).await;
                assert!(fixture.manager.shutdown().await.is_empty());
                fixture.manager = ProjectGitManager::new(
                    fixture.paths.clone(),
                    fixture.store.clone(),
                    fixture.tasks.clone(),
                    fixture.environment.clone(),
                )
                .unwrap();
                let root = fixture
                    .manager
                    .restore_root(&fixture.project.manifest.id, &original);
                let journal = fs::read(root.join("journal.json")).unwrap();
                let missing = !fixture.project.root.join(&fixture.main).exists();
                let status = fixture
                    .manager
                    .source_restore_status(&fixture.project.manifest.id, fixture.stop.subscribe())
                    .await
                    .unwrap();
                assert_eq!(status.state, "pending", "{:?}", status.issues);
                assert_eq!(status.operations.len(), 1);
                let operation = &status.operations[0];
                assert!(
                    operation.can_continue && operation.can_undo,
                    "{:?}",
                    operation.issues
                );
                assert_eq!(
                    fs::read(root.join("journal.json")).unwrap(),
                    journal,
                    "status must not write or stabilize"
                );
                assert_eq!(!fixture.project.root.join(&fixture.main).exists(), missing);
                let task = fixture
                    .manager
                    .source_restore_recover(
                        &fixture.project.manifest.id,
                        GitSourceRestoreRecoverInput {
                            restore_task_id: original,
                            expected_plan_digest: operation.plan_digest.clone(),
                            expected_journal_digest: operation.journal_digest.clone(),
                            action,
                        },
                        fixture.stop.subscribe(),
                    )
                    .await
                    .unwrap();
                let done = fixture.wait(&task).await;
                assert_eq!(
                    done.status,
                    crate::TaskStatus::Succeeded,
                    "{:?}",
                    done.error
                );
                fixture.body(action == RestoreAction::Continue);
                Fixture::unchanged(&preserved);
                let completed = fixture
                    .manager
                    .source_restore_status(&fixture.project.manifest.id, fixture.stop.subscribe())
                    .await
                    .unwrap();
                assert_eq!(completed.state, "clear");
                fixture.finish().await;
            }
        }
    }
}
