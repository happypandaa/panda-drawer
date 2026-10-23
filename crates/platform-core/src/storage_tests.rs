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
        let maintenance =
            MaintenanceManager::new(paths.clone(), store.clone(), tasks.clone()).unwrap();
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
        self.maintenance.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
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
    let (build, lease) = f.tasks.create_operation(&f.id, "build").unwrap();
    f.tasks
        .append(&build.task_id, b"completed build log")
        .unwrap();
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
async fn interrupted_cleanup_preserves_checkpoint_and_never_replays_deletion() {
    let f = Fixture::new().await;
    f.write("app/node_modules/residual", b"do not replay");
    let task = f
        .tasks
        .create_readonly_operation(&f.id, "storageCleanup")
        .unwrap();
    f.tasks.phase(&task.task_id, "deleting").unwrap();
    let journal = CleanupJournal {
        schema_version: SCHEMA,
        project_id: f.id.clone(),
        task_id: task.task_id.clone(),
        resolved: false,
        result: json!({"deleted":[{"category":"dependencies","fileCount":1,"logicalBytes":9}],"skipped":[],"failures":[],"logicalBytes":9,"fileCount":1,"operationOutcome":"in_progress"}),
        in_flight: Some(f.root.join("app/node_modules")),
    };
    builds::atomic_json(&f.storage.cleanup_journal(&task.task_id), &journal).unwrap();
    f.storage.shutdown().await;
    f.maintenance.shutdown().await;
    f.tasks.shutdown().await;
    let tasks = BuildManager::new(
        f.paths.clone(),
        f.store.clone(),
        "http://127.0.0.1:9".into(),
    )
    .unwrap();
    let maintenance =
        MaintenanceManager::new(f.paths.clone(), f.store.clone(), tasks.clone()).unwrap();
    let restarted = StorageManager::new(
        f.paths.clone(),
        f.store.clone(),
        tasks.clone(),
        maintenance.clone(),
    )
    .unwrap();
    let task = tasks.task(&task.task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Interrupted);
    assert_eq!(task.result.as_ref().unwrap()["logicalBytes"], 9);
    assert_eq!(
        task.result.as_ref().unwrap()["operationOutcome"],
        "interrupted"
    );
    assert_eq!(
        fs::read(f.root.join("app/node_modules/residual")).unwrap(),
        b"do not replay"
    );
    restarted.shutdown().await;
    maintenance.shutdown().await;
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
