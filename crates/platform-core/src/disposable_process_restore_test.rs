//! Isolated test harness only; never compiled into desktop/CLI production.
//! Parent holds the actual Child handle, observes a real published slot, and
//! SIGKILLs only that child. No transaction failpoint or journal edits are used.
use super::*;

struct DisposableChild(std::process::Child);

impl std::ops::Deref for DisposableChild {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for DisposableChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for DisposableChild {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[tokio::test]
#[ignore = "explicit isolated process interruption acceptance; no production root"]
async fn disposable_real_process_data_restore_worker() {
    let cycle = PathBuf::from(std::env::var("DISPOSABLE_RESTORE_CYCLE").unwrap());
    assert!(cycle.is_absolute());
    let mode = std::env::var("DISPOSABLE_RESTORE_MODE").unwrap();
    if mode == "apply" {
        let fixture = Fixture::new().await;
        fixture.sql("CREATE TABLE notes(body TEXT)").await;
        fixture.sql("INSERT INTO notes VALUES('target')").await;
        let attachments = fixture
            .project
            .root
            .join(&fixture.project.manifest.attachments_dir);
        for index in 0..400 {
            fs::write(
                attachments.join(format!("filler-{index:04}.txt")),
                format!("target filler {index}: 中文 🌱\n").repeat(8),
            )
            .unwrap();
        }
        fs::write(attachments.join("value.txt"), b"target attachment").unwrap();
        let target = fixture.save().await;
        fixture
            .sql("UPDATE notes SET body='before process death'")
            .await;
        fs::write(
            attachments.join("value.txt"),
            b"before process death attachment",
        )
        .unwrap();
        let input = fixture.input(&target).await;
        let ready = json!({
            "dataRoot": fixture.manager.paths.data_root(),
            "workspaceRoot": fixture.manager.paths.workspace_root(),
            "projectRoot": fixture.project.root,
            "projectId": fixture.project.manifest.id,
            "targetCommit": target,
            "headSha256": format!("{:x}", Sha256::digest(fs::read(fixture.project.root.join(".git/HEAD")).unwrap())),
            "indexSha256": format!("{:x}", Sha256::digest(fs::read(fixture.project.root.join(".git/index")).unwrap())),
        });
        fs::write(
            cycle.join("ready.json"),
            serde_json::to_vec_pretty(&ready).unwrap(),
        )
        .unwrap();
        let task = fixture.apply(input).await;
        fs::write(
            cycle.join("finished-before-kill.json"),
            serde_json::to_vec_pretty(&task).unwrap(),
        )
        .unwrap();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    assert_eq!(mode, "recover");
    let ready: Value =
        serde_json::from_slice(&fs::read(cycle.join("ready.json")).unwrap()).unwrap();
    let paths = RuntimePaths::for_disposable_interruption_test(PathBuf::from(
        ready["dataRoot"].as_str().unwrap(),
    ))
    .unwrap();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1234".into()).unwrap();
    let tool = PathBuf::from(std::env::var("LOCAL_PROJECTS_TEST_GIT_ROOT").unwrap());
    let git = GitEnvironmentManager::new(paths.data_root().into(), Some(tool));
    let manager = ProjectGitManager::new(paths, store.clone(), tasks.clone(), git).unwrap();
    let (_sender, stop) = watch::channel(false);
    let id = ready["projectId"].as_str().unwrap();
    let before = manager.data_restore_status(id, stop.clone()).await.unwrap();
    fs::write(
        cycle.join("pending-after-process-death.json"),
        serde_json::to_vec_pretty(&before).unwrap(),
    )
    .unwrap();
    assert_eq!(before.state, "pending", "{before:?}");
    assert!(before.business_blocked);
    assert_eq!(before.operations.len(), 1);
    let op = &before.operations[0];
    assert!(op.original_retained);
    assert!(op.can_continue);
    let action = match std::env::var("DISPOSABLE_RESTORE_ACTION").unwrap().as_str() {
        "continue" => RestoreAction::Continue,
        "undo" => RestoreAction::Undo,
        _ => panic!("explicit action required"),
    };
    let committed = std::env::var("DISPOSABLE_RESTORE_BOUNDARY").unwrap() == "completion";
    assert_eq!(op.data_state, if committed { "applied" } else { "before" });
    assert_eq!(
        op.operation_outcome,
        if committed {
            "committed"
        } else {
            "not_committed"
        }
    );
    assert_eq!(op.can_undo, !committed);
    assert!(store.schema(id, None).await.is_err());
    let task = manager
        .data_restore_recover(
            id,
            GitDataRestoreRecoverInput {
                restore_task_id: op.restore_task_id.clone(),
                expected_plan_digest: op.plan_digest.clone(),
                expected_journal_digest: op.journal_digest.clone(),
                action,
            },
            stop.clone(),
        )
        .await
        .unwrap();
    let done = wait(&tasks, &task.task_id).await;
    fs::write(
        cycle.join("recovered-task.json"),
        serde_json::to_vec_pretty(&done).unwrap(),
    )
    .unwrap();
    assert_eq!(done.status, TaskStatus::Succeeded, "{done:?}");
    assert_eq!(
        manager.data_restore_status(id, stop).await.unwrap().state,
        "clear"
    );
    let fresh = store.project(id).unwrap();
    let db = rusqlite::Connection::open(fresh.root.join(&fresh.manifest.database)).unwrap();
    let actual: String = db
        .query_row("SELECT body FROM notes", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        actual,
        if action == RestoreAction::Continue {
            "target"
        } else {
            "before process death"
        }
    );
    drop(db);
    let attachments = fresh.root.join(&fresh.manifest.attachments_dir);
    assert_eq!(
        fs::read(attachments.join("value.txt")).unwrap(),
        if action == RestoreAction::Continue {
            b"target attachment".as_slice()
        } else {
            b"before process death attachment".as_slice()
        }
    );
    assert_eq!(fs::read_dir(&attachments).unwrap().count(), 401);
    for index in 0..400 {
        assert_eq!(
            fs::read_to_string(attachments.join(format!("filler-{index:04}.txt"))).unwrap(),
            format!("target filler {index}: 中文 🌱\n").repeat(8)
        );
    }
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(fs::read(fresh.root.join(".git/HEAD")).unwrap())
        ),
        ready["headSha256"]
    );
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(fs::read(fresh.root.join(".git/index")).unwrap())
        ),
        ready["indexSha256"]
    );
    assert!(manager.shutdown().await.is_empty());
    assert!(tasks.shutdown().await.is_empty());
    assert!(store.shutdown().await.is_empty());
}

#[test]
#[ignore = "explicit isolated child-process SIGKILL acceptance, never native GUI"]
fn disposable_real_process_data_restore_interrupt_and_recover() {
    use std::os::unix::process::ExitStatusExt;
    let evidence = PathBuf::from(std::env::var("DISPOSABLE_RESTORE_EVIDENCE").unwrap());
    let worker = "project_git::data_restore::tests::disposable_process::disposable_real_process_data_restore_worker";
    for (label, boundary, action) in [
        ("before-continue", "slot", "continue"),
        ("before-undo", "slot", "undo"),
        ("after-continue", "completion", "continue"),
    ] {
        let cycle = evidence.join(label);
        fs::create_dir(&cycle).unwrap();
        let log = File::create(cycle.join("apply-worker.log")).unwrap();
        let mut child = DisposableChild(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([worker, "--exact", "--ignored", "--nocapture"])
                .env("DISPOSABLE_RESTORE_CYCLE", &cycle)
                .env("DISPOSABLE_RESTORE_MODE", "apply")
                .stdout(log.try_clone().unwrap())
                .stderr(log)
                .spawn()
                .unwrap(),
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        let ready: Value = loop {
            if let Ok(bytes) = fs::read(cycle.join("ready.json")) {
                if let Ok(value) = serde_json::from_slice(&bytes) {
                    break value;
                }
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "apply worker exited early: {label}"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "worker setup deadline: {label}"
            );
            std::thread::sleep(Duration::from_millis(1));
        };
        let operations = PathBuf::from(ready["workspaceRoot"].as_str().unwrap())
            .join(".platform/project-state/data-restore")
            .join(ready["projectId"].as_str().unwrap());
        let observed = loop {
            if let Ok(entries) = fs::read_dir(&operations) {
                let mut matched = None;
                for entry in entries.flatten() {
                    let op = entry.path();
                    let trigger = if boundary == "slot" {
                        op.join("slots/database/before")
                    } else {
                        op.join("completion.json")
                    };
                    if trigger.exists() && !op.join("terminal.json").exists() {
                        matched = Some(op);
                        break;
                    }
                }
                if let Some(op) = matched {
                    break op;
                }
            }
            assert!(
                !cycle.join("finished-before-kill.json").exists(),
                "missed process boundary: {label}"
            );
            assert!(
                child.try_wait().unwrap().is_none(),
                "apply worker exited early: {label}"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "process boundary deadline: {label}"
            );
            std::thread::sleep(Duration::from_micros(100));
        };
        let pid = child.id();
        child.kill().unwrap();
        let killed = child.wait().unwrap();
        assert_eq!(killed.signal(), Some(9));
        fs::write(cycle.join("process-interruption.json"), serde_json::to_vec_pretty(&json!({"childPid":pid,"signal":9,"observedOperation":observed,"observedBoundary":boundary,"manualJournalOrSlotEdits":false,"productionFailpoints":false})).unwrap()).unwrap();
        let log = File::create(cycle.join("recovery-worker.log")).unwrap();
        let recovered = std::process::Command::new(std::env::current_exe().unwrap())
            .args([worker, "--exact", "--ignored", "--nocapture"])
            .env("DISPOSABLE_RESTORE_CYCLE", &cycle)
            .env("DISPOSABLE_RESTORE_MODE", "recover")
            .env("DISPOSABLE_RESTORE_ACTION", action)
            .env("DISPOSABLE_RESTORE_BOUNDARY", boundary)
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .status()
            .unwrap();
        assert!(recovered.success(), "{label}: see recovery-worker.log");
        eprintln!("actual child SIGKILL and fresh-process {action} passed: {label}");
    }
}
