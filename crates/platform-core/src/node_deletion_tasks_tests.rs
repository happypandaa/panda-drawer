use super::*;
use std::time::Duration;

struct Fixture {
    _temp: tempfile::TempDir,
    data: PathBuf,
    source: PathBuf,
    apps: NodeApps,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let data = root.join("disposable-deletion-data");
        fs::create_dir(&data).unwrap();
        let source = root.join("disposable-external-author-source");
        fs::create_dir_all(source.join("public")).unwrap();
        fs::create_dir(source.join("server")).unwrap();
        fs::write(source.join("public/index.html"), "disposable").unwrap();
        fs::write(
            source.join("server/main.mjs"),
            "throw Error('never executed')",
        )
        .unwrap();
        fs::write(
            source.join("author-sentinel"),
            b"preserve external author files",
        )
        .unwrap();
        crate::builds::atomic_json(&source.join("local-app.json"), &json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"test.disposable.persistent-delete","name":"可丢弃持久删除","version":"1"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"protocolVersion":1,"root":"server","entry":"main.mjs","nodeMajor":24}
        })).unwrap();
        let apps = NodeApps::new(data.clone(), "/disposable/nonexistent-node".into()).unwrap();
        Self {
            _temp: temp,
            data,
            source,
            apps,
        }
    }
    async fn register(&self) -> String {
        self.apps.register(self.source.clone()).await.unwrap()["id"]
            .as_str()
            .unwrap()
            .into()
    }
    async fn input(&self, id: &str) -> DeleteNodeAppInput {
        serde_json::from_value(json!({"expectedRevision":self.apps.delete_preview(id).await.unwrap()["expectedRevision"]})).unwrap()
    }
    fn metadata(&self, id: &str) -> Value {
        let slot = self.data.join("node-apps").join(id);
        let before = fs::read_to_string(slot.join("registration.json")).unwrap();
        json!({"appId":id,"name":"可丢弃持久删除","ownership":"external","sourcePath":self.source,
            "expectedRegistrationRevision":deletion::revision(&slot).unwrap(),"registrationBefore":serde_json::from_str::<Value>(&before).unwrap(),"registrationBeforeText":before,
            "rootIdentity":PersistedIdentity::capture(&slot).unwrap(),"preservedPaths":[self.source,slot.join("runs")]})
    }
    fn assert_source(&self) {
        assert_eq!(
            fs::read(self.source.join("author-sentinel")).unwrap(),
            b"preserve external author files"
        );
    }
}
async fn phase(apps: &NodeApps, task: &Value, expected: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let value = apps
                .deletion_task_status(
                    task["taskId"].as_str().unwrap(),
                    task["appId"].as_str().unwrap(),
                )
                .unwrap();
            if value["phase"] == expected {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "task did not reach {expected}: {}",
            apps.deletion_task_status(
                task["taskId"].as_str().unwrap(),
                task["appId"].as_str().unwrap()
            )
            .unwrap()
        )
    })
}

#[tokio::test]
async fn pending_abandon_keeps_gate_until_actual_teardown_and_never_stops_a_later_instance() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let old = fixture.apps.admit_operation(&id, "Git 操作").unwrap();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let before: NodeDeletionBeforeStop = Box::new(move |_| {
        Box::pin(async move {
            entered_tx.send(()).unwrap();
            release_rx.await.unwrap();
            Ok(())
        })
    });
    let task = fixture
        .apps
        .submit_deletion_with(&id, fixture.input(&id).await, before)
        .await
        .unwrap();
    let duplicate = fixture
        .apps
        .submit_deletion(&id, fixture.input(&id).await)
        .await
        .unwrap();
    assert_eq!(task["taskId"], duplicate["taskId"]);
    assert_eq!(
        task["targetRevision"],
        fixture.apps.delete_preview(&id).await.unwrap()["expectedRevision"]
    );
    assert!(!fixture
        .apps
        .list()
        .await
        .unwrap()
        .iter()
        .any(|value| value["id"] == id));
    let abandoned = fixture
        .apps
        .abandon_deletion(
            task["taskId"].as_str().unwrap(),
            &id,
            task["journalRevision"].as_u64().unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(abandoned["phase"], "abandoned");
    assert_eq!(abandoned["abandonRequested"], true);
    tokio::time::timeout(Duration::from_secs(2), entered_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fixture
            .apps
            .admit_operation(&id, "启动")
            .err()
            .unwrap()
            .code,
        "NODE_APP_DELETING"
    );
    release_tx.send(()).unwrap();
    phase(&fixture.apps, &task, "abandoned").await;
    assert!(fixture
        .apps
        .admit_operation(&id, "用户后来明确打开")
        .is_ok());
    assert!(fixture
        .apps
        .list()
        .await
        .unwrap()
        .iter()
        .any(|value| value["id"] == id));
    assert!(fixture
        .data
        .join("node-apps")
        .join(&id)
        .join("registration.json")
        .exists());
    drop(old);
    fixture.assert_source();
}

#[tokio::test]
async fn stopped_or_unconfirmed_failure_can_be_abandoned_without_faking_runtime_recovery() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let slot = fixture.data.join("node-apps").join(&id);
    fs::write(
        slot.join("active.json"),
        br#"{"version":1,"hostPid":2147483646}"#,
    )
    .unwrap();
    let original = fs::read(slot.join("active.json")).unwrap();
    let task = fixture
        .apps
        .submit_deletion(&id, fixture.input(&id).await)
        .await
        .unwrap();
    let failed = phase(&fixture.apps, &task, "failed").await;
    assert_eq!(failed["canAbandon"], true);
    assert_eq!(failed["canRetry"], true);
    let retry = fixture
        .apps
        .retry_deletion(
            task["taskId"].as_str().unwrap(),
            &id,
            failed["journalRevision"].as_u64().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry["taskId"], task["taskId"]);
    let failed = phase(&fixture.apps, &task, "failed").await;
    fixture
        .apps
        .abandon_deletion(
            task["taskId"].as_str().unwrap(),
            &id,
            failed["journalRevision"].as_u64().unwrap(),
        )
        .await
        .unwrap();
    phase(&fixture.apps, &task, "abandoned").await;
    assert_eq!(fs::read(slot.join("active.json")).unwrap(), original);
    let detail = fixture.apps.configuration(&id).await.unwrap();
    assert_eq!(detail["configuration"]["canStart"], false);
    assert_eq!(detail["runRecovery"]["state"], "unconfirmed");
    fixture.assert_source();
}

#[tokio::test]
async fn normal_delete_preserves_external_source_other_instance_and_actual_final_receipt() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let other_source = fixture
        .source
        .with_file_name("disposable-independent-source");
    fs::create_dir_all(other_source.join("public")).unwrap();
    fs::create_dir(other_source.join("server")).unwrap();
    fs::copy(
        fixture.source.join("local-app.json"),
        other_source.join("local-app.json"),
    )
    .unwrap();
    fs::copy(
        fixture.source.join("public/index.html"),
        other_source.join("public/index.html"),
    )
    .unwrap();
    fs::copy(
        fixture.source.join("server/main.mjs"),
        other_source.join("server/main.mjs"),
    )
    .unwrap();
    let other = fixture.apps.register(other_source.clone()).await.unwrap();
    let other_id = other["id"].as_str().unwrap();
    let other_registration = fs::read(
        fixture
            .data
            .join("node-apps")
            .join(other_id)
            .join("registration.json"),
    )
    .unwrap();
    let task = fixture
        .apps
        .submit_deletion(&id, fixture.input(&id).await)
        .await
        .unwrap();
    let result = fixture
        .apps
        .wait_deletion_result(task["taskId"].as_str().unwrap(), &id)
        .await
        .unwrap();
    assert_eq!(result["operationOutcome"], "deleted");
    let done = phase(&fixture.apps, &task, "deleted").await;
    assert_eq!(done["canAbandon"], false);
    assert_eq!(done["renamedAny"], true);
    assert!(fixture
        .apps
        .abandon_deletion(
            task["taskId"].as_str().unwrap(),
            &id,
            done["journalRevision"].as_u64().unwrap()
        )
        .await
        .is_err());
    assert_eq!(
        fs::read(
            fixture
                .data
                .join("node-apps")
                .join(other_id)
                .join("registration.json")
        )
        .unwrap(),
        other_registration
    );
    assert!(other_source.join("server/main.mjs").exists());
    fixture.assert_source();
}

#[tokio::test]
async fn restarted_staged_task_stays_out_of_launcher_and_resumes_same_journal() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let mut journal = NodeDeletionJournal::create(&fixture.data, fixture.metadata(&id)).unwrap();
    assert!(journal.stage_next().unwrap());
    let snapshot = journal.snapshot();
    let restored =
        NodeApps::new(fixture.data.clone(), "/disposable/nonexistent-node".into()).unwrap();
    assert!(restored.list().await.unwrap().is_empty());
    assert_eq!(
        restored.deletion_tasks().unwrap()[0]["taskId"],
        snapshot["taskId"]
    );
    assert!(restored
        .abandon_deletion(
            snapshot["taskId"].as_str().unwrap(),
            &id,
            snapshot["journalRevision"].as_u64().unwrap()
        )
        .await
        .is_err());
    restored.resume_deletions().await.unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        restored.wait_deletion_result(snapshot["taskId"].as_str().unwrap(), &id),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!fixture
        .data
        .join("node-apps")
        .join(&id)
        .join("registration.json")
        .exists());
    fixture.assert_source();
}

#[tokio::test]
async fn requested_abandon_survives_restart_without_executing_any_directory_move() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let mut journal = NodeDeletionJournal::create(&fixture.data, fixture.metadata(&id)).unwrap();
    journal.request_abandon().unwrap();
    let restored =
        NodeApps::new(fixture.data.clone(), "/disposable/nonexistent-node".into()).unwrap();
    let tasks = restored.deletion_tasks().unwrap();
    assert_eq!(tasks[0]["phase"], "abandoned");
    restored.resume_deletions().await.unwrap();
    assert!(restored
        .list()
        .await
        .unwrap()
        .iter()
        .any(|value| value["id"] == id));
    assert!(fixture
        .data
        .join("node-apps")
        .join(&id)
        .join("data")
        .is_dir());
    fixture.assert_source();
}

#[tokio::test]
async fn accepted_deletion_is_visible_to_shared_workspace_coordination_before_worker_poll() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let workspace = fixture
        .data
        .with_file_name("disposable-migration-workspace");
    fs::create_dir(&workspace).unwrap();
    let store = Arc::new(ProjectStore::new(workspace).unwrap());
    fixture.apps.set_deletion_store(store.clone()).unwrap();
    let old = fixture.apps.admit_operation(&id, "Git 操作").unwrap();
    let task = fixture
        .apps
        .submit_deletion(&id, fixture.input(&id).await)
        .await
        .unwrap();
    assert!(
        store.has_active_tasks(),
        "accepted task must be visible before spawning its worker"
    );
    drop(old);
    tokio::time::timeout(
        Duration::from_secs(5),
        fixture
            .apps
            .wait_deletion_result(task["taskId"].as_str().unwrap(), &id),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while store.has_active_tasks() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn legacy_partial_removal_cannot_be_relabelled_as_reversible() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let registration = fixture
        .data
        .join("node-apps")
        .join(&id)
        .join("registration.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&registration).unwrap()).unwrap();
    value["removalPending"] = json!(true);
    crate::builds::atomic_json(&registration, &value).unwrap();
    let old = fixture.apps.admit_operation(&id, "旧安装收尾").unwrap();
    let task = fixture
        .apps
        .submit_deletion(&id, fixture.input(&id).await)
        .await
        .unwrap();
    assert_eq!(task["canAbandon"], false);
    assert!(fixture
        .apps
        .abandon_deletion(
            task["taskId"].as_str().unwrap(),
            &id,
            task["journalRevision"].as_u64().unwrap()
        )
        .await
        .is_err());
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(&registration).unwrap()).unwrap()
            ["removalPending"],
        true
    );
    drop(old);
    tokio::time::timeout(
        Duration::from_secs(5),
        fixture
            .apps
            .wait_deletion_result(task["taskId"].as_str().unwrap(), &id),
    )
    .await
    .unwrap()
    .unwrap();
    fixture.assert_source();
}

#[tokio::test]
async fn concurrent_retry_keeps_one_worker_and_the_original_task_identity() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let task = fixture
        .apps
        .submit_deletion_with(
            &id,
            fixture.input(&id).await,
            Box::new(|_| Box::pin(async { Err(fail("disposable first stop failure")) })),
        )
        .await
        .unwrap();
    let failed = phase(&fixture.apps, &task, "failed").await;
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let callback = || {
        let count = count.clone();
        let release = release.clone();
        Box::new(move |_| -> BoxFuture<'static, Result<()>> {
            Box::pin(async move {
                count.fetch_add(1, Ordering::SeqCst);
                release.notified().await;
                Ok(())
            })
        }) as NodeDeletionBeforeStop
    };
    let task_id = task["taskId"].as_str().unwrap();
    let revision = failed["journalRevision"].as_u64().unwrap();
    let (first, second) = tokio::join!(
        fixture
            .apps
            .retry_deletion_with(task_id, &id, revision, callback()),
        fixture
            .apps
            .retry_deletion_with(task_id, &id, revision, callback())
    );
    assert_eq!(first.unwrap()["taskId"], task["taskId"]);
    assert_eq!(second.unwrap()["taskId"], task["taskId"]);
    tokio::time::timeout(Duration::from_secs(2), async {
        while count.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    release.notify_one();
    tokio::time::timeout(
        Duration::from_secs(5),
        fixture.apps.wait_deletion_result(task_id, &id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    fixture.assert_source();
}

#[tokio::test]
async fn changed_registration_fails_before_rename_and_abandon_preserves_replacement() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let old = fixture.apps.admit_operation(&id, "旧操作").unwrap();
    let task = fixture
        .apps
        .submit_deletion(&id, fixture.input(&id).await)
        .await
        .unwrap();
    let slot = fixture.data.join("node-apps").join(&id);
    let registration = slot.join("registration.json");
    let mut changed: Value = serde_json::from_slice(&fs::read(&registration).unwrap()).unwrap();
    changed["manifestDigest"] = json!("0".repeat(64));
    crate::builds::atomic_json(&registration, &changed).unwrap();
    let replacement = fs::read(&registration).unwrap();
    drop(old);
    let failed = phase(&fixture.apps, &task, "failed").await;
    assert_eq!(failed["renamedAny"], false);
    assert_eq!(failed["canAbandon"], true);
    fixture
        .apps
        .abandon_deletion(
            task["taskId"].as_str().unwrap(),
            &id,
            failed["journalRevision"].as_u64().unwrap(),
        )
        .await
        .unwrap();
    phase(&fixture.apps, &task, "abandoned").await;
    assert_eq!(fs::read(&registration).unwrap(), replacement);
    assert!(slot.join("data").is_dir());
    fixture.assert_source();
}

#[tokio::test]
async fn platform_stop_waits_for_the_actual_accepted_file_worker() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let callback: NodeDeletionBeforeStop = Box::new(move |_| {
        Box::pin(async move {
            entered_tx.send(()).unwrap();
            release_rx.await.unwrap();
            Ok(())
        })
    });
    let task = fixture
        .apps
        .submit_deletion_with(&id, fixture.input(&id).await, callback)
        .await
        .unwrap();
    entered_rx.await.unwrap();
    let apps = fixture.apps.clone();
    let mut shutdown = tokio::spawn(async move { apps.stop_all().await });
    assert!(
        tokio::time::timeout(Duration::from_millis(40), &mut shutdown)
            .await
            .is_err()
    );
    assert_eq!(
        fixture
            .apps
            .deletion_task_status(task["taskId"].as_str().unwrap(), &id)
            .unwrap()["active"],
        true
    );
    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(
        fixture
            .apps
            .deletion_task_status(task["taskId"].as_str().unwrap(), &id)
            .unwrap()["phase"],
        "deleted"
    );
    fixture.assert_source();
}

#[tokio::test]
async fn failed_task_keeps_migration_and_cleanup_blocked_without_blocking_platform_stop() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let workspace = fixture
        .data
        .with_file_name("disposable-failed-deletion-workspace");
    fs::create_dir(&workspace).unwrap();
    let store = Arc::new(ProjectStore::new(workspace).unwrap());
    fixture.apps.set_deletion_store(store.clone()).unwrap();
    let callback: NodeDeletionBeforeStop =
        Box::new(|_| Box::pin(async { Err(fail("disposable stop failure")) }));
    let task = fixture
        .apps
        .submit_deletion_with(&id, fixture.input(&id).await, callback)
        .await
        .unwrap();
    let failed = phase(&fixture.apps, &task, "failed").await;
    tokio::time::timeout(Duration::from_secs(1), async {
        while store.has_active_tasks() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        store.begin_workspace_migration().unwrap_err().code,
        "NODE_DELETION_PENDING"
    );
    assert!(
        store.check_workspace_available().is_ok(),
        "failed admission must not freeze new management work"
    );
    assert_eq!(
        store.begin_workspace_cleanup().unwrap_err().code,
        "NODE_DELETION_PENDING"
    );
    fixture
        .apps
        .abandon_deletion(
            task["taskId"].as_str().unwrap(),
            &id,
            failed["journalRevision"].as_u64().unwrap(),
        )
        .await
        .unwrap();
    phase(&fixture.apps, &task, "abandoned").await;
    store.begin_workspace_migration().unwrap();
    tokio::time::timeout(Duration::from_secs(2), fixture.apps.stop_all())
        .await
        .unwrap()
        .unwrap();
    fixture.assert_source();
}

/// Executed only by the parent test below, never by ordinary test selection.
/// The process owns a disposable manager and stages one journal target, then
/// waits so the parent can terminate the real process through its Child handle.
#[test]
#[ignore = "Only the disposable process-interruption parent test supplies its guarded fixture"]
fn disposable_journal_checkpoint_child() {
    if std::env::var("PANDA_DRAWER_DISPOSABLE_DELETE_CHECKPOINT").as_deref() != Ok("1") {
        return;
    }
    let path = PathBuf::from(std::env::var_os("PANDA_DRAWER_DISPOSABLE_DELETE_METADATA").unwrap());
    assert_eq!(
        path.file_name().unwrap(),
        "disposable-deletion-crash-metadata.json"
    );
    let parent = path.parent().unwrap().canonicalize().unwrap();
    assert!(parent
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with(".tmp"));
    let input: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let data = PathBuf::from(input["dataRoot"].as_str().unwrap())
        .canonicalize()
        .unwrap();
    assert_eq!(data.parent().unwrap(), parent);
    assert_eq!(data.file_name().unwrap(), "disposable-deletion-data");
    let source = PathBuf::from(input["metadata"]["sourcePath"].as_str().unwrap())
        .canonicalize()
        .unwrap();
    assert_eq!(source.parent().unwrap(), parent);
    assert_eq!(
        source.file_name().unwrap(),
        "disposable-external-author-source"
    );
    let _manager = NodeApps::new(data.clone(), "/disposable/nonexistent-node".into()).unwrap();
    let mut journal = NodeDeletionJournal::create(&data, input["metadata"].clone()).unwrap();
    assert!(journal.stage_next().unwrap());
    crate::builds::atomic_json(
        &parent.join("disposable-deletion-crash-checkpoint.json"),
        &journal.snapshot(),
    )
    .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("Disposable journal parent did not terminate its checkpoint child in time");
}

#[tokio::test]
async fn killed_disposable_manager_process_resumes_the_same_staged_task_and_preserves_other_instance(
) {
    struct RetainedChild(Option<std::process::Child>);
    impl Drop for RetainedChild {
        fn drop(&mut self) {
            if let Some(child) = self.0.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let other_source = fixture
        .source
        .with_file_name("disposable-crash-other-source");
    fs::create_dir_all(other_source.join("public")).unwrap();
    fs::create_dir(other_source.join("server")).unwrap();
    for relative in ["local-app.json", "public/index.html", "server/main.mjs"] {
        fs::copy(fixture.source.join(relative), other_source.join(relative)).unwrap();
    }
    fs::write(
        other_source.join("other-author-sentinel"),
        b"preserve another author tree",
    )
    .unwrap();
    let other = fixture.apps.register(other_source.clone()).await.unwrap();
    let other_id = other["id"].as_str().unwrap();
    let other_slot = fixture.data.join("node-apps").join(other_id);
    fs::write(
        other_slot.join("data/other-instance-sentinel"),
        b"preserve another instance data",
    )
    .unwrap();
    let other_registration = fs::read(other_slot.join("registration.json")).unwrap();
    let root = fixture.data.parent().unwrap();
    let metadata_path = root.join("disposable-deletion-crash-metadata.json");
    crate::builds::atomic_json(
        &metadata_path,
        &json!({"dataRoot":fixture.data,"metadata":fixture.metadata(&id)}),
    )
    .unwrap();
    let mut child = RetainedChild(Some(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "node_apps::delete_tasks::tests::disposable_journal_checkpoint_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("PANDA_DRAWER_DISPOSABLE_DELETE_CHECKPOINT", "1")
            .env("PANDA_DRAWER_DISPOSABLE_DELETE_METADATA", &metadata_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let checkpoint_path = root.join("disposable-deletion-crash-checkpoint.json");
    tokio::time::timeout(Duration::from_secs(20), async {
        while !checkpoint_path.exists() {
            if let Some(status) = child.0.as_mut().unwrap().try_wait().unwrap() {
                let mut diagnostic = String::new();
                if let Some(mut stderr) = child.0.as_mut().unwrap().stderr.take() {
                    std::io::Read::read_to_string(&mut stderr, &mut diagnostic).unwrap();
                }
                panic!("Disposable child exited before checkpoint: {status}; {diagnostic}");
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let checkpoint: Value = serde_json::from_slice(&fs::read(checkpoint_path).unwrap()).unwrap();
    assert_eq!(checkpoint["appId"], id);
    assert_eq!(checkpoint["renamedAny"], true);
    assert!(child.0.as_mut().unwrap().try_wait().unwrap().is_none());
    // Only the retained original Child is killed. No disk PID is read or used.
    child.0.as_mut().unwrap().kill().unwrap();
    let status = child.0.as_mut().unwrap().wait().unwrap();
    assert!(!status.success());
    child.0.take();
    let restored =
        NodeApps::new(fixture.data.clone(), "/disposable/nonexistent-node".into()).unwrap();
    let pending = restored
        .deletion_tasks()
        .unwrap()
        .into_iter()
        .find(|task| task["taskId"] == checkpoint["taskId"])
        .unwrap();
    assert_eq!(pending["appId"], id);
    assert_eq!(pending["canAbandon"], false);
    assert!(!restored
        .list()
        .await
        .unwrap()
        .iter()
        .any(|app| app["id"] == id));
    assert!(restored
        .list()
        .await
        .unwrap()
        .iter()
        .any(|app| app["id"] == other_id));
    restored.resume_deletions().await.unwrap();
    let done = phase(&restored, &checkpoint, "deleted").await;
    assert_eq!(done["taskId"], checkpoint["taskId"]);
    assert_eq!(done["renamedAny"], true);
    assert!(!fixture
        .data
        .join("node-apps")
        .join(&id)
        .join("registration.json")
        .exists());
    assert_eq!(
        fs::read(other_slot.join("registration.json")).unwrap(),
        other_registration
    );
    assert_eq!(
        fs::read(other_slot.join("data/other-instance-sentinel")).unwrap(),
        b"preserve another instance data"
    );
    assert_eq!(
        fs::read(other_source.join("other-author-sentinel")).unwrap(),
        b"preserve another author tree"
    );
    assert_eq!(
        restored.configuration(other_id).await.unwrap()["configuration"]["canStart"],
        true
    );
    assert!(restored.owned_process_ids().await.is_empty());
    fixture.assert_source();
}
