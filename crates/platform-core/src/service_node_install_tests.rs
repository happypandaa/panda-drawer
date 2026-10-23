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
