//! Real disposable files and publication transactions; no repository scripts run.
use super::*;
use crate::{builds::BuildProvenance, CreateProjectInput};
use serde_json::json;

struct Fixture {
    _temp: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    project: ProjectHandle,
    access: ArtifactAccess,
}
impl Fixture {
    async fn new() -> Self {
        let (temp, paths) = RuntimePaths::for_test();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
        let info = store
            .create(CreateProjectInput {
                name: "Disposable webpage generations".into(),
                description: None,
            })
            .await
            .unwrap();
        let project = store.project(&info.id).unwrap();
        fs::create_dir_all(project.root.join("dist")).unwrap();
        fs::write(
            project.root.join("dist/index.html"),
            "<html><body>initial</body></html>",
        )
        .unwrap();
        fs::write(
            project.root.join("dist/late.js"),
            "window.generation='initial'",
        )
        .unwrap();
        let access = ArtifactAccess::new(paths.clone(), store.clone());
        Self {
            _temp: temp,
            paths,
            store,
            project,
            access,
        }
    }
    fn prepare(&self, text: &str) -> (String, PathBuf, BuildProvenance) {
        let id = format!("task_{}", Uuid::new_v4().simple());
        let task = self.project.root.join(".platform/builds").join(&id);
        let staging = task.join("dist");
        fs::create_dir_all(&staging).unwrap();
        fs::write(
            staging.join("index.html"),
            format!("<html><body>{text}</body></html>"),
        )
        .unwrap();
        fs::write(
            staging.join("late.js"),
            format!("window.generation='{text}'"),
        )
        .unwrap();
        builds::copy_tree(&self.project.root.join("dist"), &task.join("previous")).unwrap();
        let tool = crate::environment::ExecutableInfo {
            path: "/disposable/not-executed".into(),
            version: "fixture".into(),
        };
        let record = BuildProvenance {
            schema_version: 1,
            build_id: id.clone(),
            task_id: id.clone(),
            completed_at: 1,
            digest_rule_version: 1,
            input_digest: "a".repeat(64),
            artifact_digest: builds::tree_digest(&staging, false).unwrap(),
            node: tool.clone(),
            npm: tool,
            sdk_version: None,
            entry_relative: "index.html".into(),
        };
        (id, staging, record)
    }
    fn publish(&self, text: &str) -> String {
        let (id, staging, record) = self.prepare(text);
        let publication_result = {
            let _publication = self.access.publication().unwrap();
            builds::publish(
                &self.paths,
                &self.project,
                &staging,
                &record,
                Some(builds::tree_digest(&self.project.root.join("dist"), false).unwrap()),
            )
        };
        publication_result.unwrap();
        let report = self
            .access
            .cleanup(&self.project, std::slice::from_ref(&id));
        assert!(!report.cleanup_pending, "{report:?}");
        id
    }
    fn generation(&self, id: &str) -> PathBuf {
        self.project.root.join(".platform/artifacts").join(id)
    }
    async fn wait_removed(&self, path: &std::path::Path) {
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while path.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn window_snapshot_copy_is_a_short_registered_project_operation() {
    let fixture = Fixture::new().await;
    let (entered, copying) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    *fixture.access.0.capture_pause.lock().unwrap() = Some((entered, blocked));
    let access = fixture.access.clone();
    let project = fixture.project.clone();
    let capture = tokio::task::spawn_blocking(move || access.current(&project, false));
    copying.await.unwrap();
    // The preparing receipt already exists, while the payload writer remains
    // blocked. Both the control write and source read are admitted together.
    assert!(fixture.store.has_active_tasks());
    assert!(fixture
        .project
        .operation_gate
        .clone()
        .try_write_owned()
        .is_err());
    let issue = match fixture.store.reserve_transient_operation(
        &fixture.project.manifest.id,
        "disposable competing operation",
    ) {
        Ok(_) => panic!("snapshot copy must remain registered until its writer ends"),
        Err(issue) => issue,
    };
    assert_eq!(issue.code, "PROJECT_BUSY");
    let views = fixture
        .access
        .views_root(&fixture.project.manifest.id)
        .unwrap();
    let container = fs::read_dir(views).unwrap().next().unwrap().unwrap().path();
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(container.join("view.json")).unwrap()).unwrap();
    assert_eq!(receipt["phase"], "preparing");
    assert!(!container.join("dist").exists());
    release.send(()).unwrap();
    let (_, window) = capture.await.unwrap().unwrap();
    assert!(container.join("dist/index.html").is_file());
    assert!(!fixture.store.has_active_tasks());
    // A long-lived window pin does not hold the short writer/read admission.
    drop(
        fixture
            .project
            .operation_gate
            .clone()
            .try_write_owned()
            .unwrap(),
    );
    drop(window);
    assert!(fixture.access.shutdown().await.is_empty());
}

#[tokio::test]
async fn rejected_window_snapshot_admission_creates_no_control_material() {
    let fixture = Fixture::new().await;
    let views = fixture
        .access
        .views_root(&fixture.project.manifest.id)
        .unwrap();
    // Creation registered this disposable owner up front. Remove only that
    // fixture's control directory so an accidental late initialize is visible.
    fs::remove_dir_all(views.parent().unwrap()).unwrap();
    fixture.store.begin_workspace_migration().unwrap();
    let issue = match fixture.access.current(&fixture.project, false) {
        Ok(_) => panic!("migration freeze must reject a new control writer"),
        Err(issue) => issue,
    };
    assert_eq!(issue.code, "WORKSPACE_MIGRATING");
    assert!(!views.parent().unwrap().exists());
    assert!(!fixture.store.has_active_tasks());
    fixture.store.resume_workspace();
    let exclusive = fixture.project.operation_gate.clone().write_owned().await;
    let issue = match fixture.access.current(&fixture.project, false) {
        Ok(_) => panic!("an exclusive project operation must reject snapshot capture"),
        Err(issue) => issue,
    };
    assert_eq!(issue.code, "PROJECT_BUSY");
    assert!(!views.parent().unwrap().exists());
    assert!(!fixture.store.has_active_tasks());
    drop(exclusive);
    assert!(fixture.access.shutdown().await.is_empty());
}

#[tokio::test]
async fn three_publications_keep_only_current_and_remove_successful_intermediate_material() {
    let fixture = Fixture::new().await;
    let first = fixture.publish("first");
    let second = fixture.publish("second");
    let third = fixture.publish("third");
    assert!(!fixture.generation(&first).exists());
    assert!(!fixture.generation(&second).exists());
    assert!(fixture.generation(&third).join("dist/late.js").is_file());
    for id in [&first, &second, &third] {
        assert!(!fixture
            .project
            .root
            .join(".platform/builds")
            .join(id)
            .exists());
    }
    assert_eq!(
        fs::read_dir(fixture.project.root.join(".platform/artifacts"))
            .unwrap()
            .count(),
        1
    );
    assert!(
        fs::read_to_string(fixture.project.root.join("dist/index.html"))
            .unwrap()
            .contains("third")
    );
    assert!(fixture.access.shutdown().await.is_empty());
}

#[tokio::test]
async fn window_pin_preserves_late_resources_and_actual_release_retires_its_old_generation() {
    let fixture = Fixture::new().await;
    let first = fixture.publish("first");
    let (_, window) = fixture.access.current(&fixture.project, false).unwrap();
    let window = window.unwrap();
    fixture.publish("second");
    let third = fixture.publish("third");
    let reader = fixture.access.reader(&fixture.project, &first).unwrap();
    assert!(fs::read_to_string(reader.root().join("late.js"))
        .unwrap()
        .contains("first"));
    drop(window);
    assert!(
        fixture.generation(&first).exists(),
        "the actual producer still owns its reader"
    );
    drop(reader);
    fixture.wait_removed(&fixture.generation(&first)).await;
    assert!(fixture.access.shutdown().await.is_empty());
    assert!(fixture
        .paths
        .workspace_root()
        .join(".platform/cache/storage/invalidations/projects")
        .join(format!("{}.json", fixture.project.manifest.id))
        .is_file());
    assert!(fixture.generation(&third).exists());
}

#[tokio::test]
async fn a_provenance_free_window_gets_a_fixed_owned_snapshot_without_modifying_live_output() {
    let fixture = Fixture::new().await;
    let before = builds::tree_digest(&fixture.project.root.join("dist"), false).unwrap();
    let (generation, window) = fixture.access.current(&fixture.project, false).unwrap();
    let generation = generation.unwrap();
    assert!(valid_view_id(&generation.build_id));
    assert_eq!(
        builds::tree_digest(&fixture.project.root.join("dist"), false).unwrap(),
        before
    );
    assert!(generation
        .root
        .starts_with(fixture.paths.data_root().join("web-project-ownership")));
    fixture.publish("first real build");
    let reader = fixture
        .access
        .reader(&fixture.project, &generation.build_id)
        .unwrap();
    assert!(fs::read_to_string(reader.root().join("late.js"))
        .unwrap()
        .contains("initial"));
    drop(window);
    assert!(generation.root.exists());
    drop(reader);
    fixture
        .wait_removed(generation.root.parent().unwrap())
        .await;
    assert!(fixture.access.shutdown().await.is_empty());
}

#[tokio::test]
async fn missing_generation_does_not_leave_a_pin_or_a_locked_registry() {
    let fixture = Fixture::new().await;
    let missing = format!("task_{}", Uuid::new_v4().simple());
    assert!(fixture.access.reader(&fixture.project, &missing).is_err());
    assert!(fixture.access.0.state.try_lock().unwrap().pins.is_empty());
    let (_, pin) = fixture.access.current(&fixture.project, true).unwrap();
    drop(pin);
    assert!(fixture.access.shutdown().await.is_empty());
}

#[tokio::test]
async fn sealed_source_access_drains_actual_readers_without_waiting_for_window_pins() {
    let fixture = Fixture::new().await;
    fixture.publish("published");
    let (generation, window) = fixture.access.current(&fixture.project, false).unwrap();
    let reader = fixture
        .access
        .reader(&fixture.project, &generation.unwrap().build_id)
        .unwrap();
    fixture.access.seal_access();
    assert!(fixture.access.current(&fixture.project, true).is_err());
    let access = fixture.access.clone();
    let waiter = tokio::spawn(async move { access.wait_readers_drained().await });
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished());
    drop(reader);
    tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap();
    assert!(!fixture.access.0.state.lock().unwrap().pins.is_empty());
    drop(window);
    assert!(fixture.access.shutdown().await.is_empty());
}

#[tokio::test]
async fn a_new_state_cannot_adopt_another_live_or_unknown_scope_view() {
    let fixture = Fixture::new().await;
    let (generation, window) = fixture.access.current(&fixture.project, false).unwrap();
    let generation = generation.unwrap();
    let other = ArtifactAccess::new(fixture.paths.clone(), fixture.store.clone());
    assert!(!other.cleanup(&fixture.project, &[]).cleanup_pending);
    assert!(generation.root.exists());
    assert!(other
        .reader(&fixture.project, &generation.build_id)
        .is_err());
    let receipt_path = generation.root.parent().unwrap().join("view.json");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
    receipt["scope"] = json!(Uuid::new_v4().to_string());
    builds::atomic_json(&receipt_path, &receipt).unwrap();
    assert!(!other.cleanup(&fixture.project, &[]).cleanup_pending);
    assert!(
        generation.root.exists(),
        "same-host unknown scope remains unclaimed"
    );
    drop(window);
    assert!(fixture.access.shutdown().await.is_empty());
    assert!(other.shutdown().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_cannot_miss_cleanup_accepted_before_the_job_registration_boundary() {
    let fixture = Fixture::new().await;
    let (generation, window) = fixture.access.current(&fixture.project, false).unwrap();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    *fixture.access.0.registration_pause.lock().unwrap() = Some((entered, blocked));
    let dropping = tokio::task::spawn_blocking(move || drop(window));
    ready.await.unwrap();
    let access = fixture.access.clone();
    let shutdown = tokio::spawn(async move { access.shutdown_all().await });
    tokio::task::yield_now().await;
    assert!(!shutdown.is_finished());
    release.send(()).unwrap();
    dropping.await.unwrap();
    assert!(shutdown.await.unwrap().is_empty());
    assert!(!generation.unwrap().root.exists());
    assert!(fixture.access.0.jobs.lock().unwrap().is_empty());
    assert!(fixture.access.0.state.lock().unwrap().cleaning.is_empty());
}

#[tokio::test]
async fn pending_publication_or_missing_current_artifact_never_authorizes_old_generation_cleanup() {
    let fixture = Fixture::new().await;
    let old = fixture.publish("old");
    let (_, window) = fixture.access.current(&fixture.project, false).unwrap();
    let current = fixture.publish("current");
    let journal = fixture
        .paths
        .workspace_root()
        .join(".platform/project-state/builds")
        .join(format!("{}.publication.json", fixture.project.manifest.id));
    fs::write(&journal, "pending publication").unwrap();
    assert!(
        fixture
            .access
            .cleanup(&fixture.project, &[])
            .cleanup_pending
    );
    assert!(fixture.generation(&old).exists());
    fs::remove_file(journal).unwrap();
    fs::rename(
        fixture.generation(&current),
        fixture.project.root.join("disposable-moved-current"),
    )
    .unwrap();
    assert!(
        fixture
            .access
            .cleanup(&fixture.project, &[])
            .cleanup_pending
    );
    assert!(fixture.generation(&old).exists());
    drop(window);
    assert!(fixture.access.shutdown().await.is_empty());
}

#[tokio::test]
async fn late_snapshot_release_neither_needs_the_retired_source_nor_touches_a_replacement_owner() {
    let fixture = Fixture::new().await;
    let (generation, window) = fixture.access.current(&fixture.project, false).unwrap();
    let generation = generation.unwrap();
    let owner = generation
        .root
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let preserved = fixture.paths.data_root().join("disposable-preserved-owner");
    fs::rename(owner, &preserved).unwrap();
    fs::create_dir(owner).unwrap();
    fs::write(owner.join("replacement.txt"), "must survive").unwrap();
    fixture.access.seal_access();
    drop(window);
    assert!(fixture.access.shutdown().await.is_empty());
    assert_eq!(
        fs::read_to_string(owner.join("replacement.txt")).unwrap(),
        "must survive"
    );
    assert!(preserved
        .join("artifact-views")
        .join(&generation.build_id)
        .exists());
}

#[tokio::test]
async fn restart_recovers_the_publication_before_retiring_old_generations_and_intermediate_material(
) {
    let fixture = Fixture::new().await;
    let old = fixture.publish("old");
    let previous = builds::tree_digest(&fixture.project.root.join("dist"), false).unwrap();
    let (id, staging, provenance) = fixture.prepare("recovered new");
    let generation = fixture.generation(&id).join("dist");
    fs::create_dir_all(generation.parent().unwrap()).unwrap();
    fs::rename(staging, &generation).unwrap();
    let task_root = fixture.project.root.join(".platform/builds").join(&id);
    builds::copy_tree(
        &fixture.project.root.join("dist"),
        &task_root.join("publication"),
    )
    .unwrap();
    fs::remove_dir_all(fixture.project.root.join("dist")).unwrap();
    builds::copy_tree(&generation, &fixture.project.root.join("dist")).unwrap();
    let state = fixture
        .paths
        .workspace_root()
        .join(".platform/project-state/builds");
    builds::atomic_json(&state.join(format!("{}.publication.json", fixture.project.manifest.id)),
        &json!({"projectId":fixture.project.manifest.id,"provenance":provenance,"previousDigest":previous})).unwrap();
    let task = crate::TaskRecord {
        task_id: id.clone(),
        project_id: Some(fixture.project.manifest.id.clone()),
        task_type: "build".into(),
        status: crate::TaskStatus::Running,
        phase: "publish".into(),
        result: None,
        error: None,
    };
    let task_dir = fixture
        .paths
        .workspace_root()
        .join(".platform/tasks")
        .join(&id);
    fs::create_dir_all(&task_dir).unwrap();
    builds::atomic_json(&task_dir.join("task.json"), &task).unwrap();
    let manager = crate::builds::BuildManager::new(
        fixture.paths.clone(),
        fixture.store.clone(),
        "http://127.0.0.1:9".into(),
    )
    .unwrap();
    assert_eq!(
        builds::artifact_generation(&fixture.project)
            .unwrap()
            .unwrap()
            .build_id,
        id
    );
    assert!(!fixture.generation(&old).exists());
    assert!(generation.join("late.js").is_file());
    assert!(!task_root.exists());
    assert!(!state
        .join(format!("{}.publication.json", fixture.project.manifest.id))
        .exists());
    assert_eq!(
        manager.task(&id).unwrap().result.unwrap()["operationOutcome"],
        "committed"
    );
    assert!(manager.shutdown().await.is_empty());
    assert!(fixture.access.shutdown().await.is_empty());
}

#[tokio::test]
async fn retired_scope_snapshot_release_does_not_need_or_recreate_its_old_project_path() {
    let fixture = Fixture::new().await;
    let (generation, window) = fixture.access.current(&fixture.project, false).unwrap();
    let generation = generation.unwrap();
    let moved = fixture
        .project
        .root
        .with_file_name("disposable-preserved-old-project");
    fs::rename(&fixture.project.root, &moved).unwrap();
    fs::create_dir(&fixture.project.root).unwrap();
    fs::write(fixture.project.root.join("replacement.txt"), "must survive").unwrap();
    fixture.access.seal_access();
    drop(window);
    fixture
        .wait_removed(generation.root.parent().unwrap())
        .await;
    assert!(fixture.access.shutdown().await.is_empty());
    assert_eq!(
        fs::read_to_string(fixture.project.root.join("replacement.txt")).unwrap(),
        "must survive"
    );
    assert!(moved.join("project.json").is_file());
}

#[tokio::test]
async fn deletion_seals_captured_instances_without_sealing_a_new_epoch_with_the_same_project_id() {
    let fixture = Fixture::new().await;
    let (old, window) = fixture.access.current(&fixture.project, false).unwrap();
    fixture
        .access
        .prepare_project_deletion(&fixture.project.manifest.id)
        .await
        .unwrap();
    fixture
        .store
        .rebind(&fixture.project.manifest.id, true)
        .unwrap();
    let replacement = fixture.store.project(&fixture.project.manifest.id).unwrap();
    assert_ne!(replacement.document_epoch, fixture.project.document_epoch);
    let (new, new_window) = fixture.access.current(&replacement, false).unwrap();
    drop(window);
    assert!(!fixture.access.cleanup(&replacement, &[]).cleanup_pending);
    assert!(!old.unwrap().root.exists());
    assert!(new.as_ref().unwrap().root.exists());
    drop(new_window);
    assert!(fixture.access.shutdown().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deletion_seals_the_last_pin_retirement_gap_before_a_late_control_cleanup_can_start() {
    let fixture = Fixture::new().await;
    let (generation, window) = fixture.access.current(&fixture.project, false).unwrap();
    let generation = generation.unwrap();
    let owner = generation
        .root
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    *fixture.access.0.retirement_pause.lock().unwrap() = Some((entered, blocked));
    let dropping = tokio::task::spawn_blocking(move || drop(window));
    ready.await.unwrap();
    assert!(fixture.access.0.state.lock().unwrap().pins.is_empty());
    fixture
        .access
        .prepare_project_deletion(&fixture.project.manifest.id)
        .await
        .unwrap();
    let expected = PersistedIdentity::capture(&owner).unwrap();
    crate::history_cleanup::remove_owned_directory(&owner, &expected).unwrap();
    fs::create_dir(&owner).unwrap();
    fs::write(owner.join("replacement.txt"), "new owner must survive").unwrap();
    release.send(()).unwrap();
    dropping.await.unwrap();
    assert!(fixture.access.0.jobs.lock().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(owner.join("replacement.txt")).unwrap(),
        "new owner must survive"
    );
    assert_eq!(fs::read_dir(&owner).unwrap().count(), 1);
    assert!(fixture.access.shutdown().await.is_empty());
}
