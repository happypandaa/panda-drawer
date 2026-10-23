//! Real official Node and native job tests. These use disposable files only.
use super::*;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::File,
    io::Read,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::PathBuf,
    time::Duration,
};
use tokio::io::AsyncReadExt;
use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
    Storage::FileSystem::SYNCHRONIZE,
    System::Threading::*,
};

fn raw(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle().cast()
}
fn owned(handle: HANDLE) -> std::io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.cast()) })
}

// Independent fixture pin, verified against the official archive/checksums.
pub(crate) fn private_node() -> PathBuf {
    let path = std::env::var_os("NODE_RUNTIME_TRIAL_TEST_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.test-artifacts/node-official-win24/node-v24.22.0-win-x64/node.exe")
        });
    let mut file =
        File::open(&path).expect("prepare the independently verified private Windows Node fixture");
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(bytes)),
        "ba93f9a70bd86fce3ebbb39ec758501458453467aa92a34a0a3602d1e3fc2b9c"
    );
    path.canonicalize().unwrap()
}
fn env(root: &Path) -> BTreeMap<OsString, OsString> {
    BTreeMap::from([
        ("SYSTEMROOT".into(), windows_directory().unwrap()),
        ("TEMP".into(), root.as_os_str().into()),
        ("TMP".into(), root.as_os_str().into()),
    ])
}
fn tree_script(root: &Path, detached: bool) -> PathBuf {
    let script = root.join("tree.cjs");
    std::fs::write(&script, r#"const fs=require('fs'),path=require('path'),cp=require('child_process');
const root=process.argv[2],depth=Number(process.argv[3]),detached=DETACHED_FLAG;
const marker=path.join(root,'pid-'+depth);fs.writeFileSync(marker+'.pending',String(process.pid));fs.renameSync(marker+'.pending',marker);
if(depth>0)cp.spawn(process.execPath,[__filename,root,String(depth-1)],{stdio:'ignore',windowsHide:true,detached}).unref();
if(depth===2&&process.argv[4]==='exit'){
 const poll=setInterval(()=>{if(fs.existsSync(path.join(root,'allow-leader-exit'))) {clearInterval(poll);process.exit(0);}},10);
}else setInterval(()=>{},1000);
"#.replace("DETACHED_FLAG", if detached { "true" } else { "false" })).unwrap();
    script
}
async fn ready(root: &Path, leader: Option<&OwnedHandle>) -> Vec<OwnedHandle> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let pids: Option<Vec<u32>> = (0..3)
            .map(|depth| {
                std::fs::read_to_string(root.join(format!("pid-{depth}")))
                    .ok()?
                    .parse()
                    .ok()
            })
            .collect();
        if let Some(pids) = pids {
            return pids
                .into_iter()
                .enumerate()
                .map(|(depth, pid)| {
                    if depth == 2 {
                        if let Some(handle) = leader {
                            return handle.try_clone().unwrap();
                        }
                    }
                    owned(unsafe {
                        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid)
                    })
                    .unwrap()
                })
                .collect();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "disposable Node descendants did not become ready"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn empty(owner: &ProcessOwnership) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while owner.active().unwrap() != 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the owned job retained live processes"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn all_gone(handles: &[OwnedHandle]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while handles
        .iter()
        .any(|handle| unsafe { WaitForSingleObject(raw(handle), 0) } != WAIT_OBJECT_0)
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "a fixture process remained alive"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[test]
fn argument_and_environment_encoding_preserves_quotes_and_rejects_case_aliases() {
    for (input, expected) in [
        ("", "\"\""),
        ("a b", "\"a b\""),
        ("a\"b\\", "\"a\\\"b\\\\\""),
    ] {
        assert_eq!(
            String::from_utf16(&quote(OsStr::new(input)).unwrap()).unwrap(),
            expected
        );
    }
    assert!(environment(&BTreeMap::from([
        ("Path".into(), "one".into()),
        ("PATH".into(), "two".into())
    ]))
    .is_err());
    assert!(environment(&BTreeMap::from([("A=B".into(), "x".into())])).is_err());
    assert_eq!(environment(&BTreeMap::new()).unwrap(), [0, 0]);
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn suspended_assignment_failure_never_executes_application_instructions() {
    let temporary = tempfile::tempdir().unwrap();
    let node = private_node();
    let marker = temporary.path().join("must-not-exist");
    let args = vec![
        "-e".into(),
        "require('fs').writeFileSync(process.argv[1],'ran')".into(),
        marker.into_os_string(),
    ];
    assert!(
        Child::spawn_inner(&node, &args, temporary.path(), &env(temporary.path()), true).is_err()
    );
    assert!(!temporary.path().join("must-not-exist").exists());
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn native_pipes_preserve_chinese_spaces_quotes_and_the_exact_environment() {
    let temporary = tempfile::Builder::new()
        .prefix("Node 空格 '")
        .tempdir()
        .unwrap();
    let expected = "参数 with \"quote\" and trailing\\";
    let mut child = Child::spawn(&private_node(), &["-e".into(), "process.stdout.write(JSON.stringify({arg:process.argv[1],path:process.env.PATH,node:process.env.NODE_OPTIONS,system:!!process.env.SYSTEMROOT}))".into(), expected.into()], temporary.path(), &env(temporary.path())).unwrap();
    child.stdin.take();
    let mut output = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut output)
        .await
        .unwrap();
    assert!(child.wait().await.unwrap().success());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&output).unwrap(),
        serde_json::json!({"arg":expected,"system":true})
    );
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn job_termination_cleans_three_generations_and_preserves_an_independent_node() {
    let temporary = tempfile::tempdir().unwrap();
    let node = private_node();
    let script = tree_script(temporary.path(), true);
    let mut independent = tokio::process::Command::new(&node)
        .args(["-e", "setInterval(()=>{},1000)"])
        .creation_flags(CREATE_NO_WINDOW)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut child = Child::spawn(
        &node,
        &[
            script.into_os_string(),
            temporary.path().as_os_str().into(),
            "2".into(),
        ],
        temporary.path(),
        &env(temporary.path()),
    )
    .unwrap();
    let handles = ready(temporary.path(), Some(&child.ownership.process)).await;
    let ownership = child.ownership();
    assert!(ownership.active().unwrap() >= 3);
    ownership.signal(9).unwrap();
    child.wait().await.unwrap();
    all_gone(&handles).await;
    empty(&ownership).await;
    assert!(independent.try_wait().unwrap().is_none());
    independent.kill().await.unwrap();
    independent.wait().await.unwrap();
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn exited_leader_keeps_owned_job_and_descendants_until_cleanup() {
    let temporary = tempfile::tempdir().unwrap();
    let node = private_node();
    // libuv gives non-detached children its own kill-on-close job. Detached
    // children skip that inner job but never request Windows job breakaway;
    // they must remain in the platform's outer job after the leader exits.
    let script = tree_script(temporary.path(), true);
    let mut child = Child::spawn(
        &node,
        &[
            script.into_os_string(),
            temporary.path().as_os_str().into(),
            "2".into(),
            "exit".into(),
        ],
        temporary.path(),
        &env(temporary.path()),
    )
    .unwrap();
    let handles = ready(temporary.path(), Some(&child.ownership.process)).await;
    std::fs::write(temporary.path().join("allow-leader-exit"), b"exit").unwrap();
    assert!(child.wait().await.unwrap().success());
    assert!(
        child.ownership.has_descendants().unwrap(),
        "detached fixture descendants escaped the platform job"
    );
    child.ownership.signal(9).unwrap();
    all_gone(&handles).await;
    empty(&child.ownership).await;
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn dropping_a_child_cancels_its_owned_job_even_with_observation_handles_retained() {
    let temporary = tempfile::tempdir().unwrap();
    let node = private_node();
    let script = tree_script(temporary.path(), true);
    let child = Child::spawn(
        &node,
        &[
            script.into_os_string(),
            temporary.path().as_os_str().into(),
            "2".into(),
        ],
        temporary.path(),
        &env(temporary.path()),
    )
    .unwrap();
    let handles = ready(temporary.path(), Some(&child.ownership.process)).await;
    let owner = child.ownership();
    drop(child);
    all_gone(&handles).await;
    empty(&owner).await;
}

#[test]
fn host_crash_probe() {
    let Some(root) = std::env::var_os("PLATFORM_NODE_JOB_HOST_PROBE") else {
        return;
    };
    let root = PathBuf::from(root);
    let script = tree_script(&root, true);
    let _child = Child::spawn(
        &private_node(),
        &[script.into_os_string(), root.as_os_str().into(), "2".into()],
        &root,
        &env(&root),
    )
    .unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn killing_the_host_without_destructors_closes_job_and_all_descendants() {
    use std::os::windows::process::CommandExt;
    let temporary = tempfile::tempdir().unwrap();
    let mut host = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "node_runtime_trial::windows_process::tests::host_crash_probe",
            "--nocapture",
        ])
        .env("PLATFORM_NODE_JOB_HOST_PROBE", temporary.path())
        .env("NODE_RUNTIME_TRIAL_TEST_NODE", private_node())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let handles = ready(temporary.path(), None).await;
    host.kill().unwrap(); // The std Child retains the exact host process HANDLE.
    host.wait().unwrap();
    all_gone(&handles).await;
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn ordinary_non_detached_children_also_exit_with_their_node_parent() {
    let temporary = tempfile::tempdir().unwrap();
    let node = private_node();
    let script = tree_script(temporary.path(), false);
    let mut child = Child::spawn(
        &node,
        &[
            script.into_os_string(),
            temporary.path().as_os_str().into(),
            "2".into(),
            "exit".into(),
        ],
        temporary.path(),
        &env(temporary.path()),
    )
    .unwrap();
    let handles = ready(temporary.path(), Some(&child.ownership.process)).await;
    std::fs::write(temporary.path().join("allow-leader-exit"), b"exit").unwrap();
    assert!(child.wait().await.unwrap().success());
    // Observe libuv's ordinary child cleanup without signalling the outer job.
    all_gone(&handles).await;
    empty(&child.ownership).await;
    assert!(!child.ownership.has_descendants().unwrap());
}
