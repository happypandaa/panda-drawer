#[cfg(windows)]
use super::group::ProcessGroup;
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::AsyncReadExt;

struct Evidence(Arc<AtomicBool>);
impl Drop for Evidence {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn options() -> ProcessOptions {
    ProcessOptions {
        component: "disposable-test".into(),
        work_timeout: Duration::from_secs(3),
        cleanup_timeout: Duration::from_millis(100),
        terminate_grace: Duration::from_millis(20),
    }
}
fn command(program: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command.args(args);
    command
}
fn io(pipes: ProcessPipes) -> IoPlan<Vec<u8>> {
    let ProcessPipes {
        stdin,
        mut stdout,
        mut stderr,
        stop: _,
    } = pipes;
    drop(stdin);
    IoPlan::new(async move {
        let mut bytes = vec![];
        stdout.read_to_end(&mut bytes).await?;
        Ok(bytes)
    })
    .drain("stderr", async move {
        let mut bytes = [0u8; 1024];
        while stderr.read(&mut bytes).await? != 0 {}
        Ok(())
    })
}
async fn phase(registry: &OwnedProcessRegistry, phase: ProcessPhase) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !registry.snapshot().iter().any(|s| s.phase == phase) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "owned driver did not reach {phase:?}: {:?}",
            registry.snapshot()
        )
    });
}
async fn shutdown(registry: &OwnedProcessRegistry) -> Vec<ShutdownIssue> {
    tokio::time::timeout(Duration::from_secs(5), registry.shutdown())
        .await
        .expect("bounded registry observation")
}

#[tokio::test]
async fn owned_process_normal_output_requires_real_reap_group_and_all_joins() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let registry = OwnedProcessRegistry::default();
    let closed = Arc::new(AtomicBool::new(false));
    let run = registry
        .start(
            command("/usr/bin/printf", &["disposable UTF8 留言"]),
            options(),
            Evidence(closed.clone()),
            io,
        )
        .unwrap();
    let result = run.await.unwrap();
    assert!(result.status.success());
    assert_eq!(result.value, "disposable UTF8 留言".as_bytes());
    assert!(closed.load(Ordering::Acquire));
    assert!(shutdown(&registry).await.is_empty());
    assert!(registry
        .snapshot()
        .iter()
        .all(ProcessSnapshot::resource_closed));
}

#[tokio::test]
async fn owned_process_raw_stdin_shutdown_delivers_real_eof_and_exact_stdout() {
    if !cfg!(target_os = "macos") {
        return;
    }
    use tokio::io::AsyncWriteExt;
    let registry = OwnedProcessRegistry::default();
    let result = registry
        .start(command("/bin/cat", &[]), options(), (), |pipes| {
            let ProcessPipes {
                mut stdin,
                mut stdout,
                mut stderr,
                stop: _,
            } = pipes;
            IoPlan::new(async move {
                stdin
                    .write_all("交互 stdin\n完整 UTF8\n".as_bytes())
                    .await?;
                stdin.shutdown().await?;
                drop(stdin);
                let mut bytes = vec![];
                stdout.read_to_end(&mut bytes).await?;
                Ok(bytes)
            })
            .drain("stderr", async move {
                let mut bytes = [0u8; 1024];
                while stderr.read(&mut bytes).await? != 0 {}
                Ok(())
            })
        })
        .unwrap()
        .await
        .unwrap();
    assert!(result.status.success());
    assert_eq!(result.value, "交互 stdin\n完整 UTF8\n".as_bytes());
    assert!(shutdown(&registry).await.is_empty());
    assert!(registry.snapshot()[0].resource_closed());
}

#[tokio::test]
async fn owned_process_stdout_consumer_early_return_still_drains_actual_large_writer() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let registry = OwnedProcessRegistry::default();
    let result = registry.start(command("/usr/bin/python3", &["-c", "import sys\nfor _ in range(64): sys.stdout.buffer.write(b'x'*65536)\nsys.stdout.flush()"]), options(), (), |pipes| {
        let ProcessPipes { stdin, mut stdout, mut stderr, stop: _ } = pipes;
        drop(stdin);
        IoPlan::new(async move { let mut first = [0u8]; stdout.read_exact(&mut first).await?; Ok(first) })
            .drain("stderr", async move { let mut bytes = [0u8; 1024]; while stderr.read(&mut bytes).await? != 0 {} Ok(()) })
    }).unwrap().await.unwrap();
    assert!(
        result.status.success(),
        "consumer did not cause BrokenPipe in the real writer"
    );
    assert_eq!(result.value, [b'x']);
    assert!(shutdown(&registry).await.is_empty());
}

#[tokio::test]
async fn owned_process_nonzero_exit_is_not_a_resource_shutdown_failure() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let registry = OwnedProcessRegistry::default();
    let run = registry
        .start(command("/usr/bin/false", &[]), options(), (), io)
        .unwrap();
    assert!(!run.await.unwrap().status.success());
    assert!(shutdown(&registry).await.is_empty());
    assert!(registry.snapshot()[0].resource_closed());
}

#[tokio::test]
async fn owned_process_stops_term_ignoring_ordinary_descendant_after_leader_exit() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let directory = Arc::new(
        tempfile::Builder::new()
            .prefix("disposable-owned-process-fork-")
            .tempdir()
            .unwrap(),
    );
    let marker = directory.path().join("child.pid");
    let script = r#"import os,signal,sys,time
path=sys.argv[1]
pid=os.fork()
if pid==0:
 signal.signal(signal.SIGTERM, signal.SIG_IGN)
 os.close(1);os.close(2)
 with open(path,'w') as output: output.write(str(os.getpid()))
 while True: time.sleep(1)
while not os.path.exists(path): time.sleep(.005)
print('parent completed', flush=True)
os._exit(0)
"#;
    let registry = OwnedProcessRegistry::default();
    let mut unrelated = command("/bin/sleep", &["30"]);
    #[cfg(unix)]
    unrelated.process_group(0);
    let mut unrelated = unrelated.kill_on_drop(true).spawn().unwrap();
    let result = registry
        .start(
            command(
                "/usr/bin/python3",
                &["-c", script, marker.to_str().unwrap()],
            ),
            options(),
            directory.clone(),
            io,
        )
        .unwrap()
        .await
        .unwrap();
    assert!(result.status.success());
    assert_eq!(result.value, b"parent completed\n");
    assert!(marker.is_file());
    assert!(
        unrelated.try_wait().unwrap().is_none(),
        "unrelated process must survive"
    );
    assert!(shutdown(&registry).await.is_empty());
    assert!(registry.snapshot()[0].resource_closed());
    unrelated.kill().await.unwrap();
    unrelated.wait().await.unwrap();
}

#[tokio::test]
async fn owned_process_pending_reader_keeps_directory_and_lease_after_waiter_drop() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let registry = OwnedProcessRegistry::default();
    let dropped = Arc::new(AtomicBool::new(false));
    let (release, wait) = oneshot::channel::<()>();
    let run = registry
        .start(
            command("/usr/bin/true", &[]),
            options(),
            Evidence(dropped.clone()),
            |pipes| {
                io(pipes).drain("controlled-file-reader", async move {
                    let _ = wait.await;
                    Ok(())
                })
            },
        )
        .unwrap();
    drop(run);
    phase(&registry, ProcessPhase::CleanupPending).await;
    assert!(!dropped.load(Ordering::Acquire));
    let issues = shutdown(&registry).await;
    assert!(issues
        .iter()
        .any(|i| i.code == "PROCESS_CLEANUP_PENDING" && i.retryable && !i.resource_closed));
    release.send(()).unwrap();
    phase(&registry, ProcessPhase::Closed).await;
    assert!(shutdown(&registry).await.is_empty());
    let snapshot = &registry.snapshot()[0];
    assert!(snapshot.resource_closed());
    assert_eq!(snapshot.resolved_issues.len(), 1);
    assert!(dropped.load(Ordering::Acquire));
}

#[tokio::test]
async fn owned_process_cleanup_timeout_delivers_error_but_never_drops_reader_owner() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let registry = OwnedProcessRegistry::default();
    let dropped = Arc::new(AtomicBool::new(false));
    let (release, wait) = oneshot::channel::<()>();
    let mut run = registry
        .start(
            command("/usr/bin/true", &[]),
            options(),
            Evidence(dropped.clone()),
            |pipes| {
                io(pipes).drain("held-reader", async move {
                    let _ = wait.await;
                    Ok(())
                })
            },
        )
        .unwrap();
    assert_eq!(
        (&mut run).await.unwrap_err().code,
        "PROCESS_CLEANUP_PENDING"
    );
    assert!(
        !run.resource_closed().await,
        "a pending actual reader is not a closure receipt"
    );
    assert!(!dropped.load(Ordering::Acquire));
    assert!(!shutdown(&registry).await.is_empty());
    release.send(()).unwrap();
    phase(&registry, ProcessPhase::Closed).await;
    assert!(shutdown(&registry).await.is_empty());
    assert!(dropped.load(Ordering::Acquire));
    assert!(run.resource_closed().await);
}

#[tokio::test]
async fn owned_process_work_timeout_drains_real_child_before_final_error() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let registry = OwnedProcessRegistry::default();
    let mut options = options();
    options.work_timeout = Duration::from_millis(20);
    let run = registry
        .start(command("/bin/sleep", &["30"]), options, (), io)
        .unwrap();
    assert_eq!(run.await.unwrap_err().code, "PROCESS_WORK_TIMEOUT");
    assert!(shutdown(&registry).await.is_empty());
    assert!(registry.snapshot()[0].resource_closed());
}

#[tokio::test]
async fn owned_process_reader_panic_remains_sticky_after_later_success() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let registry = OwnedProcessRegistry::default();
    let mut run = registry
        .start(command("/bin/sleep", &["30"]), options(), (), |pipes| {
            io(pipes).drain("controlled-panic", async {
                panic!("intentional disposable reader panic")
            })
        })
        .unwrap();
    assert_eq!(
        (&mut run).await.unwrap_err().code,
        "PROCESS_DRAIN_UNCONFIRMED"
    );
    assert!(
        !run.resource_closed().await,
        "a failed actual reader stays unconfirmed"
    );
    let success = registry
        .start(command("/usr/bin/true", &[]), options(), (), io)
        .unwrap()
        .await
        .unwrap();
    assert!(success.status.success());
    let issues = shutdown(&registry).await;
    assert!(issues
        .iter()
        .any(|i| i.code == "PROCESS_IO_JOIN_FAILED" && !i.retryable));
    assert!(registry
        .snapshot()
        .iter()
        .any(|s| s.phase == ProcessPhase::Unconfirmed && !s.resource_closed()));
    assert_eq!(
        shutdown(&registry).await,
        issues,
        "None handles never erase failure"
    );
}

#[tokio::test]
async fn owned_process_builder_panic_keeps_the_already_registered_child() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let registry = OwnedProcessRegistry::default();
    let dropped = Arc::new(AtomicBool::new(false));
    let run = registry
        .start::<(), _>(
            command("/bin/sleep", &["30"]),
            options(),
            Evidence(dropped.clone()),
            |_| panic!("controlled builder panic"),
        )
        .unwrap();
    assert!(run.await.is_err());
    assert!(shutdown(&registry)
        .await
        .iter()
        .any(|i| i.code == "PROCESS_IO_BUILDER_PANICKED"));
    assert!(registry.snapshot()[0].leader_reaped);
    assert!(
        registry.snapshot()[0].io_joined,
        "actual pipe pumps were all joined despite factory panic"
    );
    assert!(
        !dropped.load(Ordering::Acquire),
        "sticky failure retains the keepalive"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn owned_process_submit_racing_seal_registers_before_seal_or_rejects() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let registry = OwnedProcessRegistry::default();
    let (entered, admitted) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    registry.0.state.lock().unwrap().submission_pause = Some((entered, released));
    let submitted = {
        let registry = registry.clone();
        tokio::task::spawn_blocking(move || {
            registry.start(command("/bin/sleep", &["30"]), options(), (), io)
        })
    };
    admitted.recv_timeout(Duration::from_secs(2)).unwrap();
    let (sealing, requested) = std::sync::mpsc::channel();
    let (sealed, sealed_reply) = std::sync::mpsc::channel();
    let stopping = {
        let registry = registry.clone();
        tokio::task::spawn_blocking(move || {
            sealing.send(()).unwrap();
            registry.seal();
            sealed.send(()).unwrap();
        })
    };
    requested.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        sealed_reply.recv_timeout(Duration::from_millis(20)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    release.send(()).unwrap();
    let run = submitted.await.unwrap().unwrap();
    stopping.await.unwrap();
    assert!(run.await.is_err());
    assert!(shutdown(&registry).await.is_empty());
    assert_eq!(registry.snapshot().len(), 1);
    assert!(registry.snapshot()[0].resource_closed());
    assert_eq!(
        registry
            .start(command("/usr/bin/true", &[]), options(), (), io)
            .err()
            .unwrap()
            .code,
        "SERVICE_STOPPING"
    );
}

#[test]
fn owned_process_no_executor_rejects_before_spawn() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let registry = OwnedProcessRegistry::default();
    assert_eq!(
        registry
            .start(command("/bin/sleep", &["30"]), options(), (), io)
            .err()
            .unwrap()
            .code,
        "PROCESS_EXECUTOR_UNAVAILABLE"
    );
    assert!(registry.snapshot().is_empty());
}

#[test]
fn owned_process_runtime_without_io_rejects_before_child_or_factory_exists() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let _entered = runtime.enter();
    let registry = OwnedProcessRegistry::default();
    let factory = Arc::new(AtomicBool::new(false));
    let factory_copy = factory.clone();
    let error = registry
        .start(command("/usr/bin/true", &[]), options(), (), move |pipes| {
            factory_copy.store(true, Ordering::Release);
            io(pipes)
        })
        .err()
        .unwrap();
    assert_eq!(error.code, "PROCESS_EXECUTOR_UNAVAILABLE");
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert!(!factory.load(Ordering::Acquire));
    assert!(registry.snapshot().is_empty());
}

#[test]
fn owned_process_dead_runtime_rejects_before_child_or_factory_exists() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let handle = runtime.handle().clone();
    drop(runtime);
    let _entered = handle.enter();
    let registry = OwnedProcessRegistry::default();
    let error = registry
        .start(command("/usr/bin/true", &[]), options(), (), io)
        .err()
        .unwrap();
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert!(registry.snapshot().is_empty());
}

#[tokio::test]
async fn owned_process_reader_error_broadcasts_stop_but_joins_every_independent_owner() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let registry = OwnedProcessRegistry::default();
    let stopped = Arc::new(AtomicBool::new(false));
    let stopped_copy = stopped.clone();
    let run = registry
        .start(
            command("/bin/sleep", &["30"]),
            options(),
            (),
            move |pipes| {
                let mut stop = pipes.stop.clone();
                io(pipes)
                    .drain("ordinary-read-error", async {
                        Err(ApiError::new(
                            "DISPOSABLE_READ_ERROR",
                            "controlled ordinary read error",
                        ))
                    })
                    .drain("cooperative-session", async move {
                        while !*stop.borrow_and_update() {
                            stop.changed()
                                .await
                                .map_err(|_| failure("STOP_LOST", "stop signal missing"))?;
                        }
                        stopped_copy.store(true, Ordering::Release);
                        Ok(())
                    })
            },
        )
        .unwrap();
    assert_eq!(run.await.unwrap_err().code, "DISPOSABLE_READ_ERROR");
    assert!(stopped.load(Ordering::Acquire));
    assert!(shutdown(&registry).await.is_empty());
    assert!(registry.snapshot()[0].resource_closed());
}

#[test]
fn owned_process_changed_member_scan_cannot_hide_a_new_grandchild() {
    use group::{scan_members, Member};
    assert!(
        scan_members(10, &[10, 11], |_| Ok(Member::Changed)).unwrap(),
        "vanished/reused member needs another scan"
    );
    assert!(
        scan_members(10, &[10, 12], |_| Ok(Member::Live)).unwrap(),
        "new ordinary grandchild must drain"
    );
    assert!(!scan_members(10, &[10, 12], |_| Ok(Member::Zombie)).unwrap());
    assert!(!scan_members(10, &[10], |_| panic!("leader remains separately pinned")).unwrap());
    for count in [-1, 0, 1, 16384, 16388] {
        assert_eq!(
            group::validate_count(count, 16384).unwrap_err().code,
            "PROCESS_GROUP_UNCONFIRMED"
        );
    }
    group::validate_count(4, 16384).unwrap();
    assert!(
        !group::stable_zombies(&[(11, 1, 1)], &[(11, 1, 1), (12, 1, 2)]),
        "new fork requires another complete pass"
    );
    assert!(
        !group::stable_zombies(&[(11, 1, 1)], &[(11, 2, 1)]),
        "reused PID is a different member"
    );
    assert!(group::stable_zombies(&[(11, 1, 1)], &[(11, 1, 1)]));
    for malformed in [vec![], vec![11], vec![10, 10], vec![10, 0], vec![10, -1]] {
        assert_eq!(
            scan_members(10, &malformed, |_| Ok(Member::Zombie))
                .unwrap_err()
                .code,
            "PROCESS_GROUP_UNCONFIRMED"
        );
    }
}

#[tokio::test]
async fn owned_process_waiter_drop_while_grandchild_writes_keeps_real_directory_until_drained() {
    if !cfg!(target_os = "macos") {
        return;
    }
    for _ in 0..6 {
        let registry = OwnedProcessRegistry::default();
        let directory = Arc::new(
            tempfile::Builder::new()
                .prefix("disposable-owned-grandchild-")
                .tempdir()
                .unwrap(),
        );
        let marker = directory.path().join("writer");
        let script = r#"import os,signal,sys,time
path=sys.argv[1]
pid=os.fork()
if pid==0:
 pid=os.fork()
 if pid!=0: os._exit(0)
 signal.signal(signal.SIGTERM,signal.SIG_IGN)
 with open(path,'ab',buffering=0) as output:
  while True:
   output.write(b'ordinary-grandchild\n');os.write(1,b'data\n');time.sleep(.001)
while not os.path.exists(path): time.sleep(.001)
os._exit(0)
"#;
        let (release, blocked) = oneshot::channel();
        let run = registry
            .start(
                command(
                    "/usr/bin/python3",
                    &["-c", script, marker.to_str().unwrap()],
                ),
                options(),
                directory.clone(),
                |pipes| {
                    io(pipes).drain("held-owned-file", async move {
                        let _ = blocked.await;
                        Ok(())
                    })
                },
            )
            .unwrap();
        // Wait for the parent's real exit observation while a same-group
        // grandchild was writing; no wall-clock sleep substitutes that proof.
        tokio::time::timeout(Duration::from_secs(3), async {
            while !marker.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        drop(directory);
        drop(run);
        phase(&registry, ProcessPhase::CleanupPending).await;
        assert!(
            marker.is_file(),
            "keepalive owns the actual TempDir after waiter loss"
        );
        assert!(registry.snapshot()[0].leader_reaped);
        release.send(()).unwrap();
        phase(&registry, ProcessPhase::Closed).await;
        assert!(shutdown(&registry).await.is_empty());
        assert!(
            !marker.exists(),
            "TempDir is released only after true child/reader completion"
        );
    }
}

#[tokio::test]
async fn owned_process_seal_rejects_spawn_without_creating_any_child() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let registry = OwnedProcessRegistry::default();
    registry.seal();
    let error = registry
        .start(command("/usr/bin/true", &[]), options(), (), io)
        .err()
        .unwrap();
    assert_eq!(error.code, "SERVICE_STOPPING");
    assert!(registry.snapshot().is_empty());
    assert!(shutdown(&registry).await.is_empty());
}

#[tokio::test]
async fn owned_process_lost_unreaped_ownership_never_authorizes_a_signal() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let mut command = command("/usr/bin/true", &[]);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn().unwrap();
    let mut group = ProcessGroup::new(child.id().unwrap());
    child.wait().await.unwrap();
    assert_eq!(group.observe().unwrap_err().code, "PROCESS_OWNERSHIP_LOST");
    assert_eq!(
        group.signal(group::KILL).unwrap_err().code,
        "PROCESS_OWNERSHIP_RELEASED"
    );
}
