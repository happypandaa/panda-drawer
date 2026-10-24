// Private Trial regressions. Real direct Child
// handles remain retained until group checks and final explicit reap.
use super::*;
#[cfg(target_os = "macos")]
#[tokio::test]
async fn trial_signal_reobserve_retained_zombie_accepts_only_fresh_empty_group() {
    use std::os::unix::process::CommandExt;
    let mut child = std::process::Command::new("/usr/bin/true")
        .process_group(0)
        .spawn()
        .unwrap();
    let owner = ProcessOwnership::new(child.id());
    assert!(wait_for_owned_exit(&owner, Duration::from_secs(2))
        .await
        .unwrap());
    owner.signal(SIGNAL_TERM).unwrap();
    owner.signal_with_reobserve(SIGNAL_KILL).await.unwrap();
    assert!(!owner.has_descendants().unwrap());
    owner.disarm_before_reap();
    child.wait().unwrap();
    assert_eq!(
        owner
            .signal_with_reobserve(SIGNAL_KILL)
            .await
            .unwrap_err()
            .code,
        "TRIAL_OWNERSHIP_RELEASED"
    );
}
#[cfg(target_os = "macos")]
#[tokio::test]
async fn trial_signal_reobserve_real_natural_exit_race_and_persistent_alive_error() {
    use std::{
        io::{Read, Write},
        os::unix::process::CommandExt,
    };
    let mut rescued = 0;
    for _ in 0..50 {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "printf R; read value; exit 0"])
            .process_group(0)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
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
        let owner = ProcessOwnership::new(child.id());
        child.stdin.as_mut().unwrap().write_all(b"go\n").unwrap();
        if let Err(error) = owner.signal(SIGNAL_TERM) {
            assert_eq!(error.code, "TRIAL_SIGNAL_FAILED");
            assert_eq!(error.details["errno"], libc::EPERM);
            rescued += 1;
        }
        owner.signal_with_reobserve(SIGNAL_TERM).await.unwrap();
        assert!(wait_for_owned_exit(&owner, Duration::from_secs(2))
            .await
            .unwrap());
        assert!(!owner.has_descendants().unwrap());
        owner.disarm_before_reap();
        child.wait().unwrap();
    }
    eprintln!("Trial signal reobserve: 50 real natural-exit cases, {rescued} immediate EPERM errors rescued; actual reap retained");
    let mut child = std::process::Command::new("/bin/sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let owner = ProcessOwnership::new(child.id());
    let error = owner.signal_with_reobserve(i32::MAX).await.unwrap_err();
    assert_eq!(error.code, "TRIAL_SIGNAL_FAILED");
    assert_eq!(error.details["errno"], libc::EINVAL);
    assert_eq!(error.details["initialSignalErrno"], libc::EINVAL);
    assert_eq!(error.details["signalObservationAttempts"], 4);
    assert!(owner.observe().unwrap().is_none());
    owner.signal_with_reobserve(SIGNAL_KILL).await.unwrap();
    assert!(wait_for_owned_exit(&owner, Duration::from_secs(2))
        .await
        .unwrap());
    owner.disarm_before_reap();
    child.wait().unwrap();
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn trial_signal_reobserve_disarm_during_async_backoff_never_resignals() {
    use std::{
        io::{Read, Write},
        os::unix::process::CommandExt,
    };
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "printf R; read value; exit 0"])
        .process_group(0)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = [0];
    child
        .stdout
        .as_mut()
        .unwrap()
        .read_exact(&mut ready)
        .unwrap();
    let owner = Arc::new(ProcessOwnership::new(child.id()));
    let worker = owner.clone();
    let (entered, first) = tokio::sync::oneshot::channel();
    let runner = tokio::spawn(async move {
        let mut entered = Some(entered);
        crate::owned_process::retry_owned_signal("TRIAL_SIGNAL_FAILED", || {
            let result = worker.signal(i32::MAX);
            if let Some(entered) = entered.take() {
                entered.send(()).unwrap();
            }
            result
        })
        .await
    });
    first.await.unwrap();
    // The std Mutex is available while retry is sleeping. Revoke before the
    // only actual reap; the next attempt may not send another numeric signal.
    assert!(owner.observe().unwrap().is_none());
    owner.disarm_before_reap();
    child.stdin.as_mut().unwrap().write_all(b"go\n").unwrap();
    child.wait().unwrap();
    let error = runner.await.unwrap().unwrap_err();
    assert_eq!(error.code, "TRIAL_OWNERSHIP_RELEASED");
    assert_eq!(error.details["initialSignalErrno"], libc::EINVAL);
    assert_eq!(owner.signals_sent.load(Ordering::Relaxed), 0);
}
