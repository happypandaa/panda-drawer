//! All fixtures are disposable workspaces; no running user project is accessed.
use super::*;
use crate::{CreateProjectInput, TaskStatus};

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
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
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
    while !f.storage.forgotten.lock().unwrap().contains(&f.id) {
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
    while !f.storage.forgotten.lock().unwrap().contains(&f.id) {
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
#[cfg(target_os = "macos")]
#[tokio::test]
async fn forgotten_storage_clears_current_and_retained_workspace_caches_from_an_old_manager() {
    let f = Fixture::new().await;
    let scanned = f.storage.submit_scan(&f.id).await.unwrap();
    f.wait(&scanned).await;
    MaintenanceManager::invalidate_storage(f.paths.workspace_root(), &f.id).unwrap();
    f.scan().await;
    f.storage.shutdown().await;
    f.tasks.shutdown().await;
    let destination = f
        ._temp
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-storage-migration");
    fs::create_dir(&destination).unwrap();
    f.store.begin_workspace_migration().unwrap();
    let migration = crate::workspace::create(&f.paths, &destination).unwrap();
    f.store.pause_workspace().await.unwrap();
    let (selected, mut journal) =
        crate::workspace::copy_and_commit(&f.paths, &f.store, &migration.task_id).unwrap();
    let store = Arc::new(ProjectStore::new(selected.workspace_root().into()).unwrap());
    store.set_node_source_data_root(selected.data_root().into());
    crate::workspace::initialize_selected(&selected, &store, &mut journal).unwrap();
    crate::workspace::mark_old_retired(&selected, &journal).unwrap();
    crate::workspace::finish(&selected, &migration.task_id, Ok(json!({"completed":true}))).unwrap();
    let new_cache = selected
        .workspace_root()
        .join(".platform/cache/storage/projects")
        .join(format!("{}.json", f.id));
    builds::atomic_json(&new_cache, &json!({"projectId":f.id})).unwrap();
    let old_cache = f.storage.cache(&f.id);
    assert!(old_cache.exists());
    assert_eq!(
        f.paths.selected_for_cleanup().unwrap().workspace_root(),
        selected.workspace_root()
    );
    let journal_path = selected
        .migrations_dir()
        .join(&migration.task_id)
        .join("journal.json");
    let good = fs::read(&journal_path).unwrap();
    let mut unavailable: Value = serde_json::from_slice(&good).unwrap();
    unavailable["sourceIdentity"]["volumeUuid"] = json!(Uuid::new_v4().to_string());
    builds::atomic_json(&journal_path, &unavailable).unwrap();
    assert!(f.storage.forget_deleted_project(&f.id).await.is_err());
    assert!(old_cache.exists());
    assert!(new_cache.exists());
    fs::write(&journal_path, &good).unwrap();
    let forgotten = f.storage.forget_deleted_project(&f.id).await.unwrap();
    assert!(forgotten.contains(&scanned.task_id));
    assert!(!old_cache.exists());
    assert!(!new_cache.exists());
    assert!(f.tasks.task(&scanned.task_id).is_err());
    store.shutdown().await;
    f.store.shutdown().await;
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
    for (path, bytes) in [
        ("app/public/photo.png", b"same-image".as_slice()),
        ("dist/photo.png", b"same-image"),
        ("meta/icon.png", b"icon"),
        ("app/node_modules/dependency", b"deps"),
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
    f.write(
        ".platform/uploads/.upload-0123456789abcdef0123456789abcdef",
        b"temporary",
    );
    let result = f.scan().await;
    let current = &result["currentProject"];
    assert_eq!(current["complete"], true, "{result}");
    assert_eq!(current["categories"].as_object().unwrap().len(), 7);
    assert_eq!(current["categories"]["source"]["fixedAssets"]["bytes"], 10);
    assert_eq!(current["categories"]["output"]["bytes"], 10);
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
async fn window_artifact_views_are_project_output_and_included_in_project_totals() {
    let f = Fixture::new().await;
    let before = f.scan().await;
    let views = f
        .paths
        .data_root()
        .join("web-project-ownership")
        .join(&f.id)
        .join("artifact-views/view_disposable");
    fs::create_dir_all(views.join("dist")).unwrap();
    let page = b"disposable immutable window page";
    let receipt = b"{\"buildId\":\"view_disposable\"}";
    fs::write(views.join("dist/index.html"), page).unwrap();
    fs::write(views.join("view.json"), receipt).unwrap();
    let added = (page.len() + receipt.len()) as u64;

    let result = f.scan().await;
    let current = &result["currentProject"];
    assert_eq!(current["complete"], true, "{result}");
    assert_eq!(
        current["categories"]["output"]["windowViews"]["bytes"],
        added
    );
    assert_eq!(
        current["categories"]["output"]["windowViews"]["fileCount"],
        2
    );
    assert_eq!(
        numeric(&current["categories"]["output"]["bytes"]),
        Some(numeric(&before["currentProject"]["categories"]["output"]["bytes"]).unwrap() + added)
    );
    assert_eq!(
        numeric(&current["categories"]["output"]["fileCount"]),
        Some(numeric(&before["currentProject"]["categories"]["output"]["fileCount"]).unwrap() + 2)
    );
    assert_eq!(
        numeric(&current["totalBytes"]),
        Some(numeric(&before["currentProject"]["totalBytes"]).unwrap() + added)
    );
    assert_eq!(
        numeric(&current["fileCount"]),
        Some(numeric(&before["currentProject"]["fileCount"]).unwrap() + 2)
    );
    f.stop().await;
}

#[tokio::test]
async fn window_artifact_reference_rows_do_not_double_count_disjoint_workspace_totals() {
    let f = Fixture::new().await;
    let views = f
        .paths
        .data_root()
        .join("web-project-ownership")
        .join(&f.id)
        .join("artifact-views/view_disposable");
    fs::create_dir_all(views.join("dist")).unwrap();
    let page = b"disposable window view";
    let receipt = b"disposable view receipt";
    fs::write(views.join("dist/index.html"), page).unwrap();
    fs::write(views.join("view.json"), receipt).unwrap();
    let added = (page.len() + receipt.len()) as u64;
    let project = scope_scan(&f.root, json!({"kind":"expectedProject"}), &[], |_| {
        "project".into()
    });
    let workspace = scope_scan(
        f.paths.workspace_root(),
        json!({"kind":"expectedWorkspace"}),
        &[],
        |_| "workspace".into(),
    );
    let data = scope_scan(
        f.paths.data_root(),
        json!({"kind":"expectedApplicationData"}),
        &[f.paths
            .workspace_root()
            .strip_prefix(f.paths.data_root())
            .unwrap()
            .into()],
        |_| "applicationData".into(),
    );
    let expected =
        numeric(&workspace["totalBytes"]).unwrap() + numeric(&data["totalBytes"]).unwrap();
    let result = workspace_scan(&f.paths, None, "disposable-window-views").unwrap();
    let row = result["workspace"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["scope"]["projectId"] == f.id)
        .unwrap();
    assert_eq!(row["referenceOnly"], true);
    assert_eq!(row["windowViews"]["bytes"], added);
    assert_eq!(row["windowViews"]["fileCount"], 2);
    assert_eq!(row["windowViews"]["complete"], true);
    assert_eq!(
        numeric(&row["totalBytes"]),
        Some(numeric(&project["totalBytes"]).unwrap() + added)
    );
    assert_eq!(
        numeric(&row["fileCount"]),
        Some(numeric(&project["fileCount"]).unwrap() + 2)
    );
    assert_eq!(
        numeric(&result["knownLocalTotal"]["totalBytes"]),
        Some(expected)
    );
    f.stop().await;
}

#[tokio::test]
async fn window_artifact_control_links_are_not_followed_or_reported_complete() {
    let f = Fixture::new().await;
    let external = f.paths.data_root().join("disposable-external-view");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("must-not-count"), vec![0; 100_000]).unwrap();
    let views = f
        .paths
        .data_root()
        .join("web-project-ownership")
        .join(&f.id)
        .join("artifact-views");
    fs::create_dir_all(views.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&external, &views).unwrap();
    let result = f.scan().await;
    let current = &result["currentProject"];
    assert_eq!(current["complete"], false);
    assert_eq!(current["categories"]["output"]["windowViews"]["bytes"], 0);
    assert_eq!(
        current["categories"]["output"]["windowViews"]["complete"],
        false
    );
    assert!(current["issues"]
        .as_array()
        .unwrap()
        .iter()
        .any(|issue| issue["reason"] == "window_views_unavailable"));
    f.stop().await;
}

#[tokio::test]
async fn window_view_metadata_changes_invalidate_cached_counts_without_workspace_markers() {
    let f = Fixture::new().await;
    let marker = f
        .paths
        .workspace_root()
        .join(".platform/cache/storage/invalidations/projects")
        .join(format!("{}.json", f.id));
    let original_marker = fs::read(&marker).ok();
    f.scan().await;
    assert_eq!(
        f.storage.info(&f.id).await.unwrap()["currentProject"]["stale"],
        false
    );

    // Old cached bindings have no control-view signature, even if no view was
    // present at the time. They must be measured again after the upgrade.
    let cache = f.storage.cache(&f.id);
    let mut old: Value = read_json(&cache).unwrap();
    assert!(old["binding"]
        .as_object_mut()
        .unwrap()
        .remove("windowViewsSignature")
        .is_some());
    builds::atomic_json(&cache, &old).unwrap();
    assert_eq!(
        f.storage.info(&f.id).await.unwrap()["currentProject"]["stale"],
        true
    );
    f.scan().await;

    let views = window_views_root(&f.paths, &f.id).join("view_disposable");
    fs::create_dir_all(views.join("dist")).unwrap();
    fs::write(views.join("dist/index.html"), b"disposable window artifact").unwrap();
    fs::write(views.join("view.json"), b"prepared view receipt").unwrap();
    assert_eq!(
        f.storage.info(&f.id).await.unwrap()["currentProject"]["stale"],
        true
    );
    f.scan().await;
    assert_eq!(
        f.storage.info(&f.id).await.unwrap()["currentProject"]["stale"],
        false
    );

    // Keep the container's directory mtime unchanged: replacing the receipt
    // still invalidates the binding because the receipt inode is included.
    let modified = fs::metadata(&views).unwrap().modified().unwrap();
    let previous_inode = fs::metadata(views.join("view.json")).unwrap().ino();
    fs::write(views.join("next-view.json"), b"ready view receipt").unwrap();
    fs::rename(views.join("next-view.json"), views.join("view.json")).unwrap();
    assert_ne!(
        fs::metadata(views.join("view.json")).unwrap().ino(),
        previous_inode
    );
    File::open(&views)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert_eq!(
        f.storage.info(&f.id).await.unwrap()["currentProject"]["stale"],
        true
    );
    f.scan().await;

    fs::rename(
        &views,
        f.paths.data_root().join("disposable-moved-window-view"),
    )
    .unwrap();
    assert_eq!(
        f.storage.info(&f.id).await.unwrap()["currentProject"]["stale"],
        true
    );
    let result = f.scan().await;
    assert_eq!(
        result["currentProject"]["categories"]["output"]["windowViews"]["bytes"],
        0
    );
    assert_eq!(fs::read(&marker).ok(), original_marker);
    f.stop().await;
}

#[tokio::test]
async fn symlinks_and_invalid_manifest_report_partial_values_without_counting_external_data() {
    let f = Fixture::new().await;
    let external = f.paths.data_root().join("outside");
    fs::write(&external, vec![0; 100_000]).unwrap();
    std::os::unix::fs::symlink(&external, f.root.join("data/attachments/outside")).unwrap();
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
        .any(|i| i["reason"] == "symlink_not_counted"));
    fs::write(f.root.join("project.json"), b"broken manifest").unwrap();
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
    fs::copy(old.join("project.json"), f.root.join("project.json")).unwrap();
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
async fn cleanup_only_deletes_identified_dependencies_completed_cache_and_logs() {
    let f = Fixture::new().await;
    let control_before = f.store.control(&f.id).unwrap().get();
    assert_eq!(control_before.mode, "ready");
    assert!(control_before.maintenance_id.is_none());
    assert!(control_before.baseline_backup_id.is_none());
    let backups = f.paths.workspace_root().join("backups").join(&f.id);
    assert!(!backups.exists());
    let (build, lease) = f.tasks.create_operation(&f.id, "build").unwrap();
    f.tasks.finish(&build.task_id, Ok(json!({}))).unwrap();
    drop(lease);
    for path in [
        "app/node_modules/package/file",
        "data/attachments/node_modules/user",
        "dist/index.html",
        "meta/icon.png",
        ".platform/artifacts/old/dist/index.html",
        ".platform/unknown/cache",
        ".platform/dependencies.json",
    ] {
        f.write(path, b"keep or clean");
    }
    f.write(
        &format!(".platform/builds/{}/previous/old", build.task_id),
        b"clean old staging",
    );
    f.write(
        &format!(".platform/builds/{}/unknown/user", build.task_id),
        b"unknown retained",
    );
    let scan = f.scan().await;
    assert_eq!(scan["currentProject"]["categories"]["cache"]["bytes"], 17);
    fs::create_dir_all(f.root.join("app/node_modules/.bin")).unwrap();
    std::os::unix::fs::symlink(
        "../package/file",
        f.root.join("app/node_modules/.bin/command"),
    )
    .unwrap();
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
    let result = task.result.unwrap();
    assert!(numeric(&result["logicalBytes"]).unwrap() > 0);
    assert!(result["physicalBytesFreed"].is_null());
    assert!(!f.root.join("app/node_modules").exists());
    assert!(!f
        .root
        .join(format!(".platform/builds/{}/previous", build.task_id))
        .exists());
    for path in [
        "data/attachments/node_modules/user",
        "dist/index.html",
        "meta/icon.png",
        ".platform/artifacts/old/dist/index.html",
        ".platform/unknown/cache",
        ".platform/dependencies.json",
    ] {
        assert!(f.root.join(path).is_file(), "{path}");
    }
    assert!(f
        .root
        .join(format!(".platform/builds/{}/unknown/user", build.task_id))
        .is_file());
    assert!(f
        .paths
        .workspace_root()
        .join(".platform/tasks")
        .join(build.task_id)
        .join("task.json")
        .is_file());
    let control_after = f.store.control(&f.id).unwrap().get();
    assert_eq!(control_after.mode, "ready");
    assert!(control_after.maintenance_id.is_none());
    assert!(control_after.baseline_backup_id.is_none());
    assert!(!backups.exists());
    f.stop().await;
}
#[tokio::test]
async fn cleanup_waits_for_project_readers_and_rechecks_new_recovery_protection() {
    let f = Fixture::new().await;
    f.write("app/node_modules/kept", b"dependency");
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
        fs::read(f.root.join("app/node_modules/kept")).unwrap(),
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
        fs::read(f.root.join("app/node_modules/kept")).unwrap(),
        b"dependency"
    );
    f.stop().await;
}
#[tokio::test]
async fn cleanup_respects_task_maintenance_and_recovery_protection() {
    let f = Fixture::new().await;
    f.write("app/node_modules/kept", b"dependency");
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
    assert!(f.root.join("app/node_modules/kept").exists());
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
    f.write("app/node_modules/ordinary", b"deleted");
    let external = f.paths.data_root().join("external-keep");
    fs::write(&external, b"external untouched").unwrap();
    std::os::unix::fs::symlink(&external, f.root.join("app/node_modules/external")).unwrap();
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
    assert_eq!(fs::read(&external).unwrap(), b"external untouched");
    assert!(
        fs::symlink_metadata(f.root.join("app/node_modules/external"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    f.stop().await;
}
#[tokio::test]
async fn interrupted_cleanup_uses_task_status_and_never_replays_deletion() {
    let f = Fixture::new().await;
    f.write("app/node_modules/residual", b"do not replay");
    let task = f
        .tasks
        .create_readonly_operation(&f.id, "storageCleanup")
        .unwrap();
    f.tasks.phase(&task.task_id, "deleting").unwrap();
    f.storage.shutdown().await;
    f.tasks.shutdown().await;
    let tasks = BuildManager::new(
        f.paths.clone(),
        f.store.clone(),
        "http://127.0.0.1:9".into(),
    )
    .unwrap();
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
        fs::read(f.root.join("app/node_modules/residual")).unwrap(),
        b"do not replay"
    );
    restarted.shutdown().await;
    tasks.shutdown().await;
    f.stop().await;
}
#[tokio::test]
async fn workspace_ranges_are_disjoint_include_retained_copy_and_exclude_program_links() {
    let f = Fixture::new().await;
    let app = f.paths.data_root().join("Actual Installation.app");
    fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    fs::write(app.join("Contents/MacOS/test"), b"program").unwrap();
    let external = f._temp.path().join("external");
    fs::write(&external, vec![0; 10000]).unwrap();
    std::os::unix::fs::symlink(&external, app.join("Contents/external")).unwrap();
    let retained = f._temp.path().canonicalize().unwrap().join("retained");
    fs::create_dir(&retained).unwrap();
    fs::write(retained.join("evidence"), b"retained").unwrap();
    let identity = root_identity(&retained).unwrap();
    builds::atomic_json(&f.paths.migrations_dir().join("task_retained/journal.json"),&json!({"committed":true,"sourcePath":retained,"sourceIdentity":{"device":identity.device,"inode":identity.inode}})).unwrap();
    let result = workspace_scan(&f.paths, Some(&app), "test-generation").unwrap();
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
    assert!(exclusions.contains(&json!("Actual Installation.app")));
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
async fn installed_node_apps_are_reported_per_app_inside_application_data() {
    let f = Fixture::new().await;
    let node_root = f.paths.data_root().join("node-apps");
    let id = Uuid::new_v4().to_string();
    let app = node_root.join(&id);
    fs::create_dir_all(app.join("generations/gen_a")).unwrap();
    fs::create_dir_all(app.join("data")).unwrap();
    fs::write(app.join("registration.json"), b"{}").unwrap();
    fs::write(app.join("generations/gen_a/main.js"), vec![0; 30]).unwrap();
    fs::write(app.join("data/db"), vec![0; 20]).unwrap();
    // Unregistered or non-app directories are not listed as apps.
    fs::create_dir_all(node_root.join(".installations/install_a")).unwrap();
    fs::write(node_root.join(".installations/install_a/file"), b"12345").unwrap();
    fs::create_dir_all(node_root.join(Uuid::new_v4().to_string())).unwrap();
    let result = workspace_scan(&f.paths, None, "node-apps-test").unwrap();
    let rows = result["nodeApps"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["scope"]["appId"], json!(id));
    assert_eq!(rows[0]["referenceOnly"], true);
    assert_eq!(
        numeric(&rows[0]["categories"]["program"]["bytes"]),
        Some(30)
    );
    assert_eq!(numeric(&rows[0]["categories"]["data"]["bytes"]), Some(20));
    let data = result["otherLocalLocations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["scope"]["kind"] == "applicationData")
        .unwrap();
    // The whole node-apps tree, including installation material, is one category.
    assert!(numeric(&data["categories"]["nodeApps"]["bytes"]).unwrap() >= 57);
    f.stop().await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn retained_workspace_accounting_uses_persisted_identity_without_weakening_runtime_scans() {
    let f = Fixture::new().await;
    let retained = f
        ._temp
        .path()
        .canonicalize()
        .unwrap()
        .join("retained-identity");
    fs::create_dir(&retained).unwrap();
    fs::write(retained.join("evidence"), b"retained").unwrap();
    let actual = PersistedIdentity::capture(&retained).unwrap();
    let mut renumbered = actual.clone();
    renumbered.device = renumbered.device.wrapping_add(1);
    let mut different_volume = actual.clone();
    different_volume.volume_uuid = Some(Uuid::new_v4().to_string());
    let mut legacy = renumbered.clone();
    legacy.volume_uuid = None;
    for (expected, allowed) in [
        (renumbered, true),
        (different_volume, false),
        (legacy, false),
    ] {
        builds::atomic_json(
            &f.paths
                .migrations_dir()
                .join("task_retained_identity/journal.json"),
            &json!({"committed":true,"sourcePath":retained,"sourceIdentity":expected}),
        )
        .unwrap();
        let result = workspace_scan(&f.paths, None, "identity-test").unwrap();
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

fn add_disposable_direct_node_storage(
    paths: &RuntimePaths,
    source: &Path,
    platform_owned: bool,
) -> (String, PathBuf) {
    fs::create_dir_all(source.join("node_modules/tiny")).unwrap();
    fs::create_dir_all(source.join(".git")).unwrap();
    fs::write(source.join("code.js"), b"abc").unwrap();
    fs::write(source.join("node_modules/tiny/index.js"), b"12345").unwrap();
    fs::write(source.join(".git/HEAD"), b"ref: main\n").unwrap();
    let id = Uuid::new_v4().to_string();
    let slot = paths.data_root().join("node-apps").join(&id);
    for name in ["data", "configuration", "logs", "cache"] {
        fs::create_dir_all(slot.join(name)).unwrap();
    }
    fs::write(slot.join("data/business.db"), b"data").unwrap();
    fs::write(slot.join("configuration/settings.json"), b"config").unwrap();
    fs::write(slot.join("logs/current.log"), b"log").unwrap();
    fs::write(slot.join("cache/payload"), b"cache").unwrap();
    let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
        paths.data_root().into(),
        paths.workspace_root().into(),
    );
    ownership.initialize(&id).unwrap();
    if platform_owned {
        ownership.record_generated_source(&id, source).unwrap();
    }
    crate::node_apps::development::claim_source_after_verified_migration(
        paths.data_root(),
        paths.workspace_root(),
        &id,
        source,
    )
    .unwrap();
    let manifest = json!({"app":{"id":"editable-author-id"},"runtime":"node-service"}).to_string();
    let digest = format!("{:x}", Sha256::digest(manifest.as_bytes()));
    builds::atomic_json(
        &slot.join("registration.json"),
        &json!({"version":1,"id":id,"sourcePath":source,"manifestDigest":digest}),
    )
    .unwrap();
    builds::atomic_json(&slot.join("development-source.json"),&json!({"sourceRoot":source,"rootIdentity":PersistedIdentity::capture(source).unwrap(),"manifestId":"editable-author-id","registeredManifestText":manifest,"registeredManifestDigest":digest})).unwrap();
    (id, slot)
}

#[tokio::test]
async fn direct_node_storage_includes_dependencies_and_marks_source_ownership_without_double_counting(
) {
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
    let (managed_id, _) = add_disposable_direct_node_storage(&f.paths, &managed, true);
    let (external_id, _) = add_disposable_direct_node_storage(&f.paths, &external, false);
    let result = workspace_scan(&f.paths, None, "node-sources").unwrap();
    for (id, owned) in [(&managed_id, true), (&external_id, false)] {
        let row = result["nodeApps"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["scope"]["appId"] == *id)
            .unwrap();
        assert_eq!(row["projectFolder"]["totalBytes"], 18);
        assert_eq!(
            row["projectFolder"]["categories"]["dependencies"]["bytes"],
            5
        );
        assert_eq!(row["projectFolder"]["categories"]["git"]["bytes"], 10);
        assert_eq!(row["projectFolder"]["categories"]["source"]["bytes"], 3);
        assert_eq!(
            row["sourceOwnership"],
            if owned { "platform" } else { "external" }
        );
        assert_eq!(row["sourcePreservedOnDelete"], !owned);
        assert_eq!(row["instance"]["categories"]["data"]["bytes"], 4);
        assert_eq!(row["instance"]["categories"]["configuration"]["bytes"], 6);
        assert_eq!(row["instance"]["categories"]["logs"]["bytes"], 3);
        assert_eq!(row["instance"]["categories"]["cache"]["bytes"], 5);
        assert_eq!(
            numeric(&row["totalBytes"]),
            Some(18 + numeric(&row["instance"]["totalBytes"]).unwrap())
        );
        assert_eq!(row["referenceOnly"], true);
        assert_eq!(row["projectFolder"]["referenceOnly"], true);
    }
    let other = result["otherLocalLocations"].as_array().unwrap();
    assert!(other
        .iter()
        .any(|scope| scope["scope"]["kind"] == "nodeSource"
            && scope["root"] == json!(external)
            && scope["totalBytes"] == 18));
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
async fn missing_or_replaced_node_source_is_not_measured_as_the_original_project() {
    for replaced in [false, true] {
        let f = Fixture::new().await;
        let source = f
            ._temp
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-replaceable-node");
        let (id, _) = add_disposable_direct_node_storage(&f.paths, &source, false);
        fs::rename(&source, source.with_file_name("disposable-original-node")).unwrap();
        if replaced {
            fs::create_dir(&source).unwrap();
            fs::write(source.join("unrelated-user-file"), [0u8; 512]).unwrap();
        }
        let result = workspace_scan(&f.paths, None, "missing-node-source").unwrap();
        let row = result["nodeApps"]
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
