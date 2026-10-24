//! Real retained children/pipe fixtures for the macOS exit/signal race. No
//! signals target an ID after reap; incomplete observations remain unknown.
use super::*;
#[cfg(target_os = "macos")]
use std::{
    io::{Read, Write},
    os::unix::process::CommandExt,
};
#[cfg(target_os = "macos")]
fn paused_child() -> std::process::Child {
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "printf R; read value; exit 0"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    let mut ready = [0];
    child
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut ready)
        .unwrap();
    assert_eq!(ready, [b'R']);
    child
}
#[cfg(target_os = "macos")]
async fn exited(group: &mut ProcessGroup) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while group.observe().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}
#[cfg(target_os = "macos")]
async fn reap(group: &mut ProcessGroup, child: &mut std::process::Child) {
    exited(group).await;
    assert!(!group.has_descendants().unwrap());
    group.disarm_before_reap();
    child.wait().unwrap();
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn signal_reobserve_real_natural_exit_race_does_not_stick_the_owned_group() {
    let mut errors = 0;
    for index in 0..100 {
        let mut child = paused_child();
        let mut group = ProcessGroup::new(child.id());
        child.stdin.as_mut().unwrap().write_all(b"go\n").unwrap();
        if index % 3 != 0 {
            tokio::task::yield_now().await;
        }
        if let Err(error) = group.signal(group::TERMINATE) {
            assert_eq!(error.code, "PROCESS_SIGNAL_FAILED");
            assert_eq!(error.details["errno"], libc::EPERM);
            errors += 1;
        }
        retry_owned_signal("PROCESS_SIGNAL_FAILED", || group.signal(group::TERMINATE))
            .await
            .unwrap();
        reap(&mut group, &mut child).await;
    }
    eprintln!("signal reobserve: 100 real natural-exit cases, {errors} immediate EPERM errors rescued; all actual leaders reaped");
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn signal_reobserve_persistent_actual_syscall_error_yields_and_never_claims_closed() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let mut child = paused_child();
    let mut group = ProcessGroup::new(child.id());
    let pulse = Arc::new(AtomicBool::new(false));
    let signal = pulse.clone();
    let ticker = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1)).await;
        signal.store(true, Ordering::Release);
    });
    let mut calls = 0;
    let error = retry_owned_signal("PROCESS_SIGNAL_FAILED", || {
        calls += 1;
        group.signal(i32::MAX)
    })
    .await
    .unwrap_err();
    assert_eq!(error.code, "PROCESS_SIGNAL_FAILED");
    assert_eq!(error.details["errno"], libc::EINVAL);
    assert_eq!(error.details["initialSignalErrno"], libc::EINVAL);
    assert_eq!(error.details["signalObservationAttempts"], 4);
    assert_eq!(calls, 4);
    assert!(pulse.load(Ordering::Acquire));
    assert!(group.observe().unwrap().is_none());
    ticker.await.unwrap();
    retry_owned_signal("PROCESS_SIGNAL_FAILED", || group.signal(group::KILL))
        .await
        .unwrap();
    reap(&mut group, &mut child).await;
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn signal_reobserve_lost_child_identity_returns_before_any_retry() {
    let mut child = std::process::Command::new("/usr/bin/true")
        .process_group(0)
        .spawn()
        .unwrap();
    let mut group = ProcessGroup::new(child.id());
    child.wait().unwrap();
    let mut calls = 0;
    let error = retry_owned_signal("PROCESS_SIGNAL_FAILED", || {
        calls += 1;
        group.signal(group::KILL)
    })
    .await
    .unwrap_err();
    assert_eq!(error.code, "PROCESS_OWNERSHIP_LOST");
    assert_eq!(calls, 1);
    assert_eq!(
        group.signal(group::KILL).unwrap_err().code,
        "PROCESS_OWNERSHIP_RELEASED"
    );
}
#[tokio::test]
async fn signal_reobserve_incomplete_group_evidence_is_not_a_retryable_signal_failure() {
    let mut calls = 0;
    let error = retry_owned_signal("PROCESS_SIGNAL_FAILED", || {
        calls += 1;
        Err(
            ApiError::new("PROCESS_GROUP_UNCONFIRMED", "artificial incomplete scan")
                .outcome("unknown"),
        )
    })
    .await
    .unwrap_err();
    assert_eq!(error.code, "PROCESS_GROUP_UNCONFIRMED");
    assert_eq!(calls, 1);
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn signal_reobserve_empty_original_group_still_does_not_close_a_separate_setsid_writer() {
    use std::os::{fd::OwnedFd, unix::net::UnixStream};
    let (mut read, write) = UnixStream::pair().unwrap();
    // This directly-owned sibling models a detached writer; it is not a claim
    // that the platform owns or can recover an arbitrary escaped grandchild.
    let mut holder=std::process::Command::new("/usr/bin/python3")
        .args(["-c","import os,sys;os.setsid();sys.stdout.write('R');sys.stdout.flush();sys.stdin.buffer.read(1)"])
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::from(OwnedFd::from(write)))
        .stderr(std::process::Stdio::null()).spawn().unwrap();
    let mut ready = [0];
    read.read_exact(&mut ready).unwrap();
    assert_eq!(ready, [b'R']);
    let mut holder_group = ProcessGroup::new(holder.id());
    let mut leader = std::process::Command::new("/usr/bin/true")
        .process_group(0)
        .spawn()
        .unwrap();
    let mut group = ProcessGroup::new(leader.id());
    exited(&mut group).await;
    retry_owned_signal("PROCESS_SIGNAL_FAILED", || group.signal(group::TERMINATE))
        .await
        .unwrap();
    assert!(!group.has_descendants().unwrap());
    read.set_nonblocking(true).unwrap();
    let mut read = tokio::net::UnixStream::from_std(read).unwrap();
    let mut output = vec![];
    assert!(
        tokio::time::timeout(Duration::from_millis(20), read.read_to_end(&mut output))
            .await
            .is_err()
    );
    retry_owned_signal("PROCESS_SIGNAL_FAILED", || holder_group.signal(group::KILL))
        .await
        .unwrap();
    reap(&mut holder_group, &mut holder).await;
    tokio::time::timeout(Duration::from_secs(1), read.read_to_end(&mut output))
        .await
        .unwrap()
        .unwrap();
    reap(&mut group, &mut leader).await;
}
