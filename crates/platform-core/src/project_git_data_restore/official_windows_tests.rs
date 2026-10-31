//! Opt-in public-manager validation against the fixed verified official MinGit.
use super::*;
use crate::{SqlCaller, SqlOptions, SqlRequest, TaskStatus};

#[tokio::test]
#[ignore = "set PANDA_W4_GIT_SAMPLE_ROOT to the verified fixed official MinGit payload"]
async fn official_mingit_raw_object_writer_keeps_the_real_lease_gate_and_candidate_until_join() {
    let mut fixture = Official::new().await;
    fixture.seed().await;
    let payload = fixture
        .project
        .root
        .join(".platform/disposable-raw-object.bin");
    let mut payload_file = File::create(&payload).unwrap();
    payload_file
        .write_all(&vec![b'x'; 9 * 1024 * 1024])
        .unwrap();
    payload_file.sync_all().unwrap();
    drop(payload_file);
    let oid = String::from_utf8(
        fixture
            .git(
                &[
                    "hash-object",
                    "-w",
                    "--no-filters",
                    "--",
                    ".platform/disposable-raw-object.bin",
                ],
                vec![],
            )
            .await,
    )
    .unwrap()
    .trim()
    .to_string();
    let operation = Arc::new(
        fixture
            .store
            .reserve_transient_operation(&fixture.project.manifest.id, "disposable-raw-data-writer")
            .unwrap(),
    );
    let phase = Arc::new(fixture.project.operation_gate.clone().read_owned().await);
    let (stop, receiver) = watch::channel(false);
    let (_cancel, cancelled) = watch::channel(false);
    let mut context = Context::new(receiver, cancelled).unwrap();
    context.bind(fixture.store.processes(), operation);
    context.phase(phase);
    let candidate = Arc::new(
        tempfile::Builder::new()
            .prefix("panda-disposable-mingit-raw-writer-")
            .tempdir()
            .unwrap(),
    );
    let destination = candidate.path().canonicalize().unwrap();
    context.retain(candidate);
    let private = context.root().to_path_buf();
    let blobs = BTreeMap::from([(
        "version-data/attachments/payload.bin".into(),
        data_preview::objects::Blob {
            oid,
            bytes: 9 * 1024 * 1024,
        },
    )]);
    let (entered, observed) = std::sync::mpsc::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    struct Release(Option<std::sync::mpsc::Sender<()>>);
    impl Drop for Release {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }
    let release = Release(Some(release));
    context.writer_pause = Some((entered, blocked));
    let project = fixture.project.clone();
    let tools = fixture.tools.clone();
    let output = destination.clone();
    let request = tokio::spawn(async move {
        data_preview::objects::materialize(&project, &tools, &mut context, &output, &blobs).await
    });
    tokio::task::spawn_blocking(move || observed.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap()
        .unwrap();
    assert!(destination.join("attachments/payload.bin").is_file());
    stop.send(true).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "PROCESS_CLEANUP_PENDING");
    assert!(fixture.store.has_active_tasks());
    assert!(fixture.project.operation_gate.try_write().is_err());
    assert!(destination.is_dir() && private.is_dir());
    assert!(fixture
        .store
        .processes()
        .shutdown()
        .await
        .iter()
        .any(|issue| issue.code == "PROCESS_CLEANUP_PENDING"));
    drop(release);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let registry = fixture.store.processes();
            if registry.shutdown().await.is_empty()
                && registry
                    .snapshot()
                    .iter()
                    .all(|state| state.resource_closed())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!fixture.store.has_active_tasks());
    assert!(fixture.project.operation_gate.try_write().is_ok());
    assert!(!destination.exists() && !private.exists());
    fixture.finish().await;
}

struct Official {
    _temporary: TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    manager: Arc<ProjectGitManager>,
    project: ProjectHandle,
    selected: PathBuf,
    tools: GitToolchain,
    context: Context,
    sender: watch::Sender<bool>,
    _cancel: watch::Sender<bool>,
}
impl Official {
    async fn new() -> Self {
        let selected = PathBuf::from(
            std::env::var_os("PANDA_W4_GIT_SAMPLE_ROOT")
                .expect("explicit official pinned disposable MinGit payload"),
        );
        crate::managed_git::verify(selected.clone()).await.unwrap();
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let created = store
            .create(crate::CreateProjectInput {
                name: "Disposable Windows public data apply 中文".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&created.id).unwrap();
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let environment = GitEnvironmentManager::with_processes(
            paths.data_root().into(),
            Some(selected.clone()),
            store.processes().clone(),
        );
        let manager = ProjectGitManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            environment.clone(),
        )
        .unwrap();
        let (sender, stop) = watch::channel(false);
        let (cancel, cancelled) = watch::channel(false);
        let mut context = Context::new(stop, cancelled).unwrap();
        context.bind_keep_alive(store.processes().clone(), Arc::new(()));
        let tools = context.resolve(&environment).await.unwrap();
        Self {
            _temporary: temporary,
            paths,
            store,
            tasks,
            manager,
            project,
            selected,
            tools,
            context,
            sender,
            _cancel: cancel,
        }
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
    async fn sql(&self, sql: &str) {
        self.store
            .execute(
                &self.project.manifest.id,
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
    async fn seed(&mut self) -> String {
        self.sql("CREATE TABLE disposable_notes(value TEXT)").await;
        self.sql("INSERT INTO disposable_notes VALUES('target')")
            .await;
        let attachments = self
            .project
            .root
            .join(&self.project.manifest.attachments_dir);
        fs::write(attachments.join("value.txt"), b"target attachment").unwrap();
        fs::create_dir(attachments.join("empty-target")).unwrap();
        let lease = version_data::SnapshotLease::acquire(
            self.project.clone(),
            self.sender.subscribe(),
            self._cancel.subscribe(),
        )
        .await
        .unwrap();
        self.store
            .close_database(&self.project.manifest.id)
            .await
            .unwrap();
        let prepared = tempfile::Builder::new()
            .prefix("panda-disposable-external-git-data-seed-")
            .tempdir_in(self.paths.workspace_root().join(".platform"))
            .unwrap();
        let candidate = prepared.path().join("data");
        version_data::prepare_snapshot(
            lease,
            &candidate,
            version_data::SnapshotLimits::default(),
            &self.sender.subscribe(),
            &self._cancel.subscribe(),
        )
        .unwrap();
        // This is fixture-authored Git history. The public platform save/init
        // APIs stay retired; prepare outside the project before placing the
        // complete data version for the external normal Git commit below.
        platform_native::windows::files::rename_directory_noreplace(
            &candidate,
            &self.project.root.join("version-data"),
        )
        .unwrap();
        fs::write(
            self.project.root.join(".gitignore"),
            b"data/\n.platform/\ndist/\nnode_modules/\n",
        )
        .unwrap();
        fs::write(
            self.project.root.join(".gitattributes"),
            b"* -text -filter -ident -working-tree-encoding\n",
        )
        .unwrap();
        self.git(&["init", "--initial-branch=main"], vec![]).await;
        self.git(&["add", "--all", "--", "."], vec![]).await;
        self.git(
            &[
                "-c",
                "user.name=Disposable",
                "-c",
                "user.email=disposable@example.invalid",
                "commit",
                "-m",
                "disposable data version",
            ],
            vec![],
        )
        .await;
        String::from_utf8(self.git(&["rev-parse", "HEAD"], vec![]).await)
            .unwrap()
            .trim()
            .into()
    }
    async fn input(&self, target: &str) -> GitDataRestoreApplyInput {
        let preview = self
            .manager
            .data_restore_preview(
                &self.project.manifest.id,
                GitDataRestorePreviewInput {
                    target_commit: target.into(),
                },
                self.sender.subscribe(),
            )
            .await
            .unwrap();
        assert!(preview.can_apply && preview.apply_implemented);
        GitDataRestoreApplyInput {
            target_commit: target.into(),
            expected_head: preview.head.oid.unwrap(),
            expected_branch: preview.head.branch.unwrap(),
            expected_document_epoch: preview.document_epoch,
            expected_snapshot: preview.snapshot_digest,
            expected_live_data_digest: preview.live_data_digest,
            expected_target_snapshot_digest: preview.target_snapshot_digest,
            expected_plan_digest: preview.plan_digest,
        }
    }
    async fn wait(&self, task: &str) -> TaskRecord {
        tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let record = self.tasks.task(task).unwrap();
                if record.status.is_final() {
                    break record;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn restart(&mut self) {
        assert!(self.manager.shutdown().await.is_empty());
        assert!(self.tasks.shutdown().await.is_empty());
        assert!(self.store.shutdown().await.is_empty());
        self.store = Arc::new(ProjectStore::new(self.paths.workspace_root().into()).unwrap());
        self.tasks = BuildManager::new(
            self.paths.clone(),
            self.store.clone(),
            "http://127.0.0.1:9".into(),
        )
        .unwrap();
        let environment = GitEnvironmentManager::with_processes(
            self.paths.data_root().into(),
            Some(self.selected.clone()),
            self.store.processes().clone(),
        );
        self.manager = ProjectGitManager::new(
            self.paths.clone(),
            self.store.clone(),
            self.tasks.clone(),
            environment,
        )
        .unwrap();
    }
    fn value(&self) -> String {
        let connection =
            rusqlite::Connection::open(self.project.root.join(&self.project.manifest.database))
                .unwrap();
        connection
            .query_row("SELECT value FROM disposable_notes", [], |row| row.get(0))
            .unwrap()
    }
    async fn finish(self) {
        assert!(self.manager.shutdown().await.is_empty());
        assert!(self.tasks.shutdown().await.is_empty());
        assert!(self.store.shutdown().await.is_empty());
        assert!(self.store.processes().shutdown().await.is_empty());
    }
}

#[tokio::test]
#[ignore = "set PANDA_W4_GIT_SAMPLE_ROOT to the verified fixed official MinGit payload"]
async fn official_mingit_public_preview_apply_and_epoch_reopen_preserve_source_git_and_original_data(
) {
    let mut fixture = Official::new().await;
    let target = fixture.seed().await;
    fixture
        .sql("UPDATE disposable_notes SET value='live original'")
        .await;
    fs::write(
        fixture.project.root.join("data/attachments/value.txt"),
        b"live original attachment",
    )
    .unwrap();
    fs::write(
        fixture.project.root.join("data/attachments/extra.txt"),
        b"retained original",
    )
    .unwrap();
    let before_database = PersistedIdentity::capture(
        &fixture
            .project
            .root
            .join(&fixture.project.manifest.database),
    )
    .unwrap();
    let index = fs::read(fixture.project.root.join(".git/index")).unwrap();
    let head = fs::read(fixture.project.root.join(".git/HEAD")).unwrap();
    let source = fs::read(fixture.project.root.join("project.json")).unwrap();
    assert_eq!(
        fixture
            .manager
            .data_save_status(&fixture.project.manifest.id, fixture.sender.subscribe())
            .await
            .unwrap()["state"],
        "clear"
    );
    let input = fixture.input(&target).await;
    let old_epoch = input.expected_document_epoch.clone();
    let task = fixture
        .manager
        .data_restore_apply(
            &fixture.project.manifest.id,
            input,
            fixture.sender.subscribe(),
        )
        .await
        .unwrap();
    let record = fixture.wait(&task.task_id).await;
    assert_eq!(record.status, TaskStatus::Succeeded, "{record:?}");
    assert_eq!(
        record.result.as_ref().unwrap()["operationOutcome"],
        "committed"
    );
    assert_eq!(fixture.value(), "target");
    assert_ne!(
        fixture
            .store
            .project(&fixture.project.manifest.id)
            .unwrap()
            .document_epoch,
        old_epoch
    );
    assert_eq!(
        fixture
            .manager
            .data_restore_status(&fixture.project.manifest.id, fixture.sender.subscribe())
            .await
            .unwrap()
            .state,
        "clear"
    );
    assert_eq!(
        fs::read(fixture.project.root.join(".git/index")).unwrap(),
        index
    );
    assert_eq!(
        fs::read(fixture.project.root.join(".git/HEAD")).unwrap(),
        head
    );
    assert_eq!(
        fs::read(fixture.project.root.join("project.json")).unwrap(),
        source
    );
    let operation = fixture
        .paths
        .workspace_root()
        .join(".platform/project-state/data-restore")
        .join(&fixture.project.manifest.id)
        .join(&task.task_id);
    assert!(before_database
        .matches_path(&operation.join("slots/database/before"))
        .unwrap());
    assert_eq!(
        fs::read(operation.join("slots/attachments/before/extra.txt")).unwrap(),
        b"retained original"
    );
    fixture.restart().await;
    assert_eq!(fixture.value(), "target");
    assert_eq!(
        fixture
            .manager
            .data_restore_status(&fixture.project.manifest.id, fixture.sender.subscribe())
            .await
            .unwrap()
            .state,
        "clear"
    );
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set PANDA_W4_GIT_SAMPLE_ROOT to the verified fixed official MinGit payload"]
async fn official_mingit_public_pending_status_and_new_manager_undo_keep_the_original_data_identity(
) {
    let mut fixture = Official::new().await;
    let target = fixture.seed().await;
    fixture
        .sql("UPDATE disposable_notes SET value='original to undo'")
        .await;
    let original = PersistedIdentity::capture(
        &fixture
            .project
            .root
            .join(&fixture.project.manifest.database),
    )
    .unwrap();
    let input = fixture.input(&target).await;
    fixture.manager.test_data_restore_fault(u32::MAX);
    let task = fixture
        .manager
        .data_restore_apply(
            &fixture.project.manifest.id,
            input,
            fixture.sender.subscribe(),
        )
        .await
        .unwrap();
    assert_eq!(fixture.wait(&task.task_id).await.status, TaskStatus::Failed);
    let status = fixture
        .manager
        .data_restore_status(&fixture.project.manifest.id, fixture.sender.subscribe())
        .await
        .unwrap();
    assert_eq!(status.state, "pending");
    assert!(status.business_blocked);
    let pending = &status.operations[0];
    assert!(pending.can_continue && pending.can_undo);
    let input = GitDataRestoreRecoverInput {
        restore_task_id: pending.restore_task_id.clone(),
        expected_plan_digest: pending.plan_digest.clone(),
        expected_journal_digest: pending.journal_digest.clone(),
        action: RestoreAction::Undo,
    };
    fixture.restart().await;
    let task = fixture
        .manager
        .data_restore_recover(
            &fixture.project.manifest.id,
            input,
            fixture.sender.subscribe(),
        )
        .await
        .unwrap();
    let record = fixture.wait(&task.task_id).await;
    assert_eq!(record.status, TaskStatus::Succeeded, "{record:?}");
    assert_eq!(fixture.value(), "original to undo");
    assert!(original
        .matches_path(
            &fixture
                .project
                .root
                .join(&fixture.project.manifest.database)
        )
        .unwrap());
    assert_eq!(
        fixture
            .manager
            .data_restore_status(&fixture.project.manifest.id, fixture.sender.subscribe())
            .await
            .unwrap()
            .state,
        "clear"
    );
    fixture.finish().await;
}
