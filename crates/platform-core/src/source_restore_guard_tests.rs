//! Every journal and project in these checks belongs to an isolated TempDir.
use super::*;
use crate::{builds::BuildManager, CreateProjectInput, ProjectStore, RuntimePaths, RuntimeService};
use serde_json::{json, Value};
use std::{os::unix::fs::symlink, sync::Arc, time::Duration};

fn journal(workspace: &Path, project: &str, task: &str, phase: &str) -> PathBuf {
    let path = workspace
        .join(ROOT)
        .join(project)
        .join(task)
        .join("journal.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "schemaVersion":1,"operation":"sourceRestore","projectId":project,"taskId":task,
            "phase":phase,"planDigest":"a".repeat(64),"transactionPayload":{"kept":true}
        }))
        .unwrap(),
    )
    .unwrap();
    path
}

#[test]
fn source_restore_guard_only_exact_terminal_phases_release_development() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    assert!(inspect(&root, "prj_fixture").check().is_ok());
    assert!(
        !root.join(ROOT).exists(),
        "read must not create a control directory"
    );
    for phase in ["prepared", "applying", "applied", "undoing"] {
        journal(&root, "prj_fixture", "task_restore", phase);
        let state = inspect(&root, "prj_fixture");
        assert_eq!(
            state.check().unwrap_err().code,
            "PROJECT_SOURCE_RESTORE_PENDING"
        );
        assert_eq!(state.pending[0].phase, phase);
    }
    for phase in ["completed", "undone", "not-applied"] {
        journal(&root, "prj_fixture", "task_restore", phase);
        assert!(inspect(&root, "prj_fixture").check().is_ok());
        assert!(check_workspace_available(&root).is_ok());
    }
    let path = journal(&root, "prj_fixture", "task_restore", "unknown");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["resolved"] = json!(true);
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert_eq!(
        inspect(&root, "prj_fixture").check().unwrap_err().code,
        "PROJECT_SOURCE_RESTORE_INVALID"
    );
}

#[test]
fn source_restore_guard_recovery_permit_binds_task_project_plan_and_exact_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = journal(&root, "prj_fixture", "task_restore", "applying");
    let state = inspect(&root, "prj_fixture");
    let pending = &state.pending[0];
    let permit = validate_recovery(
        &root,
        "prj_fixture",
        "task_restore",
        &pending.plan_digest,
        &pending.journal_digest,
    )
    .unwrap();
    assert!(permit.revalidate(&root, "prj_fixture").is_ok());
    assert!(permit.revalidate(&root, "prj_other").is_err());
    assert!(permit
        .revalidate(&root.join("other-workspace"), "prj_fixture")
        .is_err());
    for (task, plan, digest) in [
        (
            "task_other",
            pending.plan_digest.clone(),
            pending.journal_digest.clone(),
        ),
        (
            "task_restore",
            "b".repeat(64),
            pending.journal_digest.clone(),
        ),
        ("task_restore", pending.plan_digest.clone(), "b".repeat(64)),
    ] {
        assert!(validate_recovery(&root, "prj_fixture", task, &plan, &digest).is_err());
    }
    let original = fs::read(&path).unwrap();
    let mut changed_bytes = original.clone();
    changed_bytes.push(b' ');
    fs::write(&path, changed_bytes).unwrap();
    assert!(
        permit.revalidate(&root, "prj_fixture").is_err(),
        "equivalent JSON is still a changed journal"
    );
    fs::write(&path, original).unwrap();
    journal(&root, "prj_fixture", "task_second", "prepared");
    assert!(
        permit.revalidate(&root, "prj_fixture").is_err(),
        "one permit never exempts another pending task"
    );
}

#[test]
fn source_restore_guard_corruption_is_local_and_unsafe_files_are_not_followed() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = journal(&root, "prj_bad", "task_restore", "applying");
    fs::write(&path, b"not-json").unwrap();
    assert_eq!(inspect(&root, "prj_bad").issues.len(), 1);
    assert!(inspect(&root, "prj_good").check().is_ok());
    assert!(check_workspace_available(&root).is_err());
    let outside = root.join("outside.json");
    fs::write(&outside, b"do not read or modify this").unwrap();
    fs::remove_file(&path).unwrap();
    symlink(&outside, &path).unwrap();
    assert_eq!(
        inspect(&root, "prj_bad").check().unwrap_err().code,
        "PROJECT_SOURCE_RESTORE_INVALID"
    );
    fs::remove_file(&path).unwrap();
    fs::hard_link(&outside, &path).unwrap();
    assert!(inspect(&root, "prj_bad").check().is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"do not read or modify this");
    fs::remove_file(&path).unwrap();
    assert!(
        inspect(&root, "prj_bad").check().is_err(),
        "a task directory without a journal is evidence"
    );
}

#[test]
fn source_restore_guard_header_identity_and_journal_budget_are_strict() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for (field, replacement) in [
        ("schemaVersion", json!(2)),
        ("operation", json!("other")),
        ("projectId", json!("prj_other")),
        ("taskId", json!("task_other")),
        ("planDigest", json!("A".repeat(64))),
        ("phase", Value::Null),
    ] {
        let path = journal(&root, "prj_fixture", "task_restore", "completed");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value[field] = replacement;
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(inspect(&root, "prj_fixture").check().is_err(), "{field}");
    }
    let path = journal(&root, "prj_fixture", "task_restore", "completed");
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(MAX_JOURNAL + 1)
        .unwrap();
    assert!(inspect(&root, "prj_fixture").check().is_err());
}

#[test]
fn source_restore_guard_aggregate_budget_never_treats_unread_journals_as_clear() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    // Each file is individually valid and within the per-file limit, but their
    // total exceeds the real per-project read budget.
    for index in 0..=MAX_PROJECT_BYTES / MAX_JOURNAL {
        let path = journal(&root, "prj_large", &format!("task_{index}"), "completed");
        let mut bytes = fs::read(&path).unwrap();
        bytes.resize(MAX_JOURNAL as usize, b' ');
        fs::write(path, bytes).unwrap();
    }
    assert_eq!(
        inspect(&root, "prj_large").check().unwrap_err().code,
        "PROJECT_SOURCE_RESTORE_LIMIT"
    );
    assert_eq!(
        fs::read_dir(root.join(ROOT).join("prj_large"))
            .unwrap()
            .count(),
        5,
        "budget exhaustion never cleans evidence"
    );
    let global = tempfile::tempdir().unwrap();
    let root = global.path().canonicalize().unwrap();
    let first = journal(&root, "prj_one", "task_first", "completed");
    let second = journal(&root, "prj_two", "task_second", "completed");
    let total = fs::metadata(first).unwrap().len() + fs::metadata(second).unwrap().len();
    assert!(inspect(&root, "prj_one").check().is_ok());
    assert!(inspect(&root, "prj_two").check().is_ok());
    let error = check_workspace_with_budget(
        &root,
        &mut ReadBudget {
            remaining_bytes: total - 1,
            remaining_entries: MAX_WORKSPACE_ENTRIES,
        },
    )
    .unwrap_err();
    assert_eq!(
        error.code, "PROJECT_SOURCE_RESTORE_LIMIT",
        "workspace bytes must be shared across projects"
    );
    let error = check_workspace_with_budget(
        &root,
        &mut ReadBudget {
            remaining_bytes: MAX_WORKSPACE_BYTES,
            remaining_entries: 3,
        },
    )
    .unwrap_err();
    assert_eq!(
        error.code, "PROJECT_SOURCE_RESTORE_LIMIT",
        "project directories and task entries share one scan budget"
    );
}

fn publication_limits() -> PublicationLimits {
    PublicationLimits {
        project_bytes: MAX_PROJECT_BYTES,
        project_tasks: MAX_TASKS,
        workspace_bytes: MAX_WORKSPACE_BYTES,
        workspace_entries: MAX_WORKSPACE_ENTRIES,
    }
}

#[test]
fn source_restore_guard_publication_capacity_keeps_completed_evidence_development_usable() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for index in 0..MAX_PROJECT_BYTES / MAX_JOURNAL {
        let path = journal(&root, "prj_full", &format!("task_{index}"), "completed");
        let mut bytes = fs::read(&path).unwrap();
        bytes.resize(MAX_JOURNAL as usize, b' ');
        fs::write(path, bytes).unwrap();
    }
    assert!(check_development_available(&root, "prj_full").is_ok());
    assert_eq!(
        reserve_publication(&root, "prj_full", "task_next", 1024)
            .err()
            .unwrap()
            .code,
        "PROJECT_SOURCE_RESTORE_CAPACITY"
    );
    assert!(check_development_available(&root, "prj_full").is_ok());
    assert!(check_workspace_available(&root).is_ok());
    assert_eq!(
        fs::read_dir(root.join(ROOT).join("prj_full"))
            .unwrap()
            .count(),
        4
    );
    assert!(!root.join(ROOT).join("prj_full/task_next").exists());
    // A refused admission creates neither a reservation nor a permanent lock.
    drop(reserve_publication(&root, "prj_other", "task_next", 1024).unwrap());
    drop(reserve_publication(&root, "prj_other", "task_next", 1024).unwrap());
    assert!(!root.join(ROOT).join("prj_other").exists());
}

#[test]
fn source_restore_guard_publication_reserves_pending_terminal_growth() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = journal(&root, "prj_old", "task_old", "prepared");
    let before = fs::read(&path).unwrap();
    let growth = (LONGEST_PHASE - "prepared".len()) as u64;
    let total = before.len() as u64 + growth + 1024;
    let mut limits = publication_limits();
    limits.workspace_bytes = total - 1;
    assert_eq!(
        reserve_publication_with_limits(&root, "prj_new", "task_new", 1024, limits)
            .err()
            .unwrap()
            .code,
        "PROJECT_SOURCE_RESTORE_CAPACITY"
    );
    limits.workspace_bytes = total;
    drop(reserve_publication_with_limits(&root, "prj_new", "task_new", 1024, limits).unwrap());
    let mut terminal: Value = serde_json::from_slice(&before).unwrap();
    terminal["phase"] = json!("not-applied");
    let after = serde_json::to_vec(&terminal).unwrap();
    assert_eq!(after.len() as u64, before.len() as u64 + growth);
    fs::write(path, after).unwrap();
    drop(reserve_publication_with_limits(&root, "prj_new", "task_new", 1024, limits).unwrap());
    // The caller's maximal serialization must itself fit the per-file budget.
    assert_eq!(
        reserve_publication(&root, "prj_new", "task_new", MAX_JOURNAL + 1)
            .err()
            .unwrap()
            .code,
        "PROJECT_SOURCE_RESTORE_CAPACITY"
    );
    assert!(reserve_publication(&root, "prj_new", "task_new", 0).is_err());
}

#[test]
fn source_restore_guard_publication_counts_new_project_and_task() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    journal(&root, "prj_old", "task_old", "completed");
    let mut limits = publication_limits();
    limits.project_tasks = 1;
    assert_eq!(
        reserve_publication_with_limits(&root, "prj_old", "task_new", 1024, limits)
            .err()
            .unwrap()
            .code,
        "PROJECT_SOURCE_RESTORE_CAPACITY"
    );
    limits.project_tasks = 2;
    limits.workspace_entries = 3;
    drop(reserve_publication_with_limits(&root, "prj_old", "task_new", 1024, limits).unwrap());
    assert_eq!(
        reserve_publication_with_limits(&root, "prj_new", "task_new", 1024, limits)
            .err()
            .unwrap()
            .code,
        "PROJECT_SOURCE_RESTORE_CAPACITY",
        "a new project and its new task each consume an entry"
    );
    limits.workspace_entries = 4;
    drop(reserve_publication_with_limits(&root, "prj_new", "task_new", 1024, limits).unwrap());
}

#[test]
fn source_restore_guard_publication_serializes_cross_project_admission() {
    use std::sync::mpsc;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut limits = publication_limits();
    limits.workspace_entries = 2; // Exactly one project with one retained task.
    let first =
        reserve_publication_with_limits(&root, "prj_one", "task_one", 1024, limits).unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let second_root = root.clone();
    let thread = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let error =
            reserve_publication_with_limits(&second_root, "prj_two", "task_two", 1024, limits)
                .err()
                .map(|error| error.code);
        done_tx.send(error).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let waiting = done_rx.recv_timeout(Duration::from_millis(100));
    journal(&root, "prj_one", "task_one", "completed");
    drop(first);
    let error = done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    thread.join().unwrap();
    assert!(matches!(waiting, Err(mpsc::RecvTimeoutError::Timeout)));
    assert_eq!(error.as_deref(), Some("PROJECT_SOURCE_RESTORE_CAPACITY"));
    assert!(check_workspace_available(&root).is_ok());
    assert!(!root.join(ROOT).join("prj_two").exists());
}

#[test]
fn source_restore_guard_publication_preserves_existing_or_unsafe_records() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = journal(&root, "prj_old", "task_old", "completed");
    let original = fs::read(&path).unwrap();
    assert_eq!(
        reserve_publication(&root, "prj_old", "task_old", 1024)
            .err()
            .unwrap()
            .code,
        "PROJECT_SOURCE_RESTORE_CHANGED"
    );
    journal(&root, "prj_old", "task_old", "applying");
    assert_eq!(
        reserve_publication(&root, "prj_old", "task_new", 1024)
            .err()
            .unwrap()
            .code,
        "PROJECT_SOURCE_RESTORE_PENDING"
    );
    fs::write(&path, &original).unwrap();
    let outside = root.join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, root.join(ROOT).join("prj_link")).unwrap();
    assert_eq!(
        reserve_publication(&root, "prj_new", "task_new", 1024)
            .err()
            .unwrap()
            .code,
        "PROJECT_SOURCE_RESTORE_INVALID"
    );
    assert_eq!(fs::read(path).unwrap(), original);
    assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    assert!(check_development_available(&root, "prj_old").is_ok());
}

#[tokio::test]
async fn source_restore_guard_all_project_task_routes_close_but_read_leases_remain() {
    let (_temp, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let project = store
        .create(CreateProjectInput {
            name: "Disposable guard routes".into(),
            description: None,
        })
        .await
        .unwrap();
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
    journal(
        paths.workspace_root(),
        &project.id,
        "task_restore",
        "applying",
    );
    assert_eq!(
        tasks.submit(&project.id).await.unwrap_err().code,
        "PROJECT_SOURCE_RESTORE_PENDING"
    );
    for kind in [
        "gitSave",
        "gitBranchCreate",
        "gitSourceRestoreApply",
        "backup",
        "maintenanceBegin",
        "restore",
        "export",
        "copy",
        "delete",
        "trashRestore",
        "trashPurge",
        "storageCleanup",
    ] {
        let error = tasks.create_operation(&project.id, kind).err().unwrap();
        assert_eq!(error.code, "PROJECT_SOURCE_RESTORE_PENDING", "{kind}");
        assert!(
            !store.has_active_tasks(),
            "failed admission must release the lease"
        );
    }
    assert!(store.reserve_task(&project.id, "task_copy_target").is_err());
    for label in ["git-status", "git-versions"] {
        let lease = store
            .reserve_transient_operation(&project.id, label)
            .unwrap();
        drop(lease);
    }
    for key in ["source:fixture", "node-install:fixture"] {
        let lease = store.reserve_task(key, "task_nonproject").unwrap();
        drop(lease);
    }
    let pending = inspect(paths.workspace_root(), &project.id)
        .pending
        .remove(0);
    let permit = validate_recovery(
        paths.workspace_root(),
        &project.id,
        "task_restore",
        &pending.plan_digest,
        &pending.journal_digest,
    )
    .unwrap();
    assert!(tasks
        .create_source_restore_recovery_operation(&project.id, "build", &permit)
        .is_err());
    let (record, lease) = tasks
        .create_source_restore_recovery_operation(&project.id, "gitSourceRestoreUndo", &permit)
        .unwrap();
    assert_eq!(record.task_type, "gitSourceRestoreUndo");
    tasks
        .finish(&record.task_id, Ok(json!({"testOnly":true})))
        .unwrap();
    drop(lease);
    journal(
        paths.workspace_root(),
        &project.id,
        "task_restore",
        "undoing",
    );
    assert!(tasks
        .create_source_restore_recovery_operation(&project.id, "gitSourceRestoreContinue", &permit)
        .is_err());
    assert!(!store.has_active_tasks());
    tasks.shutdown().await;
    store.shutdown().await;
}

async fn browser_session(service: &RuntimeService, id: &str) -> (String, String) {
    let launch = service.handle().launch(id).await.unwrap();
    let response = reqwest::Client::new()
        .post(format!("{}/_platform/session", service.info().base_url))
        .header("origin", service.info().base_url)
        .json(&json!({"ticket":launch.launch_url.split("#ticket=").nth(1).unwrap()}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    (cookie, launch.document_epoch)
}

#[tokio::test]
async fn source_restore_guard_pending_keeps_webpage_sql_attachments_and_metadata_readable() {
    let (_temp, paths) = RuntimePaths::for_test();
    let service = RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
        .await
        .unwrap();
    let project = service
        .store()
        .create(CreateProjectInput {
            name: "Disposable live source guard".into(),
            description: None,
        })
        .await
        .unwrap();
    fs::create_dir_all(project.project_root.join("dist")).unwrap();
    fs::write(
        project.project_root.join("dist/index.html"),
        b"<!doctype html><h1>still running</h1>",
    )
    .unwrap();
    let (cookie, epoch) = browser_session(&service, &project.id).await;
    journal(
        paths.workspace_root(),
        &project.id,
        "task_restore",
        "applying",
    );
    let handle = service.handle();
    let info = handle.project_metadata(&project.id).await.unwrap();
    assert!(info["metadataRevision"].is_null());
    assert_eq!(
        info["sourceRestore"]["pending"][0]["taskId"],
        "task_restore"
    );
    assert_eq!(
        handle.handoff(&project.id).await.unwrap_err().code,
        "PROJECT_SOURCE_RESTORE_PENDING"
    );
    assert_eq!(
        handle
            .editor_handoff_lease(
                &project.id,
                &project.project_root,
                &handle.info().instance_id
            )
            .err()
            .unwrap()
            .code,
        "PROJECT_SOURCE_RESTORE_PENDING"
    );
    for error in [
        handle.build_project(&project.id).await.unwrap_err(),
        handle
            .project_copy(&project.id, json!({"name":"blocked copy"}))
            .await
            .unwrap_err(),
    ] {
        assert_eq!(error.code, "PROJECT_SOURCE_RESTORE_PENDING");
    }
    assert_eq!(
        handle
            .project_delete(&project.id, json!({}))
            .await
            .unwrap_err()
            .code,
        "PROJECT_DELETE_CONFIRMATION_REQUIRED"
    );
    let store = service.store();
    let pending = inspect(paths.workspace_root(), &project.id)
        .pending
        .remove(0);
    let permit = validate_recovery(
        paths.workspace_root(),
        &project.id,
        "task_restore",
        &pending.plan_digest,
        &pending.journal_digest,
    )
    .unwrap();
    let restore_lease = store
        .reserve_source_restore_recovery_task(&project.id, "task_disposable_recovery", &permit)
        .unwrap();
    let read_gate = store.gate(&project.id).read_owned().await;
    let client = reqwest::Client::new();
    let base = service.info().base_url;
    let response = client
        .get(format!("{base}/p/{}/", project.id))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(response.text().await.unwrap().contains("still running"));
    let response: Value = client
        .post(format!("{base}/p/{}/api/v1/db/execute", project.id))
        .header("cookie", &cookie)
        .header("origin", &base)
        .header("x-platform-document-epoch", &epoch)
        .json(&json!({"sql":"CREATE TABLE still_writable (value TEXT)"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(response["ok"], true, "{response}");
    let url = format!(
        "{base}/p/{}/api/v1/file?path=still-writable.txt",
        project.id
    );
    let response = client
        .put(&url)
        .header("cookie", &cookie)
        .header("origin", &base)
        .header("x-platform-document-epoch", &epoch)
        .body("persistent attachment")
        .send()
        .await
        .unwrap();
    assert!(
        response.status().is_success(),
        "{}",
        response.text().await.unwrap()
    );
    let bytes = client
        .get(url)
        .header("cookie", &cookie)
        .header("x-platform-document-epoch", &epoch)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(bytes, "persistent attachment");
    assert_eq!(
        service.store().project(&project.id).unwrap().document_epoch,
        epoch
    );
    drop(read_gate);
    drop(restore_lease);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn source_restore_guard_bad_journal_does_not_prevent_service_start_or_other_projects() {
    let (_temp, paths) = RuntimePaths::for_test();
    let path = journal(
        paths.workspace_root(),
        "prj_bad",
        "task_restore",
        "applying",
    );
    fs::write(path, b"broken").unwrap();
    let service = RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
        .await
        .unwrap();
    let project = service
        .store()
        .create(CreateProjectInput {
            name: "Unaffected disposable project".into(),
            description: None,
        })
        .await
        .unwrap();
    assert!(service
        .store()
        .reserve_task(&project.id, "task_unaffected")
        .is_ok());
    assert!(inspect(paths.workspace_root(), "prj_bad").check().is_err());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn source_restore_guard_migration_rejects_before_pausing_live_project() {
    let (temp, paths) = RuntimePaths::for_test();
    let service = RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
        .await
        .unwrap();
    let project = service
        .store()
        .create(CreateProjectInput {
            name: "Disposable migration guard".into(),
            description: None,
        })
        .await
        .unwrap();
    let store = service.store();
    let live = store.project(&project.id).unwrap();
    journal(
        paths.workspace_root(),
        &project.id,
        "task_restore",
        "applying",
    );
    let destination = temp.path().canonicalize().unwrap().join("destination");
    fs::create_dir(&destination).unwrap();
    let handle = service.handle();
    let task = handle
        .workspace_migrate(json!({"destinationPath":destination}))
        .await
        .unwrap();
    let record = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            assert!(
                !live.is_workspace_paused(),
                "pending source restore must be rejected before runtime pause"
            );
            let record = handle.task(task["taskId"].as_str().unwrap()).await.unwrap();
            if record["final"] == true {
                break record;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(record["status"], "failed", "{record}");
    assert_eq!(record["error"]["code"], "PROJECT_SOURCE_RESTORE_PENDING");
    assert_eq!(handle.info().workspace_root, paths.workspace_root());
    assert!(!destination.join("projects").exists());
    assert!(!live.is_workspace_paused());
    service.shutdown().await.unwrap();
}
