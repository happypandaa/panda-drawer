//! All fixtures are disposable workspaces; no running user project is accessed.
use super::*;
use crate::{CreateProjectInput, TaskStatus};
use platform_native::windows::files as native;
use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt};
fn junction(link: &Path, target: &Path) {
    let command = platform_native::windows::system_paths::SystemPaths::current()
        .unwrap()
        .system
        .join("cmd.exe");
    let output = std::process::Command::new(command)
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Fixture {
    _temp: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    maintenance: Arc<MaintenanceManager>,
    storage: Arc<StorageManager>,
    id: String,
    root: PathBuf,
}
impl Fixture {
    async fn new() -> Self {
        let (temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let project = store
            .create(CreateProjectInput {
                name: "Disposable storage evidence".into(),
                description: None,
            })
            .await
            .unwrap();
        store.schema(&project.id, None).await.unwrap();
        store.close_database(&project.id).await.unwrap();
        fs::create_dir(project.project_root.join("data/attachments")).unwrap();
        fs::create_dir(project.project_root.join("dist")).unwrap();
        fs::rename(
            project.project_root.join("index.html"),
            project.project_root.join("dist/index.html"),
        )
        .unwrap();
        let manifest_path = project.project_root.join("panda.json");
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        manifest["web"]["root"] = "dist".into();
        fs::write(manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let tasks = BuildManager::new(paths.clone(), store.clone()).unwrap();
        let maintenance = MaintenanceManager::new(store.clone(), tasks.clone());
        let storage = StorageManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            maintenance.clone(),
        )
        .unwrap();
        Self {
            _temp: temp,
            paths,
            store,
            tasks,
            maintenance,
            storage,
            id: project.id,
            root: project.project_root,
        }
    }
    fn write(&self, path: &str, bytes: &[u8]) {
        let path = self.root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    async fn wait(&self, t: &TaskRecord) -> TaskRecord {
        for _ in 0..3000 {
            let t = self.tasks.task(&t.task_id).unwrap();
            if t.status.is_final() {
                tokio::task::yield_now().await;
                return t;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
        panic!("storage task timed out")
    }
    async fn scan(&self) -> Value {
        let t = self.storage.submit_scan(&self.id).await.unwrap();
        let t = self.wait(&t).await;
        assert_eq!(t.status, TaskStatus::Succeeded, "{t:?}");
        t.result.unwrap()
    }
    async fn stop(self) {
        self.storage.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
}
async fn wait_pause(pause: &ScanPause) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !pause.entered.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn deleted_project_forgets_cache_results_and_generation_without_touching_another_project() {
    let f = Fixture::new().await;
    let other = f
        .store
        .create(CreateProjectInput {
            name: "Disposable unaffected storage B".into(),
            description: None,
        })
        .await
        .unwrap();
    let first = f.storage.submit_scan(&f.id).await.unwrap();
    f.wait(&first).await;
    let second = f.storage.submit_scan(&other.id).await.unwrap();
    f.wait(&second).await;
    MaintenanceManager::invalidate_storage(f.paths.workspace_root(), &f.id).unwrap();
    let global = f
        .tasks
        .create_workspace_operation("storageWorkspaceScan")
        .unwrap();
    f.tasks
        .finish(&global.task_id, Ok(json!({"projects":[f.id,other.id]})))
        .unwrap();
    for path in [
        f.paths.data_root().join("cache/storage/platform.json"),
        f.paths
            .workspace_root()
            .join(".platform/cache/storage/workspace.json"),
    ] {
        builds::atomic_json(&path, &json!({"projectId":f.id})).unwrap();
    }
    let forgotten = f.storage.forget_deleted_project(&f.id).await.unwrap();
    assert!(forgotten.contains(&first.task_id));
    assert!(forgotten.contains(&global.task_id));
    assert!(!forgotten.contains(&second.task_id));
    assert!(f.tasks.task(&first.task_id).is_err());
    assert!(f.tasks.task(&global.task_id).is_err());
    assert!(f.tasks.task(&second.task_id).is_ok());
    assert!(!f.storage.cache(&f.id).exists());
    assert!(f.storage.cache(&other.id).exists());
    assert!(!f.storage.generations.lock().unwrap().contains_key(&f.id));
    assert!(f.storage.submit_scan(&f.id).await.is_err());
    assert!(f.storage.workspace_info().await.is_err());
    assert!(!f
        .paths
        .workspace_root()
        .join(".platform/cache/storage/invalidations/projects")
        .join(format!("{}.json", f.id))
        .exists());
    assert!(!f
        .paths
        .data_root()
        .join("cache/storage/platform.json")
        .exists());
    assert_eq!(
        f.storage.forget_deleted_project(&f.id).await.unwrap(),
        Vec::<String>::new()
    );
    f.stop().await;
}
#[tokio::test]
async fn deletion_waits_for_real_scan_before_publication_then_removes_its_terminal_task() {
    let f = Fixture::new().await;
    let pause = Arc::new(ScanPause::default());
    *f.storage.scan_pause.lock().unwrap() = Some(pause.clone());
    let scan = f.storage.submit_scan(&f.id).await.unwrap();
    wait_pause(&pause).await;
    let manager = f.storage.clone();
    let id = f.id.clone();
    let deletion = tokio::spawn(async move { manager.forget_deleted_project(&id).await });
    while !f
        .storage
        .coordinator
        .forgotten
        .lock()
        .unwrap()
        .contains(&f.id)
    {
        tokio::task::yield_now().await;
    }
    assert!(!deletion.is_finished());
    assert!(f.root.exists());
    pause.release();
    let forgotten = deletion.await.unwrap().unwrap();
    assert!(forgotten.contains(&scan.task_id));
    assert!(!f.storage.cache(&f.id).exists());
    assert!(f.tasks.task(&scan.task_id).is_err());
    assert!(!f.storage.generations.lock().unwrap().contains_key(&f.id));
    f.stop().await;
}
#[tokio::test]
async fn deletion_drains_a_real_workspace_scan_and_clears_its_late_aggregate() {
    let f = Fixture::new().await;
    let pause = Arc::new(ScanPause::default());
    *f.storage.aggregate_pause.lock().unwrap() = Some(pause.clone());
    let manager = f.storage.clone();
    let scan = tokio::spawn(async move { manager.workspace_info().await });
    wait_pause(&pause).await;
    let manager = f.storage.clone();
    let id = f.id.clone();
    let deletion = tokio::spawn(async move { manager.forget_deleted_project(&id).await });
    while !f
        .storage
        .coordinator
        .forgotten
        .lock()
        .unwrap()
        .contains(&f.id)
    {
        tokio::task::yield_now().await;
    }
    assert!(!deletion.is_finished());
    pause.release();
    scan.await.unwrap().unwrap();
    deletion.await.unwrap().unwrap();
    assert!(!f
        .paths
        .workspace_root()
        .join(".platform/cache/storage/workspace.json")
        .exists());
    assert!(f.storage.workspace_info().await.is_err());
    f.stop().await;
}
#[tokio::test]
async fn workspace_project_rows_keep_internal_ids_and_actual_readable_paths() {
    let fixture = Fixture::new().await;
    assert_ne!(
        fixture.root.file_name().unwrap().to_str().unwrap(),
        fixture.id
    );
    let before = fs::read(fixture.root.join("data/main.sqlite")).unwrap();
    let result = workspace_scan(&fixture.paths, None, "readable-project-row").unwrap();
    let rows = result["workspace"]["projects"].as_array().unwrap();
    let row = rows
        .iter()
        .find(|row| row["scope"]["projectId"] == fixture.id)
        .unwrap();
    assert_eq!(row["name"], "Disposable storage evidence");
    assert_eq!(row["root"], json!(fixture.root));
    assert_eq!(row["referenceOnly"], true);
    assert_eq!(
        fs::read(fixture.root.join("data/main.sqlite")).unwrap(),
        before
    );
    fixture.stop().await;
}

#[tokio::test]
async fn logical_categories_are_disjoint_include_hidden_metadata_and_never_open_bad_sqlite() {
    let f = Fixture::new().await;
    let original_output = fs::metadata(f.root.join("dist/index.html")).unwrap().len();
    for (path, bytes) in [
        ("public/photo.png", b"same-image".as_slice()),
        ("dist/photo.png", b"same-image"),
        ("meta/icon.png", b"icon"),
        ("node_modules/dependency", b"deps"),
        ("data/attachments/node_modules/kept", b"user"),
        ("data/attachments/.git/kept", b"hidden"),
        ("data/main.sqlite", b"this is damaged sqlite"),
        ("data/main.sqlite-wal", b"raw wal"),
        ("data/main.sqlite-shm", b"raw shm"),
        (".mystery/cache/file", b"unknown"),
    ] {
        f.write(path, bytes);
    }
    let before = fs::read(f.root.join("data/main.sqlite")).unwrap();
    let uploads = f.store.private_dir(&f.id).unwrap().join("uploads");
    fs::create_dir_all(&uploads).unwrap();
    fs::write(uploads.join(".upload-disposable"), b"temporary").unwrap();
    let result = f.scan().await;
    let current = &result["currentProject"];
    assert_eq!(current["complete"], true, "{result}");
    assert_eq!(current["categories"].as_object().unwrap().len(), 7);
    assert_eq!(current["categories"]["source"]["fixedAssets"]["bytes"], 10);
    assert_eq!(
        current["categories"]["output"]["bytes"],
        original_output + 10
    );
    assert_eq!(current["categories"]["dependencies"]["bytes"], 4);
    assert_eq!(current["categories"]["attachments"]["bytes"], 10);
    assert_eq!(current["categories"]["cache"]["uploadStaging"]["bytes"], 9);
    assert_eq!(
        current["categories"]["cache"]["uploadStaging"]["protected"],
        true
    );
    assert_eq!(current["categories"]["database"]["companions"]["bytes"], 14);
    let total = current["categories"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| numeric(&v["bytes"]).unwrap())
        .sum::<u64>();
    assert_eq!(numeric(&current["totalBytes"]), Some(total));
    assert_eq!(fs::read(f.root.join("data/main.sqlite")).unwrap(), before);
    assert_eq!(
        fs::read(f.root.join("data/main.sqlite-wal")).unwrap(),
        b"raw wal"
    );
    assert_eq!(
        fs::read(f.root.join("data/main.sqlite-shm")).unwrap(),
        b"raw shm"
    );
    f.stop().await;
}

#[tokio::test]
async fn symlinks_and_invalid_manifest_report_partial_values_without_counting_external_data() {
    let f = Fixture::new().await;
    let external = f.paths.data_root().join("outside");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("outside-data"), vec![0; 100_000]).unwrap();
    junction(&f.root.join("data/attachments/outside"), &external);
    let result = f.scan().await;
    assert_eq!(result["currentProject"]["complete"], false);
    assert_eq!(
        result["currentProject"]["categories"]["attachments"]["bytes"],
        0
    );
    assert!(result["currentProject"]["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|i| i["reason"] == "reparse_not_counted"));
    fs::write(f.root.join("panda.json"), b"broken manifest").unwrap();
    let result = f.scan().await;
    assert_eq!(result["currentProject"]["complete"], false);
    assert!(numeric(&result["currentProject"]["categories"]["other"]["bytes"]).unwrap() > 0);
    f.stop().await;
}
#[tokio::test]
async fn cache_cold_corrupt_restart_and_known_invalidation_never_report_zero_or_publish_old_scan() {
    let f = Fixture::new().await;
    let cold = f.storage.info(&f.id).await.unwrap();
    assert_eq!(cold["currentProject"]["status"], "not_scanned");
    assert!(cold["currentProject"]["totalBytes"].is_null());
    let t: TaskRecord = serde_json::from_value(cold["activeTask"].clone()).unwrap();
    f.wait(&t).await;
    let warm = f.storage.info(&f.id).await.unwrap();
    assert_eq!(warm["currentProject"]["stale"], false);
    let (old, issues) = f.storage.binding(&f.id).unwrap();
    MaintenanceManager::invalidate_storage(f.paths.workspace_root(), &f.id).unwrap();
    assert_eq!(
        f.storage.scan_project(&f.id, old, issues).unwrap_err().code,
        "STORAGE_SCAN_INVALIDATED"
    );
    assert!(!f.storage.cache(&f.id).exists());
    f.scan().await;
    fs::write(f.storage.cache(&f.id), b"broken cache").unwrap();
    let response = f.storage.info(&f.id).await.unwrap();
    assert_eq!(response["currentProject"]["status"], "not_scanned");
    let t: TaskRecord = serde_json::from_value(response["activeTask"].clone()).unwrap();
    f.wait(&t).await;
    let restarted = StorageManager::new(
        f.paths.clone(),
        f.store.clone(),
        f.tasks.clone(),
        f.maintenance.clone(),
    )
    .unwrap();
    let response = restarted.info(&f.id).await.unwrap();
    assert_eq!(response["currentProject"]["stale"], true);
    restarted.shutdown().await;
    f.stop().await;
}
#[tokio::test]
async fn root_replacement_rejects_old_scan_and_scan_submission_deduplicates() {
    let f = Fixture::new().await;
    let task = f
        .tasks
        .create_readonly_operation(&f.id, "storageScan")
        .unwrap();
    f.storage
        .scans
        .lock()
        .unwrap()
        .insert(f.id.clone(), task.task_id.clone());
    assert_eq!(
        f.storage.submit_scan(&f.id).await.unwrap().task_id,
        task.task_id
    );
    f.tasks.finish(&task.task_id, Ok(json!({}))).unwrap();
    let (binding, issues) = f.storage.binding(&f.id).unwrap();
    let old = f.root.with_extension("retained");
    fs::rename(&f.root, &old).unwrap();
    fs::create_dir(&f.root).unwrap();
    fs::copy(old.join("panda.json"), f.root.join("panda.json")).unwrap();
    assert_eq!(
        f.storage
            .scan_project(&f.id, binding, issues)
            .unwrap_err()
            .code,
        "STORAGE_SCAN_INVALIDATED"
    );
    assert!(!f.storage.cache(&f.id).exists());
    f.stop().await;
}
#[tokio::test]
async fn attachments_use_bounded_pages_stable_ties_and_reject_changed_cursors() {
    let f = Fixture::new().await;
    for n in 0..1105 {
        f.write(&format!("data/attachments/photos/{n:04}.png"), b"img");
    }
    f.write("data/attachments/other.txt", b"text");
    let query = json!({"type":"image","prefix":"photos/","sort":"size","order":"desc","limit":500});
    let first = f.storage.files(&f.id, query.clone()).await.unwrap();
    assert_eq!(first["files"].as_array().unwrap().len(), 500);
    assert_eq!(first["files"][0]["path"], "photos/0000.png");
    assert_eq!(first["files"][0]["size"], 3);
    assert_eq!(first["files"][0]["mediaType"], "image/png");
    let mut next = query.clone();
    next["cursor"] = first["nextCursor"].clone();
    let second = f.storage.files(&f.id, next.clone()).await.unwrap();
    assert_eq!(second["files"][0]["path"], "photos/0500.png");
    assert_eq!(second["files"].as_array().unwrap().len(), 500);
    next["cursor"] = second["nextCursor"].clone();
    let third = f.storage.files(&f.id, next).await.unwrap();
    assert_eq!(third["files"].as_array().unwrap().len(), 105);
    assert!(third["nextCursor"].is_null());
    f.write("data/attachments/photos/new.png", b"new");
    let mut stale = query;
    stale["cursor"] = first["nextCursor"].clone();
    assert_eq!(
        f.storage.files(&f.id, stale).await.unwrap_err().code,
        "STORAGE_CURSOR_INVALID"
    );
    assert!(f
        .storage
        .files(&f.id, json!({"prefix":"../"}))
        .await
        .is_err());
    assert!(f.storage.files(&f.id, json!({"limit":501})).await.is_err());
    f.stop().await;
}

#[tokio::test]
async fn legal_long_utf16_attachment_components_scan_and_filter_without_utf8_byte_rejection() {
    let f = Fixture::new().await;
    let directory = "中".repeat(86);
    let file = format!("{}.png", "文".repeat(86));
    assert!(directory.len() > 255 && directory.encode_utf16().count() < 255);
    assert!(file.len() > 255 && file.encode_utf16().count() < 255);
    let relative = format!("{directory}/{file}");
    f.write(
        &format!("data/attachments/{relative}"),
        b"native long name attachment",
    );
    let path = f.root.join("data/attachments").join(&directory).join(&file);
    assert!(path.as_os_str().encode_wide().count() > 260);
    let before = native::open_no_reparse(&path).unwrap().identity;
    let scan = f.scan().await;
    assert_eq!(
        scan["currentProject"]["categories"]["attachments"]["bytes"],
        b"native long name attachment".len()
    );
    let page = f
        .storage
        .files(&f.id, json!({"prefix":directory,"type":"image"}))
        .await
        .unwrap();
    assert_eq!(page["complete"], true);
    assert_eq!(page["files"].as_array().unwrap().len(), 1);
    assert_eq!(page["files"][0]["path"], relative);
    assert_eq!(
        page["files"][0]["size"],
        b"native long name attachment".len()
    );
    assert!(before.same_object(&native::open_no_reparse(&path).unwrap().identity));
    assert_eq!(fs::read(path).unwrap(), b"native long name attachment");
    f.stop().await;
}

#[tokio::test]
async fn cache_binding_uses_all_native_file_id_bits_and_existing_directory_acl_is_unchanged() {
    let f = Fixture::new().await;
    let root = native::open_no_reparse(&f.root).unwrap();
    let was_private = native::verify_private(&root.file).is_ok();
    f.scan().await;
    let path = f.storage.cache(&f.id);
    let mut envelope: Value = read_json(&path).unwrap();
    let id = &mut envelope["binding"]["projectRootIdentity"]["fileId"];
    let previous = id[15].as_u64().unwrap();
    id[15] = json!(previous ^ 0x80);
    builds::atomic_json(&path, &envelope).unwrap();
    let stale = f.storage.info(&f.id).await.unwrap();
    assert_eq!(stale["currentProject"]["stale"], true);
    let task: TaskRecord = serde_json::from_value(stale["activeTask"].clone()).unwrap();
    assert_eq!(f.wait(&task).await.status, TaskStatus::Succeeded);
    assert_eq!(native::verify_private(&root.file).is_ok(), was_private);
    drop(root);
    f.stop().await;
}
#[tokio::test]
async fn cleanup_only_deletes_identified_dependencies_completed_cache_and_logs() {
    let f = Fixture::new().await;
    let (build, lease) = f.tasks.create_operation(&f.id, "build").unwrap();
    f.tasks.finish(&build.task_id, Ok(json!({}))).unwrap();
    drop(lease);
    for path in [
        "node_modules/package/file",
        "data/attachments/node_modules/user",
        "dist/index.html",
        ".git/HEAD",
        "custom-cache/author.txt",
    ] {
        f.write(path, b"keep or clean");
    }
    let private = f.store.private_dir(&f.id).unwrap();
    let completed = private.join("builds").join(&build.task_id);
    let unknown = private.join("builds/task_unknown");
    fs::create_dir_all(completed.join("home")).unwrap();
    fs::write(completed.join("home/temp"), b"completed staging").unwrap();
    fs::create_dir_all(&unknown).unwrap();
    fs::write(unknown.join("payload"), b"unknown retained").unwrap();
    fs::create_dir_all(private.join("npm-cache")).unwrap();
    fs::write(private.join("npm-cache/payload"), b"cache").unwrap();
    fs::create_dir_all(private.join("uploads")).unwrap();
    fs::write(private.join("uploads/.upload-protected"), b"upload").unwrap();
    let task = f
        .storage
        .submit_cleanup(
            &f.id,
            json!({"categories":["dependencies","buildCache","logs"]}),
        )
        .await
        .unwrap();
    let task = f.wait(&task).await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
    assert!(!f.root.join("node_modules").exists());
    assert!(!completed.exists());
    assert!(!private.join("npm-cache").exists());
    assert!(unknown.join("payload").is_file());
    assert!(private.join("uploads/.upload-protected").is_file());
    for path in [
        "data/attachments/node_modules/user",
        "dist/index.html",
        ".git/HEAD",
        "custom-cache/author.txt",
    ] {
        assert!(f.root.join(path).is_file(), "{path}");
    }
    assert!(!f.root.join(".platform").exists());
    assert_eq!(f.store.control(&f.id).unwrap().get().mode, "ready");
    f.stop().await;
}
#[tokio::test]
async fn cleanup_waits_for_project_readers_and_rechecks_new_recovery_protection() {
    let f = Fixture::new().await;
    f.write("node_modules/kept", b"dependency");
    let reader = f.store.gate(&f.id).read_owned().await;
    let task = f
        .storage
        .submit_cleanup(&f.id, json!({"categories":["dependencies"]}))
        .await
        .unwrap();
    // Admission reserves the project immediately, while deletion must wait for
    // existing readers and then inspect the current protection state again.
    tokio::task::yield_now().await;
    assert!(!f.tasks.task(&task.task_id).unwrap().status.is_final());
    assert_eq!(
        fs::read(f.root.join("node_modules/kept")).unwrap(),
        b"dependency"
    );
    assert_eq!(
        f.storage
            .submit_cleanup(&f.id, json!({"categories":["dependencies"]}))
            .await
            .unwrap_err()
            .code,
        "PROJECT_BUSY"
    );
    let control = f.store.control(&f.id).unwrap();
    let mut state = control.get();
    state.recovery_required = true;
    control.set(state).unwrap();
    drop(reader);
    let task = f.wait(&task).await;
    assert_eq!(task.status, TaskStatus::Failed, "{task:?}");
    assert_eq!(task.error.as_ref().unwrap().code, "RECOVERY_REQUIRED");
    assert_eq!(
        task.result.as_ref().unwrap()["operationOutcome"],
        "not_started"
    );
    assert_eq!(
        fs::read(f.root.join("node_modules/kept")).unwrap(),
        b"dependency"
    );
    f.stop().await;
}
#[tokio::test]
async fn cleanup_respects_task_maintenance_and_recovery_protection() {
    let f = Fixture::new().await;
    f.write("node_modules/kept", b"dependency");
    let (_t, lease) = f.tasks.create_operation(&f.id, "backup").unwrap();
    assert_eq!(
        f.storage
            .submit_cleanup(&f.id, json!({"categories":["dependencies"]}))
            .await
            .unwrap_err()
            .code,
        "PROJECT_BUSY"
    );
    drop(lease);
    let control = f.store.control(&f.id).unwrap();
    let mut state = control.get();
    state.mode = "maintenance".into();
    state.maintenance_id = Some("maintenance_test".into());
    state.ready_for_changes = false;
    control.set(state).unwrap();
    assert_eq!(
        f.storage
            .submit_cleanup(
                &f.id,
                json!({"categories":["dependencies"],"maintenanceId":"maintenance_test"})
            )
            .await
            .unwrap_err()
            .code,
        "PROJECT_MAINTENANCE"
    );
    control.set(Default::default()).unwrap();
    let operation = f
        .paths
        .workspace_root()
        .join(".platform/operations/task_protected");
    fs::create_dir_all(&operation).unwrap();
    builds::atomic_json(
        &operation.join("journal.json"),
        &json!({"projectId":f.id,"taskId":"task_protected","resolved":false}),
    )
    .unwrap();
    assert_eq!(
        f.storage
            .submit_cleanup(&f.id, json!({"categories":["dependencies"]}))
            .await
            .unwrap_err()
            .code,
        "RECOVERY_REQUIRED"
    );
    assert!(f.root.join("node_modules/kept").exists());
    assert!(f
        .storage
        .submit_cleanup(&f.id, json!({"categories":["attachments"]}))
        .await
        .is_err());
    f.stop().await;
}
#[tokio::test]
async fn cleanup_external_symlink_is_retained_and_partial_result_survives() {
    let f = Fixture::new().await;
    f.write("node_modules/ordinary", b"deleted");
    let external = f.paths.data_root().join("external-keep");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("outside-data"), b"external untouched").unwrap();
    junction(&f.root.join("node_modules/external"), &external);
    let task = f
        .storage
        .submit_cleanup(&f.id, json!({"categories":["dependencies"]}))
        .await
        .unwrap();
    let task = f.wait(&task).await;
    assert_eq!(task.status, TaskStatus::Failed);
    let result = task.result.unwrap();
    assert_eq!(result["logicalBytes"], 7);
    assert_eq!(result["fileCount"], 1);
    assert!(!result["failures"].as_array().unwrap().is_empty());
    assert_eq!(
        fs::read(external.join("outside-data")).unwrap(),
        b"external untouched"
    );
    assert!(platform_native::filesystem::is_reparse(
        &fs::symlink_metadata(f.root.join("node_modules/external")).unwrap()
    ));
    f.stop().await;
}
#[tokio::test]
async fn interrupted_cleanup_uses_task_status_and_never_replays_deletion() {
    let f = Fixture::new().await;
    f.write("node_modules/residual", b"do not replay");
    let task = f
        .tasks
        .create_readonly_operation(&f.id, "storageCleanup")
        .unwrap();
    f.tasks.phase(&task.task_id, "deleting").unwrap();
    f.storage.shutdown().await;
    f.tasks.shutdown().await;
    let tasks = BuildManager::new(f.paths.clone(), f.store.clone()).unwrap();
    let maintenance = MaintenanceManager::new(f.store.clone(), tasks.clone());
    let restarted = StorageManager::new(
        f.paths.clone(),
        f.store.clone(),
        tasks.clone(),
        maintenance.clone(),
    )
    .unwrap();
    let task = tasks.task(&task.task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Interrupted);
    assert!(task.result.is_none());
    assert!(!f
        .paths
        .workspace_root()
        .join(".platform/operations")
        .join(&task.task_id)
        .join("cleanup.json")
        .exists());
    assert_eq!(
        fs::read(f.root.join("node_modules/residual")).unwrap(),
        b"do not replay"
    );
    restarted.shutdown().await;
    tasks.shutdown().await;
    f.stop().await;
}
#[tokio::test]
async fn workspace_ranges_are_disjoint_include_retained_copy_and_exclude_program_links() {
    let f = Fixture::new().await;
    let app = f.paths.data_root().join("Actual Installation");
    fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    fs::write(app.join("Contents/MacOS/test"), b"program").unwrap();
    let external = f._temp.path().join("external");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("outside-data"), vec![0; 10000]).unwrap();
    junction(&app.join("Contents/external"), &external);
    let retained = f._temp.path().canonicalize().unwrap().join("retained");
    fs::create_dir(&retained).unwrap();
    fs::write(retained.join("evidence"), b"retained").unwrap();
    let identity = PersistedIdentity::capture(&retained).unwrap();
    builds::atomic_json(
        &f.paths.migrations_dir().join("task_retained/journal.json"),
        &json!({"committed":true,"sourcePath":retained,"sourceIdentity":identity}),
    )
    .unwrap();
    let manager = f._temp.path().join("disposable-manager.exe");
    let cli = f._temp.path().join("disposable-cli.exe");
    fs::write(&manager, []).unwrap();
    fs::write(&cli, []).unwrap();
    let installation = crate::installation_scope::InstallationScope::for_test(&manager, &cli, &app);
    let result = workspace_scan(&f.paths, Some(&installation), "test-generation").unwrap();
    assert_eq!(result["platformProgram"]["totalBytes"], 7);
    assert_eq!(result["platformProgram"]["complete"], false);
    let other = result["otherLocalLocations"].as_array().unwrap();
    assert_eq!(
        other
            .iter()
            .find(|v| v["scope"]["kind"] == "retainedWorkspace")
            .unwrap()["totalBytes"],
        8
    );
    let data = other
        .iter()
        .find(|v| v["scope"]["kind"] == "applicationData")
        .unwrap();
    let exclusions = data["excludedRoots"].as_array().unwrap();
    assert!(exclusions.contains(&json!("workspace")));
    assert!(exclusions.contains(&json!("Actual Installation")));
    let sum = numeric(&result["workspace"]["totalBytes"]).unwrap()
        + 7
        + other
            .iter()
            .map(|v| numeric(&v["totalBytes"]).unwrap())
            .sum::<u64>();
    assert_eq!(numeric(&result["knownLocalTotal"]["totalBytes"]), Some(sum));
    assert_eq!(result["knownLocalTotal"]["complete"], false);
    assert!(f
        .paths
        .data_root()
        .join("cache/storage/platform.json")
        .is_file());
    f.stop().await;
}

#[tokio::test]
async fn installed_apps_are_reported_per_app_inside_application_data() {
    let f = Fixture::new().await;
    let node_root = f.paths.data_root().join("apps");
    let source = f
        .paths
        .workspace_root()
        .join("project-sources/disposable-node-accounting");
    let (id, app) = add_disposable_app_storage(&f.paths, &source, true).await;
    // Unregistered or non-app directories are not listed as apps.
    fs::create_dir_all(node_root.join(".disposable-scratch")).unwrap();
    fs::write(node_root.join(".disposable-scratch/file"), b"12345").unwrap();
    fs::create_dir_all(node_root.join(Uuid::new_v4().to_string())).unwrap();
    let result = workspace_scan(&f.paths, None, "node-apps-test").unwrap();
    let rows = result["apps"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let row = rows.iter().find(|row| row["scope"]["appId"] == id).unwrap();
    assert_eq!(row["referenceOnly"], true);
    assert_eq!(row["instance"]["categories"]["configuration"]["bytes"], 6);
    assert_eq!(row["instance"]["categories"]["logs"]["bytes"], 3);
    assert_eq!(row["instance"]["categories"]["cache"]["bytes"], 5);
    assert!(row["instance"]["categories"].get("data").is_none());
    assert!(row["instance"]["categories"].get("program").is_none());
    assert_eq!(row["projectFolder"]["categories"]["data"]["bytes"], 4);
    assert_eq!(
        numeric(&row["projectFolder"]["totalBytes"]),
        Some(app_source_bytes(&source) + 5 + 10 + 4)
    );
    assert!(!app.join("data").exists());
    let data = result["otherLocalLocations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["scope"]["kind"] == "applicationData")
        .unwrap();
    // The app row is a reference into these disjoint scopes, never an extra total.
    assert_eq!(
        numeric(&data["categories"]["apps"]["bytes"]),
        Some(
            rows.iter()
                .map(|row| numeric(&row["instance"]["totalBytes"]).unwrap())
                .sum::<u64>()
                + 5
        )
    );
    let sum = numeric(&result["workspace"]["totalBytes"]).unwrap()
        + result["otherLocalLocations"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|scope| numeric(&scope["totalBytes"]))
            .sum::<u64>();
    assert_eq!(numeric(&result["knownLocalTotal"]["totalBytes"]), Some(sum));
    f.stop().await;
}

#[tokio::test]
async fn retained_workspace_accounting_requires_actual_volume_and_all_128_file_id_bits() {
    let f = Fixture::new().await;
    let retained = f
        ._temp
        .path()
        .canonicalize()
        .unwrap()
        .join("retained-full-windows-identity");
    fs::create_dir(&retained).unwrap();
    fs::write(retained.join("evidence"), b"retained").unwrap();
    let actual = PersistedIdentity::capture(&retained).unwrap();
    let mut wrong_volume = actual.clone();
    wrong_volume.device = wrong_volume.device.wrapping_add(1);
    let mut high_bits = actual.clone();
    high_bits.windows_file_id.as_mut().unwrap()[15] ^= 0x80;
    let mut incomplete = actual.clone();
    incomplete.windows_file_id = None;
    for (expected, allowed) in [
        (actual, true),
        (wrong_volume, false),
        (high_bits, false),
        (incomplete, false),
    ] {
        builds::atomic_json(
            &f.paths
                .migrations_dir()
                .join("task_retained_identity/journal.json"),
            &json!({"committed":true,"sourcePath":retained,"sourceIdentity":expected}),
        )
        .unwrap();
        let result = workspace_scan(&f.paths, None, "full-windows-identity").unwrap();
        let scope = result["otherLocalLocations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|scope| scope["scope"]["kind"] == "retainedWorkspace")
            .unwrap();
        assert_eq!(
            scope["totalBytes"],
            if allowed { json!(8) } else { Value::Null },
            "{scope}"
        );
    }
    assert_eq!(fs::read(retained.join("evidence")).unwrap(), b"retained");
    f.stop().await;
}
#[test]
fn zero_and_large_integer_counters_remain_exact() {
    assert_eq!(int(0), json!(0));
    assert_eq!(
        int(9_007_199_254_740_992),
        json!({"$int64":"9007199254740992"})
    );
    let temp = tempfile::tempdir().unwrap();
    let result = scope_scan(
        &temp.path().canonicalize().unwrap(),
        json!({"kind":"empty"}),
        &[],
        |_| "other".into(),
    );
    assert_eq!(result["totalBytes"], 0);
    assert_eq!(result["complete"], true);
    let missing = scope_scan(
        &temp.path().canonicalize().unwrap().join("missing"),
        json!({"kind":"missing"}),
        &[],
        |_| "other".into(),
    );
    assert!(missing["totalBytes"].is_null());
    assert_eq!(missing["complete"], false);
}

async fn add_disposable_app_storage(
    paths: &RuntimePaths,
    source: &Path,
    platform_owned: bool,
) -> (String, PathBuf) {
    fs::create_dir_all(source).unwrap();
    let store = ProjectStore::with_paths(
        paths.workspace_root().into(),
        paths.data_root().into(),
        None,
        Default::default(),
    )
    .unwrap();
    let platform_id = Uuid::new_v4().to_string();
    let task_id = format!("task_{}", Uuid::new_v4().simple());
    if platform_owned {
        store
            .reserve_managed_source(
                &platform_id,
                &task_id,
                source,
                &PersistedIdentity::capture(source).unwrap(),
            )
            .unwrap();
    }
    fs::create_dir_all(source.join("node_modules/tiny")).unwrap();
    fs::create_dir_all(source.join(".git")).unwrap();
    fs::create_dir_all(source.join("web")).unwrap();
    fs::create_dir_all(source.join("backend")).unwrap();
    fs::write(source.join("code.js"), b"abc").unwrap();
    fs::write(source.join("node_modules/tiny/index.js"), b"12345").unwrap();
    fs::write(source.join(".git/HEAD"), b"ref: main\n").unwrap();
    fs::write(source.join("web/index.html"), b"<p>Disposable storage</p>").unwrap();
    fs::write(
        source.join("backend/main.mjs"),
        b"throw Error('never execute')",
    )
    .unwrap();
    fs::write(
        source.join("panda.json"),
        serde_json::to_vec(&json!({
            "manifestVersion":2,"app":{"name":"Disposable storage"},
            "web":{"root":"web","entry":"index.html"},
            "backend":{"root":"backend","entry":"main.mjs","nodeMajor":24,"protocolVersion":1}
        }))
        .unwrap(),
    )
    .unwrap();
    let registered = if platform_owned {
        store
            .register_managed(&platform_id, &task_id, source)
            .await
            .unwrap()
    } else {
        store.register_local(source).await.unwrap()
    };
    let id = registered.id;
    let slot = paths.data_root().join("apps").join(&id);
    fs::create_dir(source.join("data")).unwrap();
    fs::write(source.join("data/business.db"), b"data").unwrap();
    fs::create_dir_all(slot.join("configuration")).unwrap();
    fs::create_dir_all(slot.join("cache")).unwrap();
    fs::write(slot.join("configuration/settings.json"), b"config").unwrap();
    fs::create_dir_all(slot.join("logs")).unwrap();
    fs::write(slot.join("logs/current.log"), b"log").unwrap();
    fs::write(slot.join("cache/payload"), b"cache").unwrap();
    (id, slot)
}
fn app_source_bytes(source: &Path) -> u64 {
    [
        "code.js",
        "panda.json",
        "web/index.html",
        "backend/main.mjs",
    ]
    .into_iter()
    .map(|path| fs::metadata(source.join(path)).unwrap().len())
    .sum()
}

#[tokio::test]
async fn app_storage_includes_dependencies_and_marks_source_ownership_without_double_counting() {
    let f = Fixture::new().await;
    let managed = f
        .paths
        .workspace_root()
        .join("project-sources/disposable-managed-node");
    let external = f
        ._temp
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-external-node");
    let (managed_id, _) = add_disposable_app_storage(&f.paths, &managed, true).await;
    let (external_id, _) = add_disposable_app_storage(&f.paths, &external, false).await;
    let result = workspace_scan(&f.paths, None, "node-sources").unwrap();
    for (id, owned, source) in [
        (&managed_id, true, &managed),
        (&external_id, false, &external),
    ] {
        let source_bytes = app_source_bytes(source);
        let folder_bytes = source_bytes + 5 + 10 + 4;
        let row = result["apps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["scope"]["appId"] == *id)
            .unwrap();
        assert_eq!(row["projectFolder"]["totalBytes"], folder_bytes);
        assert_eq!(
            row["projectFolder"]["categories"]["dependencies"]["bytes"],
            5
        );
        assert_eq!(row["projectFolder"]["categories"]["git"]["bytes"], 10);
        assert_eq!(
            row["projectFolder"]["categories"]["source"]["bytes"],
            source_bytes
        );
        assert_eq!(row["projectFolder"]["categories"]["data"]["bytes"], 4);
        assert!(!f
            .paths
            .data_root()
            .join("apps")
            .join(id)
            .join("data")
            .exists());
        assert_eq!(
            row["sourceOwnership"],
            if owned { "platform" } else { "user" }
        );
        assert_eq!(row["sourcePreservedOnDelete"], !owned);
        assert_eq!(
            row["instance"]["categories"]["data"]["bytes"]
                .as_u64()
                .unwrap_or(0),
            0
        );
        assert_eq!(row["instance"]["categories"]["configuration"]["bytes"], 6);
        assert_eq!(row["instance"]["categories"]["logs"]["bytes"], 3);
        assert_eq!(row["instance"]["categories"]["cache"]["bytes"], 5);
        assert_eq!(
            numeric(&row["totalBytes"]),
            Some(folder_bytes + numeric(&row["instance"]["totalBytes"]).unwrap())
        );
        assert_eq!(row["referenceOnly"], true);
        assert_eq!(row["projectFolder"]["referenceOnly"], true);
    }
    let other = result["otherLocalLocations"].as_array().unwrap();
    assert!(other
        .iter()
        .any(|scope| scope["scope"]["kind"] == "appSource"
            && scope["root"] == json!(external)
            && scope["totalBytes"] == app_source_bytes(&external) + 5 + 10 + 4));
    assert!(!other.iter().any(|scope| scope["root"] == json!(managed)));
    let sum = numeric(&result["workspace"]["totalBytes"]).unwrap()
        + other
            .iter()
            .filter_map(|scope| numeric(&scope["totalBytes"]))
            .sum::<u64>();
    assert_eq!(numeric(&result["knownLocalTotal"]["totalBytes"]), Some(sum));
    assert!(external.join("node_modules/tiny/index.js").is_file());
    f.stop().await;
}

#[tokio::test]
async fn missing_or_replaced_app_source_is_not_measured_as_the_original_project() {
    for replaced in [false, true] {
        let f = Fixture::new().await;
        let source = f
            ._temp
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-replaceable-node");
        let (id, _) = add_disposable_app_storage(&f.paths, &source, false).await;
        fs::rename(&source, source.with_file_name("disposable-original-node")).unwrap();
        if replaced {
            fs::create_dir(&source).unwrap();
            fs::write(source.join("unrelated-user-file"), [0u8; 512]).unwrap();
        }
        let result = workspace_scan(&f.paths, None, "missing-node-source").unwrap();
        let row = result["apps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["scope"]["appId"] == id)
            .unwrap();
        assert!(row["projectFolder"]["totalBytes"].is_null());
        assert!(row["totalBytes"].is_null());
        assert_eq!(row["complete"], false);
        assert!(row["instance"]["totalBytes"].as_u64().unwrap() > 0);
        assert!(source
            .with_file_name("disposable-original-node/code.js")
            .exists());
        if replaced {
            assert_eq!(
                fs::metadata(source.join("unrelated-user-file"))
                    .unwrap()
                    .len(),
                512
            );
        }
        f.stop().await;
    }
}
