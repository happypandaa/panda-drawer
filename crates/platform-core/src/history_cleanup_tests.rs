//! All destructive tests use uniquely owned TempDir data, never development data.
use super::*;
use crate::{
    maintenance::{MaintenanceManager, OperationInput},
    CreateProjectInput, TaskStatus,
};

struct Fixture {
    temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    maintenance: Arc<MaintenanceManager>,
    lifecycle: Arc<LifecycleManager>,
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
        let maintenance =
            MaintenanceManager::new(paths.clone(), store.clone(), tasks.clone()).unwrap();
        let lifecycle = LifecycleManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            maintenance.clone(),
        )
        .unwrap();
        let cleanup = CleanupManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            lifecycle.clone(),
            "instance-test".into(),
        )
        .unwrap();
        Self {
            temporary,
            paths,
            store,
            tasks,
            maintenance,
            lifecycle,
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
    async fn delete(&self) -> String {
        let task = self
            .lifecycle
            .submit_project(&self.project, "delete", json!({}))
            .await
            .unwrap();
        let task = self.wait(task).await;
        assert_eq!(task.status, TaskStatus::Succeeded, "{:?}", task.error);
        task.result.unwrap()["trashId"].as_str().unwrap().into()
    }
    async fn restore_original(&self) -> String {
        let backup = self
            .wait(
                self.maintenance
                    .submit(&self.project, "backup", OperationInput::default())
                    .await
                    .unwrap(),
            )
            .await;
        assert_eq!(backup.status, TaskStatus::Succeeded, "{:?}", backup.error);
        let backup_id = backup.result.unwrap()["backupId"]
            .as_str()
            .unwrap()
            .to_owned();
        let restore = self
            .wait(
                self.maintenance
                    .submit(
                        &self.project,
                        "restore",
                        OperationInput {
                            backup_id: Some(backup_id),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap(),
            )
            .await;
        assert_eq!(restore.status, TaskStatus::Succeeded, "{:?}", restore.error);
        let control = self.store.control(&self.project).unwrap().get();
        let end = self
            .wait(
                self.maintenance
                    .submit(
                        &self.project,
                        "maintenanceEnd",
                        OperationInput {
                            maintenance_id: control.maintenance_id,
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap(),
            )
            .await;
        assert_eq!(end.status, TaskStatus::Succeeded, "{:?}", end.error);
        restore.task_id
    }
    #[cfg(target_os = "macos")]
    async fn migrate(self) -> (Self, PathBuf, String) {
        self.cleanup.shutdown().await;
        self.lifecycle.shutdown().await;
        self.maintenance.shutdown().await;
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
        self.store.pause_workspace().await;
        let (paths, mut journal) =
            crate::workspace::copy_and_commit(&self.paths, &self.store, &task.task_id).unwrap();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        crate::workspace::initialize_selected(&paths, &store, &mut journal).unwrap();
        crate::workspace::mark_old_retired(&paths, &journal).unwrap();
        self.store.shutdown().await;
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
        let maintenance =
            MaintenanceManager::new(paths.clone(), store.clone(), tasks.clone()).unwrap();
        let lifecycle = LifecycleManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            maintenance.clone(),
        )
        .unwrap();
        let cleanup = CleanupManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            lifecycle.clone(),
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
                lifecycle,
                cleanup,
                project: self.project,
            },
            source,
            task.task_id,
        )
    }
    async fn stop(self) {
        self.cleanup.shutdown().await;
        self.lifecycle.shutdown().await;
        self.maintenance.shutdown().await;
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

#[tokio::test]
async fn source_restore_guard_invalidates_cleanup_plan_and_keeps_recovery_out_of_candidates() {
    let fixture = Fixture::new().await;
    let trash = fixture.delete().await;
    let plan = fixture.preview().await;
    assert_eq!(plan["items"][0]["canClean"], true);
    let journal = fixture
        .paths
        .workspace_root()
        .join(".platform/project-state/source-restore")
        .join(&fixture.project)
        .join("task_restore/journal.json");
    builds::atomic_json(
        &journal,
        &json!({
            "schemaVersion":1,"operation":"sourceRestore","projectId":fixture.project,
            "taskId":"task_restore","phase":"applying","planDigest":"a".repeat(64)
        }),
    )
    .unwrap();
    let retained = journal.parent().unwrap().join("original-source");
    fs::write(&retained, b"only recovery evidence").unwrap();
    let task = fixture
        .wait(
            fixture
                .cleanup
                .submit(input(&plan, vec![format!("trash_{trash}")]))
                .unwrap(),
        )
        .await;
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.error.unwrap().code, "CLEANUP_PLAN_CHANGED");
    let next = fixture.preview().await;
    assert_eq!(next["items"].as_array().unwrap().len(), 1);
    assert_eq!(next["items"][0]["canClean"], false);
    assert!(next["items"][0]["blockedReasons"]
        .to_string()
        .contains("PROJECT_SOURCE_RESTORE_PENDING"));
    assert!(fixture
        .paths
        .workspace_root()
        .join(".platform/trash")
        .join(trash)
        .join("project/data/main.sqlite")
        .exists());
    assert_eq!(fs::read(&retained).unwrap(), b"only recovery evidence");
    fs::write(&journal, b"bad journal stays protected").unwrap();
    let malformed = fixture.preview().await;
    assert_eq!(malformed["items"][0]["canClean"], false);
    assert!(malformed["items"][0]["blockedReasons"]
        .to_string()
        .contains("PROJECT_SOURCE_RESTORE_INVALID"));
    fixture.stop().await;
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn cleanup_preview_uses_stable_trash_identity_and_rejects_unbound_legacy_renumbering() {
    let fixture = Fixture::new().await;
    let trash = fixture.delete().await;
    let directory = fixture
        .paths
        .workspace_root()
        .join(".platform/trash")
        .join(&trash);
    let path = directory.join("record.json");
    let original = read_json(&path).unwrap();
    let actual = directory_identity(&directory.join("project")).unwrap();
    for (identity, allowed) in [
        (
            json!({"device":actual.device.wrapping_add(1),"inode":actual.inode,"volumeUuid":original["rootIdentity"]["volumeUuid"]}),
            true,
        ),
        (
            json!({"device":actual.device,"inode":actual.inode,"volumeUuid":Uuid::new_v4().to_string()}),
            false,
        ),
        (json!([actual.device.wrapping_add(1), actual.inode]), false),
        (json!([actual.device, actual.inode]), true),
    ] {
        let mut record = original.clone();
        record["rootIdentity"] = identity;
        builds::atomic_json(&path, &record).unwrap();
        let preview = fixture.preview().await;
        let item = preview["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == format!("trash_{trash}"))
            .unwrap();
        assert_eq!(item["canClean"], allowed, "{item}");
        assert_eq!(
            fs::read(directory.join("project/data/attachments/keep.txt")).unwrap(),
            b"persistent attachment"
        );
    }
    fixture.stop().await;
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
#[tokio::test]
async fn real_trash_cleanup_keeps_backups_and_record_and_consumes_plan_once() {
    let fixture = Fixture::new().await;
    let backup = fixture
        .wait(
            fixture
                .maintenance
                .submit(&fixture.project, "backup", OperationInput::default())
                .await
                .unwrap(),
        )
        .await;
    assert_eq!(backup.status, TaskStatus::Succeeded);
    let backups = fixture
        .paths
        .workspace_root()
        .join("backups")
        .join(&fixture.project);
    let before = fs::read_dir(&backups).unwrap().count();
    let trash = fixture.delete().await;
    let plan = fixture.preview().await;
    let item = plan["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == format!("trash_{trash}"))
        .unwrap();
    assert_eq!(item["canClean"], true, "{item}");
    let request = input(&plan, vec![format!("trash_{trash}")]);
    let accepted = fixture.cleanup.submit(request.clone()).unwrap();
    let duplicate = fixture.cleanup.submit(request).unwrap();
    assert_eq!(accepted.task_id, duplicate.task_id);
    let task = fixture.wait(accepted).await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{:?}", task.error);
    assert_eq!(task.result.as_ref().unwrap()["completed"], true);
    let record = read_json(
        &fixture
            .paths
            .workspace_root()
            .join(".platform/trash")
            .join(&trash)
            .join("record.json"),
    )
    .unwrap();
    assert_eq!(record["state"], "purged");
    assert_eq!(fs::read_dir(backups).unwrap().count(), before);
    assert!(!fixture
        .paths
        .workspace_root()
        .join(".platform/trash")
        .join(trash)
        .join("project")
        .exists());
    assert!(fixture.cleanup.latest_task().unwrap().is_some());
    let different = fixture
        .cleanup
        .submit(input(&plan, vec!["unknown".into()]))
        .unwrap_err();
    assert_eq!(different.code, "CLEANUP_PLAN_CONSUMED");
    fixture.stop().await;
}
#[tokio::test]
async fn plan_expiry_runtime_change_invalid_selection_and_active_operations_do_not_delete() {
    let fixture = Fixture::new().await;
    let trash = fixture.delete().await;
    let plan = fixture.preview().await;
    let request = input(&plan, vec![format!("trash_{trash}")]);
    let path = fixture
        .cleanup
        .root()
        .join("plans")
        .join(format!("{}.json", plan["planId"].as_str().unwrap()));
    let original: Plan = serde_json::from_value(read_json(&path).unwrap()).unwrap();
    let mut expired = original.clone();
    expired.expires_at = 0;
    builds::atomic_json(&path, &expired).unwrap();
    assert_eq!(
        fixture.cleanup.submit(request.clone()).unwrap_err().code,
        "CLEANUP_PLAN_EXPIRED"
    );
    let mut other_instance = original.clone();
    other_instance.instance_id = "other".into();
    builds::atomic_json(&path, &other_instance).unwrap();
    assert_eq!(
        fixture.cleanup.submit(request.clone()).unwrap_err().code,
        "CLEANUP_PLAN_EXPIRED"
    );
    builds::atomic_json(&path, &original).unwrap();
    let lease = fixture.store.reserve_task("prj_busy", "task_busy").unwrap();
    assert_eq!(
        fixture.cleanup.submit(request.clone()).unwrap_err().code,
        "WORKSPACE_BUSY"
    );
    drop(lease);
    assert!(fixture
        .cleanup
        .submit(input(&plan, vec!["unconfirmed".into()]))
        .is_err());
    let mut missing_confirm = request;
    missing_confirm["confirmation"] = json!("yes");
    assert!(fixture.cleanup.submit(missing_confirm).is_err());
    assert!(fixture
        .paths
        .workspace_root()
        .join(".platform/trash")
        .join(trash)
        .join("project")
        .exists());
    fixture.stop().await;
}
#[tokio::test]
async fn changed_preview_and_unresolved_restore_reference_block_deletion() {
    let fixture = Fixture::new().await;
    let trash = fixture.delete().await;
    let plan = fixture.preview().await;
    let payload = fixture
        .paths
        .workspace_root()
        .join(".platform/trash")
        .join(&trash)
        .join("project");
    fs::write(payload.join("new-user-file"), "draft").unwrap();
    let task = fixture
        .wait(
            fixture
                .cleanup
                .submit(input(&plan, vec![format!("trash_{trash}")]))
                .unwrap(),
        )
        .await;
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.error.unwrap().code, "CLEANUP_PLAN_CHANGED");
    assert!(payload.join("data/main.sqlite").exists());
    builds::atomic_json(
        &fixture
            .paths
            .workspace_root()
            .join(".platform/operations/task_unresolved/journal.json"),
        &json!({"taskId":"task_unresolved","projectId":fixture.project,"resolved":false}),
    )
    .unwrap();
    let plan = fixture.preview().await;
    assert_eq!(plan["items"][0]["canClean"], false);
    fixture.stop().await;
}
#[tokio::test]
async fn lifecycle_partial_purge_preserves_truthful_state_and_active_same_id_blocks_callback() {
    let fixture = Fixture::new().await;
    let trash = fixture.delete().await;
    let payload = fixture
        .paths
        .workspace_root()
        .join(".platform/trash")
        .join(&trash)
        .join("project");
    let item = candidate(&payload);
    let parent_task = fixture
        .tasks
        .create_workspace_operation("workspaceCleanup")
        .unwrap();
    fixture.store.begin_workspace_cleanup().unwrap();
    let failure = fixture
        .lifecycle
        .cleanup_trash_with(&trash, &parent_task.task_id, |_path| {
            remove_tree_with_hook(
                &item,
                &fixture.temporary.path().join("partial.jsonl"),
                |_, _| {
                    Err(ApiError::new(
                        "TEST_IO_FAILURE",
                        "Disposable injected failure",
                    ))
                },
            )
        })
        .unwrap_err();
    fixture.store.resume_workspace();
    assert_eq!(failure.code, "TEST_IO_FAILURE");
    assert_eq!(failure.details["cleanupResult"]["completed"], false);
    let record = read_json(&payload.parent().unwrap().join("record.json")).unwrap();
    assert_eq!(record["state"], "purgeIncomplete");
    assert!(payload.exists());
    fs::create_dir(
        fixture
            .paths
            .workspace_root()
            .join("projects")
            .join(&fixture.project),
    )
    .unwrap();
    let called = std::cell::Cell::new(false);
    fixture.store.begin_workspace_cleanup().unwrap();
    let failure = fixture
        .lifecycle
        .cleanup_trash_with(&trash, &parent_task.task_id, |_| {
            called.set(true);
            Ok(json!({}))
        })
        .unwrap_err();
    fixture.store.resume_workspace();
    assert!(!called.get());
    assert!(matches!(
        failure.code.as_str(),
        "PROJECT_ID_CONFLICT" | "PROJECT_ID_RESERVED"
    ));
    fixture.stop().await;
}
#[tokio::test]
async fn completed_original_cleanup_preserves_journal_backup_and_current_project() {
    let fixture = Fixture::new().await;
    let restore = fixture.restore_original().await;
    let root = fixture.store.project(&fixture.project).unwrap().root;
    let database = hash_file(&root.join("data/main.sqlite")).unwrap();
    let plan = fixture.preview().await;
    let id = format!("original_{restore}");
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
    let operation = fixture
        .paths
        .workspace_root()
        .join(".platform/operations")
        .join(restore);
    assert!(!operation.join("original").exists());
    assert!(operation.join("journal.json").exists());
    assert_eq!(hash_file(&root.join("data/main.sqlite")).unwrap(), database);
    assert!(!fs::read_dir(
        fixture
            .paths
            .workspace_root()
            .join("backups")
            .join(&fixture.project)
    )
    .unwrap()
    .collect::<Vec<_>>()
    .is_empty());
    fixture.stop().await;
}
#[tokio::test]
async fn unknown_original_records_and_wrong_task_types_remain_protected() {
    let fixture = Fixture::new().await;
    let restore = fixture.restore_original().await;
    let original_journal = read_json(
        &fixture
            .paths
            .workspace_root()
            .join(".platform/operations")
            .join(restore)
            .join("journal.json"),
    )
    .unwrap();
    let fake = fixture
        .tasks
        .create_workspace_operation("storageScan")
        .unwrap();
    fixture.tasks.finish(&fake.task_id, Ok(json!({}))).unwrap();
    let root = fixture
        .paths
        .workspace_root()
        .join(".platform/operations")
        .join(&fake.task_id)
        .join("original");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("unknown"), "preserve").unwrap();
    let mut journal = original_journal;
    journal["taskId"] = json!(fake.task_id);
    let identity = directory_identity(&root).unwrap();
    journal["originalIdentity"] = json!([identity.device, identity.inode]);
    builds::atomic_json(&root.parent().unwrap().join("journal.json"), &journal).unwrap();
    let plan = fixture.preview().await;
    let item = plan["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == format!("original_{}", fake.task_id))
        .unwrap();
    assert_eq!(item["canClean"], false);
    assert!(root.join("unknown").exists());
    fixture.stop().await;
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
        fixture.lifecycle.clone(),
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
async fn modified_old_source_missing_current_trash_and_target_identity_all_block_cleanup() {
    let fixture = Fixture::new().await;
    let trash = fixture.delete().await;
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
    let payload = fixture
        .paths
        .workspace_root()
        .join(".platform/trash")
        .join(trash)
        .join("project");
    let retained = payload.with_file_name("disposable-hidden-for-test");
    fs::rename(&payload, &retained).unwrap();
    let missing = fixture.preview().await;
    assert_eq!(
        missing["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == id)
            .unwrap()["canClean"],
        false
    );
    fs::rename(retained, payload).unwrap();
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
async fn migrated_original_requires_matching_journal_and_payload_inventory() {
    let fixture = Fixture::new().await;
    let restore = fixture.restore_original().await;
    let (fixture, _source, _migration) = fixture.migrate().await;
    let id = format!("original_{restore}");
    let path = fixture
        .paths
        .workspace_root()
        .join(".platform/operations")
        .join(&restore)
        .join("original");
    let journal = read_json(&path.parent().unwrap().join("journal.json")).unwrap();
    let actual = directory_identity(&path).unwrap();
    assert_ne!(
        journal["originalIdentity"],
        json!([actual.device, actual.inode])
    );
    let plan = fixture.preview().await;
    let item = plan["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap();
    assert_eq!(item["canClean"], true, "{item}");
    fs::write(path.join("unknown-new-file"), "preserve").unwrap();
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
    fs::remove_file(path.join("unknown-new-file")).unwrap();
    let plan = fixture.preview().await;
    let task = fixture
        .wait(fixture.cleanup.submit(input(&plan, vec![id])).unwrap())
        .await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{:?}", task.error);
    assert!(!path.exists());
    assert!(path.parent().unwrap().join("journal.json").exists());
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
