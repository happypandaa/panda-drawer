//! Actual private official Node exercises the generic executor without the app
//! ready protocol. All files are named disposable evidence under this checkout.
use super::*;
use crate::windows_process::{self, JobChild};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::Stdio,
    sync::OnceLock,
};
use windows_sys::Win32::{
    Foundation::{HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0},
    Storage::FileSystem::SYNCHRONIZE,
    System::Threading::{
        OpenProcess, WaitForSingleObject, CREATE_NO_WINDOW, PROCESS_QUERY_LIMITED_INFORMATION,
    },
};

fn artifacts() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(".test-artifacts")
}
fn root() -> PathBuf {
    let path = artifacts().join(format!(
        "Windows command 可丢弃 空格 '{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&path).unwrap();
    WindowsSourceDirectory::open(&path)
        .unwrap()
        .path()
        .to_owned()
}
fn node() -> PathBuf {
    static VERIFIED: OnceLock<PathBuf> = OnceLock::new();
    VERIFIED
        .get_or_init(|| {
            let path = std::env::var_os("NODE_RUNTIME_TRIAL_TEST_NODE")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    Path::new(env!("CARGO_MANIFEST_DIR")).join(
                        "../../.test-artifacts/node-official-win24/node-v24.22.0-win-x64/node.exe",
                    )
                });
            assert_eq!(
                format!(
                    "{:x}",
                    Sha256::digest(fs::read(&path).expect(
                        "prepare the independently verified official private Node fixture"
                    ))
                ),
                "ba93f9a70bd86fce3ebbb39ec758501458453467aa92a34a0a3602d1e3fc2b9c"
            );
            path.canonicalize().unwrap()
        })
        .clone()
}
fn environment(path: &Path) -> BTreeMap<OsString, OsString> {
    BTreeMap::from([
        (
            "SYSTEMROOT".into(),
            windows_directory().unwrap().into_os_string(),
        ),
        ("TEMP".into(), path.as_os_str().into()),
        ("TMP".into(), path.as_os_str().into()),
        ("DISPOSABLE_VALUE".into(), "中文 value ' with spaces".into()),
    ])
}
fn spec(path: &Path, script: &str) -> CommandSpec {
    CommandSpec {
        executable: node(),
        args: vec!["-e".into(), script.into()],
        working_directory: path.to_owned(),
        environment: environment(path),
        stdin: Vec::new(),
        timeout: Duration::from_secs(10),
        stdout_limit: 1024 * 1024,
        stderr_limit: 1024 * 1024,
    }
}
fn raw(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle().cast()
}
fn process(pid: u32) -> OwnedHandle {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE, 0, pid) };
    assert!(!handle.is_null() && handle != INVALID_HANDLE_VALUE);
    unsafe { OwnedHandle::from_raw_handle(handle.cast()) }
}
async fn ready(path: &Path) -> Vec<OwnedHandle> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let pids: Option<Vec<u32>> = (0..3)
            .map(|depth| {
                fs::read_to_string(path.join(format!("pid-{depth}")))
                    .ok()?
                    .parse()
                    .ok()
            })
            .collect();
        if let Some(pids) = pids {
            return pids.into_iter().map(process).collect();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "disposable process tree did not become ready"
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
            "an exactly observed fixture process remained alive"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
fn tree(path: &Path, leader_exits: bool) -> CommandSpec {
    let script = path.join("tree 可丢弃.cjs");
    fs::write(&script, r#"const fs=require('fs'),path=require('path'),cp=require('child_process');
const root=process.argv[2],depth=Number(process.argv[3]);
const marker=path.join(root,'pid-'+depth);fs.writeFileSync(marker+'.pending',String(process.pid));fs.renameSync(marker+'.pending',marker);
if(depth>0)cp.spawn(process.execPath,[__filename,root,String(depth-1)],{stdio:'ignore',detached:true,windowsHide:true}).unref();
if(depth===2&&process.argv[4]==='exit')setInterval(()=>{if(fs.existsSync(path.join(root,'allow-exit')))process.exit(0);},10);
else setInterval(()=>{},1000);
"#).unwrap();
    let mut command = spec(path, "unused");
    command.args = vec![script.into_os_string(), path.as_os_str().into(), "2".into()];
    if leader_exits {
        command.args.push("exit".into());
    }
    command
}

#[test]
fn output_success_requires_complete_untruncated_normal_tree_exit() {
    let mut output = CommandOutput {
        exit_code: Some(0),
        termination: CommandTermination::Exited,
        stdout: CapturedOutput::default(),
        stderr: CapturedOutput::default(),
        stdin_complete: true,
        output_truncated: false,
        tree_exited: true,
    };
    assert!(output.success());
    output.output_truncated = true;
    assert!(!output.success());
    output.output_truncated = false;
    output.termination = CommandTermination::DescendantsTerminated;
    assert!(!output.success());
    output.termination = CommandTermination::Exited;
    output.stdin_complete = false;
    assert!(!output.success());
}

#[tokio::test]
async fn invalid_limits_and_precancelled_commands_do_not_spawn_or_create_paths() {
    let missing = artifacts().join(format!(
        "windows-command-never-created-{}",
        uuid::Uuid::new_v4()
    ));
    let command = || CommandSpec {
        executable: missing.join("not-executed.exe"),
        args: Vec::new(),
        working_directory: missing.clone(),
        environment: BTreeMap::new(),
        stdin: Vec::new(),
        timeout: Duration::from_secs(1),
        stdout_limit: 1,
        stderr_limit: 1,
    };
    let (cancel, receiver) = watch::channel(true);
    let output = run(command(), receiver).await.unwrap();
    assert_eq!(output.termination, CommandTermination::Cancelled);
    assert!(!output.success());
    assert!(output.tree_exited && output.exit_code.is_none());
    drop(cancel);
    let (_cancel, receiver) = watch::channel(false);
    let mut invalid = command();
    invalid.stdout_limit = MAX_STREAM_LIMIT + 1;
    assert_eq!(
        run(invalid, receiver).await.unwrap_err().code,
        "COMMAND_LIMIT_INVALID"
    );
    assert!(!missing.exists());
}

#[tokio::test]
async fn invalid_executable_fails_without_mutating_disposable_working_directory() {
    let path = root();
    let executable = path.join("not a PE.exe");
    fs::write(&executable, b"disposable nonexecutable evidence").unwrap();
    let (_cancel, receiver) = watch::channel(false);
    let command = CommandSpec {
        executable: executable.clone(),
        args: Vec::new(),
        working_directory: path.clone(),
        environment: environment(&path),
        stdin: Vec::new(),
        timeout: Duration::from_secs(1),
        stdout_limit: 1,
        stderr_limit: 1,
    };
    assert_eq!(
        run(command, receiver).await.unwrap_err().code,
        "COMMAND_PROCESS_FAILED"
    );
    assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
    assert_eq!(
        fs::read(executable).unwrap(),
        b"disposable nonexecutable evidence"
    );
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn binary_channels_large_stdin_quotes_and_exact_environment_are_preserved() {
    let path = root();
    let argument = "中文参数 with \"quotes\" and trailing\\";
    let mut command = spec(
        &path,
        r#"const crypto=require('crypto');
process.stdout.write(Buffer.alloc(131072,65));process.stderr.write(Buffer.alloc(131072,66));
(async()=>{let chunks=[];for await(const chunk of process.stdin)chunks.push(chunk);
process.stdout.write(Buffer.from([0,255,128]));
process.stdout.write(JSON.stringify({arg:process.argv[1],cwd:process.cwd(),inputSha:crypto.createHash('sha256').update(Buffer.concat(chunks)).digest('hex'),value:process.env.DISPOSABLE_VALUE,path:process.env.PATH??null,nodeOptions:process.env.NODE_OPTIONS??null}));
process.stderr.write(Buffer.from([255,0,128]));})();"#,
    );
    command.args.push(argument.into());
    command.stdin = (0..1024 * 1024).map(|index| index as u8).collect();
    let input_sha = format!("{:x}", Sha256::digest(&command.stdin));
    let (_cancel, receiver) = watch::channel(false);
    let output = run(command, receiver).await.unwrap();
    assert!(output.success(), "{output:?}");
    assert_eq!(&output.stdout.bytes[..131072], vec![65; 131072]);
    assert_eq!(&output.stdout.bytes[131072..131075], [0, 255, 128]);
    assert_eq!(&output.stderr.bytes[..131072], vec![66; 131072]);
    assert_eq!(&output.stderr.bytes[131072..], [255, 0, 128]);
    let result: serde_json::Value = serde_json::from_slice(&output.stdout.bytes[131075..]).unwrap();
    assert_eq!(
        result,
        serde_json::json!({"arg":argument,"cwd":path,"inputSha":input_sha,"value":"中文 value ' with spaces","path":null,"nodeOptions":null})
    );
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn independent_output_limits_drain_both_streams_and_report_truncation() {
    let path = root();
    let mut command = spec(&path, "process.stdout.write(Buffer.alloc(2097152,120));process.stderr.write(Buffer.alloc(2097152,121));");
    command.stdout_limit = 333;
    command.stderr_limit = 777;
    let (_cancel, receiver) = watch::channel(false);
    let output = run(command, receiver).await.unwrap();
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(output.termination, CommandTermination::Exited);
    assert!(!output.success());
    assert!(output.output_truncated && output.tree_exited);
    assert_eq!(output.stdout.bytes, vec![120; 333]);
    assert_eq!(output.stderr.bytes, vec![121; 777]);
    assert_eq!(output.stdout.discarded_bytes, 2097152 - 333);
    assert_eq!(output.stderr.discarded_bytes, 2097152 - 777);
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn nonzero_exit_preserves_exact_binary_error_output() {
    let path = root();
    let command = spec(
        &path,
        "process.stderr.write(Buffer.from([0,255,128,13,10]));process.exitCode=23;",
    );
    let (_cancel, receiver) = watch::channel(false);
    let output = run(command, receiver).await.unwrap();
    assert_eq!(output.exit_code, Some(23));
    assert_eq!(output.termination, CommandTermination::Exited);
    assert_eq!(output.stderr.bytes, [0, 255, 128, 13, 10]);
    assert!(!output.success());
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn timeout_closes_unread_large_stdin_and_the_entire_tree() {
    let path = root();
    let mut command = tree(&path, false);
    command.timeout = Duration::from_secs(4);
    command.stdin = vec![42; MAX_STDIN_BYTES];
    let (_cancel, receiver) = watch::channel(false);
    let execution = tokio::spawn(run(command, receiver));
    let handles = ready(&path).await;
    let output = tokio::time::timeout(Duration::from_secs(7), execution)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(output.termination, CommandTermination::TimedOut);
    assert!(output.tree_exited);
    assert!(!output.success());
    assert!(!output.stdin_complete);
    all_gone(&handles).await;
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn cancellation_stops_three_generations_and_preserves_an_independent_process() {
    let path = root();
    let mut independent = JobChild::spawn(
        &node(),
        &["-e".into(), "setInterval(()=>{},1000)".into()],
        &path,
        &environment(&path),
    )
    .unwrap();
    let (cancel, receiver) = watch::channel(false);
    let execution = tokio::spawn(run(tree(&path, false), receiver));
    let handles = ready(&path).await;
    cancel.send(true).unwrap();
    let output = execution.await.unwrap().unwrap();
    assert_eq!(output.termination, CommandTermination::Cancelled);
    assert!(!output.success());
    assert!(output.tree_exited);
    all_gone(&handles).await;
    assert!(independent.ownership().observe().unwrap().is_none());
    independent.ownership().terminate().unwrap();
    independent.wait().await.unwrap();
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn leader_exit_reclaims_detached_descendants_and_is_not_success() {
    let path = root();
    let (_cancel, receiver) = watch::channel(false);
    let execution = tokio::spawn(run(tree(&path, true), receiver));
    let handles = ready(&path).await;
    fs::write(path.join("allow-exit"), b"exit").unwrap();
    let output = execution.await.unwrap().unwrap();
    assert_eq!(output.exit_code, Some(0));
    assert_eq!(
        output.termination,
        CommandTermination::DescendantsTerminated
    );
    assert!(output.tree_exited);
    assert!(!output.success());
    all_gone(&handles).await;
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn dropping_execution_future_stops_its_owned_tree() {
    let path = root();
    let (_cancel, receiver) = watch::channel(false);
    let execution = tokio::spawn(run(tree(&path, false), receiver));
    let handles = ready(&path).await;
    execution.abort();
    assert!(execution.await.unwrap_err().is_cancelled());
    all_gone(&handles).await;
}

#[test]
fn host_crash_probe() {
    let Some(path) = std::env::var_os("PLATFORM_COMMAND_JOB_HOST_PROBE") else {
        return;
    };
    let path = PathBuf::from(path);
    let mut command = tree(&path, false);
    command.timeout = Duration::from_secs(300);
    let (_cancel, receiver) = watch::channel(false);
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(run(command, receiver))
        .unwrap();
}

#[tokio::test]
#[ignore = "Requires the independently verified official private Windows Node fixture"]
async fn killing_the_host_without_destructors_closes_all_job_descendants() {
    let path = root();
    let mut host = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "windows_command::tests::host_crash_probe",
            "--nocapture",
        ])
        .env("PLATFORM_COMMAND_JOB_HOST_PROBE", &path)
        .env("NODE_RUNTIME_TRIAL_TEST_NODE", node())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let handles = ready(&path).await;
    host.kill().unwrap();
    host.wait().unwrap();
    all_gone(&handles).await;
}

#[test]
fn environment_names_reject_windows_case_aliases_and_non_ascii_ambiguity() {
    for values in [
        BTreeMap::from([("Path".into(), "one".into()), ("PATH".into(), "two".into())]),
        BTreeMap::from([("é".into(), "one".into()), ("É".into(), "two".into())]),
    ] {
        assert!(windows_process::testing::environment_block(&values).is_err());
    }
}
