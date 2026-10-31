//! Real plan/preview/confirmation tests use only a new disposable workspace.
use super::*;
use crate::{builds, CreateProjectInput, TaskStatus};

struct Fixture {
    _temporary: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    cleanup: Arc<CleanupManager>,
    project: String,
}
impl Fixture {
    async fn new() -> Self {
        let (temporary, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let project = store
            .create(CreateProjectInput {
                name: "Disposable Windows history cleanup".into(),
                description: None,
            })
            .await
            .unwrap();
        fs::write(
            project.project_root.join("data/attachments/preserved.txt"),
            b"current project attachment",
        )
        .unwrap();
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1".into()).unwrap();
        let cleanup = CleanupManager::new(
            paths.clone(),
            store.clone(),
            tasks.clone(),
            "disposable-w5-full-history".into(),
        )
        .unwrap();
        Self {
            _temporary: temporary,
            paths,
            store,
            tasks,
            cleanup,
            project: project.id,
        }
    }
    async fn wait(&self, task: TaskRecord) -> TaskRecord {
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            loop {
                let current = self.tasks.task(&task.task_id).unwrap();
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
    fn restore(&self, phase: &str) -> (String, PathBuf) {
        let task = format!("task_{}", Uuid::new_v4().simple());
        let path = self
            .paths
            .workspace_root()
            .join(".platform/project-state/source-restore")
            .join(&self.project)
            .join(&task);
        fs::create_dir_all(path.join("original")).unwrap();
        fs::write(path.join("original/中文源码.tsx"), b"source before restore").unwrap();
        builds::atomic_json(&path.join("journal.json"), &json!({"schemaVersion":1,"operation":"sourceRestore","projectId":self.project,"taskId":task,"phase":phase,"planDigest":"a".repeat(64)})).unwrap();
        (format!("sourcerestore_{task}"), path)
    }
    async fn clean(&self, plan: &Value, ids: Vec<String>) -> TaskRecord {
        self.wait(self.cleanup.submit(input(plan, ids)).unwrap())
            .await
    }
    async fn stop(self) {
        assert!(self.cleanup.shutdown().await.is_empty());
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
}
fn input(plan: &Value, ids: Vec<String>) -> Value {
    json!({"planId":plan["planId"],"itemIds":ids,"confirmation":CONFIRMATION})
}
fn item<'a>(plan: &'a Value, id: &str) -> &'a Value {
    plan["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["id"] == id)
        .unwrap()
}

#[tokio::test]
async fn preview_is_read_only_and_confirmation_removes_only_the_finished_owned_restore() {
    let fixture = Fixture::new().await;
    let (id, original) = fixture.restore("completed");
    let identity = PersistedIdentity::capture(&original).unwrap();
    let plan = fixture.preview().await;
    assert_eq!(item(&plan, &id)["canClean"], true, "{plan}");
    assert_eq!(item(&plan, &id)["kind"], "restoreOriginal");
    assert!(identity.matches_path(&original).unwrap());
    assert_eq!(
        fs::read(original.join("original/中文源码.tsx")).unwrap(),
        b"source before restore"
    );
    let task = fixture.clean(&plan, vec![id]).await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
    assert_eq!(task.result.as_ref().unwrap()["completed"], true);
    assert!(!original.exists());
    let project = fixture.store.project(&fixture.project).unwrap();
    assert_eq!(
        fs::read(project.root.join("data/attachments/preserved.txt")).unwrap(),
        b"current project attachment"
    );
    assert!(project.root.join("data/main.sqlite").is_file());
    assert!(!fixture.paths.data_root().join("cleanup").exists());
    fixture.stop().await;
}

#[tokio::test]
async fn pending_restore_and_late_content_changes_keep_originals_and_revoke_old_preview() {
    let fixture = Fixture::new().await;
    let (finished, root) = fixture.restore("completed");
    let plan = fixture.preview().await;
    let (pending, pending_root) = fixture.restore("applying");
    let protected = fixture.preview().await;
    assert_eq!(item(&protected, &finished)["canClean"], false);
    assert_eq!(item(&protected, &pending)["canClean"], false);
    let task = fixture.clean(&plan, vec![finished]).await;
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.error.unwrap().code, "CLEANUP_PLAN_CHANGED");
    assert!(root.join("original/中文源码.tsx").is_file());
    assert!(pending_root.join("original/中文源码.tsx").is_file());
    fixture.stop().await;
}

#[tokio::test]
async fn current_content_changed_after_preview_is_retained_without_partial_deletion() {
    let fixture = Fixture::new().await;
    let (id, root) = fixture.restore("completed");
    let plan = fixture.preview().await;
    fs::write(root.join("original/中文源码.tsx"), b"edited before cleanup").unwrap();
    let task = fixture.clean(&plan, vec![id]).await;
    assert_eq!(task.status, TaskStatus::Failed);
    assert_eq!(task.error.unwrap().code, "CLEANUP_PLAN_CHANGED");
    assert_eq!(
        fs::read(root.join("original/中文源码.tsx")).unwrap(),
        b"edited before cleanup"
    );
    assert!(root.join("journal.json").is_file());
    fixture.stop().await;
}

#[tokio::test]
async fn expired_runtime_selection_confirmation_and_live_task_do_not_grant_deletion() {
    let fixture = Fixture::new().await;
    let (id, root) = fixture.restore("completed");
    let plan = fixture.preview().await;
    let key = plan["planId"].as_str().unwrap().to_string();
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
        fixture
            .cleanup
            .submit(input(&plan, vec![id.clone()]))
            .unwrap_err()
            .code,
        "CLEANUP_PLAN_EXPIRED"
    );
    let mut foreign = original.clone();
    foreign.instance_id = "other-manager".into();
    fixture
        .cleanup
        .plans
        .lock()
        .unwrap()
        .insert(key.clone(), foreign);
    assert_eq!(
        fixture
            .cleanup
            .submit(input(&plan, vec![id.clone()]))
            .unwrap_err()
            .code,
        "CLEANUP_PLAN_EXPIRED"
    );
    fixture.cleanup.plans.lock().unwrap().insert(key, original);
    let lease = fixture
        .store
        .reserve_task("prj_disposable", "task_disposable")
        .unwrap();
    assert_eq!(
        fixture
            .cleanup
            .submit(input(&plan, vec![id.clone()]))
            .unwrap_err()
            .code,
        "WORKSPACE_BUSY"
    );
    drop(lease);
    assert!(fixture
        .cleanup
        .submit(input(&plan, vec!["unconfirmed".into()]))
        .is_err());
    let mut wrong = input(&plan, vec![id]);
    wrong["confirmation"] = json!("yes");
    assert!(fixture.cleanup.submit(wrong).is_err());
    assert!(root.join("journal.json").is_file());
    fixture.stop().await;
}

#[tokio::test]
async fn consumed_plan_reuses_its_real_task_and_a_reopened_manager_requires_new_preview() {
    let fixture = Fixture::new().await;
    let (id, root) = fixture.restore("completed");
    let plan = fixture.preview().await;
    let request = input(&plan, vec![id]);
    let task = fixture.cleanup.submit(request.clone()).unwrap();
    assert_eq!(
        fixture.cleanup.submit(request.clone()).unwrap().task_id,
        task.task_id
    );
    let task = fixture.wait(task).await;
    assert_eq!(task.status, TaskStatus::Succeeded, "{task:?}");
    assert!(!root.exists());
    let next = CleanupManager::new(
        fixture.paths.clone(),
        fixture.store.clone(),
        fixture.tasks.clone(),
        "disposable-next-history".into(),
    )
    .unwrap();
    assert_eq!(
        next.submit(request).unwrap_err().code,
        "CLEANUP_PLAN_EXPIRED"
    );
    assert!(next.latest_task().unwrap().is_none());
    next.shutdown().await;
    fixture.stop().await;
}

#[test]
fn cleanup_logical_byte_wire_values_keep_exact_large_integers() {
    assert_eq!(
        wire_u64(Some(9_007_199_254_740_992)),
        json!({"$int64":"9007199254740992"})
    );
    assert_eq!(wire_u64(None), Value::Null);
}
