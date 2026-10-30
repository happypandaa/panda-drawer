//! Identity-bound observation of a previous host's Node process group.
//!
//! No signal is sent using a persisted numeric PID alone. This module does not
//! remove runtime markers and does not interpret business/protocol exit status.
use crate::ApiError;
use serde::{Deserialize, Serialize};
#[cfg(test)]
use std::path::Path;
use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

pub(crate) const RUN_IDENTITY_ENV: &str = "PANDA_DRAWER_RUN_IDENTITY";
pub(crate) const TERMINATE_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const KILL_TIMEOUT: Duration = Duration::from_secs(2);
// Signals are only sent by the macOS branch. Numeric constants also keep
// unsupported-target builds independent of platform-specific libc exports.
const SIGNAL_TERM: i32 = 15;
const SIGNAL_KILL: i32 = 9;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RuntimeProcessIdentity {
    pub pid: u32,
    pub pgid: u32,
    pub started_at_sec: u64,
    pub started_at_usec: u64,
    pub executable: PathBuf,
    pub nonce: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Observation {
    Matching,
    Stopped,
    Unconfirmed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StopReceipt {
    pub forced: bool,
}

fn unconfirmed() -> ApiError {
    ApiError::new(
        "NODE_PROCESS_IDENTITY_UNCONFIRMED",
        "无法完整核实遗留应用的运行身份，请重启电脑后重试。",
    )
    .outcome("not_deleted")
}

fn validate(value: &RuntimeProcessIdentity) -> Result<(), ApiError> {
    if value.pid <= 1
        || value.pid > i32::MAX as u32
        || value.pgid != value.pid
        || value.started_at_sec == 0
        || value.started_at_usec >= 1_000_000
        || !value.executable.is_absolute()
        || !(16..=128).contains(&value.nonce.len())
        || !value
            .nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(unconfirmed());
    }
    Ok(())
}

pub(crate) fn boot_time() -> Result<SystemTime, ApiError> {
    #[cfg(target_os = "macos")]
    {
        let mut value: libc::timeval = unsafe { std::mem::zeroed() };
        let mut size = std::mem::size_of_val(&value);
        let mut mib = [libc::CTL_KERN, libc::KERN_BOOTTIME];
        let result = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                mib.len() as _,
                (&mut value as *mut libc::timeval).cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if result != 0
            || size != std::mem::size_of_val(&value)
            || value.tv_sec <= 0
            || !(0..1_000_000).contains(&value.tv_usec)
        {
            return Err(ApiError::new(
                "NODE_BOOT_TIME_UNCONFIRMED",
                "无法核对本次系统开机时间，运行记录保持不变。",
            ));
        }
        SystemTime::UNIX_EPOCH
            .checked_add(Duration::new(
                value.tv_sec as u64,
                value.tv_usec as u32 * 1000,
            ))
            .ok_or_else(unconfirmed)
    }
    #[cfg(not(target_os = "macos"))]
    Err(unconfirmed())
}

/// Captures the actual kernel identity of a direct child reserved by its caller.
///
/// The caller must retain and verify its unreaped Child before and after this
/// read. A persisted PID or a process that another host launched does not meet
/// that ownership requirement; use `observe` for those identities instead.
pub(crate) fn capture_owned(pid: u32, nonce: &str) -> Result<RuntimeProcessIdentity, ApiError> {
    let value = read_identity(pid, nonce)?.ok_or_else(unconfirmed)?;
    validate(&value)?;
    Ok(value)
}

#[cfg(test)]
pub(crate) fn capture(
    pid: u32,
    expected_executable: &Path,
    nonce: &str,
) -> Result<RuntimeProcessIdentity, ApiError> {
    let expected = std::fs::canonicalize(expected_executable).map_err(|_| unconfirmed())?;
    let value = capture_owned(pid, nonce)?;
    if value.executable != expected {
        return Err(unconfirmed());
    }
    Ok(value)
}

pub(crate) fn observe(value: &RuntimeProcessIdentity) -> Result<Observation, ApiError> {
    validate(value)?;
    let group = group_snapshot(value.pgid)?;
    if group.is_empty() {
        return Ok(Observation::Stopped);
    }
    if read_identity(value.pid, &value.nonce)?.as_ref() != Some(value) {
        return Ok(Observation::Unconfirmed);
    }
    // Only a group whose current live members retain this run's nonce can be
    // adopted by another host. Missing or unreadable identity remains unknown.
    for member in group {
        if read_identity(member.pid, &value.nonce)?.is_none_or(|current| current.pgid != value.pgid)
        {
            return Ok(Observation::Unconfirmed);
        }
    }
    Ok(Observation::Matching)
}

pub(crate) async fn stop(value: &RuntimeProcessIdentity) -> Result<StopReceipt, ApiError> {
    stop_with_timeouts(value, TERMINATE_TIMEOUT, KILL_TIMEOUT).await
}

async fn stop_with_timeouts(
    value: &RuntimeProcessIdentity,
    terminate_timeout: Duration,
    kill_timeout: Duration,
) -> Result<StopReceipt, ApiError> {
    validate(value)?;
    let mut anchors = vec![value.clone()];
    if !signal_checked(value, &mut anchors, SIGNAL_TERM)? {
        return Ok(StopReceipt { forced: false });
    }
    if wait_for_empty(value.pgid, terminate_timeout).await? {
        return Ok(StopReceipt { forced: false });
    }
    if !signal_checked(value, &mut anchors, SIGNAL_KILL)? {
        return Ok(StopReceipt { forced: false });
    }
    if wait_for_empty(value.pgid, kill_timeout).await? {
        return Ok(StopReceipt { forced: true });
    }
    Err(ApiError::new(
        "NODE_PROCESS_STOP_FAILED",
        "无法停止该应用，请重启电脑后再删除。",
    )
    .outcome("not_deleted"))
}

async fn wait_for_empty(pgid: u32, timeout: Duration) -> Result<bool, ApiError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if group_snapshot(pgid)?.is_empty() {
            return Ok(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn signal_checked(
    value: &RuntimeProcessIdentity,
    anchors: &mut Vec<RuntimeProcessIdentity>,
    signal: i32,
) -> Result<bool, ApiError> {
    #[cfg(target_os = "macos")]
    {
        let members = group_snapshot(value.pgid)?;
        if members.is_empty() {
            return Ok(false);
        }
        let mut checked = Vec::new();
        for member in members {
            let current = read_identity(member.pid, &value.nonce)?.ok_or_else(unconfirmed)?;
            if current.pgid != value.pgid {
                return Err(unconfirmed());
            }
            checked.push(current);
        }
        // An observed member remains our anchor if TERM ended the original
        // leader. A newly reused PGID with no matching anchor is never signalled.
        let anchor = checked
            .iter()
            .find(|current| anchors.contains(current))
            .ok_or_else(unconfirmed)?;
        if read_identity(anchor.pid, &value.nonce)?.as_ref() != Some(anchor) {
            return Err(unconfirmed());
        }
        if unsafe { libc::kill(-(value.pgid as i32), signal) } != 0 {
            let errno = std::io::Error::last_os_error().raw_os_error();
            if group_snapshot(value.pgid)?.is_empty() {
                return Ok(false);
            }
            return Err(ApiError::new(
                "NODE_PROCESS_SIGNAL_FAILED",
                "无法终止已核实的应用进程组，请重启电脑后重试。",
            )
            .detail("errno", errno)
            .outcome("not_deleted"));
        }
        *anchors = checked;
        Ok(true)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (value, anchors, signal);
        Err(unconfirmed())
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GroupMember {
    pid: u32,
    sec: u64,
    usec: u64,
    zombie: bool,
}

#[cfg(target_os = "macos")]
fn bsd_info(pid: u32) -> Result<Option<libc::proc_bsdinfo>, ApiError> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Err(unconfirmed());
    }
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info);
    let received = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            1,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size as i32,
        )
    };
    if received != size as i32 {
        if received == 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        return Err(unconfirmed());
    }
    if info.pbi_pid != pid {
        return Err(unconfirmed());
    }
    Ok(Some(info))
}

#[cfg(target_os = "macos")]
fn executable(pid: u32) -> Result<PathBuf, ApiError> {
    use std::os::unix::ffi::OsStringExt;
    let mut bytes = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let count =
        unsafe { libc::proc_pidpath(pid as i32, bytes.as_mut_ptr().cast(), bytes.len() as u32) };
    if count <= 0 || count as usize >= bytes.len() {
        return Err(unconfirmed());
    }
    let length = bytes
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(unconfirmed)?;
    bytes.truncate(length);
    let path = PathBuf::from(std::ffi::OsString::from_vec(bytes));
    if !path.is_absolute() {
        return Err(unconfirmed());
    }
    std::fs::canonicalize(path).map_err(|_| unconfirmed())
}

#[cfg(target_os = "macos")]
fn process_nonce(pid: u32) -> Result<Option<String>, ApiError> {
    let mut bytes = vec![0u8; 1024 * 1024];
    let mut size = bytes.len();
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as _,
            bytes.as_mut_ptr().cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || size > bytes.len() {
        return Err(unconfirmed());
    }
    bytes.truncate(size);
    // This buffer may contain business credentials. Only the dedicated nonce
    // leaves this function; errors never contain argv or environment bytes.
    let result = nonce_from_procargs(&bytes);
    bytes.fill(0);
    result
}

#[cfg(any(test, target_os = "macos"))]
fn nonce_from_procargs(bytes: &[u8]) -> Result<Option<String>, ApiError> {
    let argc = bytes
        .get(..4)
        .and_then(|raw| raw.try_into().ok())
        .map(i32::from_ne_bytes)
        .ok_or_else(unconfirmed)?;
    if !(1..=65_536).contains(&argc) {
        return Err(unconfirmed());
    }
    let mut offset = 4usize;
    fn field<'a>(bytes: &'a [u8], offset: &mut usize) -> Result<&'a [u8], ApiError> {
        let rest = bytes.get(*offset..).ok_or_else(unconfirmed)?;
        let length = rest
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(unconfirmed)?;
        *offset += length + 1;
        Ok(&rest[..length])
    }
    let _executable = field(bytes, &mut offset)?;
    while bytes.get(offset) == Some(&0) {
        offset += 1;
    }
    for _ in 0..argc {
        let _argument = field(bytes, &mut offset)?;
    }
    let mut found = None;
    while offset < bytes.len() {
        let value = field(bytes, &mut offset)?;
        if value.is_empty() {
            break;
        }
        if let Some(value) = value.strip_prefix(format!("{RUN_IDENTITY_ENV}=").as_bytes()) {
            if found.is_some() {
                return Err(unconfirmed());
            }
            found = Some(
                std::str::from_utf8(value)
                    .map_err(|_| unconfirmed())?
                    .to_owned(),
            );
        }
    }
    Ok(found)
}

fn read_identity(
    pid: u32,
    expected_nonce: &str,
) -> Result<Option<RuntimeProcessIdentity>, ApiError> {
    #[cfg(target_os = "macos")]
    {
        let Some(before) = bsd_info(pid)? else {
            return Ok(None);
        };
        if before.pbi_status == libc::SZOMB || before.pbi_uid != unsafe { libc::geteuid() } {
            return Ok(None);
        }
        let path = executable(pid)?;
        if process_nonce(pid)?.as_deref() != Some(expected_nonce) {
            return Ok(None);
        }
        let Some(after) = bsd_info(pid)? else {
            return Ok(None);
        };
        if before.pbi_pid != after.pbi_pid
            || before.pbi_pgid != after.pbi_pgid
            || before.pbi_start_tvsec != after.pbi_start_tvsec
            || before.pbi_start_tvusec != after.pbi_start_tvusec
            || before.pbi_uid != after.pbi_uid
            || after.pbi_status == libc::SZOMB
        {
            return Ok(None);
        }
        Ok(Some(RuntimeProcessIdentity {
            pid,
            pgid: after.pbi_pgid,
            started_at_sec: after.pbi_start_tvsec,
            started_at_usec: after.pbi_start_tvusec,
            executable: path,
            nonce: expected_nonce.into(),
        }))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (pid, expected_nonce);
        Err(unconfirmed())
    }
}

#[cfg(target_os = "macos")]
fn kernel_confirms_empty_group(pgid: u32) -> Result<bool, ApiError> {
    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROC,
        libc::KERN_PROC_PGRP,
        pgid as i32,
    ];
    let mut size = 0usize;
    let query = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as _,
            std::ptr::null_mut(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if query != 0 || size > 1024 * 1024 {
        return Err(unconfirmed());
    }
    // The size query includes extra-record slop on Darwin, even for an empty
    // group. Only a successful actual fill reporting zero bytes proves empty.
    let mut bytes = vec![0u8; size.max(4096)];
    let mut received = bytes.len();
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as _,
            bytes.as_mut_ptr().cast(),
            &mut received,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 || received > bytes.len() {
        return Err(unconfirmed());
    }
    Ok(received == 0)
}

#[cfg(target_os = "macos")]
fn group_once(pgid: u32) -> Result<Vec<GroupMember>, ApiError> {
    const PROC_PGRP_ONLY: u32 = 2;
    let mut pids = vec![0i32; 65_536];
    let size = std::mem::size_of_val(pids.as_slice());
    let count =
        unsafe { libc::proc_listpids(PROC_PGRP_ONLY, pgid, pids.as_mut_ptr().cast(), size as i32) };
    if count < 0
        || count as usize >= size
        || !(count as usize).is_multiple_of(std::mem::size_of::<i32>())
    {
        return Err(unconfirmed());
    }
    if count == 0 {
        // A zero libproc result can also mean syscall failure. Require a
        // separate successful kernel enumeration, never a numeric signal-0.
        return if kernel_confirms_empty_group(pgid)? {
            Ok(Vec::new())
        } else {
            Err(unconfirmed())
        };
    }
    let values = &pids[..count as usize / std::mem::size_of::<i32>()];
    let mut seen = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    for pid in values {
        if *pid <= 0 || !seen.insert(*pid) {
            return Err(unconfirmed());
        }
        let info = bsd_info(*pid as u32)?.ok_or_else(unconfirmed)?;
        if info.pbi_pgid != pgid {
            return Err(unconfirmed());
        }
        result.push(GroupMember {
            pid: *pid as u32,
            sec: info.pbi_start_tvsec,
            usec: info.pbi_start_tvusec,
            zombie: info.pbi_status == libc::SZOMB,
        });
    }
    result.sort_by_key(|member| member.pid);
    Ok(result)
}

#[cfg(target_os = "macos")]
fn group_snapshot(pgid: u32) -> Result<Vec<GroupMember>, ApiError> {
    if pgid <= 1 || pgid > i32::MAX as u32 {
        return Err(unconfirmed());
    }
    // A changing or incompletely observed process list is never an empty-group
    // receipt. Retry a bounded number of scans to tolerate ordinary exits.
    for _ in 0..4 {
        let Ok(first) = group_once(pgid) else {
            continue;
        };
        if first.iter().any(|member| !member.zombie) {
            return Ok(first.into_iter().filter(|member| !member.zombie).collect());
        }
        let Ok(second) = group_once(pgid) else {
            continue;
        };
        if first == second {
            return Ok(Vec::new());
        }
    }
    Err(unconfirmed())
}

#[cfg(not(target_os = "macos"))]
fn group_snapshot(_pgid: u32) -> Result<Vec<RuntimeProcessIdentity>, ApiError> {
    Err(unconfirmed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procargs_extracts_only_nonce_and_rejects_duplicate_or_truncated_values() {
        let mut raw = 2i32.to_ne_bytes().to_vec();
        raw.extend_from_slice(b"/bin/sleep\0\0\0sleep\x0060\0PRIVATE_KEY=never-retained\0");
        raw.extend_from_slice(
            format!("{RUN_IDENTITY_ENV}=disposable-identity-nonce\0\0").as_bytes(),
        );
        assert_eq!(
            nonce_from_procargs(&raw).unwrap().as_deref(),
            Some("disposable-identity-nonce")
        );
        let mut duplicate = raw[..raw.len() - 1].to_vec();
        duplicate.extend_from_slice(format!("{RUN_IDENTITY_ENV}=other-nonce\0\0").as_bytes());
        assert!(nonce_from_procargs(&duplicate).is_err());
        assert!(nonce_from_procargs(&[1, 0, 0]).is_err());
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use super::*;
        use std::{
            io::{BufRead, BufReader},
            os::unix::process::CommandExt,
            process::{Child, Command, Stdio},
            sync::OnceLock,
        };

        fn fixture_program() -> &'static Path {
            static PROGRAM: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
            &PROGRAM
                .get_or_init(|| {
                    let temp = tempfile::tempdir().unwrap();
                    let source = temp.path().join("disposable-process-identity.c");
                    let executable = temp.path().join("disposable-process-identity");
                    // macOS hides argv environment for restricted Apple platform
                    // binaries such as /bin/sleep. Use our own executable so this
                    // tests the real nonce-observation path without weakening it.
                    std::fs::write(
                        &source,
                        r#"
#include <stdio.h>
#include <string.h>
#include <signal.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "ignore-term") == 0) {
        signal(SIGTERM, SIG_IGN);
        if (fork() == 0) { for (;;) pause(); }
        puts("ready"); fflush(stdout);
        for (;;) pause();
    }
    if (argc > 1 && strcmp(argv[1], "live-child") == 0) {
        pid_t child = fork();
        if (child == 0) { for (;;) pause(); }
        printf("%d\n", child); fflush(stdout);
        for (;;) pause();
    }
    if (argc > 1 && strcmp(argv[1], "child-ignores-term") == 0) {
        int ready[2]; if (pipe(ready) != 0) return 2;
        if (fork() == 0) {
            close(ready[0]); signal(SIGTERM, SIG_IGN);
            if (write(ready[1], "r", 1) != 1) return 3;
            close(ready[1]); for (;;) pause();
        }
        close(ready[1]); char value;
        if (read(ready[0], &value, 1) != 1) return 4;
        close(ready[0]); puts("ready"); fflush(stdout);
        for (;;) pause();
    }
    sleep(60);
    return 0;
}
"#,
                    )
                    .unwrap();
                    assert!(Command::new("/usr/bin/cc")
                        .arg(&source)
                        .arg("-o")
                        .arg(&executable)
                        .status()
                        .unwrap()
                        .success());
                    (temp, std::fs::canonicalize(executable).unwrap())
                })
                .1
        }

        struct Fixture {
            child: Child,
            pid: u32,
            nonce: String,
            armed: bool,
        }
        impl Fixture {
            fn spawn(arguments: &[&str]) -> Self {
                let nonce = uuid::Uuid::new_v4().simple().to_string();
                let child = Command::new(fixture_program())
                    .args(arguments)
                    .env(RUN_IDENTITY_ENV, &nonce)
                    .process_group(0)
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap();
                let pid = child.id();
                Self {
                    child,
                    pid,
                    nonce,
                    armed: true,
                }
            }
            fn identity(&self) -> RuntimeProcessIdentity {
                let deadline = std::time::Instant::now() + Duration::from_secs(2);
                loop {
                    if let Ok(value) = capture(self.pid, fixture_program(), &self.nonce) {
                        return value;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "disposable process identity did not become observable"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            fn reap(&mut self) {
                self.armed = false;
                let _ = self.child.wait().unwrap();
            }
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                if self.armed {
                    // The fixture still owns this unreaped direct Child, which
                    // reserves its group ID; cleanup never adopts a disk PID.
                    unsafe {
                        libc::kill(-(self.pid as i32), libc::SIGKILL);
                    }
                    let _ = self.child.wait();
                }
            }
        }

        #[test]
        fn boot_timestamp_is_observed_from_current_kernel() {
            let boot = boot_time().unwrap();
            assert!(boot > SystemTime::UNIX_EPOCH);
            assert!(boot < SystemTime::now());
        }

        #[tokio::test]
        async fn persisted_identity_mismatches_never_signal_unrelated_process() {
            let fixture = Fixture::spawn(&[]);
            let identity = fixture.identity();
            assert_eq!(
                capture_owned(fixture.pid, &fixture.nonce).unwrap(),
                identity
            );
            assert!(capture(fixture.pid, Path::new("/bin/false"), &fixture.nonce).is_err());
            assert_eq!(observe(&identity).unwrap(), Observation::Matching);
            for mismatch in [0, 1, 2, 3] {
                let mut changed = identity.clone();
                match mismatch {
                    0 => changed.nonce = uuid::Uuid::new_v4().simple().to_string(),
                    1 => changed.started_at_usec = (changed.started_at_usec + 1) % 1_000_000,
                    2 => changed.executable = "/bin/other-disposable-program".into(),
                    _ => changed.started_at_sec += 1,
                }
                assert_eq!(observe(&changed).unwrap(), Observation::Unconfirmed);
                assert!(stop_with_timeouts(
                    &changed,
                    Duration::from_millis(20),
                    Duration::from_millis(20)
                )
                .await
                .is_err());
                assert_eq!(observe(&identity).unwrap(), Observation::Matching);
            }
            let second = Fixture::spawn(&[]);
            let second_identity = second.identity();
            let mut reused = identity.clone();
            reused.pid = second.pid;
            reused.pgid = second.pid;
            assert_eq!(observe(&reused).unwrap(), Observation::Unconfirmed);
            assert!(stop(&reused).await.is_err());
            assert_eq!(observe(&second_identity).unwrap(), Observation::Matching);
        }

        #[tokio::test]
        async fn matching_process_stops_without_business_protocol_or_exit_requirements() {
            let mut fixture = Fixture::spawn(&[]);
            let identity = fixture.identity();
            let receipt =
                stop_with_timeouts(&identity, Duration::from_secs(1), Duration::from_secs(1))
                    .await
                    .unwrap();
            assert!(!receipt.forced);
            assert_eq!(observe(&identity).unwrap(), Observation::Stopped);
            fixture.reap();
            assert_eq!(observe(&identity).unwrap(), Observation::Stopped);
        }

        #[tokio::test]
        async fn ignoring_term_is_killed_only_after_identity_is_rechecked() {
            let mut fixture = Fixture::spawn(&["ignore-term"]);
            let mut line = String::new();
            BufReader::new(fixture.child.stdout.as_mut().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert_eq!(line, "ready\n");
            let identity = fixture.identity();
            let receipt =
                stop_with_timeouts(&identity, Duration::from_millis(75), Duration::from_secs(2))
                    .await
                    .unwrap();
            assert!(receipt.forced);
            fixture.reap();
            assert_eq!(observe(&identity).unwrap(), Observation::Stopped);
        }

        #[tokio::test]
        async fn exited_leader_with_live_child_is_not_an_empty_group_receipt() {
            let mut fixture = Fixture::spawn(&["live-child"]);
            let mut line = String::new();
            BufReader::new(fixture.child.stdout.as_mut().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert!(line.trim().parse::<u32>().unwrap() > 1);
            let identity = fixture.identity();
            fixture.child.kill().unwrap();
            for _ in 0..100 {
                if bsd_info(fixture.pid)
                    .unwrap()
                    .is_some_and(|info| info.pbi_status == libc::SZOMB)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(observe(&identity).unwrap(), Observation::Unconfirmed);
            assert!(stop(&identity).await.is_err());
            assert!(group_snapshot(identity.pgid)
                .unwrap()
                .iter()
                .any(|member| member.pid != identity.pid));
        }

        #[tokio::test]
        async fn stopped_leader_keeps_previously_verified_child_as_kill_anchor() {
            let mut fixture = Fixture::spawn(&["child-ignores-term"]);
            let mut line = String::new();
            BufReader::new(fixture.child.stdout.as_mut().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert_eq!(line, "ready\n");
            let identity = fixture.identity();
            let receipt =
                stop_with_timeouts(&identity, Duration::from_millis(75), Duration::from_secs(2))
                    .await
                    .unwrap();
            assert!(receipt.forced);
            fixture.reap();
            assert_eq!(observe(&identity).unwrap(), Observation::Stopped);
        }
    }
}
