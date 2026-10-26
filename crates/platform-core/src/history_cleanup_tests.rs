//! All destructive tests use uniquely owned TempDir data, never development data.
use super::*;
use crate::{maintenance::MaintenanceManager, CreateProjectInput, TaskStatus};

struct Fixture {
    temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    maintenance: Arc<MaintenanceManager>,
    cleanup: Arc<CleanupManager>,
    project: String,
}
impl Fixture {
    async fn new() -> Self {
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let project = store
            .create(CreateProjectInput {
                name: "Disposable history-cleanup validation".into(),
                description: None,
            })
            .await
            .unwrap();
        fs::write(
            project.project_root.join("data/attachments/keep.txt"),
            "persistent attachment",
        )
        .unwrap();
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let maintenance = MaintenanceManager::new(store.clone(), tasks.clone());
        let cleanup = CleanupManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            "instance-test".into(),
        )
        .unwrap();
        Self {
            temporary,
            paths,
            store,
            tasks,
            maintenance,
            cleanup,
            project: project.id,
        }
    }
    async fn wait(&self, record: TaskRecord) -> TaskRecord {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                let current = self.tasks.task(&record.task_id).unwrap();
                if current.status.is_final() {
                    tokio::task::yield_now().await;
                    return current;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn preview(&self) -> Value {
        let task = self.wait(self.cleanup.preview().unwrap()).await;
        assert_eq!(task.status, TaskStatus::Succeeded, "{:?}", task.error);
        task.result.unwrap()
    }
    #[cfg(target_os = "macos")]
    async fn migrate(self) -> (Self, PathBuf, String) {
        self.cleanup.shutdown().await;
        self.tasks.shutdown().await;
        let source = self.paths.workspace_root().to_owned();
        let destination = self
            .temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("migrated-workspace");
        fs::create_dir(&destination).unwrap();
        self.store.begin_workspace_migration().unwrap();
        let task = crate::workspace::create(&self.paths, &destination).unwrap();
        self.store.pause_workspace().await.unwrap();
        let (paths, mut journal) =
            crate::workspace::copy_and_commit(&self.paths, &self.store, &task.task_id).unwrap();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        crate::workspace::initialize_selected(&paths, &store, &mut journal).unwrap();
        crate::workspace::mark_old_retired(&paths, &journal).unwrap();
        self.store.shutdown().await;
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let maintenance = MaintenanceManager::new(store.clone(), tasks.clone());
        let cleanup = CleanupManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            "instance-migrated".into(),
        )
        .unwrap();
        (
            Self {
                temporary: self.temporary,
                paths,
                store,
                tasks,
                maintenance,
                cleanup,
                project: self.project,
            },
            source,
            task.task_id,
        )
    }
    async fn stop(self) {
        self.cleanup.shutdown().await;
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
}
fn candidate(root: &Path) -> Candidate {
    let canonical = root.canonicalize().unwrap();
    let root = canonical.as_path();
    let mut item = Candidate::new(
        "original_task_fixture".into(),
        "recoveryOriginal",
        "fixture",
        root.into(),
    );
    item.root_identity = Some(directory_identity(root).unwrap());
    item.persistent_root_identity = Some(persistent_identity(root).unwrap());
    item.parents = parent_identities(root).unwrap();
    item.entries = scan(root).unwrap();
    item.can_clean = true;
    item
}
fn input(plan: &Value, ids: Vec<String>) -> Value {
    json!({"planId":plan["planId"],"itemIds":ids,"confirmation":CONFIRMATION})
}

#[test]
fn persisted_chain_comparison_never_equates_unverified_mixed_or_distinct_legacy_records() {
    let missing = Path::new("/unavailable-identity-chain-fixture");
    let first: PersistedIdentity = serde_json::from_value(json!([7, 42])).unwrap();
    let same: PersistedIdentity = serde_json::from_value(json!({"device":7,"inode":42})).unwrap();
    let other_device: PersistedIdentity = serde_json::from_value(json!([8, 42])).unwrap();
    let stable: PersistedIdentity = serde_json::from_value(
        json!({"device":7,"inode":42,"volumeUuid":Uuid::new_v4().to_string()}),
    )
    .unwrap();
    assert!(same_persisted_record(&first, &same, missing));
    assert!(!same_persisted_record(&first, &other_device, missing));
    assert!(!same_persisted_record(&first, &stable, missing));
    let mut renumbered = stable.clone();
    renumbered.device = 8;
    assert!(same_persisted_record(&stable, &renumbered, missing));
    renumbered.inode = 43;
    assert!(!same_persisted_record(&stable, &renumbered, missing));
}

#[test]
fn safe_unlink_never_follows_symlinks_and_reports_actual_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("owned");
    fs::create_dir_all(root.join("nested")).unwrap();
    let outside = temporary.path().join("outside");
    fs::write(&outside, "never remove target").unwrap();
    fs::write(root.join("nested/file"), b"12345").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    let item = candidate(&root);
    let removed = remove_tree(&item, &temporary.path().join("receipt.jsonl")).unwrap();
    assert_eq!(removed["completed"], true);
    assert_eq!(removed["removedLogicalBytes"], 5);
    assert_eq!(removed["removedFileCount"], 2);
    assert!(!root.exists());
    assert_eq!(fs::read_to_string(outside).unwrap(), "never remove target");
}
#[test]
fn changed_content_and_replaced_ancestors_are_retained() {
    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().join("parent");
    let root = parent.join("owned");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a"), "original").unwrap();
    let item = candidate(&root);
    fs::write(root.join("a"), "modified").unwrap();
    assert!(remove_tree(&item, &temporary.path().join("receipt")).is_err());
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "modified");
    fs::rename(&parent, temporary.path().join("old-parent")).unwrap();
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("replacement"), "retain").unwrap();
    assert!(remove_tree(&item, &temporary.path().join("receipt2")).is_err());
    assert_eq!(
        fs::read_to_string(root.join("replacement")).unwrap(),
        "retain"
    );
}
#[test]
fn new_unplanned_files_survive_partial_deletion_with_an_honest_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("owned");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a"), "abc").unwrap();
    let item = candidate(&root);
    let failure = remove_tree_with_hook(&item, &temporary.path().join("receipt"), |_, path| {
        fs::write(path.join("new-user-file"), "new draft")?;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(failure.details["cleanupResult"]["completed"], false);
    assert_eq!(failure.details["cleanupResult"]["removedLogicalBytes"], 3);
    assert!(root.exists());
    assert_eq!(
        fs::read_to_string(root.join("new-user-file")).unwrap(),
        "new draft"
    );
}
#[test]
fn special_files_and_root_symlinks_are_never_cleanup_candidates() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("owned");
    fs::create_dir(&root).unwrap();
    let fifo = c_name(&root.join("pipe")).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(scan(&root).is_err());
    let alias = temporary.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    assert!(scan(&alias).is_err());
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn migrated_workspace_cleanup_survives_restart_without_phantom_storage() {
    let fixture = Fixture::new().await;
    let (fixture, source, migration) = fixture.migrate().await;
    let current = fixture.store.project(&fixture.project).unwrap().root;
    let database = hash_file(&current.join("data/main.sqlite")).unwrap();
    let plan = fixture.preview().await;
    let id = format!("retained_{migration}");
    let item = plan["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap();
    assert_eq!(item["canClean"], true, "{item}");
    let task = fixture
        .wait(fixture.cleanup.submit(input(&plan, vec![id])).unwrap())
        .await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{:?}", task.error);
    assert!(!source.exists());
    assert_eq!(
        hash_file(&current.join("data/main.sqlite")).unwrap(),
        database
    );
    assert!(retired_cleanup_status(&fixture.paths, &migration)
        .unwrap()
        .is_some());
    fixture.cleanup.shutdown().await;
    let restarted = CleanupManager::new(
        fixture.paths.clone(),
        fixture.store.clone(),
        fixture.tasks.clone(),
        "instance-restarted".into(),
    )
    .unwrap();
    assert_eq!(
        restarted.latest_task().unwrap().unwrap().task_id,
        task.task_id
    );
    let info = crate::workspace::info(&fixture.paths, &fixture.store).unwrap();
    assert_eq!(info["retainedRoots"][0]["cleaned"], true);
    let storage = crate::storage::StorageManager::new(
        fixture.paths.clone(),
        fixture.store.clone(),
        fixture.tasks.clone(),
        fixture.maintenance.clone(),
    )
    .unwrap();
    let space = storage.workspace_info().await.unwrap();
    assert!(!space["otherLocalLocations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|scope| scope["scope"]["kind"] == "retainedWorkspace"));
    storage.quiesce().await;
    fs::create_dir(&source).unwrap();
    fs::write(source.join("new-user-file"), "replacement").unwrap();
    assert!(retired_cleanup_status(&fixture.paths, &migration)
        .unwrap()
        .is_none());
    restarted.shutdown().await;
    fixture.stop().await;
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn modified_old_source_and_wrong_selection_marker_block_cleanup() {
    let fixture = Fixture::new().await;
    let (fixture, source, migration) = fixture.migrate().await;
    let id = format!("retained_{migration}");
    let eligible = fixture.preview().await;
    assert_eq!(
        eligible["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap()["canClean"],
        true
    );
    let new = source.join("new-user-file");
    fs::write(&new, "preserve draft").unwrap();
    let changed = fixture.preview().await;
    assert_eq!(
        changed["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap()["canClean"],
        false
    );
    fs::remove_file(new).unwrap();
    let marker = fixture.paths.workspace_root().join(SELECTION);
    let original = fs::read(&marker).unwrap();
    let mut value: Value = serde_json::from_slice(&original).unwrap();
    value["taskId"] = json!("task_wrong");
    builds::atomic_json(&marker, &value).unwrap();
    let wrong = fixture.preview().await;
    assert_eq!(
        wrong["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap()["canClean"],
        false
    );
    fs::write(marker, original).unwrap();
    assert!(source.exists());
    fixture.stop().await;
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn explicit_repreview_after_partial_marker_removal_uses_only_accepted_unlink_progress() {
    let fixture = Fixture::new().await;
    let (fixture, source, migration) = fixture.migrate().await;
    let preview = fixture.preview().await;
    let plan_path = fixture
        .cleanup
        .root()
        .join("plans")
        .join(format!("{}.json", preview["planId"].as_str().unwrap()));
    let mut plan: Plan = serde_json::from_value(read_json(&plan_path).unwrap()).unwrap();
    let id = format!("retained_{migration}");
    let item = plan
        .items
        .iter()
        .find(|item| item.id == id)
        .unwrap()
        .clone();
    assert!(item.can_clean);
    let task = fixture
        .tasks
        .create_workspace_operation("workspaceCleanup")
        .unwrap();
    plan.task_id = Some(task.task_id.clone());
    plan.selected_ids = vec![id.clone()];
    builds::atomic_json(&plan_path, &plan).unwrap();
    let receipt = fixture.cleanup.receipt(&task.task_id);
    builds::atomic_json(
        &receipt.join("selection.json"),
        &json!({"planId":plan.plan_id,"itemIds":plan.selected_ids}),
    )
    .unwrap();
    builds::atomic_json(&receipt.join("task.json"), &task).unwrap();
    fixture.store.begin_workspace_cleanup().unwrap();
    let failure = remove_tree_with_hook(
        &item,
        &receipt.join(format!("{id}.progress.jsonl")),
        |progress, _| {
            if progress
                .removed_paths
                .iter()
                .any(|path| path == Path::new(RETIRED))
            {
                return Err(ApiError::new(
                    "TEST_AFTER_MARKER",
                    "Disposable failure after marker unlink",
                ));
            }
            Ok(())
        },
    )
    .unwrap_err();
    fixture.store.resume_workspace();
    assert_eq!(failure.code, "TEST_AFTER_MARKER");
    fixture
        .tasks
        .finish_partial(&task.task_id, json!({"completed":false}), failure)
        .unwrap();
    assert!(source.exists());
    assert!(!source.join(RETIRED).exists());
    // This old plan remains unexecutable. Only its confirmed unlink progress is
    // reusable after a simulated reboot changes runtime device numbers.
    let prior = plan.items.iter_mut().find(|item| item.id == id).unwrap();
    prior.root_identity.as_mut().unwrap().device =
        prior.root_identity.as_ref().unwrap().device.wrapping_add(1);
    let durable = prior.persistent_root_identity.as_mut().unwrap();
    assert!(!durable.is_legacy());
    durable.device = durable.device.wrapping_add(1);
    let valid_inode = durable.inode;
    durable.inode = durable.inode.wrapping_add(1);
    plan.instance_id = "prior-runtime".into();
    builds::atomic_json(&plan_path, &plan).unwrap();
    let blocked = fixture.preview().await;
    assert_eq!(
        blocked["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap()["canClean"],
        false
    );
    plan.items
        .iter_mut()
        .find(|item| item.id == id)
        .unwrap()
        .persistent_root_identity
        .as_mut()
        .unwrap()
        .inode = valid_inode;
    builds::atomic_json(&plan_path, &plan).unwrap();
    // Old plans without the companion keep their strict same-device behavior.
    let saved = plan
        .items
        .iter()
        .find(|item| item.id == id)
        .unwrap()
        .clone();
    plan.items
        .iter_mut()
        .find(|item| item.id == id)
        .unwrap()
        .persistent_root_identity = None;
    builds::atomic_json(&plan_path, &plan).unwrap();
    let unbound = fixture.preview().await;
    assert_eq!(
        unbound["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap()["canClean"],
        false
    );
    plan.items
        .iter_mut()
        .find(|item| item.id == id)
        .unwrap()
        .root_identity = Some(directory_identity(&source).unwrap());
    builds::atomic_json(&plan_path, &plan).unwrap();
    let legacy = fixture.preview().await;
    assert_eq!(
        legacy["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap()["canClean"],
        true
    );
    *plan.items.iter_mut().find(|item| item.id == id).unwrap() = saved;
    builds::atomic_json(&plan_path, &plan).unwrap();
    let retry = fixture.preview().await;
    let item = retry["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap();
    assert_eq!(item["canClean"], true, "{item}");
    let task = fixture
        .wait(fixture.cleanup.submit(input(&retry, vec![id])).unwrap())
        .await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{:?}", task.error);
    assert!(!source.exists());
    fixture.stop().await;
}
#[test]
fn malformed_manifests_and_large_byte_counts_are_never_silently_coerced() {
    assert_eq!(
        wire_u64(Some(9_007_199_254_740_992)),
        json!({"$int64":"9007199254740992"})
    );
    assert_eq!(wire_u64(None), Value::Null);
    assert!(content_matches(
        &[],
        &[json!({"kind":"directory","bytes":0,"mode":448})],
        None
    )
    .is_err());
    let entry =
        json!({"path":"owned","kind":"directory","bytes":0,"mode":448,"digest":null,"target":null});
    assert!(content_matches(&[], &[entry.clone(), entry], None).is_err());
    assert!(validated_control(
        json!({"mode":"ready","recoveryRequired":false,"protectedBackupIds":"unknown"})
    )
    .is_err());
}
