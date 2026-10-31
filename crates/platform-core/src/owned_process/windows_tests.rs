//! Real Windows process/pipe/JoinHandle tests. Every directory is disposable;
//! the fixture binary never discovers or starts an application runtime.
use super::*;
use std::{
    io::{Read, Write},
    path::Path,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn options() -> ProcessOptions {
    ProcessOptions {
        component: "windows-disposable-owned-process".into(),
        work_timeout: Duration::from_secs(5),
        cleanup_timeout: Duration::from_millis(100),
        terminate_grace: Duration::from_millis(20),
    }
}

fn command(mode: &str, directory: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            "owned_process::windows_tests::child_fixture",
            "--nocapture",
        ])
        // A piped libtest fixture has no terminal. Keep its startup from
        // searching host terminfo locations before entering the fixture.
        // This is test-only; product environment inheritance is intact.
        .env_remove("TERM")
        .current_dir(directory)
        .env("PANDA_OWNED_FIXTURE", mode)
        .env("PANDA_OWNED_TRACE", directory.join("fixture-progress"))
        .env("HOME", directory)
        .env("USERPROFILE", directory)
        .env("APPDATA", directory)
        .env("LOCALAPPDATA", directory)
        .env("TEMP", directory)
        .env("TMP", directory);
    command
}

fn collect(pipes: ProcessPipes) -> IoPlan<Vec<u8>> {
    let ProcessPipes {
        stdin,
        mut stdout,
        mut stderr,
        ..
    } = pipes;
    drop(stdin);
    IoPlan::new(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await?;
        Ok(bytes)
    })
    .drain("independent-stderr", async move {
        tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await?;
        Ok(())
    })
}

async fn closed(registry: &OwnedProcessRegistry) {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let _ = registry.shutdown().await;
            if registry
                .snapshot()
                .iter()
                .all(ProcessSnapshot::resource_closed)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("real Windows root, Job, I/O and driver closure");
}

#[tokio::test]
async fn windows_created_setup_failure_retires_only_after_actual_job_pipes_and_driver_close() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let owner_root = tempfile::Builder::new()
        .prefix("panda-disposable-setup-writer-owner-")
        .tempdir()
        .unwrap();
    let writer = crate::durable_writer::WriterOwner::platform(owner_root.path()).unwrap();
    let registry = OwnedProcessRegistry::default().with_writer_owner(writer.clone());
    let source = tempfile::Builder::new()
        .prefix("panda-disposable-setup-kept-source-")
        .tempdir()
        .unwrap();
    let source_path = source.path().to_path_buf();
    let factory_called = Arc::new(AtomicBool::new(false));
    let factory_flag = factory_called.clone();
    windows::fail_next_setup_for_fixture();
    let mut run = registry
        .start(
            OwnedCommand::inherited(command("echo", &source_path)),
            options(),
            source,
            move |pipes| {
                factory_flag.store(true, Ordering::SeqCst);
                collect(pipes)
            },
        )
        .unwrap();
    // start is synchronous on this current-thread runtime: the actual root is
    // still suspended, already registered, and its durable admission exists.
    assert!(source_path.exists());
    assert!(run.snapshot().pid > 0);
    assert!(!run.snapshot().resource_closed());
    assert!(!source_path.join("fixture-progress").exists());
    let writers = owner_root.path().join("platform-writers/writers");
    assert_eq!(std::fs::read_dir(&writers).unwrap().count(), 1);
    let error = (&mut run).await.unwrap_err();
    assert_eq!(error.code, "PROCESS_FIXTURE_SETUP_FAILURE");
    assert!(!factory_called.load(Ordering::SeqCst));
    closed(&registry).await;
    assert!(run.snapshot().resource_closed());
    assert!(run
        .snapshot()
        .resolved_issues
        .iter()
        .any(|issue| issue.code == "PROCESS_FIXTURE_SETUP_FAILURE"));
    assert!(!source_path.exists());
    assert_eq!(std::fs::read_dir(&writers).unwrap().count(), 0);
    crate::durable_writer::require_owner_clear(&writer).unwrap();
}

#[test]
#[ignore = "entry point for disposable Windows core child fixtures"]
fn child_fixture() {
    fn trace(value: &str) {
        if let Some(path) = std::env::var_os("PANDA_OWNED_TRACE") {
            std::fs::write(path, value).unwrap();
        }
    }
    trace("fixture-entered");
    match std::env::var("PANDA_OWNED_FIXTURE").unwrap().as_str() {
        "echo" => {
            let mut bytes = Vec::new();
            std::io::stdin().read_to_end(&mut bytes).unwrap();
            std::io::stdout().write_all(&bytes).unwrap();
            std::io::stdout().flush().unwrap();
            std::process::exit(0);
        }
        "streams" => {
            trace("streams-before-thread");
            let error = std::thread::spawn(|| {
                std::io::stderr()
                    .write_all(&vec![b'e'; 4 * 1024 * 1024])
                    .unwrap();
            });
            trace("streams-before-stdout");
            std::io::stdout()
                .write_all(&vec![b'o'; 4 * 1024 * 1024])
                .unwrap();
            trace("streams-before-join");
            error.join().unwrap();
            trace("streams-before-exit");
            std::process::exit(0);
        }
        "env" => {
            trace("env-before-output");
            println!("PATH_PRESENT:{}", std::env::var_os("PATH").is_some());
            println!(
                "CASE_VALUE:{}",
                std::env::var("PANDA_CASE").unwrap_or_default()
            );
            trace("env-before-exit");
            std::process::exit(0);
        }
        "exit259" => std::process::exit(259),
        "exit-before-input" => std::process::exit(0),
        "sleep" => loop {
            std::thread::sleep(Duration::from_secs(60));
        },
        _ => panic!("unknown disposable Windows core fixture"),
    }
}

#[tokio::test]
async fn windows_real_stdin_eof_joins_all_owners_before_releasing_tempdir() {
    let root = tempfile::tempdir().unwrap();
    let lease = tempfile::Builder::new()
        .prefix("真实-中文 space-")
        .tempdir_in(root.path())
        .unwrap();
    let path = lease.path().canonicalize().unwrap();
    let registry = OwnedProcessRegistry::default();
    let run = registry
        .start(
            OwnedCommand::inherited(command("echo", &path)),
            options(),
            lease,
            |pipes| {
                let ProcessPipes {
                    mut stdin,
                    mut stdout,
                    mut stderr,
                    ..
                } = pipes;
                IoPlan::new(async move {
                    stdin.write_all("完整 UTF8 输入\n".as_bytes()).await?;
                    stdin.shutdown().await?;
                    drop(stdin);
                    let mut bytes = Vec::new();
                    stdout.read_to_end(&mut bytes).await?;
                    Ok(bytes)
                })
                .drain("stderr", async move {
                    tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await?;
                    Ok(())
                })
            },
        )
        .unwrap();
    let output = run.await.unwrap();
    assert!(output.status.success());
    assert!(output.value.ends_with("完整 UTF8 输入\n".as_bytes()));
    closed(&registry).await;
    assert!(!path.exists());
}

#[tokio::test]
async fn windows_real_closed_child_stdin_still_joins_registry_workers_and_driver() {
    let lease = tempfile::Builder::new()
        .prefix("panda-disposable-registry-early-stdin-")
        .tempdir()
        .unwrap();
    let path = lease.path().canonicalize().unwrap();
    let registry = OwnedProcessRegistry::default();
    let run = registry
        .start(
            OwnedCommand::inherited(command("exit-before-input", &path)),
            options(),
            lease,
            |pipes| {
                let ProcessPipes {
                    mut stdin,
                    mut stdout,
                    mut stderr,
                    ..
                } = pipes;
                IoPlan::new(async move {
                    let mut bytes = Vec::new();
                    // Actual process exit closes stdout and the inherited stdin.
                    stdout.read_to_end(&mut bytes).await?;
                    let error = stdin
                        .write_all(&vec![b'x'; 256 * 1024])
                        .await
                        .expect_err("the actual child input is gone");
                    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
                    drop(stdin);
                    Ok(bytes)
                })
                .drain("early-exit-stderr", async move {
                    tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await?;
                    Ok(())
                })
            },
        )
        .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success());
    closed(&registry).await;
    let states = registry.snapshot();
    assert!(!states.is_empty());
    assert!(states.iter().all(ProcessSnapshot::resource_closed));
    assert!(registry.shutdown().await.is_empty());
    assert!(!path.exists());
}

#[tokio::test]
async fn windows_consumer_early_drop_still_drains_real_four_mib_streams() {
    let directory = tempfile::tempdir().unwrap();
    let registry = OwnedProcessRegistry::default();
    let result = registry
        .start(
            OwnedCommand::inherited(command("streams", directory.path())),
            options(),
            (),
            |pipes| {
                let ProcessPipes {
                    stdin,
                    stdout,
                    mut stderr,
                    ..
                } = pipes;
                drop(stdin);
                drop(stdout);
                IoPlan::new(async { Ok(()) }).drain("stderr", async move {
                    let mut bytes = Vec::new();
                    stderr.read_to_end(&mut bytes).await?;
                    assert_eq!(bytes.len(), 4 * 1024 * 1024);
                    assert!(bytes.iter().all(|byte| *byte == b'e'));
                    Ok(())
                })
            },
        )
        .unwrap()
        .await
        .unwrap();
    assert!(result.status.success());
    closed(&registry).await;
}

#[tokio::test]
async fn windows_pending_reader_preserves_actual_tempdir_after_waiter_drop() {
    let root = tempfile::tempdir().unwrap();
    let lease = tempfile::Builder::new()
        .prefix("pending-owned-")
        .tempdir_in(root.path())
        .unwrap();
    let path = lease.path().canonicalize().unwrap();
    let completion = root.path().join("actual-reader-completion");
    let completed = completion.clone();
    let (entered, entry) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let writer =
        crate::durable_writer::WriterOwner::platform(&root.path().canonicalize().unwrap()).unwrap();
    let registry = OwnedProcessRegistry::default().with_writer_owner(writer.clone());
    let mut run = registry
        .start(
            OwnedCommand::inherited(command("echo", &path)),
            options(),
            lease,
            move |pipes| {
                let ProcessPipes {
                    stdin,
                    mut stdout,
                    mut stderr,
                    ..
                } = pipes;
                drop(stdin);
                IoPlan::new(async move {
                    tokio::io::copy(&mut stdout, &mut tokio::io::sink()).await?;
                    let _ = entered.send(());
                    let _ = released.await;
                    std::fs::write(completed, b"actual read and write completed")?;
                    Ok(())
                })
                .drain("stderr", async move {
                    tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await?;
                    Ok(())
                })
            },
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), entry)
        .await
        .unwrap()
        .unwrap();
    let marker_directory = root.path().join("platform-writers/writers");
    let markers = std::fs::read_dir(&marker_directory)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(markers.len(), 1);
    let marker = markers[0].path();
    let marker_bytes = std::fs::read(&marker).unwrap();
    let marker_identity = platform_native::windows::files::open_no_reparse(&marker)
        .unwrap()
        .identity;
    assert_eq!(
        crate::durable_writer::require_owner_clear(&writer)
            .unwrap_err()
            .code,
        "PROJECT_WRITER_UNCONFIRMED"
    );
    assert_eq!(
        (&mut run).await.unwrap_err().code,
        "PROCESS_CLEANUP_PENDING"
    );
    assert!(path.exists());
    assert_eq!(std::fs::read(&marker).unwrap(), marker_bytes);
    assert!(marker_identity.same_object(
        &platform_native::windows::files::open_no_reparse(&marker)
            .unwrap()
            .identity
    ));
    assert!(!completion.exists());
    assert!(!run.snapshot().resource_closed());
    assert!(!run.snapshot().io_joined);
    drop(run);
    assert!(!registry.shutdown().await.is_empty());
    assert!(path.exists());
    release.send(()).unwrap();
    closed(&registry).await;
    assert!(!marker.exists());
    assert_eq!(std::fs::read_dir(marker_directory).unwrap().count(), 0);
    crate::durable_writer::require_owner_clear(&writer).unwrap();
    assert_eq!(
        std::fs::read(completion).unwrap(),
        b"actual read and write completed"
    );
    assert!(!path.exists());
}

#[tokio::test]
async fn windows_timeout_uses_the_owned_job_and_real_exit_259_is_preserved() {
    let directory = tempfile::tempdir().unwrap();
    let registry = OwnedProcessRegistry::default();
    let mut budget = options();
    budget.work_timeout = Duration::from_millis(30);
    let error = registry
        .start(
            OwnedCommand::inherited(command("sleep", directory.path())),
            budget,
            (),
            collect,
        )
        .unwrap()
        .await
        .unwrap_err();
    assert_eq!(error.code, "PROCESS_WORK_TIMEOUT");
    closed(&registry).await;
    let registry = OwnedProcessRegistry::default();
    let output = registry
        .start(
            OwnedCommand::inherited(command("exit259", directory.path())),
            options(),
            (),
            collect,
        )
        .unwrap()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(259));
    closed(&registry).await;
}

#[tokio::test]
async fn windows_explicit_environment_policy_controls_actual_child_inheritance() {
    let directory = tempfile::tempdir().unwrap();
    let registry = OwnedProcessRegistry::default();
    let mut inherited = command("env", directory.path());
    inherited
        .env("PANDA_CASE", "set")
        .env("panda_case", "replaced");
    let result = registry
        .start(OwnedCommand::inherited(inherited), options(), (), collect)
        .unwrap()
        .await;
    assert!(
        result.is_ok(),
        "actual child progress={:?}, result={:?}, snapshot={:?}",
        std::fs::read_to_string(directory.path().join("fixture-progress")),
        result,
        registry.snapshot()
    );
    let output = result.unwrap();
    let output = String::from_utf8(output.value).unwrap();
    assert!(output.contains("PATH_PRESENT:true"));
    assert!(output.contains("CASE_VALUE:replaced"));
    closed(&registry).await;
    let registry = OwnedProcessRegistry::default();
    let mut cleared = command("env", directory.path());
    cleared.env_clear().env("PANDA_OWNED_FIXTURE", "env");
    let output = registry
        .start(OwnedCommand::cleared(cleared), options(), (), collect)
        .unwrap()
        .await
        .unwrap();
    assert!(String::from_utf8(output.value)
        .unwrap()
        .contains("PATH_PRESENT:false"));
    closed(&registry).await;
}

#[tokio::test]
async fn windows_unspecified_policy_and_sealed_admission_create_no_record() {
    let directory = tempfile::tempdir().unwrap();
    let registry = OwnedProcessRegistry::default();
    let error = registry
        .start(command("echo", directory.path()), options(), (), collect)
        .err()
        .unwrap();
    assert_eq!(error.code, "PROCESS_ENVIRONMENT_UNSPECIFIED");
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert!(registry.snapshot().is_empty());
    registry.seal();
    let error = registry
        .start(
            OwnedCommand::inherited(command("echo", directory.path())),
            options(),
            (),
            collect,
        )
        .err()
        .unwrap();
    assert_eq!(error.code, "SERVICE_STOPPING");
    assert!(registry.snapshot().is_empty());
}

#[test]
fn windows_no_executor_rejects_before_child_and_factory() {
    let directory = tempfile::tempdir().unwrap();
    let registry = OwnedProcessRegistry::default();
    let error = registry
        .start(
            OwnedCommand::inherited(command("echo", directory.path())),
            options(),
            (),
            collect,
        )
        .err()
        .unwrap();
    assert_eq!(error.code, "PROCESS_EXECUTOR_UNAVAILABLE");
    assert!(registry.snapshot().is_empty());
}

#[tokio::test]
#[ignore = "requires PANDA_NATIVE_TEST_NODE pointing to the verified official Windows Node"]
async fn windows_official_node_has_real_registry_pipe_and_driver_receipts() {
    let node = std::env::var_os("PANDA_NATIVE_TEST_NODE")
        .expect("explicit verified official Windows Node is required");
    let lease = tempfile::Builder::new()
        .prefix("panda-core-node-中文 space-")
        .tempdir()
        .unwrap();
    let path = lease.path().canonicalize().unwrap();
    let mut command = Command::new(node);
    command
        .args([
            "-e",
            "process.stdout.write('官方 Node 中文'); process.stderr.write('e'.repeat(200000));",
        ])
        .current_dir(&path);
    let registry = OwnedProcessRegistry::default();
    let output = registry
        .start(
            OwnedCommand::inherited(command),
            options(),
            lease,
            |pipes| {
                let ProcessPipes {
                    stdin,
                    mut stdout,
                    mut stderr,
                    ..
                } = pipes;
                drop(stdin);
                IoPlan::new(async move {
                    let mut bytes = Vec::new();
                    stdout.read_to_end(&mut bytes).await?;
                    Ok(bytes)
                })
                .drain("official-node-stderr", async move {
                    let mut bytes = Vec::new();
                    stderr.read_to_end(&mut bytes).await?;
                    assert_eq!(bytes, vec![b'e'; 200000]);
                    Ok(())
                })
            },
        )
        .unwrap()
        .await
        .unwrap();
    assert_eq!(output.value, "官方 Node 中文".as_bytes());
    assert!(output.status.success());
    closed(&registry).await;
    assert!(!path.exists());
}
