//! Startup recovery uses real disposable records and publication trees. No
//! repository scripts, development data, or external tools are executed.
use super::*;
use crate::{
    builds::{self, BuildManager, BuildProvenance, Journal},
    tasks::TaskStatus,
    CreateProjectInput, ProjectHandle, ProjectStore, RuntimePaths,
};
use std::sync::Arc;
use uuid::Uuid;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-build-startup-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    (temporary, root)
}

fn task(status: TaskStatus) -> TaskRecord {
    TaskRecord {
        task_id: format!("task_{}", Uuid::new_v4().simple()),
        project_id: Some("prj_disposable".into()),
        task_type: "build".into(),
        status,
        phase: "dependencies".into(),
        result: None,
        error: None,
    }
}

fn save_task(root: &Path, record: &TaskRecord) -> PathBuf {
    let path = root
        .join(".platform/tasks")
        .join(&record.task_id)
        .join("task.json");
    // Current platform records publish both their task directory and file with
    // the real private-control writer. Ordinary inherited fixture descendants
    // can lose inherited access when a parent is admitted as a private leaf.
    builds::atomic_json(&path, record).unwrap();
    path
}

fn provenance(id: &str, digest: String) -> BuildProvenance {
    let tool = crate::environment::ExecutableInfo {
        path: "disposable-not-executed.exe".into(),
        version: "fixture".into(),
    };
    BuildProvenance {
        schema_version: 1,
        build_id: id.into(),
        task_id: id.into(),
        completed_at: 123,
        digest_rule_version: 1,
        input_digest: "a".repeat(64),
        artifact_digest: digest,
        node: tool.clone(),
        npm: tool,
        sdk_version: None,
        entry_relative: "index.html".into(),
    }
}

#[test]
fn missing_empty_and_current_records_are_admitted_without_changing_siblings() {
    let (_temporary, root) = fixture();
    fs::write(root.join("retained"), b"disposable unrelated bytes").unwrap();
    require_supported_inventory(&root).unwrap();
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    fs::create_dir_all(root.join(".platform/tasks")).unwrap();
    fs::create_dir_all(root.join(".platform/project-state/builds")).unwrap();
    require_supported_inventory(&root).unwrap();
    let record = task(TaskStatus::Succeeded);
    let task_path = save_task(&root, &record);
    let before = fs::read(&task_path).unwrap();
    let build = provenance(&record.task_id, "b".repeat(64));
    let path = builds::provenance_path(&root, "prj_disposable");
    fs::write(&path, serde_json::to_vec(&build).unwrap()).unwrap();
    require_supported_inventory(&root).unwrap();
    assert_eq!(fs::read(task_path).unwrap(), before);
    assert_eq!(
        fs::read(root.join("retained")).unwrap(),
        b"disposable unrelated bytes"
    );
}

#[test]
fn a_real_parent_junction_is_rejected_without_reading_or_changing_its_target() {
    let (_temporary, root) = fixture();
    let target = root.join("disposable-foreign-platform");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("retained"), b"disposable junction bytes").unwrap();
    let link = root.join(".platform");
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&target)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(
        require_supported_inventory(&root).unwrap_err().code,
        "BUILD_STARTUP_INVENTORY_UNCONFIRMED"
    );
    assert_eq!(
        fs::read(target.join("retained")).unwrap(),
        b"disposable junction bytes"
    );
    assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
    fs::remove_dir(link).unwrap();
}

#[tokio::test]
async fn final_hardlinked_task_keeps_the_original_read_and_timestamp_label_policy() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = ProjectStore::new(paths.workspace_root().into()).unwrap();
    fs::create_dir_all(
        paths
            .workspace_root()
            .join(".platform/project-state/builds"),
    )
    .unwrap();
    let record = task(TaskStatus::Succeeded);
    // Deliberately keep this read-only record ordinary: recovery must not add
    // an ACL admission or reject the hard link, even though real task writes
    // use the private-control publisher in save_task above.
    let path = paths
        .workspace_root()
        .join(".platform/tasks")
        .join(&record.task_id)
        .join("task.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let ordinary = native::open_no_reparse(&path).unwrap();
    assert!(native::verify_private(&ordinary.file).is_err());
    drop(ordinary);
    let other = paths.workspace_root().join("unrelated-task-alias.json");
    fs::hard_link(&path, &other).unwrap();
    let before = fs::read(&other).unwrap();
    let (records, times) = recover_startup(&paths, &store, "http://127.0.0.1:1234").unwrap();
    assert_eq!(records[&record.task_id].status, TaskStatus::Succeeded);
    assert_eq!(times[&record.task_id], None);
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(fs::read(other).unwrap(), before);
    store.shutdown().await;
}

#[tokio::test]
async fn malformed_task_or_publication_material_is_preserved_and_reports_failure() {
    for publication in [false, true] {
        let (_temporary, paths) = RuntimePaths::for_test();
        let store = ProjectStore::new(paths.workspace_root().into()).unwrap();
        fs::create_dir_all(paths.workspace_root().join(".platform/tasks")).unwrap();
        fs::create_dir_all(
            paths
                .workspace_root()
                .join(".platform/project-state/builds"),
        )
        .unwrap();
        let damaged = if publication {
            builds::journal_path(paths.workspace_root(), "prj_disposable")
        } else {
            save_task(paths.workspace_root(), &task(TaskStatus::Queued))
        };
        fs::write(&damaged, b"{disposable damaged JSON").unwrap();
        assert!(recover_startup(&paths, &store, "http://127.0.0.1:1234").is_err());
        assert_eq!(fs::read(damaged).unwrap(), b"{disposable damaged JSON");
        store.shutdown().await;
    }
}

#[tokio::test]
async fn mismatched_task_identity_is_preserved_and_rejected() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = ProjectStore::new(paths.workspace_root().into()).unwrap();
    fs::create_dir_all(
        paths
            .workspace_root()
            .join(".platform/project-state/builds"),
    )
    .unwrap();
    let record = task(TaskStatus::Running);
    let path = paths
        .workspace_root()
        .join(".platform/tasks")
        .join(format!("task_{}", Uuid::new_v4().simple()))
        .join("task.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let before = serde_json::to_vec(&record).unwrap();
    fs::write(&path, &before).unwrap();
    let error = recover_startup(&paths, &store, "http://127.0.0.1:1234").unwrap_err();
    assert_eq!(error.code, "TASK_RECORD_INVALID");
    assert_eq!(fs::read(path).unwrap(), before);
    store.shutdown().await;
}

#[tokio::test]
async fn non_task_and_non_publication_material_retains_the_mac_skip_policy() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = ProjectStore::new(paths.workspace_root().into()).unwrap();
    let tasks = paths.workspace_root().join(".platform/tasks");
    let state = paths
        .workspace_root()
        .join(".platform/project-state/builds");
    fs::create_dir_all(tasks.join("incomplete-directory")).unwrap();
    fs::create_dir_all(&state).unwrap();
    fs::write(tasks.join("unrecognized-file"), b"retained task bytes").unwrap();
    fs::write(state.join("unrecognized.record"), b"retained state bytes").unwrap();
    fs::write(
        state.join("prj_unselected.json"),
        b"retained unselected provenance",
    )
    .unwrap();
    let (records, times) = recover_startup(&paths, &store, "http://127.0.0.1:1234").unwrap();
    assert!(records.is_empty());
    assert!(times.is_empty());
    assert!(tasks.join("incomplete-directory").is_dir());
    assert_eq!(
        fs::read(tasks.join("unrecognized-file")).unwrap(),
        b"retained task bytes"
    );
    assert_eq!(
        fs::read(state.join("unrecognized.record")).unwrap(),
        b"retained state bytes"
    );
    assert_eq!(
        fs::read(state.join("prj_unselected.json")).unwrap(),
        b"retained unselected provenance"
    );
    store.shutdown().await;
}

#[tokio::test]
async fn late_current_records_are_read_and_interrupted_without_running_commands() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = ProjectStore::new(paths.workspace_root().into()).unwrap();
    require_supported_inventory(paths.workspace_root()).unwrap();
    let mut record = task(TaskStatus::Running);
    record.phase = "dependencies".into();
    let path = save_task(paths.workspace_root(), &record);
    let before = fs::metadata(&path).unwrap().modified().unwrap();
    fs::create_dir_all(
        paths
            .workspace_root()
            .join(".platform/project-state/builds"),
    )
    .unwrap();
    // This invalid selection would fail if startup attempted tool discovery.
    fs::write(paths.data_root().join("toolchain.json"), b"not tool JSON").unwrap();
    let (records, times) = recover_startup(&paths, &store, "http://127.0.0.1:1234").unwrap();
    assert_eq!(times[&record.task_id], Some(before));
    let recovered = &records[&record.task_id];
    assert_eq!(recovered.status, TaskStatus::Interrupted);
    assert_eq!(recovered.phase, record.phase);
    assert_eq!(recovered.error.as_ref().unwrap().code, "TASK_INTERRUPTED");
    let persisted: TaskRecord = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(persisted.status, TaskStatus::Interrupted);
    assert_eq!(
        fs::read(paths.data_root().join("toolchain.json")).unwrap(),
        b"not tool JSON"
    );
    store.shutdown().await;
}

#[tokio::test]
async fn final_records_keep_their_exact_bytes_and_original_modification_time() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = ProjectStore::new(paths.workspace_root().into()).unwrap();
    fs::create_dir_all(
        paths
            .workspace_root()
            .join(".platform/project-state/builds"),
    )
    .unwrap();
    for status in [
        TaskStatus::Succeeded,
        TaskStatus::Failed,
        TaskStatus::Interrupted,
    ] {
        let record = task(status);
        let path = save_task(paths.workspace_root(), &record);
        let bytes = fs::read(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let (records, times) = recover_startup(&paths, &store, "http://127.0.0.1:1234").unwrap();
        assert_eq!(records[&record.task_id].status, status);
        assert_eq!(times[&record.task_id], Some(modified));
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
    }
    store.shutdown().await;
}

async fn project(store: &ProjectStore) -> ProjectHandle {
    let info = store
        .create(CreateProjectInput {
            name: "Disposable startup publications".into(),
            description: None,
        })
        .await
        .unwrap();
    store.project(&info.id).unwrap()
}

fn generation(project: &ProjectHandle, id: &str, content: &str) -> PathBuf {
    let root = project.root.join(format!(".platform/artifacts/{id}/dist"));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("index.html"), content).unwrap();
    root
}

#[tokio::test]
async fn current_publication_is_recovered_and_task_commit_is_reconciled() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = ProjectStore::new(paths.workspace_root().into()).unwrap();
    let project = project(&store).await;
    let mut record = task(TaskStatus::Running);
    record.project_id = Some(project.manifest.id.clone());
    record.phase = "publish".into();
    save_task(paths.workspace_root(), &record);
    let root = generation(&project, &record.task_id, "<html>new generation</html>");
    let next = provenance(&record.task_id, builds::tree_digest(&root, false).unwrap());
    fs::create_dir_all(project.root.join("dist")).unwrap();
    fs::write(
        project.root.join("dist/index.html"),
        "<html>new generation</html>",
    )
    .unwrap();
    let journal = Journal {
        project_id: project.manifest.id.clone(),
        provenance: next.clone(),
        previous_digest: Some("b".repeat(64)),
    };
    let journal_path = builds::journal_path(paths.workspace_root(), &project.manifest.id);
    builds::atomic_json(&journal_path, &journal).unwrap();
    let (records, _) = recover_startup(&paths, &store, "http://127.0.0.1:1234").unwrap();
    let recovered = &records[&record.task_id];
    assert_eq!(recovered.status, TaskStatus::Interrupted);
    assert_eq!(
        recovered.error.as_ref().unwrap().details["operationOutcome"],
        "committed"
    );
    assert_eq!(recovered.result.as_ref().unwrap()["recovered"], true);
    assert!(!journal_path.exists());
    let actual: BuildProvenance = builds::read_optional(&builds::provenance_path(
        paths.workspace_root(),
        &project.manifest.id,
    ))
    .unwrap()
    .unwrap();
    assert_eq!(actual.build_id, next.build_id);
    store.shutdown().await;
}

#[tokio::test]
async fn publication_with_an_unmatched_output_keeps_its_journal_and_artifacts() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = ProjectStore::new(paths.workspace_root().into()).unwrap();
    let project = project(&store).await;
    fs::create_dir_all(paths.workspace_root().join(".platform/tasks")).unwrap();
    let id = format!("task_{}", Uuid::new_v4().simple());
    let root = generation(&project, &id, "<html>new generation</html>");
    let next = provenance(&id, builds::tree_digest(&root, false).unwrap());
    fs::create_dir_all(project.root.join("dist")).unwrap();
    fs::write(
        project.root.join("dist/index.html"),
        "<html>foreign output</html>",
    )
    .unwrap();
    let journal = Journal {
        project_id: project.manifest.id.clone(),
        provenance: next,
        previous_digest: Some("b".repeat(64)),
    };
    let journal_path = builds::journal_path(paths.workspace_root(), &project.manifest.id);
    builds::atomic_json(&journal_path, &journal).unwrap();
    let before = fs::read(&journal_path).unwrap();
    let error = recover_startup(&paths, &store, "http://127.0.0.1:1234").unwrap_err();
    assert_eq!(error.code, "BUILD_RECOVERY_REQUIRED");
    assert_eq!(fs::read(journal_path).unwrap(), before);
    assert_eq!(
        fs::read(root.join("index.html")).unwrap(),
        b"<html>new generation</html>"
    );
    assert_eq!(
        fs::read(project.root.join("dist/index.html")).unwrap(),
        b"<html>foreign output</html>"
    );
    assert!(!builds::provenance_path(paths.workspace_root(), &project.manifest.id).exists());
    store.shutdown().await;
}

#[tokio::test]
async fn manager_startup_retires_old_generations_and_completed_staging() {
    let (_temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = project(&store).await;
    let mut ids = Vec::new();
    let mut latest = None;
    for text in ["<html>one</html>", "<html>two</html>", "<html>three</html>"] {
        let mut record = task(TaskStatus::Succeeded);
        record.project_id = Some(project.manifest.id.clone());
        save_task(paths.workspace_root(), &record);
        let root = generation(&project, &record.task_id, text);
        latest = Some(provenance(
            &record.task_id,
            builds::tree_digest(&root, false).unwrap(),
        ));
        fs::create_dir_all(project.root.join(".platform/builds").join(&record.task_id)).unwrap();
        fs::write(
            project
                .root
                .join(".platform/builds")
                .join(&record.task_id)
                .join("previous"),
            b"old staging",
        )
        .unwrap();
        ids.push(record.task_id);
    }
    fs::create_dir_all(project.root.join("dist")).unwrap();
    fs::write(project.root.join("dist/index.html"), "<html>three</html>").unwrap();
    builds::atomic_json(
        &builds::provenance_path(paths.workspace_root(), &project.manifest.id),
        &latest.unwrap(),
    )
    .unwrap();
    let manager = BuildManager::new(paths, store.clone(), "http://127.0.0.1:1234".into()).unwrap();
    for (index, id) in ids.iter().enumerate() {
        assert_eq!(
            project.root.join(".platform/artifacts").join(id).exists(),
            index == 2
        );
        assert!(!project.root.join(".platform/builds").join(id).exists());
    }
    assert_eq!(
        fs::read(project.root.join("dist/index.html")).unwrap(),
        b"<html>three</html>"
    );
    manager.shutdown().await;
    store.shutdown().await;
}
