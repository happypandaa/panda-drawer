use super::*;
use std::{sync::Arc, time::Instant};

const SAVE_HOST: &str = "node_apps::windows_runtime_tests::configuration_concurrency::disposable_runtime_bound_configuration_host";
const SAVE_ROOT: &str = "PANDA_DISPOSABLE_W5_RUNTIME_CONFIGURATION_ROOT";

#[test]
#[ignore = "Explicit PANDA_W3_OFFICIAL_NODE required; bounded fresh Runtime-bound configuration host"]
fn official_runtime_bound_save_with_changed_declaration_and_concurrent_desktop_reads() {
    let _node = official_node();
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w3-nodeapps-runtime-save-")
        .tempdir()
        .unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", SAVE_HOST, "--ignored", "--nocapture"])
        .env(SAVE_ROOT, temporary.path())
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(
                status.success(),
                "disposable Runtime-bound save host failed"
            );
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("disposable Runtime-bound configuration host did not finish");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "Only the bounded official Runtime-bound configuration parent starts this host"]
async fn disposable_runtime_bound_configuration_host() {
    let root = PathBuf::from(std::env::var_os(SAVE_ROOT).unwrap())
        .canonicalize()
        .unwrap();
    let mut fixture = Fixture::create(&root, "normal");
    let declaration_path = fixture.source.join("local-app.json");
    let declared_bytes = fs::read(&declaration_path).unwrap();
    let mut initial: Value = serde_json::from_slice(&declared_bytes).unwrap();
    initial.as_object_mut().unwrap().remove("configuration");
    fs::write(&declaration_path, serde_json::to_vec(&initial).unwrap()).unwrap();

    let (_runtime_temporary, paths) = crate::RuntimePaths::for_test();
    fixture.data = paths.data_root().into();
    let service = crate::RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
        .await
        .unwrap();
    let handle = service.handle();
    let apps = NodeApps::with_runtime(fixture.data.clone(), fixture.node.clone(), &handle).unwrap();
    assert!(apps.processes.shares_registry(&service.store().processes()));
    let registration = apps.register(fixture.source.clone()).await.unwrap();
    let id = registration["id"].as_str().unwrap().to_owned();
    let start_apps = apps.clone();
    let start_id = id.clone();
    let started = handle
        .io_lifetime()
        .admit()
        .unwrap()
        .run(async move { start_apps.start(&start_id).await })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(started["state"], "running");

    // Match the GUI trigger: add required configuration while the old launch
    // still owns its original declaration, rather than only updating fields
    // already present when the process was launched.
    fs::write(&declaration_path, &declared_bytes).unwrap();
    let current = apps.configuration(&id).await.unwrap();
    assert_ne!(current["manifestDigest"], registration["manifestDigest"]);
    assert_eq!(current["configuration"]["canStart"], false);
    let barrier = Arc::new(tokio::sync::Barrier::new(3));
    let mut pollers = Vec::new();
    for list in [false, true] {
        let apps = apps.clone();
        let id = id.clone();
        let io = handle.io_lifetime();
        let barrier = barrier.clone();
        pollers.push(tokio::spawn(async move {
            barrier.wait().await;
            for _ in 0..48 {
                let apps = apps.clone();
                let request_id = id.clone();
                let value = io
                    .admit()
                    .unwrap()
                    .run(async move {
                        if list {
                            apps.list().await.map(Value::Array)
                        } else {
                            apps.configuration(&request_id).await
                        }
                    })
                    .await
                    .unwrap()
                    .unwrap();
                assert!(value.is_array() || value["id"] == id);
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }));
    }
    barrier.wait().await;
    let save_apps = apps.clone();
    let save_id = id.clone();
    let input = SaveConfigurationRequest {
        revision: current["configuration"]["revision"].as_u64().unwrap(),
        manifest_digest: Some(current["manifestDigest"].as_str().unwrap().into()),
        values: BTreeMap::from([("GREETING".into(), json!("saved while old launch runs"))]),
        secrets: BTreeMap::from([(
            "API_KEY".into(),
            SecretAction::Replace {
                value: FIRST_SECRET.into(),
            },
        )]),
    };
    let saved = tokio::time::timeout(
        Duration::from_secs(10),
        handle
            .io_lifetime()
            .admit()
            .unwrap()
            .run(async move { save_apps.save(&save_id, input).await }),
    )
    .await
    .expect("desktop-style save must return after commit")
    .unwrap()
    .unwrap();
    for poller in pollers {
        poller.await.unwrap();
    }
    assert_eq!(saved["id"], id);
    assert_eq!(saved["state"], "running");
    assert_eq!(saved["restartRequired"], true);
    assert_eq!(saved["configuration"]["revision"], 1);
    assert_eq!(saved["configuration"]["secretStatus"]["API_KEY"], "set");
    assert!(!saved.to_string().contains(FIRST_SECRET));
    assert_eq!(apps.owned_process_ids().await.len(), 1);

    let restarted = apps.restart(&id).await.unwrap();
    assert_eq!(restarted["restartRequired"], false);
    let running = Browser::open(&apps, &id).await.state().await;
    assert_eq!(running["greeting"], "saved while old launch runs");
    let process = CapturedProcess::capture(running["pid"].as_u64().unwrap().try_into().unwrap());
    apps.stop_all().await.unwrap();
    stopped(&[process]).await;
    absent(&fixture.slot(&id).join("active.json"));
    no_cleanup(&fixture.slot(&id));
    let processes = service.store().processes();
    let io = handle.io_lifetime();
    service.shutdown().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), io.drain())
        .await
        .unwrap();
    assert!(processes
        .snapshot()
        .iter()
        .all(|record| record.resource_closed()));
}
