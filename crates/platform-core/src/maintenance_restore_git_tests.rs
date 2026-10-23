//! Disposable legacy-restore fixtures; never use an installed application root.
use super::*;
use crate::{CreateProjectInput, TaskStatus};
use std::{
    collections::BTreeMap,
    ffi::CString,
    os::unix::{ffi::OsStrExt, fs::symlink},
};

struct Fixture {
    _temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    manager: Arc<MaintenanceManager>,
    id: String,
    root: PathBuf,
    backup_id: String,
}

impl Fixture {
    async fn new() -> Self {
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let project = store
            .create(CreateProjectInput {
                name: "Disposable legacy restore Git protection".into(),
                description: Some("Isolated temporary test fixture".into()),
            })
            .await
            .unwrap();
        let root = store.project(&project.id).unwrap().root;
        fs::write(root.join("data/attachments/preserved.txt"), b"backup data").unwrap();
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let manager = MaintenanceManager::new(paths.clone(), store.clone(), tasks.clone()).unwrap();
        let task = manager
            .submit(&project.id, "backup", OperationInput::default())
            .await
            .unwrap();
        let task = wait(&tasks, &task.task_id).await;
        assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
        let backup_id = task.result.unwrap()["backupId"]
            .as_str()
            .unwrap()
            .to_owned();
        Self {
            _temporary: temporary,
            paths,
            store,
            tasks,
            manager,
            id: project.id,
            root,
            backup_id,
        }
    }

    async fn restore(&self) -> Result<TaskRecord, ApiError> {
        self.manager
            .submit(
                &self.id,
                "restore",
                OperationInput {
                    backup_id: Some(self.backup_id.clone()),
                    ..Default::default()
                },
            )
            .await
    }

    async fn stop(self) {
        self.manager.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
}

async fn wait(tasks: &BuildManager, id: &str) -> TaskRecord {
    for _ in 0..1000 {
        let task = tasks.task(id).unwrap();
        if task.status.is_final() {
            return task;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("disposable task did not complete");
}

fn tree(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let value = if metadata.file_type().is_symlink() {
            format!("link:{}", fs::read_link(path).unwrap().display()).into_bytes()
        } else if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), result);
            }
            b"directory".to_vec()
        } else if metadata.is_file() {
            let mut value = b"file:".to_vec();
            value.extend(fs::read(path).unwrap());
            value
        } else {
            // A special file has no safe readable contents. In particular,
            // never open the FIFO used by the malformed-manifest regression.
            b"special-file".to_vec()
        };
        result.insert(path.strip_prefix(root).unwrap().into(), value);
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[tokio::test]
async fn legacy_restore_rejects_all_root_and_source_git_marker_types_before_any_writes() {
    for location in [".git", "app/.git"] {
        for kind in ["directory", "file", "dangling-link"] {
            let fixture = Fixture::new().await;
            let marker = fixture.root.join(location);
            match kind {
                "directory" => {
                    fs::create_dir(&marker).unwrap();
                    fs::write(marker.join("HEAD"), b"local-only Git history").unwrap();
                }
                "file" => fs::write(&marker, b"gitdir: /disposable/external-git\n").unwrap(),
                _ => symlink("missing-disposable-git", &marker).unwrap(),
            }
            let before = tree(fixture.paths.workspace_root());
            let error = fixture.restore().await.unwrap_err();
            assert_eq!(
                error.code, "BACKUP_RESTORE_GIT_PROTECTED",
                "{location} {kind}"
            );
            assert_eq!(error.details["operationOutcome"], "not_started");
            assert_eq!(tree(fixture.paths.workspace_root()), before);
            fixture.stop().await;
        }
    }
}

#[tokio::test]
async fn legacy_restore_rejects_an_unverified_source_root_before_any_writes() {
    let fixture = Fixture::new().await;
    fs::write(
        fixture.root.join("project.json"),
        b"invalid disposable manifest",
    )
    .unwrap();
    let before = tree(fixture.paths.workspace_root());
    let error = fixture.restore().await.unwrap_err();
    assert_eq!(error.code, "BACKUP_RESTORE_GIT_UNVERIFIED");
    assert_eq!(tree(fixture.paths.workspace_root()), before);
    fixture.stop().await;
}

#[tokio::test]
async fn legacy_restore_rejects_a_fifo_manifest_without_waiting_or_writing() {
    let fixture = Fixture::new().await;
    let manifest = fixture.root.join("project.json");
    fs::remove_file(&manifest).unwrap();
    let name = CString::new(manifest.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    let before = tree(fixture.paths.workspace_root());
    let manager = fixture.manager.clone();
    let id = fixture.id.clone();
    let backup_id = fixture.backup_id.clone();
    let runtime = tokio::runtime::Handle::current();
    let mut submitted = tokio::task::spawn_blocking(move || {
        runtime.block_on(manager.submit(
            &id,
            "restore",
            OperationInput {
                backup_id: Some(backup_id),
                ..Default::default()
            },
        ))
    });
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), &mut submitted).await;
    if result.is_err() {
        // Make a regressed blocking open finish so the test reports a failure
        // instead of leaving the runtime waiting forever for a FIFO reader.
        let _rescue = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&manifest)
            .unwrap();
        let _ = submitted.await;
        panic!("a FIFO manifest must be rejected without waiting for a writer");
    }
    let error = result.unwrap().unwrap().unwrap_err();
    assert_eq!(error.code, "BACKUP_RESTORE_GIT_UNVERIFIED");
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert_eq!(tree(fixture.paths.workspace_root()), before);
    fixture.stop().await;
}

#[tokio::test]
async fn legacy_restore_protects_nested_repositories_in_git_development_roots() {
    for location in [
        "app/packages/tool/.git",
        "app/vendor/sdk/.git",
        "docs/.git",
        "meta/.git",
    ] {
        let fixture = Fixture::new().await;
        let marker = fixture.root.join(location);
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(&marker, b"gitdir: /disposable/external-history\n").unwrap();
        let before = tree(fixture.paths.workspace_root());
        let error = fixture.restore().await.unwrap_err();
        assert_eq!(error.code, "BACKUP_RESTORE_GIT_PROTECTED", "{location}");
        assert_eq!(error.details["path"], json!(marker));
        assert_eq!(tree(fixture.paths.workspace_root()), before);
        fixture.stop().await;
    }
}

#[tokio::test]
async fn legacy_restore_does_not_treat_business_git_named_files_as_repositories() {
    let mut fixture = Fixture::new().await;
    let attachment = fixture.root.join("data/attachments/.git/record.txt");
    fs::create_dir_all(attachment.parent().unwrap()).unwrap();
    fs::write(&attachment, b"ordinary business attachment").unwrap();
    let backup = fixture
        .manager
        .submit(&fixture.id, "backup", OperationInput::default())
        .await
        .unwrap();
    let backup = wait(&fixture.tasks, &backup.task_id).await;
    assert_eq!(backup.status, TaskStatus::Succeeded, "{backup:?}");
    fixture.backup_id = backup.result.unwrap()["backupId"]
        .as_str()
        .unwrap()
        .to_owned();
    let task = fixture.restore().await.unwrap();
    let task = wait(&fixture.tasks, &task.task_id).await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
    assert_eq!(
        fs::read(&attachment).unwrap(),
        b"ordinary business attachment"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn legacy_restore_rechecks_git_before_creating_a_candidate_or_pre_restore_zip() {
    let fixture = Fixture::new().await;
    let backups_before = tree(&fixture.paths.workspace_root().join("backups"));
    // The current-thread executor cannot run the spawned restore task until
    // this test yields, modeling an editor creating Git after admission.
    let task = fixture.restore().await.unwrap();
    fs::create_dir(fixture.root.join(".git")).unwrap();
    fs::write(fixture.root.join(".git/HEAD"), b"created after admission").unwrap();
    let original = tree(&fixture.root);
    let task = wait(&fixture.tasks, &task.task_id).await;
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.error.unwrap().code, "BACKUP_RESTORE_GIT_PROTECTED");
    assert_eq!(tree(&fixture.root), original);
    assert_eq!(
        tree(&fixture.paths.workspace_root().join("backups")),
        backups_before
    );
    assert!(!fixture
        .paths
        .workspace_root()
        .join(".platform/operations")
        .join(&task.task_id)
        .exists());
    fixture.stop().await;
}

#[tokio::test]
async fn legacy_restore_rechecks_git_immediately_before_moving_the_original() {
    let fixture = Fixture::new().await;
    fs::write(
        fixture.root.join("data/attachments/preserved.txt"),
        b"current data",
    )
    .unwrap();
    let expected = Arc::new(Mutex::new(None));
    let captured = expected.clone();
    *fixture.manager.before_restore_replace.lock().unwrap() = Some(Box::new(move |root| {
        fs::create_dir(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), b"created before replacement").unwrap();
        *captured.lock().unwrap() = Some(tree(root));
    }));
    let task = fixture.restore().await.unwrap();
    let task = wait(&fixture.tasks, &task.task_id).await;
    assert_eq!(task.status, TaskStatus::Failed);
    let error = task.error.unwrap();
    assert_eq!(error.code, "BACKUP_RESTORE_GIT_PROTECTED");
    assert_eq!(error.details["operationOutcome"], "rolled_back");
    assert_eq!(
        tree(&fixture.root),
        expected.lock().unwrap().take().unwrap()
    );
    let operation = fixture
        .paths
        .workspace_root()
        .join(".platform/operations")
        .join(&task.task_id);
    assert!(!operation.join("original").exists());
    assert!(
        operation.join("candidate").is_dir(),
        "prepared recovery evidence must remain"
    );
    let journal: Value =
        serde_json::from_slice(&fs::read(operation.join("journal.json")).unwrap()).unwrap();
    assert_eq!(journal["resolved"], true);
    assert_eq!(journal["committed"], false);
    fixture.stop().await;
}

#[tokio::test]
async fn legacy_restore_without_git_still_recovers_a_missing_database() {
    let fixture = Fixture::new().await;
    fixture.store.close_database(&fixture.id).await;
    fs::remove_file(fixture.root.join("data/main.sqlite")).unwrap();
    fs::write(
        fixture.root.join("data/attachments/preserved.txt"),
        b"current data",
    )
    .unwrap();
    let task = fixture.restore().await.unwrap();
    let task = wait(&fixture.tasks, &task.task_id).await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
    assert!(fixture.root.join("data/main.sqlite").is_file());
    assert_eq!(
        fs::read(fixture.root.join("data/attachments/preserved.txt")).unwrap(),
        b"backup data"
    );
    let result = task.result.unwrap();
    let original = PathBuf::from(result["originalPath"].as_str().unwrap());
    assert!(!original.join("data/main.sqlite").exists());
    assert_eq!(
        fs::read(original.join("data/attachments/preserved.txt")).unwrap(),
        b"current data"
    );
    fixture.stop().await;
}
