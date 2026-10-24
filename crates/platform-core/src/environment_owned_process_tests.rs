//! Real disposable OS processes test lifecycle ownership, not Node compatibility.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

struct KeepAlive {
    _directory: Arc<tempfile::TempDir>,
    dropped: Arc<AtomicBool>,
}
impl Drop for KeepAlive {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::Release);
    }
}

async fn closed(registry: &OwnedProcessRegistry) {
    assert!(
        tokio::time::timeout(Duration::from_secs(5), registry.shutdown())
            .await
            .unwrap()
            .is_empty()
    );
    assert!(registry
        .snapshot()
        .iter()
        .all(|record| record.resource_closed()));
}

#[tokio::test]
async fn owned_environment_version_probe_stops_real_term_ignoring_descendant() {
    let directory = Arc::new(
        tempfile::Builder::new()
            .prefix("disposable-environment-child-")
            .tempdir()
            .unwrap(),
    );
    let marker = directory.path().join("writer");
    let registry = OwnedProcessRegistry::default();
    let dropped = Arc::new(AtomicBool::new(false));
    let script = "import os,signal,sys,time\npid=os.fork()\nif pid==0:\n signal.signal(signal.SIGTERM,signal.SIG_IGN)\n os.close(1);os.close(2)\n with open(sys.argv[1],'ab',buffering=0) as f:\n  while True: f.write(b'owned child\\n');time.sleep(.001)\nwhile not os.path.exists(sys.argv[1]): time.sleep(.001)\nprint('v24.22.0',flush=True)\nos._exit(0)";
    let mut command = Command::new("/usr/bin/python3");
    command.args(["-c", script, marker.to_str().unwrap()]);
    let value = output(
        command,
        &registry,
        KeepAlive {
            _directory: directory,
            dropped: dropped.clone(),
        },
    )
    .await
    .unwrap();
    assert_eq!(value, "v24.22.0");
    assert!(dropped.load(Ordering::Acquire));
    assert!(
        !marker.exists(),
        "candidate directory released after real group and readers finished"
    );
    closed(&registry).await;
}

#[tokio::test]
async fn owned_environment_stdout_limit_stops_writer_and_drains_all_registered_tasks() {
    let registry = OwnedProcessRegistry::default();
    let mut command = Command::new("/usr/bin/python3");
    command.args([
        "-c",
        "import sys\nfor _ in range(256): sys.stdout.buffer.write(b'x'*65536)\nsys.stdout.flush()",
    ]);
    assert_eq!(
        output(command, &registry, ()).await.unwrap_err().code,
        "TOOLCHAIN_PROBE_FAILED"
    );
    closed(&registry).await;
}

#[tokio::test]
async fn owned_environment_cancelled_waiter_keeps_actual_process_owner() {
    let registry = OwnedProcessRegistry::default();
    let directory = Arc::new(
        tempfile::Builder::new()
            .prefix("disposable-environment-cancel-")
            .tempdir()
            .unwrap(),
    );
    let dropped = Arc::new(AtomicBool::new(false));
    let waiter = {
        let registry = registry.clone();
        let dropped = dropped.clone();
        tokio::spawn(async move {
            let mut command = Command::new("/bin/sleep");
            command.arg("30");
            output(
                command,
                &registry,
                KeepAlive {
                    _directory: directory,
                    dropped,
                },
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        while registry.snapshot().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    waiter.abort();
    let _ = waiter.await;
    if dropped.load(Ordering::Acquire) {
        let record = &registry.snapshot()[0];
        assert!(record.leader_reaped && record.group_exited && record.io_joined);
    }
    closed(&registry).await;
    assert!(dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn owned_environment_sealed_registry_cannot_replace_tool_selection() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::Builder::new()
        .prefix("disposable-environment-selection-")
        .tempdir()
        .unwrap();
    let data = directory.path().join("data");
    fs::create_dir(&data).unwrap();
    let node = directory.path().join("node");
    fs::write(&node, "#!/bin/sh\nprintf 'v24.22.0\\n'\n").unwrap();
    fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
    let settings = data.join("toolchain.json");
    let before = b"{\"schemaVersion\":1,\"nodePath\":null}\n";
    fs::write(&settings, before).unwrap();
    let registry = OwnedProcessRegistry::default();
    let manager = EnvironmentManager::with_processes(data, None, registry.clone());
    assert!(manager.processes.shares_registry(&registry));
    registry.seal();
    assert_eq!(
        manager.configure_node(Some(node)).await.unwrap_err().code,
        "SERVICE_STOPPING"
    );
    assert_eq!(fs::read(settings).unwrap(), before);
    assert!(registry.snapshot().is_empty());
}
