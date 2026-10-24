//! Real RuntimeService/NodeApps admission and actual inherited OS pipes. A
//! self-ending test holder deliberately owns its own session; it is released by
//! a private file, never signalled by a stale PID. This is no escape tracking.
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]
use super::*;
use crate::{builds::atomic_json, owned_process::ProcessPhase};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

struct Release(PathBuf);
impl Drop for Release {
    fn drop(&mut self) {
        let _ = fs::write(&self.0, b"self-ending test holder may finish");
    }
}
struct Fixture {
    _release: Release,
    _temporary: tempfile::TempDir,
    paths: RuntimePaths,
    service: Arc<RuntimeService>,
    handle: RuntimeHandle,
    apps: Arc<NodeApps>,
    id: String,
    source: PathBuf,
    arm: PathBuf,
    marker: PathBuf,
    release: PathBuf,
}
impl Fixture {
    async fn new(real_git: Option<PathBuf>) -> Self {
        let (temporary, paths, service, _client) = crate::service::tests::start().await;
        let base = temporary.path().canonicalize().unwrap();
        let source = base.join("disposable-service-owned-node-author");
        fs::create_dir_all(source.join("public")).unwrap();
        fs::create_dir(source.join("server")).unwrap();
        fs::write(source.join("public/index.html"), "disposable").unwrap();
        fs::write(
            source.join("server/main.mjs"),
            "throw Error('not started by ownership checks')",
        )
        .unwrap();
        atomic_json(&source.join("local-app.json"),&json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "dataContract":{"id":"disposable.service-owned-node","version":1},
            "app":{"id":"disposable.service-owned-node","name":"可丢弃服务 Node Git 归属","version":"1.0.0"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"root":"server","entry":"main.mjs","nodeMajor":24,"protocolVersion":1}
        })).unwrap();
        let arm = base.join("private-arm");
        let marker = base.join("private-pipe-held");
        let release = base.join("private-holder-release");
        let holder="import os,sys,time\nopen(sys.argv[1],'w').write(str(os.getpid()))\nend=time.monotonic()+18\nwhile not os.path.exists(sys.argv[2]) and time.monotonic()<end: time.sleep(.01)\n";
        let tool = base.join("disposable-owned-git");
        let real = real_git.as_ref().map(|p| p.to_string_lossy().into_owned());
        let script = format!(
            r#"#!/usr/bin/python3
import os,sys,time,subprocess
arm={arm}
marker={marker}
release={release}
real={real}
version='--version' in sys.argv[1:]
if os.path.exists(arm) and ((open(arm).read()=='probe' and version) or (open(arm).read()=='command' and not version)):
 subprocess.Popen(['/usr/bin/python3','-c',{holder},marker,release],start_new_session=True)
 while not os.path.exists(marker): time.sleep(.005)
if real is not None: os.execv(real,[real,*sys.argv[1:]])
if version: print('git version 2.56.0')
else: raise RuntimeError('artificial probe fixture never executes repository commands')
"#,
            arm = serde_json::to_string(&arm).unwrap(),
            marker = serde_json::to_string(&marker).unwrap(),
            release = serde_json::to_string(&release).unwrap(),
            real = real.map_or("None".into(), |v| serde_json::to_string(&v).unwrap()),
            holder = serde_json::to_string(holder).unwrap()
        );
        fs::write(&tool, script).unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o700)).unwrap();
        let service = Arc::new(service);
        let handle = service.handle();
        handle
            .set_git_path(Some(tool.to_string_lossy().into_owned()))
            .await
            .unwrap();
        let apps = Arc::new(
            NodeApps::new(
                paths.data_root().into(),
                "/nonexistent-disposable-runtime".into(),
            )
            .unwrap(),
        );
        let id = apps.register(source.clone()).await.unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        fs::write(
            paths
                .data_root()
                .join("node-apps")
                .join(&id)
                .join("data/retained.txt"),
            b"retained Chinese data: \xe4\xb8\xad\xe6\x96\x87\n",
        )
        .unwrap();
        Self {
            _release: Release(release.clone()),
            _temporary: temporary,
            paths,
            service,
            handle,
            apps,
            id,
            source,
            arm,
            marker,
            release,
        }
    }
    async fn wait_holder(&self) {
        tokio::time::timeout(Duration::from_secs(4), async {
            while !self.marker.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn assert_admission_is_retained(&self) {
        assert!(self.handle.store().has_active_tasks());
        assert_eq!(
            self.handle
                .store()
                .reserve_transient_operation(
                    &format!("node-development:{}", self.id),
                    "late-development"
                )
                .err()
                .unwrap()
                .code,
            "PROJECT_BUSY"
        );
        assert!(self
            .handle
            .node_development_info(&self.apps, &self.id)
            .await
            .is_err());
        assert_eq!(
            self.apps
                .development_source_lease(&self.id)
                .await
                .err()
                .unwrap()
                .code,
            "NODE_DEVELOPMENT_BUSY"
        );
        assert_eq!(
            self.apps.delete_preview(&self.id).await.unwrap_err().code,
            "NODE_DEVELOPMENT_BUSY"
        );
        assert_eq!(
            self.apps.start(&self.id).await.unwrap_err().code,
            "NODE_DEVELOPMENT_BUSY"
        );
    }
    async fn release_and_join(&self) {
        fs::write(
            &self.release,
            b"test holder may close its actual inherited pipes",
        )
        .unwrap();
        let registry = self.handle.store().processes();
        tokio::time::timeout(Duration::from_secs(5), async {
            while registry
                .snapshot()
                .iter()
                .any(|r| r.phase != ProcessPhase::Closed)
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    async fn assert_released(&self) {
        assert!(!self.handle.store().has_active_tasks());
        self.apps
            .development_source_lease(&self.id)
            .await
            .unwrap()
            .validate()
            .unwrap();
        assert_eq!(
            fs::read(
                self.paths
                    .data_root()
                    .join("node-apps")
                    .join(&self.id)
                    .join("data/retained.txt")
            )
            .unwrap(),
            b"retained Chinese data: \xe4\xb8\xad\xe6\x96\x87\n"
        );
        assert!(self.source.join("local-app.json").is_file());
    }
    async fn shutdown(&self) {
        assert!(self.service.quiesce().await.is_complete());
    }
}

#[tokio::test]
async fn service_node_git_probe_unknown_retains_actual_source_task_and_public_migration_waits() {
    let f = Fixture::new(None).await;
    let before = fs::read(f.source.join("local-app.json")).unwrap();
    fs::write(&f.arm, "probe").unwrap();
    let error = f
        .handle
        .node_development_git(&f.apps, &f.id, NodeGitAction::Status)
        .await
        .unwrap_err();
    assert_eq!(error.code, "PROCESS_CLEANUP_PENDING");
    assert_eq!(error.details["processResourceClosed"], false);
    assert_eq!(error.details["operationOutcome"], "unknown");
    f.assert_admission_is_retained().await;
    assert_eq!(
        f.handle.current().active.load(Ordering::Acquire),
        0,
        "process G must not retain service Active"
    );
    assert!(f
        .handle
        .store()
        .processes()
        .snapshot()
        .iter()
        .any(|r| r.component == "git-environment" && r.phase == ProcessPhase::CleanupPending));
    let destination = f
        .paths
        .data_root()
        .parent()
        .unwrap()
        .join("disposable-node-owner-migrated-workspace");
    fs::create_dir(&destination).unwrap();
    let instance = f.service.info().instance_id;
    let task = f
        .handle
        .workspace_migrate(json!({"destinationPath":destination}))
        .await
        .unwrap();
    let task_id = task["taskId"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if crate::workspace::task(&f.paths, task_id)
                .unwrap()
                .unwrap()
                .phase
                == "waiting-for-project-tasks"
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(f.service.info().instance_id, instance);
    assert!(f.handle.store().has_active_tasks());
    assert!(!crate::workspace::task(&f.paths, task_id)
        .unwrap()
        .unwrap()
        .status
        .is_final());
    f.release_and_join().await;
    tokio::time::timeout(Duration::from_secs(7), async {
        while f.service.info().instance_id == instance
            || f.service.shared.migration_active.load(Ordering::Acquire)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        crate::workspace::task(&f.paths, task_id)
            .unwrap()
            .unwrap()
            .status,
        TaskStatus::Succeeded
    );
    f.assert_released().await;
    assert_eq!(fs::read(f.source.join("local-app.json")).unwrap(), before);
    f.shutdown().await;
}

#[tokio::test]
async fn service_node_git_lost_native_reply_does_not_put_global_io_inside_process_owner() {
    let f = Fixture::new(None).await;
    fs::write(&f.arm, "probe").unwrap();
    let io = f.handle.io_lifetime();
    let lease = io.admit().unwrap();
    let handle = f.handle.clone();
    let apps = f.apps.clone();
    let id = f.id.clone();
    let caller = tokio::spawn(lease.run(async move {
        handle
            .node_development_git(&apps, &id, NodeGitAction::Status)
            .await
    }));
    f.wait_holder().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(5), io.drain())
        .await
        .expect(
            "actual native request ends at its pending receipt; process G cannot hold global IO",
        );
    f.assert_admission_is_retained().await;
    assert!(f
        .handle
        .store()
        .processes()
        .snapshot()
        .iter()
        .any(|r| r.component == "git-environment" && r.phase == ProcessPhase::CleanupPending));
    f.release_and_join().await;
    f.assert_released().await;
    f.shutdown().await;
}

#[tokio::test]
#[ignore = "explicit packaged Git, disposable RuntimeService/NodeApps and inherited-pipe fault fixture"]
async fn service_node_git_real_commands_pending_and_caller_loss_keep_actual_scope_until_join() {
    let git = std::env::var_os("LOCAL_PROJECTS_TEST_GIT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../apps/desktop/src-tauri/resources/development-git")
        })
        .join("bin/git")
        .canonicalize()
        .unwrap();
    for abort in [false, true] {
        let f = Fixture::new(Some(git.clone())).await;
        let status = f
            .handle
            .node_development_git(&f.apps, &f.id, NodeGitAction::Status)
            .await
            .unwrap();
        f.handle
            .node_development_git(
                &f.apps,
                &f.id,
                NodeGitAction::Initialize(crate::project_git::GitInitInput {
                    expected_snapshot: status["snapshotDigest"].as_str().unwrap().into(),
                    initial_branch: "main".into(),
                }),
            )
            .await
            .unwrap();
        assert!(f.source.join(".git/HEAD").is_file());
        fs::write(&f.arm, "command").unwrap();
        let handle = f.handle.clone();
        let apps = f.apps.clone();
        let id = f.id.clone();
        let caller = tokio::spawn(async move {
            handle
                .node_development_git(&apps, &id, NodeGitAction::Status)
                .await
        });
        f.wait_holder().await;
        if abort {
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
        } else {
            let e = caller.await.unwrap().unwrap_err();
            assert_eq!(e.code, "PROCESS_CLEANUP_PENDING");
            assert_eq!(e.details["processResourceClosed"], false);
            assert_eq!(e.details["operationOutcome"], "unknown");
        }
        f.assert_admission_is_retained().await;
        assert!(f
            .handle
            .store()
            .processes()
            .snapshot()
            .iter()
            .any(|r| r.component == "project-git"
                && matches!(
                    r.phase,
                    ProcessPhase::Running | ProcessPhase::Stopping | ProcessPhase::CleanupPending
                )));
        f.release_and_join().await;
        f.assert_released().await;
        f.shutdown().await;
    }
}

#[tokio::test]
#[ignore = "explicit packaged Git and actual service lease, deterministic post-receipt stopping boundary"]
async fn service_node_git_post_validation_preserves_actual_commit_receipt_without_guessing_read_outcomes(
) {
    let git_root = std::env::var_os("LOCAL_PROJECTS_TEST_GIT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../apps/desktop/src-tauri/resources/development-git")
        });
    let f = Fixture::new(Some(git_root.join("bin/git").canonicalize().unwrap())).await;
    let lease = f
        .handle
        .node_development_lease(&f.apps, &f.id, false)
        .await
        .unwrap();
    let git = crate::node_development_git::NodeGit::new(lease.active.0.git_environment.clone())
        .with_stop(lease.active.0.stop.subscribe())
        .with_process_scope(lease.active.0.store.processes(), lease.keep_alive());
    let status = git.status(&lease.source.source_root).await.unwrap();
    let read_receipt = json!(status);
    let committed = git
        .initialize(
            &lease.source.source_root,
            crate::project_git::GitInitInput {
                expected_snapshot: read_receipt["snapshotDigest"].as_str().unwrap().into(),
                initial_branch: "main".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(committed["operationOutcome"], "committed");
    assert!(f.source.join(".git/HEAD").is_file());
    assert!(f
        .handle
        .store()
        .processes()
        .snapshot()
        .iter()
        .any(|r| r.component == "project-git" && r.phase == ProcessPhase::Closed));
    struct Resume(Arc<HttpState>);
    impl Drop for Resume {
        fn drop(&mut self) {
            self.0.stopping.store(false, Ordering::Release);
        }
    }
    let state = lease.active.0.clone();
    let resume = Resume(state.clone());
    state.stopping.store(true, Ordering::Release);
    let error = finalize_node_git_result(&lease, committed.clone()).unwrap_err();
    assert_eq!(error.code, "SERVICE_STOPPING");
    assert_eq!(error.details["operationOutcome"], "committed");
    assert_eq!(error.details["result"], committed);
    let read_error = finalize_node_git_result(&lease, read_receipt.clone()).unwrap_err();
    assert_eq!(read_error.code, "SERVICE_STOPPING");
    assert!(
        read_error.details["operationOutcome"].is_null(),
        "a read receipt is never a guessed commit"
    );
    assert_eq!(read_error.details["result"], read_receipt);
    drop(resume);
    drop(git);
    drop(lease);
    f.assert_released().await;
    f.shutdown().await;
}
