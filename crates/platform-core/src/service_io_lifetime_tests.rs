//! Artificial TempDir resources and loopback only; no installed application data.
use super::*;

#[tokio::test]
async fn shutdown_waits_native_io_even_when_the_native_reply_is_lost() {
    let (_temp, _paths, service, _client) = tests::start().await;
    let project = tests::create(&service, "Disposable I/O drain").await;
    let handle = service.handle();
    let gate = handle
        .store()
        .project(&project.id)
        .unwrap()
        .operation_gate
        .clone();
    let registry = handle.io_lifetime();
    let (started, ready) = oneshot::channel();
    let (release, wait) = oneshot::channel();
    let lease = registry.admit().unwrap();
    let caller = tokio::spawn(lease.run(async move {
        let _gate = gate.read_owned().await;
        let _ = started.send(());
        let _ = wait.await;
    }));
    ready.await.unwrap();
    caller.abort();
    let _ = caller.await;
    let mut shutdown = tokio::spawn(service.shutdown());
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut shutdown)
            .await
            .is_err()
    );
    assert_eq!(registry.admit().err().unwrap().code, "SERVICE_STOPPING");
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn unpolled_download_body_cannot_hide_file_or_project_gate_from_shutdown() {
    let (_temp, paths, service, _client) = tests::start().await;
    let project = tests::create(&service, "Disposable response drain").await;
    let gate = service
        .store()
        .project(&project.id)
        .unwrap()
        .operation_gate
        .clone();
    let path = paths.workspace_root().join("artificial-stream.bin");
    std::fs::write(&path, vec![7u8; 1_000_000]).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    let registry = service.handle().io_lifetime();
    let response = stream_file(
        file,
        "test.bin",
        &HeaderMap::new(),
        true,
        gate.clone().read_owned().await,
        None,
        &registry,
    )
    .await
    .unwrap();
    // Keep the returned Body alive without polling it. Its bounded producer
    // fills and retains the real descriptor/gate until close signals it.
    assert!(
        tokio::time::timeout(Duration::from_millis(30), gate.write())
            .await
            .is_err()
    );
    tokio::time::timeout(Duration::from_secs(5), service.shutdown())
        .await
        .unwrap()
        .unwrap();
    let _exclusive = tokio::time::timeout(Duration::from_secs(1), gate.write())
        .await
        .unwrap();
    drop(response);
}

#[tokio::test]
async fn abandoning_download_body_releases_actual_producer_without_service_shutdown() {
    let (_temp, paths, service, _client) = tests::start().await;
    let gate = Arc::new(tokio::sync::RwLock::new(()));
    let path = paths
        .workspace_root()
        .join("artificial-abandoned-stream.bin");
    std::fs::write(&path, vec![9u8; 1_000_000]).unwrap();
    let registry = service.handle().io_lifetime();
    let response = stream_file(
        std::fs::File::open(path).unwrap(),
        "test.bin",
        &HeaderMap::new(),
        false,
        gate.clone().read_owned().await,
        None,
        &registry,
    )
    .await
    .unwrap();
    drop(response);
    tokio::time::timeout(Duration::from_secs(2), registry.drain())
        .await
        .unwrap();
    let _exclusive = gate.try_write().unwrap();
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn closing_native_admission_also_refuses_new_loopback_http_work() {
    let (_temp, _paths, service, client) = tests::start().await;
    service.handle().io_lifetime().close();
    let response = client
        .get(format!("{}/healthz", service.info().base_url))
        .send()
        .await
        .unwrap();
    let value: Value = response.json().await.unwrap();
    assert_eq!(value["error"]["code"], "SERVICE_STOPPING");
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn disconnected_http_caller_cannot_detach_a_blocking_handler_from_shutdown() {
    let (_temp, paths, service, client) = tests::start().await;
    let path = paths.workspace_root().join("artificial-handler-input");
    std::fs::write(&path, b"artificial local handler content").unwrap();
    let file = std::fs::File::open(path).unwrap();
    let (started, ready) = oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let input = Arc::new(Mutex::new(Some((file, started, wait))));
    let state = service.shared.snapshot();
    *service.shared.router.write().unwrap() = Router::new()
        .route(
            "/artificial-blocking-handler",
            get(move || {
                let (mut file, started, wait) = input.lock().unwrap().take().unwrap();
                async move {
                    tokio::task::spawn_blocking(move || {
                        let _ = started.send(());
                        wait.recv().unwrap();
                        let mut bytes = Vec::new();
                        file.read_to_end(&mut bytes).unwrap();
                        assert_eq!(bytes, b"artificial local handler content");
                    })
                    .await
                    .unwrap();
                    StatusCode::OK
                }
            }),
        )
        .layer(middleware::from_fn_with_state(state, validate_host));
    let url = format!("{}/artificial-blocking-handler", service.info().base_url);
    let caller = tokio::spawn(async move { client.get(url).send().await });
    ready.await.unwrap();
    caller.abort();
    let _ = caller.await;
    let mut shutdown = tokio::spawn(service.shutdown());
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut shutdown)
            .await
            .is_err()
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn an_unpolled_static_body_keeps_its_generation_until_the_actual_file_producer_exits() {
    let (_temp, _paths, service, _client) = tests::start().await;
    let project = tests::create(&service, "Disposable pinned static producer").await;
    std::fs::write(
        project.project_root.join("dist/download.bin"),
        vec![7u8; 1_000_000],
    )
    .unwrap();
    let project = service.store().project(&project.id).unwrap();
    let access = service.handle().artifact_access();
    let (generation, window) = access.current(&project, false).unwrap();
    let generation = generation.unwrap();
    let reader = access.reader(&project, &generation.build_id).unwrap();
    let response = stream_file(
        std::fs::File::open(reader.root().join("download.bin")).unwrap(),
        "download.bin",
        &HeaderMap::new(),
        false,
        project.operation_gate.clone().read_owned().await,
        Some(reader),
        &service.handle().io_lifetime(),
    )
    .await
    .unwrap();
    drop(window);
    access.seal_access();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), access.wait_readers_drained())
            .await
            .is_err()
    );
    assert!(
        generation.root.exists(),
        "a closed window is not a file producer completion receipt"
    );
    drop(response);
    tokio::time::timeout(Duration::from_secs(3), access.wait_readers_drained())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while generation.root.parent().unwrap().exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    service.shutdown().await.unwrap();
}
