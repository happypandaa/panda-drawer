use super::*;
use crate::projects::CreateProjectInput;

struct ReleasePipe(PathBuf);
impl Drop for ReleasePipe {
    fn drop(&mut self) {
        let _ = fs::write(&self.0, b"close artificial inherited pipe");
    }
}
struct Fixture {
    _temporary: TempDir,
    store: Arc<ProjectStore>,
    project: ProjectHandle,
    context: Option<Context>,
    stop: watch::Sender<bool>,
    _cancel: watch::Sender<bool>,
}
async fn fixture() -> Fixture {
    let (temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
    let created = store
        .create(CreateProjectInput {
            name: "Disposable owned Git process".into(),
            description: None,
        })
        .await
        .unwrap();
    let project = store.project(&created.id).unwrap();
    let operation = Arc::new(
        store
            .reserve_transient_operation(&created.id, "owned-git-fixture")
            .unwrap(),
    );
    let gate = Arc::new(project.operation_gate.clone().read_owned().await);
    let (stop, receiver) = watch::channel(false);
    let (cancel, cancellation) = watch::channel(false);
    let mut context = Context::new(receiver, cancellation).unwrap();
    context.bind(store.processes(), operation);
    context.phase(gate);
    Fixture {
        _temporary: temporary,
        store,
        project,
        context: Some(context),
        stop,
        _cancel: cancel,
    }
}
async fn wait_file(path: &Path) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
async fn closed(store: &ProjectStore) {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let processes = store.processes();
            if processes.shutdown().await.is_empty()
                && processes
                    .snapshot()
                    .iter()
                    .all(|state| state.resource_closed())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned process/session did not complete");
}
fn helper(root: &Path, body: &str) -> Command {
    let file = root.join("synthetic-owned-process.py");
    fs::write(&file, format!("#!/usr/bin/python3\n{body}\n")).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o700)).unwrap();
    Command::new(file)
}
fn holder(root: &Path) -> (String, PathBuf, ReleasePipe) {
    let marker = root.join("inherited-reader-started");
    let release = root.join("release-inherited-reader");
    let child = "import os,sys,time\nopen(sys.argv[1],'w').write(str(os.getpid()))\nend=time.monotonic()+12\nwhile not os.path.exists(sys.argv[2]) and time.monotonic()<end: time.sleep(.01)\n";
    let code = format!("subprocess.Popen(['/usr/bin/python3','-c',{}, {}, {}],start_new_session=True)\nwhile not os.path.exists({}): time.sleep(.01)\n",
        serde_json::to_string(child).unwrap(), serde_json::to_string(&marker).unwrap(), serde_json::to_string(&release).unwrap(), serde_json::to_string(&marker).unwrap());
    (code, marker, ReleasePipe(release))
}

#[tokio::test]
async fn failed_or_dropped_metadata_request_retains_actual_lease_gate_directory_and_index_fd_until_io_join(
) {
    for abort in [false, true] {
        let mut sample = fixture().await;
        let mut context = sample.context.take().unwrap();
        let private = context.root().to_path_buf();
        let lock_path = sample.project.root.join("owned-index.lock");
        let lock = IndexLock::acquire(&lock_path).unwrap();
        context.retain(lock.retained.clone());
        let (holder, marker, release) = holder(&sample.project.root);
        let output = if abort {
            "time.sleep(30)".to_string()
        } else {
            format!("import sys;sys.stdout.buffer.write(b'x'*{});sys.stdout.buffer.flush();time.sleep(30)",MAX_OUTPUT+1)
        };
        let command = helper(
            &sample.project.root,
            &format!("import os,subprocess,time\n{holder}\n{output}"),
        );
        let request = tokio::spawn(async move {
            let _lock = lock;
            context.run(command, vec![]).await
        });
        wait_file(&marker).await;
        if abort {
            request.abort();
            assert!(request.await.err().unwrap().is_cancelled());
        } else {
            let error = tokio::time::timeout(Duration::from_secs(4), request)
                .await
                .unwrap()
                .unwrap()
                .err()
                .unwrap();
            assert_eq!(error.code, "PROCESS_CLEANUP_PENDING");
            assert_eq!(error.details["processClosed"], false);
            assert_eq!(error.details["processResourceClosed"], false);
        }
        assert!(sample.store.has_active_tasks());
        assert!(sample.project.operation_gate.try_write().is_err());
        assert!(private.is_dir());
        assert!(
            lock_path.is_file(),
            "a dropped request removed a still-owned index lock"
        );
        assert!(sample
            .store
            .processes()
            .shutdown()
            .await
            .iter()
            .any(|issue| issue.code == "PROCESS_CLEANUP_PENDING"));
        assert!(lock_path.is_file());
        assert!(sample.project.operation_gate.try_write().is_err());
        drop(release);
        closed(&sample.store).await;
        assert!(!sample.store.has_active_tasks());
        assert!(sample.project.operation_gate.try_write().is_ok());
        assert!(!private.exists());
        assert!(!lock_path.exists());
        assert!(sample.store.shutdown().await.is_empty());
    }
}

#[tokio::test]
async fn commit_publication_and_pending_cleanup_have_independent_receipts() {
    for (action, acknowledge, expected_outcome) in [
        ("abort", false, "unknown"),
        ("commit", false, "unknown"),
        ("commit", true, "committed"),
    ] {
        let mut sample = fixture().await;
        let mut context = sample.context.take().unwrap();
        let private = context.root().to_path_buf();
        let (hold, marker, release) = holder(&sample.project.root);
        let body = format!(
            r#"import os,subprocess,sys,time

def token():
    data=bytearray()
    while True:
        byte=sys.stdin.buffer.read(1)
        if not byte: return bytes(data)
        if byte==b'\0': return bytes(data)
        data.extend(byte)

while token()!=b'prepare': pass
sys.stdout.buffer.write(b'start: ok\nprepare: ok\n');sys.stdout.buffer.flush()
action=token()
if action==b'abort':
    sys.stdout.buffer.write(b'abort: ok\n');sys.stdout.buffer.flush()
{ack}
{hold}
time.sleep(30)
"#,
            ack = if acknowledge {
                "if action==b'commit':\n    sys.stdout.buffer.write(b'commit: ok\\n');sys.stdout.buffer.flush()"
            } else {
                ""
            }
        );
        let command = helper(&sample.project.root, &body);
        let (acknowledged, receipt) = tokio::sync::oneshot::channel();
        if acknowledge {
            context.commit_ack_observed = Some(acknowledged);
        } else {
            drop(acknowledged);
        }
        let abort = action == "abort";
        let request = tokio::spawn(async move {
            context
                .commit_transaction(command, b"start\0prepare\0".to_vec(), move || {
                    if abort {
                        Err(stale())
                    } else {
                        Ok(())
                    }
                })
                .await
        });
        wait_file(&marker).await;
        if acknowledge {
            tokio::time::timeout(Duration::from_secs(2), receipt)
                .await
                .unwrap()
                .unwrap();
        }
        // Keep the leader demonstrably live until this explicit stop. Natural
        // os._exit racing the signal tests a different group transition, not
        // the intended ACK-versus-still-open-reader ownership boundary.
        sample.stop.send(true).unwrap();
        let error = tokio::time::timeout(Duration::from_secs(4), request)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error.code, "PROCESS_CLEANUP_PENDING",
            "action={action}, acknowledge={acknowledge}, error={error:?}"
        );
        assert_eq!(error.details["safeAbort"], false);
        assert_eq!(error.details["commitSent"], action == "commit");
        assert_eq!(error.details["commitAcknowledged"], acknowledge);
        assert_eq!(error.details["operationOutcome"], expected_outcome);
        assert_eq!(error.details["processClosed"], false);
        assert_eq!(error.details["processResourceClosed"], false);
        assert!(sample.store.has_active_tasks());
        assert!(sample.project.operation_gate.try_write().is_err());
        assert!(private.is_dir());
        drop(release);
        closed(&sample.store).await;
        assert!(!sample.store.has_active_tasks());
        assert!(sample.project.operation_gate.try_write().is_ok());
        assert!(!private.exists());
        assert!(sample.store.shutdown().await.is_empty());
    }
}

#[tokio::test]
async fn build_git_capture_uses_the_injected_registry_and_actual_phase_owner() {
    let mut sample = fixture().await;
    let context = sample.context.take().unwrap();
    let scope = context.keep_alive();
    drop(context);
    let git_dir = sample.project.root.join(".git");
    private_dir(&git_dir.join("objects")).unwrap();
    private_dir(&git_dir.join("refs")).unwrap();
    fs::write(git_dir.join("HEAD"), b"ref: refs/heads/main\n").unwrap();
    fs::write(
        git_dir.join("config"),
        b"[core]\nrepositoryformatversion = 0\nbare = false\n",
    )
    .unwrap();
    let (hold, marker, release) = holder(&sample.project.root);
    let body = format!("import os,subprocess,sys,time\nif '--version' in sys.argv:\n print('git version 2.56.0');sys.exit(0)\nif 'config' in sys.argv: sys.exit(1)\nif 'ls-files' in sys.argv:\n{}\n",hold.lines().map(|line|format!(" {line}")).collect::<Vec<_>>().join("\n"));
    let command = helper(&sample.project.root, &body);
    let git = PathBuf::from(command.as_std().get_program());
    let settings = sample.project.root.join("synthetic-tool-settings");
    private_dir(&settings).unwrap();
    fs::write(
        settings.join("git-toolchain.json"),
        serde_json::to_vec(&json!({"schemaVersion":1,"gitPath":git})).unwrap(),
    )
    .unwrap();
    let registry = sample.store.processes();
    let environment = GitEnvironmentManager::with_processes(settings, None, registry.clone());
    let project = sample.project.clone();
    let stop = sample.stop.subscribe();
    let mut request = tokio::spawn(async move {
        BuildGitGuard::capture_with_keep_alive(&project, &environment, stop, registry, scope).await
    });
    tokio::select! {
        result = &mut request => panic!("capture ended before reader fixture: {:?}", result.unwrap().err()),
        _ = wait_file(&marker) => (),
    }
    let error = tokio::time::timeout(Duration::from_secs(4), request)
        .await
        .unwrap()
        .unwrap()
        .err()
        .unwrap();
    assert_eq!(error.code, "PROCESS_CLEANUP_PENDING");
    assert_eq!(error.details["processResourceClosed"], false);
    assert!(sample.store.has_active_tasks());
    assert!(sample.project.operation_gate.try_write().is_err());
    assert!(sample
        .store
        .processes()
        .snapshot()
        .iter()
        .any(|state| state.component == "project-git" && !state.io_joined));
    drop(release);
    closed(&sample.store).await;
    assert!(!sample.store.has_active_tasks());
    assert!(sample.project.operation_gate.try_write().is_ok());
    assert!(sample.store.shutdown().await.is_empty());
}

#[tokio::test]
async fn cancelled_prepared_probe_retains_actual_scope_and_build_translation_reports_unclosed_resources(
) {
    let mut sample = fixture().await;
    let mut context = sample.context.take().unwrap();
    let private = context.root().to_path_buf();
    let (hold, marker, release) = holder(&sample.project.root);
    let command = helper(&sample.project.root, &format!("import os,subprocess,sys,time\n{hold}\nprint('git version 2.56.0',flush=True)\ntime.sleep(30)"));
    let git = PathBuf::from(command.as_std().get_program());
    let settings = sample.project.root.join("probe-tool-settings");
    private_dir(&settings).unwrap();
    fs::write(
        settings.join("git-toolchain.json"),
        serde_json::to_vec(&json!({"schemaVersion":1,"gitPath":git})).unwrap(),
    )
    .unwrap();
    let environment =
        GitEnvironmentManager::with_processes(settings, None, sample.store.processes());
    let request = tokio::spawn(async move { context.resolve(&environment).await });
    wait_file(&marker).await;
    sample.stop.send(true).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(2), request)
        .await
        .unwrap()
        .unwrap()
        .err()
        .unwrap();
    assert_eq!(error.code, "PROJECT_GIT_INTERRUPTED");
    assert_eq!(error.details["operationOutcome"], "unknown");
    assert_eq!(error.details["processResourceClosed"], false);
    assert_eq!(error.details["processClosed"], false);
    assert!(sample.store.has_active_tasks());
    assert!(sample.project.operation_gate.try_write().is_err());
    assert!(private.exists());
    let build_error = super::super::build_guard::unavailable(error);
    assert_eq!(build_error.code, "BUILD_INTERRUPTED");
    assert_eq!(build_error.details["operationOutcome"], "unknown");
    assert_eq!(build_error.details["processResourceClosed"], false);
    assert_eq!(build_error.details["processClosed"], false);
    drop(release);
    closed(&sample.store).await;
    assert!(!sample.store.has_active_tasks());
    assert!(sample.project.operation_gate.try_write().is_ok());
    assert!(!private.exists());
    assert!(sample.store.shutdown().await.is_empty());
}

#[test]
fn unknown_process_receipts_never_claim_resource_closure() {
    for code in [
        "PROCESS_REPLY_LOST",
        "PROCESS_IO_BUILDER_PANICKED",
        "PROCESS_DRAIN_UNCONFIRMED",
        "PROCESS_CLEANUP_PENDING",
        "PROCESS_OWNERSHIP_LOST",
    ] {
        let error = environment_error(ApiError::new(code, "artificial missing closure receipt"));
        assert_eq!(error.details["processResourceClosed"], false);
        assert_eq!(error.details["operationOutcome"], "unknown");
        let error = closure(error, true);
        assert_eq!(error.details["processClosed"], false);
    }
    let error = closure(
        ApiError::new("PROCESS_SPAWN_FAILED", "before spawn").outcome("not_started"),
        true,
    );
    assert_eq!(error.details["processResourceClosed"], true);
}
