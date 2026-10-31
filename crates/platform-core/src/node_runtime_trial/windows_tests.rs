//! Real disposable Windows processes. Official Node tests require an explicitly
//! supplied independently pinned binary and never discover installed app data.
use super::*;
use std::io::{Read, Write};

const CHILD: &str = "node_runtime_trial::platform::tests::native_fixture_child";
const CHILD_MODE: &str = "PANDA_DISPOSABLE_RUNTIME_CHILD_MODE";
const CHILD_ROOT: &str = "PANDA_DISPOSABLE_RUNTIME_CHILD_ROOT";

#[test]
#[ignore = "Finite or Job-owned child fixture; only launched with a new disposable root"]
fn native_fixture_child() {
    let root = PathBuf::from(std::env::var_os(CHILD_ROOT).expect("explicit disposable root"));
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("panda-disposable-w3-runtime-"));
    let mode = std::env::var(CHILD_MODE).unwrap();
    if mode == "descendant" {
        loop {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    fs::write(root.join("executed"), b"real child code executed").unwrap();
    if mode == "descendant-root" {
        let mut descendant = std::process::Command::new(std::env::current_exe().unwrap());
        descendant
            .args([
                "--ignored",
                "--exact",
                CHILD,
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_MODE, "descendant")
            .env(CHILD_ROOT, &root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = descendant.spawn().unwrap();
        println!("DESCENDANT:{}", child.id());
        std::io::stdout().flush().unwrap();
        // This fixture intentionally exits its actual root while the created
        // descendant remains in the retained Job. Waiting here would remove
        // the condition the parent regression must observe and clean up.
        std::process::exit(0);
    }
    println!("RUNTIME-READY");
    std::io::stdout().flush().unwrap();
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    fs::write(root.join("input"), input).unwrap();
}

fn fixture(mode: &str) -> (tempfile::TempDir, Child) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w3-runtime-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--ignored",
            "--exact",
            CHILD,
            "--nocapture",
            "--test-threads=1",
        ])
        .current_dir(&root)
        .env_clear()
        .env(CHILD_ROOT, &root)
        .env(CHILD_MODE, mode);
    let child = spawn(command).unwrap();
    (temporary, child)
}

async fn drain(child: &mut Child) -> Vec<JoinHandle<Vec<u8>>> {
    [child.stdout.take().unwrap(), child.stderr.take().unwrap()]
        .into_iter()
        .map(|mut pipe| {
            tokio::spawn(async move {
                let mut bytes = Vec::new();
                pipe.read_to_end(&mut bytes).await.unwrap();
                bytes
            })
        })
        .collect()
}

#[tokio::test]
async fn suspended_runtime_keeps_owner_and_real_pumps_until_explicit_resume_and_full_join() {
    let (temporary, mut child) = fixture("input");
    let owner = ownership(&child, child.id().unwrap());
    let readers = drain(&mut child).await;
    assert!(!temporary.path().join("executed").exists());
    assert!(owner.observe().unwrap().is_none());
    activate(&mut child).unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"actual stdin bytes")
        .await
        .unwrap();
    child.stdin.take();
    assert!(wait_for_owned_exit(&owner, Duration::from_secs(5))
        .await
        .unwrap());
    assert!(!owner.has_descendants().unwrap());
    owner.disarm_before_reap();
    assert!(child.wait().await.unwrap().success());
    for reader in readers {
        reader.await.unwrap();
    }
    finish_io(&owner, tokio::time::Instant::now() + Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(
        fs::read(temporary.path().join("input")).unwrap(),
        b"actual stdin bytes"
    );
    let state = owner.owner.0.lock().unwrap();
    assert!(state.process.is_none() && state.workers.is_empty());
}

#[tokio::test]
async fn an_actual_resume_failure_retains_the_created_owner_until_real_cleanup() {
    let (temporary, mut child) = fixture("input");
    let owner = ownership(&child, child.id().unwrap());
    let readers = drain(&mut child).await;
    owner.signal(SIGNAL_KILL).unwrap();
    assert!(wait_for_owned_exit(&owner, Duration::from_secs(5))
        .await
        .unwrap());
    let error = activate(&mut child).unwrap_err();
    assert_eq!(error.code, "TRIAL_PROCESS_RESUME_FAILED");
    assert!(!spawn_failed_before_creation(&error));
    assert!(!temporary.path().join("executed").exists());
    {
        let state = owner.owner.0.lock().unwrap();
        assert!(state.process.is_some());
        assert_eq!(state.workers.len(), 3);
    }
    child.stdin.take();
    owner.disarm_before_reap();
    child.wait().await.unwrap();
    for reader in readers {
        reader.await.unwrap();
    }
    finish_io(&owner, tokio::time::Instant::now() + Duration::from_secs(3))
        .await
        .unwrap();
}

#[tokio::test]
async fn an_exited_root_keeps_its_real_job_until_remaining_descendants_are_stopped() {
    let (_temporary, mut child) = fixture("descendant-root");
    let owner = ownership(&child, child.id().unwrap());
    let readers = drain(&mut child).await;
    activate(&mut child).unwrap();
    child.stdin.take();
    assert!(wait_for_owned_exit(&owner, Duration::from_secs(5))
        .await
        .unwrap());
    assert!(owner.has_descendants().unwrap());
    owner.signal_with_reobserve(SIGNAL_KILL).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while owner.has_descendants().unwrap() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    owner.disarm_before_reap();
    child.wait().await.unwrap();
    for reader in readers {
        reader.await.unwrap();
    }
    finish_io(&owner, tokio::time::Instant::now() + Duration::from_secs(3))
        .await
        .unwrap();
}

#[test]
fn ordinary_same_user_storage_preflight_does_not_require_or_repair_a_private_dacl() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w3-runtime-storage-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    for leaf in ["data", "cache", "backend", "trial"] {
        fs::create_dir(root.join(leaf)).unwrap();
    }
    let data = root.join("data");
    let before = native_files::open_no_reparse(&data).unwrap();
    assert!(native_files::verify_private(&before.file).is_err());
    checked_storage(
        data.clone(),
        root.join("cache"),
        &root.join("backend"),
        &root.join("trial"),
    )
    .unwrap();
    let after = native_files::open_no_reparse(&data).unwrap();
    assert!(before.identity.same_object(&after.identity));
    assert!(native_files::verify_private(&after.file).is_err());
}

#[test]
fn runtime_environment_is_cleared_with_native_system_root_and_explicit_temporary_paths() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w3-runtime-environment-")
        .tempdir()
        .unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.env_clear();
    configure_environment(&mut command, temporary.path()).unwrap();
    let request = crate::owned_process::windows::request(OwnedCommand::cleared(command)).unwrap();
    let environment = request.environment.into_iter().collect::<BTreeMap<_, _>>();
    let system = platform_native::windows::system_paths::SystemPaths::current().unwrap();
    assert_eq!(environment.len(), 3);
    assert_eq!(
        environment[std::ffi::OsStr::new("SystemRoot")],
        system.windows.as_os_str()
    );
    for name in ["TEMP", "TMP"] {
        assert_eq!(
            environment[std::ffi::OsStr::new(name)],
            argument(temporary.path()).unwrap().as_os_str()
        );
    }
}

fn official_fixture(behavior: &str) -> (tempfile::TempDir, RuntimeLaunchConfig) {
    let node = PathBuf::from(
        std::env::var_os("PANDA_W3_OFFICIAL_NODE")
            .expect("set explicit pinned official Node executable"),
    );
    let release: Value = serde_json::from_str(crate::node_tool_layout::RUNTIME_RELEASE).unwrap();
    let digest = release["binary"]["sha256"].as_str().unwrap().to_string();
    verify_node_digest(&node, &digest).unwrap();
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w3-official-node-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    for leaf in ["app", "data", "cache", "node-runtime-trial-first"] {
        fs::create_dir(root.join(leaf)).unwrap();
    }
    fs::write(
        root.join("app/entry.cjs"),
        include_str!("windows_node_fixture.cjs"),
    )
    .unwrap();
    (
        temporary,
        RuntimeLaunchConfig {
            node_path: node,
            expected_node_sha256: digest,
            entry_path: root.join("app/entry.cjs"),
            working_directory: root.join("app"),
            trial_root: root.join("node-runtime-trial-first"),
            business_env: BTreeMap::from([
                ("TRIAL_BEHAVIOR".into(), behavior.into()),
                ("AI_KEY".into(), "artificial-runtime-secret".into()),
            ]),
            control_directory: root,
            redaction_values: vec!["artificial-runtime-secret".into()],
        },
    )
}

#[tokio::test]
#[ignore = "Explicit pinned official Node executable required; creates only new disposable application files"]
async fn official_node_protocol_large_stderr_private_logs_storage_and_repeated_stop() {
    let (_temporary, config) = official_fixture("verbose");
    let control = config.control_directory.clone();
    let next = RuntimeLaunchConfig {
        node_path: config.node_path.clone(),
        expected_node_sha256: config.expected_node_sha256.clone(),
        entry_path: config.entry_path.clone(),
        working_directory: config.working_directory.clone(),
        trial_root: control.join("node-runtime-trial-second"),
        business_env: config.business_env.clone(),
        control_directory: control.clone(),
        redaction_values: config.redaction_values.clone(),
    };
    let mut runtime =
        NodeRuntimeTrial::launch_with_storage(config, control.join("data"), control.join("cache"))
            .await
            .unwrap();
    runtime.ping().await.unwrap();
    runtime.verify_health().await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while runtime.status().unwrap().stderr_bytes_received <= 64 * 1024 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(runtime.status().unwrap().stderr_bytes_received > 64 * 1024);
    let information = runtime.get_json("/environment").await.unwrap();
    assert_eq!(information["inherited"], Value::Null);
    assert_eq!(information["configured"], "artificial-runtime-secret");
    assert_eq!(information["count"], 1);
    assert_eq!(information["temporary"], information["declaredTemporary"]);
    assert_eq!(
        information["systemRoot"],
        platform_native::windows::system_paths::SystemPaths::current()
            .unwrap()
            .windows
            .to_str()
            .unwrap()
    );
    let identity = runtime.process_identity().unwrap();
    assert!(identity.windows_creation_100ns > 0);
    let stopped = runtime.stop().await.unwrap();
    assert!(stopped.process_group_exited && stopped.stopping_observed && !stopped.forced);
    assert_eq!(stopped.state, "stopped");
    assert_eq!(runtime.stop().await.unwrap().state, "stopped");
    let log = fs::read_to_string(control.join("logs/current.log")).unwrap();
    assert!(!log.contains("artificial-runtime-secret"));
    assert!(log.contains("***"));
    native_files::verify_private(
        &native_files::open_no_reparse(&control.join("logs/current.log"))
            .unwrap()
            .file,
    )
    .unwrap();
    fs::create_dir(&next.trial_root).unwrap();
    let mut second =
        NodeRuntimeTrial::launch_with_storage(next, control.join("data"), control.join("cache"))
            .await
            .unwrap();
    assert_eq!(second.get_json("/environment").await.unwrap()["count"], 2);
    assert_ne!(runtime.status.run_directory, second.status.run_directory);
    assert!(second.stop().await.unwrap().process_group_exited);
    assert!(control.join("logs/previous.log").is_file());
    assert_eq!(
        fs::read_to_string(control.join("data/disposable-count.txt")).unwrap(),
        "2"
    );
}

#[tokio::test]
#[ignore = "Explicit pinned official Node executable required; Job-owned uncooperative children are disposable"]
async fn official_node_protocol_timeout_forces_the_actual_job_and_joins_every_pipe() {
    let (_temporary, config) = official_fixture("hang");
    let mut runtime = NodeRuntimeTrial::launch(config).await.unwrap();
    let stopped = runtime.stop().await.unwrap();
    assert!(stopped.forced && stopped.process_group_exited);
    assert_eq!(stopped.state, "failed");
    assert!(runtime.readers.is_empty());
    let state = runtime.ownership.owner.0.lock().unwrap();
    assert!(state.process.is_none() && state.workers.is_empty());
}
