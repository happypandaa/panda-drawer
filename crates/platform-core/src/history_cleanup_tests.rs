//! All destructive tests use uniquely owned TempDir data, never development data.
use super::*;
use crate::{builds, maintenance::MaintenanceManager, CreateProjectInput, TaskStatus};

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
    let removed = remove_tree(&item).unwrap();
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
    assert!(remove_tree(&item).is_err());
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "modified");
    fs::rename(&parent, temporary.path().join("old-parent")).unwrap();
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("replacement"), "retain").unwrap();
    assert!(remove_tree(&item).is_err());
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
    let failure = remove_tree_with_hook(&item, |_, path| {
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
    assert!(restarted.latest_task().unwrap().is_none());
    assert!(!fixture.paths.data_root().join("cleanup/plans").exists());
    assert!(!fixture.paths.data_root().join("cleanup/receipts").exists());
    let info = crate::workspace::info(&fixture.paths, &fixture.store).unwrap();
    assert_eq!(info["retainedRoots"][0]["absent"], true);
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
async fn explicit_repreview_after_partial_marker_removal_uses_fresh_owned_inventory() {
    let fixture = Fixture::new().await;
    let (fixture, source, migration) = fixture.migrate().await;
    let preview = fixture.preview().await;
    let id = format!("retained_{migration}");
    let plan = fixture.cleanup.plans.lock().unwrap()[preview["planId"].as_str().unwrap()].clone();
    let item = plan.items.iter().find(|item| item.id == id).unwrap();
    assert!(item.can_clean);
    fixture.store.begin_workspace_cleanup().unwrap();
    let failure = remove_tree_with_hook(item, |progress, _| {
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
    })
    .unwrap_err();
    fixture.store.resume_workspace();
    assert_eq!(failure.code, "TEST_AFTER_MARKER");
    assert!(!source.join(RETIRED).exists());
    assert!(!fixture.paths.data_root().join("cleanup/receipts").exists());
    let fresh = fixture.preview().await;
    assert_eq!(
        plan_item(&fresh, &id)["canClean"],
        true,
        "{}",
        plan_item(&fresh, &id)
    );
    let task = fixture
        .wait(fixture.cleanup.submit(input(&fresh, vec![id])).unwrap())
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

fn hex32() -> String {
    Uuid::new_v4().simple().to_string()
}
fn plan_item<'a>(plan: &'a Value, id: &str) -> &'a Value {
    plan["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == id)
        .unwrap_or_else(|| panic!("{id} missing from {plan}"))
}
fn blocked_text(plan: &Value, id: &str) -> String {
    plan_item(plan, id)["blockedReasons"].to_string()
}
impl Fixture {
    fn source(&self, status: &str) -> (String, PathBuf) {
        let id = format!("src_{}", hex32());
        let owner = Uuid::new_v4().to_string();
        let ownership = crate::node_project_ownership::NodeProjectOwnership::new(
            self.paths.data_root().into(),
            self.paths.workspace_root().into(),
        );
        ownership.initialize(&owner).unwrap();
        ownership.record_source(&owner, &id).unwrap();
        let slot = self
            .paths
            .workspace_root()
            .join(".platform/sources")
            .join(&id);
        fs::create_dir_all(slot.join("repository/src")).unwrap();
        fs::write(slot.join("repository/src/app.js"), "disposable source copy").unwrap();
        ownership.record_source(&owner, &id).unwrap();
        builds::atomic_json(
            &slot.join("source.json"),
            &json!({"schemaVersion":1,"sourceId":id,"ownerAppId":owner,"name":"Disposable source","status":status}),
        )
        .unwrap();
        (id, slot)
    }
    fn node_root(&self) -> PathBuf {
        self.paths.data_root().join("node-apps")
    }
    fn installation(&self, state: &str, app: &str) -> (String, PathBuf) {
        let id = format!("install_{}", hex32());
        let root = self.node_root().join(".installations").join(&id);
        fs::create_dir_all(root.join("npm-cache")).unwrap();
        fs::write(root.join("npm-cache/package.tgz"), "disposable cache").unwrap();
        let generation = format!("gen_{}", hex32());
        let provenance = json!({"sourceId":format!("src_{}", hex32()),"sourceDigest":"c".repeat(64),
            "sourceCommit":null,"node":{"path":"/disposable/node","version":"v24.0.0"},
            "npm":{"path":"/disposable/npm","version":"10.0.0"},"preparedAt":1});
        let prepared = matches!(state, "prepared" | "committing" | "committed").then(|| {
            json!({"candidateId":id,"appId":app,"generation":generation,
                "manifest":{"name":"Disposable Node app"},"programDigest":"b".repeat(64),
                "provenance":provenance})
        });
        let registration = matches!(state, "committing" | "committed").then(|| {
            json!({"version":2,"id":app,"kind":"managed","generation":generation,
                "manifestDigest":"a".repeat(64),"programDigest":"b".repeat(64),"provenance":provenance})
        });
        builds::atomic_json(
            &root.join("install.json"),
            &json!({"version":1,"candidateId":id,"prepareTaskId":format!("task_{}", hex32()),
                "applyTaskId":null,"state":state,
                "target":{"appId":app,"registrationDigest":null,"generation":null,"manifest":null,"configurationRevision":0},
                "prepared":prepared,"registration":registration,"error":null}),
        )
        .unwrap();
        (id, root)
    }
    /// A managed v2 app with its current generation and one replaced generation.
    fn managed_app(&self, source: &str) -> (String, PathBuf, String, PathBuf) {
        let app = read_json(
            &self
                .paths
                .workspace_root()
                .join(".platform/sources")
                .join(source)
                .join("source.json"),
        )
        .unwrap()["ownerAppId"]
            .as_str()
            .unwrap()
            .to_owned();
        let slot = self.node_root().join(&app);
        let current = format!("gen_{}", hex32());
        let old = format!("gen_{}", hex32());
        for generation in [&current, &old] {
            let program = slot.join("generations").join(generation);
            fs::create_dir_all(program.join("server")).unwrap();
            fs::write(program.join("server/main.mjs"), "disposable program").unwrap();
            builds::atomic_json(
                &program.join("local-app.json"),
                &json!({"name":"Disposable Node app"}),
            )
            .unwrap();
        }
        builds::atomic_json(
            &slot.join("registration.json"),
            &json!({"version":2,"id":app,"kind":"managed","generation":current,
                "manifestDigest":"a".repeat(64),"programDigest":"b".repeat(64),
                "provenance":{"sourceId":source,"sourceDigest":"c".repeat(64),"sourceCommit":null,
                    "node":{"path":"/disposable/node","version":"v24.0.0"},
                    "npm":{"path":"/disposable/npm","version":"10.0.0"},"preparedAt":1}}),
        )
        .unwrap();
        (
            app.clone(),
            slot.join("generations").join(&current),
            old.clone(),
            slot.join("generations").join(&old),
        )
    }
    fn source_restore(&self, task: &str, phase: &str) -> PathBuf {
        let root = self
            .paths
            .workspace_root()
            .join(".platform/project-state/source-restore")
            .join(&self.project)
            .join(task);
        fs::create_dir_all(root.join("original")).unwrap();
        fs::write(root.join("original/App.tsx"), "source before restore").unwrap();
        builds::atomic_json(
            &root.join("journal.json"),
            &json!({"schemaVersion":1,"operation":"sourceRestore","projectId":self.project,
                "taskId":task,"phase":phase,"planDigest":"a".repeat(64)}),
        )
        .unwrap();
        root
    }
    async fn clean(&self, plan: &Value, ids: Vec<String>) -> TaskRecord {
        self.wait(self.cleanup.submit(input(plan, ids)).unwrap())
            .await
    }
}

#[tokio::test]
async fn source_copies_are_cleanable_only_when_finished_and_unused() {
    let fixture = Fixture::new().await;
    let (unused, unused_slot) = fixture.source("ready");
    let (acquiring, acquiring_slot) = fixture.source("acquiring");
    let (installed, installed_slot) = fixture.source("ready");
    let (_app, current, _, _) = fixture.managed_app(&installed);
    let plan = fixture.preview().await;
    assert_eq!(plan_item(&plan, &unused)["canClean"], true, "{plan}");
    assert_eq!(plan_item(&plan, &unused)["kind"], "sourceCopy");
    assert_eq!(plan_item(&plan, &unused)["name"], "Disposable source");
    assert_eq!(plan_item(&plan, &acquiring)["canClean"], false);
    assert!(blocked_text(&plan, &acquiring).contains("仍在获取中"));
    assert_eq!(plan_item(&plan, &installed)["canClean"], false);
    assert!(blocked_text(&plan, &installed).contains("已安装的应用仍在使用"));

    // Another project's installation cannot claim or protect this owner's copy.
    let (_, preparing) = fixture.installation("preparing", &Uuid::new_v4().to_string());
    let guarded = fixture.preview().await;
    assert_eq!(plan_item(&guarded, &unused)["canClean"], true);
    fs::remove_dir_all(&preparing).unwrap();

    let plan = fixture.preview().await;
    let task = fixture.clean(&plan, vec![unused.clone()]).await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{:?}", task.error);
    assert!(!unused_slot.exists());
    assert!(acquiring_slot.join("repository/src/app.js").is_file());
    assert!(installed_slot.join("repository/src/app.js").is_file());
    assert!(current.join("server/main.mjs").is_file());
    fixture.stop().await;
}
#[tokio::test]
async fn source_cleanup_uses_only_its_admitted_owner_and_never_deep_json_references() {
    let fixture = Fixture::new().await;
    let (source, slot) = fixture.source("ready");
    let foreign = fixture.node_root().join(Uuid::new_v4().to_string());
    fs::create_dir_all(&foreign).unwrap();
    builds::atomic_json(
        &foreign.join("registration.json"),
        &json!({"notes":{"nested":[source,slot]}}),
    )
    .unwrap();
    let preview = fixture.preview().await;
    assert_eq!(plan_item(&preview, &source)["canClean"], true);
    let record = slot.join("source.json");
    let mut changed = read_json(&record).unwrap();
    changed["ownerAppId"] = json!(Uuid::new_v4().to_string());
    builds::atomic_json(&record, &changed).unwrap();
    let unknown = fixture.preview().await;
    assert_eq!(plan_item(&unknown, &source)["canClean"], false);
    assert!(slot.exists());
    fixture.stop().await;
}
#[tokio::test]
async fn deleting_one_owner_invalidates_its_history_plans_and_latest_but_keeps_other_drafts() {
    let fixture = Fixture::new().await;
    let (source, slot) = fixture.source("ready");
    let (other, other_slot) = fixture.source("ready");
    let owner = read_json(&slot.join("source.json")).unwrap()["ownerAppId"]
        .as_str()
        .unwrap()
        .to_owned();
    let preview = fixture.preview().await;
    let original_id = preview["planId"].as_str().unwrap().to_owned();
    let mut unrelated = fixture.cleanup.plans.lock().unwrap()[&original_id].clone();
    unrelated.plan_id = format!("cleanup_plan_{}", hex32());
    unrelated.items.retain(|item| item.id == other);
    let task = fixture
        .tasks
        .create_workspace_operation("workspaceCleanupPreview")
        .unwrap();
    unrelated.preview_task_id = task.task_id.clone();
    fixture
        .tasks
        .finish(&task.task_id, Ok(unrelated.public()))
        .unwrap();
    let other_plan = unrelated.plan_id.clone();
    fixture
        .cleanup
        .plans
        .lock()
        .unwrap()
        .insert(other_plan.clone(), unrelated);
    let cleaned = fixture.clean(&preview, vec![source.clone()]).await;
    assert_eq!(cleaned.status, TaskStatus::Succeeded);
    let forgotten = fixture
        .cleanup
        .forget_deleted_project(
            &owner,
            std::slice::from_ref(&source),
            std::slice::from_ref(&slot),
        )
        .await
        .unwrap();
    assert!(forgotten.contains(&cleaned.task_id));
    assert!(!forgotten.contains(&task.task_id));
    assert!(fixture.cleanup.latest_task().unwrap().is_none());
    assert!(!fixture
        .cleanup
        .plans
        .lock()
        .unwrap()
        .contains_key(&original_id));
    assert!(fixture
        .cleanup
        .plans
        .lock()
        .unwrap()
        .contains_key(&other_plan));
    assert!(fixture.tasks.task(&cleaned.task_id).is_err());
    assert!(fixture.tasks.task(&task.task_id).is_ok());
    assert!(other_slot.exists());
    assert!(fixture.cleanup.preview().is_err());
    fixture.cleanup.finish_deleted_project(&owner);
    assert!(fixture.cleanup.preview().is_ok());
    fixture.stop().await;
}
#[tokio::test]
async fn abandoned_diagnostic_database_is_project_owned_and_full_delete_keeps_other_diagnostics() {
    let fixture = Fixture::new().await;
    let other = fixture
        .store
        .create(CreateProjectInput {
            name: "Disposable diagnostic B".into(),
            description: None,
        })
        .await
        .unwrap();
    let diagnostics = fixture
        .paths
        .workspace_root()
        .join(".platform/project-state/diagnostics");
    let abandoned = diagnostics
        .join(&fixture.project)
        .join("interrupted-check/database.sqlite");
    let unrelated = diagnostics
        .join(&other.id)
        .join("interrupted-check/database.sqlite");
    for path in [&abandoned, &unrelated] {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"disposable diagnostic business data").unwrap();
    }
    let storage = crate::storage::StorageManager::new(
        fixture.paths.clone(),
        fixture.store.clone(),
        fixture.tasks.clone(),
        fixture.maintenance.clone(),
    )
    .unwrap();
    storage
        .forget_deleted_project(&fixture.project)
        .await
        .unwrap();
    assert!(abandoned.exists());
    assert!(unrelated.exists());
    let lifecycle = crate::lifecycle::LifecycleManager::new(
        fixture.paths.clone(),
        fixture.store.clone(),
        fixture.tasks.clone(),
        fixture.maintenance.clone(),
    )
    .unwrap();
    let task=lifecycle.submit_project(&fixture.project,"delete",json!({"expectedDocumentEpoch":fixture.store.registered_epoch(&fixture.project).unwrap().unwrap()})).await.unwrap();
    let deleted = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let current = lifecycle
                .task_for_project(&task.task_id, &fixture.project, "delete")
                .unwrap();
            if current.status.is_final() {
                break current;
            }
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(deleted.status, TaskStatus::Succeeded, "{deleted:?}");
    assert!(!diagnostics.join(&fixture.project).exists());
    assert_eq!(
        fs::read(&unrelated).unwrap(),
        b"disposable diagnostic business data"
    );
    assert!(!fixture
        .paths
        .data_root()
        .join("cleanup/diagnostics")
        .exists());
    lifecycle.shutdown().await;
    storage.shutdown().await;
    fixture.stop().await;
}
#[tokio::test]
async fn restarted_history_clears_only_typed_matching_tasks_and_owner_identity_wins_over_paths() {
    let fixture = Fixture::new().await;
    let (source, slot) = fixture.source("ready");
    let owner = read_json(&slot.join("source.json")).unwrap()["ownerAppId"]
        .as_str()
        .unwrap()
        .to_owned();
    let preview = fixture.preview().await;
    let a_task = fixture.cleanup.plans.lock().unwrap()[preview["planId"].as_str().unwrap()]
        .preview_task_id
        .clone();
    let other = Uuid::new_v4().to_string();
    let b_task = fixture
        .tasks
        .create_workspace_operation("workspaceCleanupPreview")
        .unwrap();
    fixture
        .tasks
        .finish(
            &b_task.task_id,
            Ok(json!({"items":[{"id":source,"projectId":other,"path":slot}]})),
        )
        .unwrap();
    fixture.cleanup.shutdown().await;
    fixture.tasks.shutdown().await;
    let tasks = BuildManager::new(
        fixture.paths.clone(),
        fixture.store.clone(),
        "http://127.0.0.1:9".into(),
    )
    .unwrap();
    let restarted = CleanupManager::new(
        fixture.paths.clone(),
        fixture.store.clone(),
        tasks.clone(),
        "disposable-restarted-history".into(),
    )
    .unwrap();
    assert!(restarted.plans.lock().unwrap().is_empty());
    let forgotten = restarted
        .forget_deleted_project(
            &owner,
            std::slice::from_ref(&source),
            std::slice::from_ref(&slot),
        )
        .await
        .unwrap();
    assert!(forgotten.contains(&a_task));
    assert!(!forgotten.contains(&b_task.task_id));
    assert!(tasks.task(&a_task).is_err());
    assert!(tasks.task(&b_task.task_id).is_ok());
    assert!(!fixture
        .paths
        .workspace_root()
        .join(".platform/tasks")
        .join(&a_task)
        .exists());
    restarted.shutdown().await;
    tasks.shutdown().await;
    fixture.stop().await;
}

#[tokio::test]
async fn node_installations_and_replaced_generations_follow_install_state() {
    let fixture = Fixture::new().await;
    let (source, _) = fixture.source("ready");
    let (app, current, old, old_path) = fixture.managed_app(&source);
    let (failed, failed_path) = fixture.installation("failed", &app);
    let (prepared, prepared_path) = fixture.installation("prepared", &app);
    let plan = fixture.preview().await;
    assert_eq!(plan_item(&plan, &failed)["canClean"], true, "{plan}");
    assert_eq!(plan_item(&plan, &failed)["kind"], "nodeInstallation");
    assert_eq!(plan_item(&plan, &prepared)["canClean"], false);
    assert!(blocked_text(&plan, &prepared).contains("还可以继续安装"));
    assert_eq!(plan_item(&plan, &old)["canClean"], true);
    assert_eq!(plan_item(&plan, &old)["name"], "Disposable Node app");
    let current_name = current.file_name().unwrap().to_str().unwrap();
    assert!(!plan["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["id"] == current_name));

    // An unfinished commit for the same app protects every program version.
    let (committing, committing_path) = fixture.installation("committing", &app);
    let busy = fixture.preview().await;
    assert_eq!(plan_item(&busy, &old)["canClean"], false);
    assert!(blocked_text(&busy, &old).contains("正在进行或需要处理的安装"));
    assert_eq!(plan_item(&busy, &committing)["canClean"], false);
    fs::remove_dir_all(&committing_path).unwrap();

    let plan = fixture.preview().await;
    let task = fixture
        .clean(&plan, vec![failed.clone(), old.clone()])
        .await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{:?}", task.error);
    assert!(!failed_path.exists());
    assert!(!old_path.exists());
    assert!(current.join("server/main.mjs").is_file());
    assert!(prepared_path.join("install.json").is_file());
    fixture.stop().await;
}

#[tokio::test]
async fn finished_source_restore_original_is_cleanable_until_a_pending_one_appears() {
    let fixture = Fixture::new().await;
    let finished = format!("task_{}", hex32());
    let finished_path = fixture.source_restore(&finished, "completed");
    let id = format!("sourcerestore_{finished}");
    let plan = fixture.preview().await;
    assert_eq!(plan_item(&plan, &id)["canClean"], true, "{plan}");
    assert_eq!(plan_item(&plan, &id)["kind"], "restoreOriginal");
    assert_eq!(plan_item(&plan, &id)["projectId"], fixture.project);

    let pending = format!("task_{}", hex32());
    let pending_path = fixture.source_restore(&pending, "applying");
    let guarded = fixture.preview().await;
    assert_eq!(plan_item(&guarded, &id)["canClean"], false);
    assert!(blocked_text(&guarded, &id).contains("未完成或无法核对"));
    let pending_id = format!("sourcerestore_{pending}");
    assert_eq!(plan_item(&guarded, &pending_id)["canClean"], false);
    // The earlier plan is still bound to its preview; execution revalidates.
    let task = fixture.clean(&plan, vec![id.clone()]).await;
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.error.unwrap().code, "CLEANUP_PLAN_CHANGED");
    assert!(finished_path.join("original/App.tsx").is_file());
    assert!(pending_path.join("original/App.tsx").is_file());

    fs::remove_dir_all(&pending_path).unwrap();
    let plan = fixture.preview().await;
    let task = fixture.clean(&plan, vec![id]).await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{:?}", task.error);
    assert!(!finished_path.exists());
    assert!(fixture
        .store
        .project(&fixture.project)
        .unwrap()
        .root
        .join("data/main.sqlite")
        .is_file());
    fixture.stop().await;
}

#[tokio::test]
async fn plan_expiry_runtime_change_invalid_selection_and_active_operations_do_not_delete() {
    let fixture = Fixture::new().await;
    let (source, slot) = fixture.source("ready");
    let plan = fixture.preview().await;
    let request = input(&plan, vec![source.clone()]);
    let key = plan["planId"].as_str().unwrap().to_owned();
    let original = fixture.cleanup.plans.lock().unwrap()[&key].clone();
    let mut expired = original.clone();
    expired.expires_at = 0;
    fixture
        .cleanup
        .plans
        .lock()
        .unwrap()
        .insert(key.clone(), expired);
    assert_eq!(
        fixture.cleanup.submit(request.clone()).unwrap_err().code,
        "CLEANUP_PLAN_EXPIRED"
    );
    let mut other_instance = original.clone();
    other_instance.instance_id = "other".into();
    fixture
        .cleanup
        .plans
        .lock()
        .unwrap()
        .insert(key.clone(), other_instance);
    assert_eq!(
        fixture.cleanup.submit(request.clone()).unwrap_err().code,
        "CLEANUP_PLAN_EXPIRED"
    );
    fixture.cleanup.plans.lock().unwrap().insert(key, original);
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
    assert!(slot.join("repository/src/app.js").is_file());
    fixture.stop().await;
}

#[tokio::test]
async fn changed_preview_and_new_protection_block_deletion() {
    let fixture = Fixture::new().await;
    let (source, slot) = fixture.source("ready");
    let plan = fixture.preview().await;
    fs::write(slot.join("repository/new-user-file"), "draft").unwrap();
    let task = fixture.clean(&plan, vec![source.clone()]).await;
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.error.unwrap().code, "CLEANUP_PLAN_CHANGED");
    assert!(slot.join("repository/new-user-file").is_file());
    let (_app, _, _, _) = fixture.managed_app(&source);
    let plan = fixture.preview().await;
    assert_eq!(plan_item(&plan, &source)["canClean"], false);
    fixture.stop().await;
}

#[test]
fn owned_dependency_removal_is_log_free_and_does_not_read_payload_contents() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-node-modules");
    fs::create_dir_all(root.join("icons")).unwrap();
    for index in 0..4_000 {
        fs::write(
            root.join("icons").join(format!("icon-{index}.svg")),
            b"<svg/>",
        )
        .unwrap();
    }
    let unreadable = root.join("unreadable.js");
    fs::write(
        &unreadable,
        b"need not hash a dependency being permanently removed",
    )
    .unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();
    let expected = PersistedIdentity::capture(&root).unwrap();
    let started = std::time::Instant::now();
    let result = remove_owned_directory(&root, &expected).unwrap();
    eprintln!(
        "log-free permanent removal: 4001 files in {:?}",
        started.elapsed()
    );
    assert_eq!(result["removedFileCount"], 4_001);
    assert_eq!(result["completed"], true);
    assert!(!root.exists());
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 0);
}
