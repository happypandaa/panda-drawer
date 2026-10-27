//! Real service/migration lifetime with an artificial, disposable npm process.
//! This does not launch an application, use network access, or read user data.
use super::*;
use crate::{
    builds::atomic_json, node_install_tasks::PrepareNodeInstallInput,
    source_acquisition::CaptureLocalSourceInput,
};
use std::{fs, os::unix::fs::PermissionsExt};

async fn final_task(handle: &RuntimeHandle, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let task = handle.task(id).await.unwrap();
            if task["final"] == true {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

async fn idle(handle: &RuntimeHandle) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while handle.current().store.has_active_tasks() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

/// Real acquisition/install/cleanup metadata and restart, using an artificial
/// offline npm process. This never launches an application or proves native UI.
#[tokio::test]
async fn captured_app_install_cleanup_preserves_registered_program_and_reopens_after_restart() {
    let (temp, paths, service, _client) = tests::start().await;
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("disposable-import-cleanup-author-source");
    fs::create_dir_all(source.join("public")).unwrap();
    fs::create_dir_all(source.join("server")).unwrap();
    fs::write(
        source.join("public/index.html"),
        "disposable installed content",
    )
    .unwrap();
    fs::write(
        source.join("server/main.mjs"),
        "throw Error('must not execute');",
    )
    .unwrap();
    atomic_json(
        &source.join("local-app.json"),
        &json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"disposable.import.cleanup","name":"可丢弃导入清理集成","version":"1.0.0"},
            "dataContract":{"id":"disposable.import.cleanup.data","version":1},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"root":"server","entry":"main.mjs","nodeMajor":24,"protocolVersion":1}
        }),
    )
    .unwrap();
    atomic_json(
        &source.join("package.json"),
        &json!({"name":"disposable-import-cleanup","version":"1.0.0"}),
    )
    .unwrap();
    atomic_json(&source.join("package-lock.json"), &json!({"name":"disposable-import-cleanup","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"disposable-import-cleanup","version":"1.0.0"}}})).unwrap();
    let bin = root.join("disposable-import-cleanup-toolchain/bin");
    fs::create_dir_all(&bin).unwrap();
    let node = bin.join("node");
    fs::write(&node, "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'v24.22.0\\n'; exit 0; fi\nexec /bin/sh \"$@\"\n").unwrap();
    fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(bin.join("npm"), "set -eu\nif [ \"$1\" = --version ]; then printf '11.9.0\\n'; exit 0; fi\nmkdir -p node_modules\nprintf 'offline dependency preparation' > node_modules/disposable-marker\n").unwrap();
    let handle = service.handle();
    handle
        .set_node_path(Some(node.to_string_lossy().into_owned()))
        .await
        .unwrap();
    let captured = handle
        .source_capture(CaptureLocalSourceInput {
            source_path: source.clone(),
            name: Some("disposable import cleanup integration".into()),
        })
        .await
        .unwrap();
    let source_task = captured["taskId"].as_str().unwrap().to_owned();
    let captured = final_task(&handle, &source_task).await;
    assert_eq!(captured["status"], "succeeded", "{captured}");
    let ready = &captured["result"];
    let copy = PathBuf::from(ready["sourcePath"].as_str().unwrap());
    let source_slot = PathBuf::from(ready["evidencePath"].as_str().unwrap());
    let source_id = ready["sourceId"].as_str().unwrap().to_owned();
    assert!(source_slot.join(".acquiring").is_dir());
    let nodes = Arc::new(
        crate::node_apps::NodeApps::new(
            paths.data_root().into(),
            root.join("must-not-start-runtime"),
        )
        .unwrap(),
    );
    idle(&handle).await;
    let prepared = handle
        .node_install_prepare(
            nodes.clone(),
            PrepareNodeInstallInput {
                build_source: false,
                source_id: source_id.clone(),
                expected_source_digest: ready["contentDigest"].as_str().unwrap().into(),
                target_app_id: None,
            },
        )
        .await
        .unwrap();
    let prepared = final_task(&handle, prepared["taskId"].as_str().unwrap()).await;
    assert_eq!(prepared["status"], "succeeded", "{prepared}");
    let candidate = prepared["result"]["candidate"]["candidateId"]
        .as_str()
        .unwrap()
        .to_owned();
    let stage = paths
        .data_root()
        .join("node-apps/.installations")
        .join(&candidate);
    assert!(stage.join("install.json").is_file());
    idle(&handle).await;
    let applied = handle
        .node_install_apply(nodes.clone(), &candidate)
        .await
        .unwrap();
    assert_eq!(applied["status"], "succeeded", "{applied}");
    let app_id = applied["result"]["appId"].as_str().unwrap().to_owned();
    let generation = applied["result"]["generation"].as_str().unwrap();
    let app_slot = paths.data_root().join("node-apps").join(&app_id);
    let program = app_slot.join("generations").join(generation);
    let registration = fs::read(app_slot.join("registration.json")).unwrap();
    fs::create_dir_all(app_slot.join("data")).unwrap();
    fs::create_dir_all(app_slot.join("configuration")).unwrap();
    fs::write(app_slot.join("data/disposable-keep"), "business marker").unwrap();
    fs::write(
        app_slot.join("configuration/disposable-keep"),
        "configuration marker",
    )
    .unwrap();
    idle(&handle).await;
    let cleaned = handle
        .source_import_cleanup(source_task.clone())
        .await
        .unwrap();
    assert_eq!(cleaned["completed"], true, "{cleaned}");
    assert_eq!(cleaned["sourceRetained"], true);
    assert!(!stage.exists());
    assert!(!source_slot.join(".acquiring").exists());
    assert_eq!(
        fs::read(program.join("public/index.html")).unwrap(),
        b"disposable installed content"
    );
    assert_eq!(
        fs::read(copy.join("public/index.html")).unwrap(),
        b"disposable installed content"
    );
    assert_eq!(
        fs::read(source.join("public/index.html")).unwrap(),
        b"disposable installed content"
    );
    assert_eq!(
        fs::read(app_slot.join("registration.json")).unwrap(),
        registration
    );
    assert_eq!(
        fs::read_to_string(app_slot.join("data/disposable-keep")).unwrap(),
        "business marker"
    );
    assert_eq!(
        fs::read_to_string(app_slot.join("configuration/disposable-keep")).unwrap(),
        "configuration marker"
    );
    assert!(nodes
        .list()
        .await
        .unwrap()
        .iter()
        .any(|app| app["id"] == app_id));
    let repeated = handle
        .source_import_cleanup(source_task.clone())
        .await
        .unwrap();
    assert_eq!(repeated["completed"], true);
    assert_eq!(repeated["sourceId"], cleaned["sourceId"]);
    assert_eq!(repeated["sourceRetained"], cleaned["sourceRetained"]);
    assert!(repeated["removedPaths"].as_array().unwrap().is_empty());
    assert!(!paths.data_root().join("cleanup/imports").exists());

    // A real unsupported capture is also removable without touching its author
    // directory or the installed app from the first half of this test.
    let unsupported = root.join("disposable-unsupported-import-cleanup");
    fs::create_dir(&unsupported).unwrap();
    fs::write(unsupported.join("README.md"), "unsupported author material").unwrap();
    let captured = handle
        .source_capture(CaptureLocalSourceInput {
            source_path: unsupported.clone(),
            name: None,
        })
        .await
        .unwrap();
    let unsupported_task = captured["taskId"].as_str().unwrap().to_owned();
    let captured = final_task(&handle, &unsupported_task).await;
    assert_eq!(captured["result"]["inspection"]["kind"], "needs-adaptation");
    let unused_slot = PathBuf::from(captured["result"]["evidencePath"].as_str().unwrap());
    idle(&handle).await;
    let removed = handle
        .source_import_cleanup(unsupported_task)
        .await
        .unwrap();
    assert_eq!(removed["completed"], true, "{removed}");
    assert_eq!(removed["sourceRetained"], false);
    assert!(!unused_slot.exists());
    assert_eq!(
        fs::read_to_string(unsupported.join("README.md")).unwrap(),
        "unsupported author material"
    );
    nodes.stop_all().await.unwrap();
    service.shutdown().await.unwrap();

    let restarted = RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
        .await
        .unwrap();
    let reopened = crate::node_apps::NodeApps::new(
        paths.data_root().into(),
        root.join("must-not-start-runtime"),
    )
    .unwrap();
    let listed = reopened.list().await.unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!(listed[0]["id"], app_id);
    assert_eq!(
        restarted.handle().node_install_list().unwrap()["candidates"],
        json!([])
    );
    assert_eq!(restarted.handle().node_install_issues(), json!([]));
    assert_eq!(
        restarted.handle().source_info(&source_id).await.unwrap()["status"],
        "ready"
    );
    let rescanned = restarted
        .handle()
        .source_import_cleanup(source_task)
        .await
        .unwrap();
    assert_eq!(rescanned["completed"], true);
    assert_eq!(rescanned["sourceId"], cleaned["sourceId"]);
    assert_eq!(rescanned["sourceRetained"], cleaned["sourceRetained"]);
    assert!(rescanned["removedPaths"].as_array().unwrap().is_empty());
    assert!(!paths.data_root().join("cleanup/imports").exists());
    assert_eq!(
        fs::read(app_slot.join("registration.json")).unwrap(),
        registration
    );
    assert_eq!(
        fs::read_to_string(app_slot.join("data/disposable-keep")).unwrap(),
        "business marker"
    );
    reopened.stop_all().await.unwrap();
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_signals_install_before_waiting_for_public_workspace_migration() {
    let (temp, paths, service, _client) = tests::start().await;
    let root = temp.path().canonicalize().unwrap();
    let source = root.join("disposable-shutdown-source");
    fs::create_dir_all(source.join("public")).unwrap();
    fs::create_dir(source.join("server")).unwrap();
    fs::write(source.join("public/index.html"), "preserved source").unwrap();
    fs::write(
        source.join("server/main.mjs"),
        "throw Error('must not execute');",
    )
    .unwrap();
    atomic_json(
        &source.join("local-app.json"),
        &json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"disposable.shutdown","name":"可丢弃退出样本","version":"1.0.0"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"root":"server","entry":"main.mjs","nodeMajor":24,"protocolVersion":1}
        }),
    )
    .unwrap();
    atomic_json(
        &source.join("package.json"),
        &json!({"name":"disposable-shutdown","version":"1.0.0"}),
    )
    .unwrap();
    atomic_json(&source.join("package-lock.json"), &json!({"name":"disposable-shutdown","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"disposable-shutdown","version":"1.0.0"}}})).unwrap();
    let bin = root.join("disposable-toolchain/bin");
    fs::create_dir_all(&bin).unwrap();
    let node = bin.join("node");
    fs::write(&node, "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'v24.22.0\\n'; exit 0; fi\nexec /bin/sh \"$@\"\n").unwrap();
    fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
    fs::write(bin.join("npm"), "set -eu\nif [ \"$1\" = --version ]; then printf '11.9.0\\n'; exit 0; fi\nmkdir -p node_modules\nprintf '%s' \"$$\" > node_modules/disposable-pid\n/bin/sleep 60 & wait\n").unwrap();
    let handle = service.handle();
    handle
        .set_node_path(Some(node.to_string_lossy().into_owned()))
        .await
        .unwrap();
    let captured = handle
        .source_capture(CaptureLocalSourceInput {
            source_path: source.clone(),
            name: Some("disposable shutdown capture".into()),
        })
        .await
        .unwrap();
    let captured = final_task(&handle, captured["taskId"].as_str().unwrap()).await;
    assert_eq!(captured["status"], "succeeded", "{captured}");
    let ready = &captured["result"];
    let nodes = Arc::new(
        crate::node_apps::NodeApps::new(
            paths.data_root().into(),
            root.join("must-not-start-runtime"),
        )
        .unwrap(),
    );
    let task = handle
        .node_install_prepare(
            nodes.clone(),
            PrepareNodeInstallInput {
                build_source: false,
                source_id: ready["sourceId"].as_str().unwrap().into(),
                expected_source_digest: ready["contentDigest"].as_str().unwrap().into(),
                target_app_id: None,
            },
        )
        .await
        .unwrap();
    let pid = tokio::time::timeout(Duration::from_secs(5), async {
        'found: loop {
            if let Ok(entries) = fs::read_dir(paths.data_root().join("node-apps/.installations")) {
                for entry in entries.flatten() {
                    let marker = entry.path().join("program/node_modules/disposable-pid");
                    if let Ok(pid) = fs::read_to_string(marker)
                        .and_then(|value| value.parse::<i32>().map_err(std::io::Error::other))
                    {
                        break 'found pid;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let destination = root.join("disposable-migrated-workspace");
    fs::create_dir(&destination).unwrap();
    let migration = handle
        .workspace_migrate(json!({"destinationPath":destination}))
        .await
        .unwrap();
    let old = handle.current();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handle
                .task(migration["taskId"].as_str().unwrap())
                .await
                .unwrap()["phase"]
                == "waiting-for-project-tasks"
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(old.store.has_active_tasks());
    let mut shutdown = Box::pin(service.shutdown());
    let timely = match tokio::time::timeout(Duration::from_secs(5), &mut shutdown).await {
        Ok(result) => {
            result.unwrap();
            true
        }
        Err(_) => {
            // Even a regression must clean up the artificial child and service.
            let _ = old.stop.send(true);
            tokio::time::timeout(Duration::from_secs(10), shutdown)
                .await
                .unwrap()
                .unwrap();
            false
        }
    };
    assert!(
        timely,
        "shutdown waited for migration before signalling its leased installation"
    );
    assert_ne!(unsafe { libc::kill(-pid, 0) }, 0);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    let completed = handle.task(task["taskId"].as_str().unwrap()).await.unwrap();
    assert_eq!(completed["status"], "interrupted", "{completed}");
    assert_eq!(completed["error"]["code"], "NODE_INSTALL_INTERRUPTED");
    let migrated = handle
        .task(migration["taskId"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(migrated["status"], "succeeded", "{migrated}");
    assert_eq!(handle.info().workspace_root, destination);
    assert!(
        *handle.current().stop.borrow(),
        "the migrated state must stop too"
    );
    assert!(!old.store.has_active_tasks());
    assert_eq!(
        fs::read(source.join("public/index.html")).unwrap(),
        b"preserved source"
    );
    assert!(nodes.list().await.unwrap().is_empty());
    nodes.stop_all().await.unwrap();
}
