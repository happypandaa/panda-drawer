//! Every cache, runner and task belongs to a fresh disposable runtime root.
use super::*;
use platform_native::windows::files as native;
use std::{
    fs,
    path::{Path, PathBuf},
};

struct Fixture {
    _temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    storage: Arc<StorageManager>,
}
impl Fixture {
    fn new() -> Self {
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let maintenance = MaintenanceManager::new(store.clone(), tasks.clone());
        let storage =
            StorageManager::new(paths.clone(), store.clone(), tasks.clone(), maintenance).unwrap();
        Self {
            _temporary: temporary,
            paths,
            store,
            tasks,
            storage,
        }
    }
    async fn stop(self) {
        assert!(self.storage.shutdown().await.is_empty());
        assert!(self.tasks.shutdown().await.is_empty());
        assert!(self.store.shutdown().await.is_empty());
    }
}
fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}
fn project_cache(paths: &RuntimePaths, id: &str) -> PathBuf {
    paths
        .workspace_root()
        .join(".platform/cache/storage/projects")
        .join(format!("{id}.json"))
}

#[tokio::test]
async fn forget_removes_only_original_project_and_global_caches_and_actual_derived_task() {
    let fixture = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    let cache = project_cache(&fixture.paths, &id);
    let other_cache = project_cache(&fixture.paths, &other);
    let storage = fixture
        .paths
        .workspace_root()
        .join(".platform/cache/storage");
    let selected = vec![
        cache.clone(),
        storage
            .join("invalidations/projects")
            .join(format!("{id}.json")),
        storage.join("workspace.json"),
        storage.join("invalidations/workspace.json"),
        fixture
            .paths
            .data_root()
            .join("cache/storage/platform.json"),
    ];
    for path in &selected {
        write(path, b"disposable cache");
    }
    write(&other_cache, b"other project cache must remain");
    let other_identity = native::open_no_reparse(&other_cache).unwrap().identity;
    native::set_read_only(&cache, true).unwrap();
    let task = fixture
        .tasks
        .create_workspace_operation("storageWorkspaceScan")
        .unwrap();
    fixture
        .tasks
        .finish(&task.task_id, Ok(json!({"disposable":true})))
        .unwrap();
    let removed = fixture.storage.forget_deleted_project(&id).await.unwrap();
    assert_eq!(removed, vec![task.task_id.clone()]);
    for path in selected {
        assert!(!path.exists(), "{}", path.display());
    }
    assert!(fixture.tasks.task(&task.task_id).is_err());
    assert!(!fixture
        .paths
        .workspace_root()
        .join(".platform/tasks")
        .join(task.task_id)
        .exists());
    assert_eq!(
        fs::read(&other_cache).unwrap(),
        b"other project cache must remain"
    );
    assert!(other_identity.same_object(&native::open_no_reparse(&other_cache).unwrap().identity));
    assert!(fixture
        .storage
        .coordinator
        .forgotten
        .lock()
        .unwrap()
        .contains(&id));
    fixture.storage.finish_deleted_project(&id);
    assert!(!fixture
        .storage
        .coordinator
        .forgotten
        .lock()
        .unwrap()
        .contains(&id));
    fixture.stop().await;
}

#[tokio::test]
async fn forget_joins_the_selected_real_runner_and_aggregate_reader_without_stopping_other_project()
{
    let fixture = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    let cache = project_cache(&fixture.paths, &id);
    write(&cache, b"before scan finishes");
    let (release, released) = tokio::sync::oneshot::channel();
    let (done, completed) = tokio::sync::oneshot::channel();
    let writer_path = cache.clone();
    fixture
        .storage
        .coordinator
        .runners
        .lock()
        .unwrap()
        .push(coordinator::StorageRunner {
            project_id: id.clone(),
            handle: tokio::spawn(async move {
                released.await.unwrap();
                fs::write(
                    writer_path,
                    b"late scan publication must precede cache retirement",
                )
                .unwrap();
                done.send(()).unwrap();
            }),
        });
    let (other_release, other_wait) = tokio::sync::oneshot::channel();
    fixture
        .storage
        .coordinator
        .runners
        .lock()
        .unwrap()
        .push(coordinator::StorageRunner {
            project_id: other.clone(),
            handle: tokio::spawn(async {
                other_wait.await.unwrap();
            }),
        });
    {
        let reader = fixture
            .storage
            .coordinator
            .readers
            .clone()
            .read_owned()
            .await;
        let forget = fixture.storage.forget_deleted_project(&id);
        tokio::pin!(forget);
        tokio::select! { biased;
            _ = &mut forget => panic!("the selected actual runner is still held"),
            _ = tokio::task::yield_now() => (),
        }
        release.send(()).unwrap();
        completed.await.unwrap();
        tokio::select! { biased;
            _ = &mut forget => panic!("the actual aggregate reader is still held"),
            _ = tokio::task::yield_now() => (),
        }
        assert_eq!(
            fs::read(&cache).unwrap(),
            b"late scan publication must precede cache retirement"
        );
        drop(reader);
        tokio::time::timeout(std::time::Duration::from_secs(5), &mut forget)
            .await
            .unwrap()
            .unwrap();
        assert!(!cache.exists());
        assert_eq!(fixture.storage.coordinator.runners.lock().unwrap().len(), 1);
        assert_eq!(
            fixture.storage.coordinator.runners.lock().unwrap()[0].project_id,
            other
        );
        other_release.send(()).unwrap();
    }
    fixture.stop().await;
}

#[test]
fn cache_retirement_preserves_other_hardlink_name_bytes_readonly_and_parent_access() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w5-storage-hardlink-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let cache = root.join("cache.json");
    let other = root.join("other-owner-name.json");
    fs::write(&cache, b"independent cache name, shared fixture bytes").unwrap();
    fs::hard_link(&cache, &other).unwrap();
    native::set_read_only(&cache, true).unwrap();
    let identity = native::open_no_reparse(&other).unwrap().identity;
    let parent = native::open_no_reparse(&root).unwrap();
    let was_private = native::verify_private(&parent.file).is_ok();
    io::remove_storage_cache(&cache).unwrap();
    assert!(!cache.exists());
    assert!(identity.same_object(&native::open_no_reparse(&other).unwrap().identity));
    assert_eq!(
        fs::read(&other).unwrap(),
        b"independent cache name, shared fixture bytes"
    );
    assert!(fs::metadata(&other).unwrap().permissions().readonly());
    assert_eq!(native::verify_private(&parent.file).is_ok(), was_private);
    native::set_read_only(&other, false).unwrap();
}

#[test]
fn a_real_shared_reader_keeps_cache_retirement_pending_until_actual_handle_release() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w5-storage-reader-")
        .tempdir()
        .unwrap();
    let path = temporary.path().join("cache.json");
    fs::write(&path, b"a real reader still holds the selected cache").unwrap();
    let reader = native::open_read_file(&path).unwrap();
    let failure = io::remove_storage_cache(&path).unwrap_err();
    assert_eq!(failure.code, "STORAGE_CACHE_REMOVE_PENDING");
    assert_eq!(failure.details["remainingPath"], json!(path));
    drop(reader);
    io::remove_storage_cache(&path).unwrap();
    assert!(!path.exists());
}

#[test]
fn a_directory_in_a_selected_cache_file_name_is_preserved() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w5-storage-foreign-")
        .tempdir()
        .unwrap();
    let path = temporary.path().join("cache.json");
    fs::create_dir(&path).unwrap();
    fs::write(path.join("foreign"), b"must remain").unwrap();
    let identity = native::open_no_reparse(&path).unwrap().identity;
    assert_eq!(
        io::remove_storage_cache(&path).unwrap_err().code,
        "UNSAFE_PATH"
    );
    assert!(identity.same_object(&native::open_no_reparse(&path).unwrap().identity));
    assert_eq!(fs::read(path.join("foreign")).unwrap(), b"must remain");
}
