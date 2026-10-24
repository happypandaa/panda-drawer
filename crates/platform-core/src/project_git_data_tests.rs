use super::*;
use crate::{CreateProjectInput, SqlCaller, SqlOptions, SqlRequest, TaskStatus};

async fn wait(tasks: &BuildManager, id: &str) -> TaskRecord {
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
struct Fixture {
    _temp: tempfile::TempDir,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    manager: Arc<ProjectGitManager>,
    project: ProjectHandle,
    tools: GitToolchain,
    _stop: watch::Sender<bool>,
    receiver: watch::Receiver<bool>,
}
impl Fixture {
    async fn new() -> Self {
        let tool = PathBuf::from(
            std::env::var("LOCAL_PROJECTS_TEST_GIT_ROOT")
                .expect("explicit disposable official Git required"),
        )
        .canonicalize()
        .unwrap();
        assert!(tool.starts_with(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.test-artifacts")
                .canonicalize()
                .unwrap()
        ));
        let (temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let info = store
            .create(CreateProjectInput {
                name: "Disposable Git database versions".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&info.id).unwrap();
        let tasks = BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into())
            .unwrap();
        let git = GitEnvironmentManager::new(paths.data_root().into(), Some(tool));
        let tools = git.resolve().await.unwrap();
        let manager = ProjectGitManager::new(paths, store.clone(), tasks.clone(), git).unwrap();
        let (sender, receiver) = watch::channel(false);
        let preview = manager.status(&info.id, receiver.clone()).await.unwrap();
        let task = manager
            .initialize(
                &info.id,
                GitInitInput {
                    expected_snapshot: preview.snapshot_digest,
                    initial_branch: "main".into(),
                },
                receiver.clone(),
            )
            .await
            .unwrap();
        assert_eq!(
            wait(&tasks, &task.task_id).await.status,
            TaskStatus::Succeeded
        );
        Self {
            _temp: temp,
            store,
            tasks,
            manager,
            project,
            tools,
            _stop: sender,
            receiver,
        }
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
    async fn save_data(&self, message: &str) -> TaskRecord {
        let status = self
            .manager
            .status(&self.project.manifest.id, self.receiver.clone())
            .await
            .unwrap();
        assert_eq!(status.database["canSave"], true, "{status:?}");
        let task = self
            .manager
            .save_data(
                &self.project.manifest.id,
                GitDataSaveInput {
                    expected_head: head_oid(&status).map(str::to_owned),
                    expected_snapshot: status.snapshot_digest,
                    message: message.into(),
                    author: GitAuthor {
                        name: "Data fixture".into(),
                        email: "data-fixture@example.invalid".into(),
                    },
                },
                self.receiver.clone(),
            )
            .await
            .unwrap();
        wait(&self.tasks, &task.task_id).await
    }
    async fn git(&self, args: &[&str]) -> Vec<u8> {
        let (_sender, cancel) = watch::channel(false);
        let mut context = Context::new(self.receiver.clone(), cancel).unwrap();
        let mut command = git_command(&self.tools, context.root(), Some(&self.project));
        command.args(args);
        context
            .run(command, vec![])
            .await
            .unwrap()
            .success()
            .unwrap()
    }
    async fn finish(self) {
        self.manager.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to the explicit disposable official payload"]
async fn official_git_data_saves_preserve_source_and_live_data_and_cleanup_temporary_copies() {
    let fixture = Fixture::new().await;
    fixture.sql("CREATE TABLE notes(body TEXT)").await;
    fixture
        .sql("INSERT INTO notes VALUES('first saved data')")
        .await;
    let source = fixture.project.root.join("app/src/version-fixture.txt");
    fs::write(&source, "first source\n").unwrap();
    let attachment = fixture.project.root.join("data/attachments/记录.txt");
    fs::write(&attachment, "first attachment\n").unwrap();
    let first = fixture.save_data("first source and data").await;
    assert_eq!(first.status, TaskStatus::Succeeded, "{first:?}");
    let first_commit = first.result.as_ref().unwrap()["commit"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        !fixture
            .manager
            .journal_path(&fixture.project.manifest.id, &first.task_id)
            .with_extension("files")
            .exists(),
        "normal data saving must not create a second snapshot history"
    );
    let first_tree = fixture
        .git(&["rev-parse", &format!("{first_commit}:version-data")])
        .await;
    fs::write(&source, "source changed while data stays the same\n").unwrap();
    let unchanged_data = fixture.save_data("source and unchanged data").await;
    assert_eq!(
        unchanged_data.status,
        TaskStatus::Succeeded,
        "{unchanged_data:?}"
    );
    assert!(!fixture
        .manager
        .journal_path(&fixture.project.manifest.id, &unchanged_data.task_id)
        .with_extension("files")
        .exists());
    assert_eq!(
        fixture.git(&["rev-parse", "HEAD:version-data"]).await,
        first_tree
    );
    fixture
        .sql("INSERT INTO notes VALUES('unsaved later data')")
        .await;
    fs::write(&attachment, "later attachment\n").unwrap();
    fs::write(&source, "second source\n").unwrap();
    let status = fixture
        .manager
        .status(&fixture.project.manifest.id, fixture.receiver.clone())
        .await
        .unwrap();
    assert!(status.can_save, "{status:?}");
    let task = fixture
        .manager
        .save(
            &fixture.project.manifest.id,
            GitSaveInput {
                expected_head: head_oid(&status).map(str::to_owned),
                expected_snapshot: status.snapshot_digest,
                message: "only source".into(),
                author: GitAuthor {
                    name: "Source fixture".into(),
                    email: "source@example.invalid".into(),
                },
            },
            fixture.receiver.clone(),
        )
        .await
        .unwrap();
    let saved = wait(&fixture.tasks, &task.task_id).await;
    assert_eq!(saved.status, TaskStatus::Succeeded, "{saved:?}");
    assert_eq!(
        fixture.git(&["rev-parse", "HEAD:version-data"]).await,
        first_tree
    );
    let preview = fixture
        .manager
        .source_restore_preview(
            &fixture.project.manifest.id,
            GitSourceRestorePreviewInput {
                target_commit: first_commit,
            },
            fixture.receiver.clone(),
        )
        .await
        .unwrap();
    assert!(preview
        .changes
        .iter()
        .all(|change| !is_data_path(&change.path)));
    let task = fixture
        .manager
        .source_restore_apply(
            &fixture.project.manifest.id,
            GitSourceRestoreApplyInput {
                target_commit: preview.target_commit,
                expected_head: preview.head.oid.unwrap(),
                expected_branch: preview.head.branch.unwrap(),
                expected_snapshot: preview.snapshot_digest,
                expected_plan_digest: preview.plan_digest,
            },
            fixture.receiver.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        wait(&fixture.tasks, &task.task_id).await.status,
        TaskStatus::Succeeded
    );
    assert_eq!(fs::read_to_string(source).unwrap(), "first source\n");
    assert_eq!(
        fs::read_to_string(&attachment).unwrap(),
        "later attachment\n"
    );
    assert_eq!(
        fixture.git(&["rev-parse", "HEAD:version-data"]).await,
        first_tree
    );
    let second = fixture.save_data("second data snapshot").await;
    assert_eq!(second.status, TaskStatus::Succeeded, "{second:?}");
    assert!(!fixture
        .manager
        .journal_path(&fixture.project.manifest.id, &second.task_id)
        .with_extension("files")
        .exists());
    let database =
        rusqlite::Connection::open(fixture.project.root.join("version-data/database.sqlite"))
            .unwrap();
    assert_eq!(
        database
            .query_row("SELECT count(*) FROM notes", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    let source_paths =
        String::from_utf8(fixture.git(&["ls-tree", "-r", "--name-only", "HEAD"]).await).unwrap();
    assert!(!source_paths.lines().any(|path| path.starts_with("data/")
        || path.starts_with(".platform/")
        || path.ends_with("-wal")
        || path.ends_with("-shm")));
    let unchanged = fixture.save_data("no empty version").await;
    assert!(!fixture
        .manager
        .journal_path(&fixture.project.manifest.id, &unchanged.task_id)
        .with_extension("files")
        .exists());
    assert_eq!(unchanged.error.unwrap().code, "PROJECT_GIT_NO_CHANGES");
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to the explicit disposable official payload"]
async fn official_git_committed_data_save_finalizes_owned_index_and_snapshot_after_each_fault() {
    let fixture = Fixture::new().await;
    fixture.sql("CREATE TABLE notes(body TEXT)").await;
    for point in [1, 2, 3, 4] {
        fixture
            .sql(&format!("INSERT INTO notes VALUES('fault {point}')"))
            .await;
        fixture
            .manager
            .data_save_fault
            .store(point, Ordering::Release);
        let task = fixture
            .save_data(&format!("snapshot with fault {point}"))
            .await;
        assert_eq!(task.status, TaskStatus::Failed, "{task:?}");
        assert_eq!(
            task.error.as_ref().unwrap().details["operationOutcome"],
            if point == 4 {
                "not_committed"
            } else {
                "committed"
            }
        );
        let state = fixture
            .manager
            .data_save_status(&fixture.project.manifest.id, fixture.receiver.clone())
            .await
            .unwrap();
        assert_eq!(state["state"], "pending", "{state}");
        assert_eq!(
            crate::workspace::validate_idle(&fixture.manager.paths)
                .unwrap_err()
                .code,
            "PROJECT_GIT_DATA_RECOVERY_REQUIRED"
        );
        assert_eq!(
            fixture
                .store
                .check_development_available(&fixture.project.manifest.id)
                .unwrap_err()
                .code,
            "PROJECT_GIT_DATA_RECOVERY_REQUIRED"
        );
        assert_eq!(
            fixture
                .store
                .reserve_delete_task(&fixture.project.manifest.id, "task_disposable_delete_probe")
                .err()
                .unwrap()
                .code,
            "PROJECT_GIT_DATA_RECOVERY_REQUIRED"
        );
        let read = fixture
            .store
            .execute(
                &fixture.project.manifest.id,
                SqlRequest {
                    sql: "SELECT count(*) FROM notes".into(),
                    params: vec![],
                    options: SqlOptions::default(),
                },
                None,
                SqlCaller::Management,
            )
            .await
            .unwrap();
        assert_eq!(read.rows[0]["count(*)"], point);
        let operation = &state["operations"][0];
        assert_eq!(operation["canFinalize"], true, "{state}");
        let input = GitDataSaveFinalizeInput {
            save_task_id: task.task_id.clone(),
            expected_journal_digest: operation["journalDigest"].as_str().unwrap().into(),
        };
        let finish = fixture
            .manager
            .finalize_data_save(
                &fixture.project.manifest.id,
                input,
                fixture.receiver.clone(),
            )
            .await
            .unwrap();
        let result = wait(&fixture.tasks, &finish.task_id).await;
        assert_eq!(result.status, TaskStatus::Succeeded, "{result:?}");
        assert_eq!(result.result.unwrap()["operation"], "save-data-finalize");
        let retained = fixture
            .manager
            .journal_path(&fixture.project.manifest.id, &task.task_id)
            .with_extension("files")
            .exists();
        assert_eq!(
            retained,
            point == 4,
            "only the explicitly uncommitted failure candidate is retained"
        );
        let status = fixture
            .manager
            .status(&fixture.project.manifest.id, fixture.receiver.clone())
            .await
            .unwrap();
        assert!(status.can_save, "{status:?}");
        assert_eq!(
            fixture
                .manager
                .data_save_status(&fixture.project.manifest.id, fixture.receiver.clone())
                .await
                .unwrap()["state"],
            "clear"
        );
        let live = rusqlite::Connection::open(
            fixture
                .project
                .root
                .join(&fixture.project.manifest.database),
        )
        .unwrap();
        assert_eq!(
            live.query_row("SELECT count(*) FROM notes", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            point as i64
        );
    }
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to the explicit disposable official payload"]
async fn official_git_data_save_refuses_source_only_and_preserves_unknown_cleanup_material() {
    let fixture = Fixture::new().await;
    fixture.manager.data_save_fault.store(2, Ordering::Release);
    let task = fixture
        .save_data("explicit data save interrupted after index")
        .await;
    assert_eq!(task.status, TaskStatus::Failed);
    let operation = fixture
        .manager
        .journal_path(&fixture.project.manifest.id, &task.task_id)
        .with_extension("files");
    fs::write(
        operation.join("unknown-user-material.txt"),
        "do not remove unknown material",
    )
    .unwrap();
    let state = fixture
        .manager
        .data_save_status(&fixture.project.manifest.id, fixture.receiver.clone())
        .await
        .unwrap();
    assert_eq!(state["state"], "pending");
    let stale = fixture
        .manager
        .finalize_data_save(
            &fixture.project.manifest.id,
            GitDataSaveFinalizeInput {
                save_task_id: task.task_id.clone(),
                expected_journal_digest: "a".repeat(64),
            },
            fixture.receiver.clone(),
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code, "PROJECT_GIT_STALE_PREVIEW");
    let finish = fixture
        .manager
        .finalize_data_save(
            &fixture.project.manifest.id,
            GitDataSaveFinalizeInput {
                save_task_id: task.task_id.clone(),
                expected_journal_digest: state["operations"][0]["journalDigest"]
                    .as_str()
                    .unwrap()
                    .into(),
            },
            fixture.receiver.clone(),
        )
        .await
        .unwrap();
    let result = wait(&fixture.tasks, &finish.task_id).await;
    assert_eq!(result.status, TaskStatus::Succeeded, "{result:?}");
    assert!(result.result.unwrap()["cleanupWarning"].is_object());
    assert_eq!(
        fs::read_to_string(operation.join("unknown-user-material.txt")).unwrap(),
        "do not remove unknown material"
    );
    fs::rename(
        fixture.project.root.join(".git"),
        fixture.project.root.join("app/.git"),
    )
    .unwrap();
    let status = fixture
        .manager
        .status(&fixture.project.manifest.id, fixture.receiver.clone())
        .await
        .unwrap();
    assert_eq!(status.repository.state, "source-only");
    assert_eq!(status.database["canSave"], false);
    let task = fixture
        .manager
        .save_data(
            &fixture.project.manifest.id,
            GitDataSaveInput {
                expected_head: head_oid(&status).map(str::to_owned),
                expected_snapshot: status.snapshot_digest,
                message: "must not copy data from a source-only repository".into(),
                author: GitAuthor {
                    name: "Fixture".into(),
                    email: "fixture@example.invalid".into(),
                },
            },
            fixture.receiver.clone(),
        )
        .await
        .unwrap();
    let result = wait(&fixture.tasks, &task.task_id).await;
    assert_eq!(result.status, TaskStatus::Failed);
    assert!(!fixture
        .manager
        .journal_path(&fixture.project.manifest.id, &task.task_id)
        .with_extension("files")
        .exists());
    assert!(fixture.project.root.join("app/.git").is_dir());
    assert!(!fixture.project.root.join(".git").exists());
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "set LOCAL_PROJECTS_TEST_GIT_ROOT to the explicit disposable official payload"]
async fn official_git_data_save_public_cancel_stops_waiting_and_tampered_candidate_stays_blocked() {
    let fixture = Fixture::new().await;
    let status = fixture
        .manager
        .status(&fixture.project.manifest.id, fixture.receiver.clone())
        .await
        .unwrap();
    let read = fixture.project.operation_gate.clone().read_owned().await;
    let task = fixture
        .manager
        .save_data(
            &fixture.project.manifest.id,
            GitDataSaveInput {
                expected_head: head_oid(&status).map(str::to_owned),
                expected_snapshot: status.snapshot_digest,
                message: "cancel while business operation is active".into(),
                author: GitAuthor {
                    name: "Fixture".into(),
                    email: "fixture@example.invalid".into(),
                },
            },
            fixture.receiver.clone(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !fixture.project.control.frozen.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        fixture.manager.cancel(&task.task_id).unwrap().task_type,
        "gitDataSave"
    );
    let cancelled = wait(&fixture.tasks, &task.task_id).await;
    assert_eq!(
        cancelled.error.unwrap().code,
        "PROJECT_DATA_SNAPSHOT_CANCELLED"
    );
    assert!(!fixture.project.control.frozen.load(Ordering::Acquire));
    drop(read);
    assert!(!fixture.project.root.join("version-data").exists());
    fixture.manager.data_save_fault.store(2, Ordering::Release);
    let task = fixture
        .save_data("candidate will be modified by the external test actor")
        .await;
    assert_eq!(task.status, TaskStatus::Failed);
    let operation = fixture
        .manager
        .journal_path(&fixture.project.manifest.id, &task.task_id)
        .with_extension("files");
    fs::write(
        operation.join("data/database.sqlite"),
        "tampered candidate must be preserved",
    )
    .unwrap();
    let state = fixture
        .manager
        .data_save_status(&fixture.project.manifest.id, fixture.receiver.clone())
        .await
        .unwrap();
    assert_eq!(state["state"], "blocked");
    assert_eq!(state["operations"][0]["canFinalize"], false);
    let task = fixture
        .manager
        .finalize_data_save(
            &fixture.project.manifest.id,
            GitDataSaveFinalizeInput {
                save_task_id: task.task_id,
                expected_journal_digest: state["operations"][0]["journalDigest"]
                    .as_str()
                    .unwrap()
                    .into(),
            },
            fixture.receiver.clone(),
        )
        .await
        .unwrap();
    let failed = wait(&fixture.tasks, &task.task_id).await;
    assert_eq!(failed.status, TaskStatus::Failed);
    assert_eq!(
        failed.error.unwrap().details["operationOutcome"],
        "committed"
    );
    assert_eq!(
        fs::read_to_string(operation.join("data/database.sqlite")).unwrap(),
        "tampered candidate must be preserved"
    );
    assert!(!fixture.project.root.join("version-data").exists());
    fixture.finish().await;
}

#[test]
fn combined_tree_budget_matches_the_following_head_and_index_parser_limit() {
    assert!(check_combined_entry_budget(MAX_FILES).is_ok());
    assert_eq!(
        check_combined_entry_budget(MAX_FILES + 1).unwrap_err().code,
        "PROJECT_GIT_COMBINED_LIMIT"
    );
    assert!(check_combined_entry_budget(6000 + 5000).is_err());
}
