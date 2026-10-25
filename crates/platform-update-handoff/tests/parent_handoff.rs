#![cfg(all(target_os = "macos", feature = "test-process-fixture"))]
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
struct Driver {
    root: tempfile::TempDir,
    parent: Child,
    lines: Receiver<String>,
    reader: Option<JoinHandle<()>>,
    nonce: Option<String>,
    pending_file: bool,
    accepted: bool,
}
impl Driver {
    fn new(mode: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut parent = Command::new(env!("CARGO_BIN_EXE_update-handoff-test-fixture"))
            .args(["parent", root.path().to_str().unwrap(), mode])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = parent.stdout.take().unwrap();
        let (send, lines) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if send.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        Self {
            root,
            parent,
            lines,
            reader: Some(reader),
            nonce: None,
            pending_file: false,
            accepted: false,
        }
    }
    fn until(&mut self, prefix: &str) -> String {
        for _ in 0..20 {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(4))
                .unwrap_or_else(|e| panic!("missing {prefix}: {e}"));
            if let Some(nonce) = line.strip_prefix("REGISTERED ") {
                self.nonce = Some(nonce.into());
            }
            if line == "ACTIVE_FILE_PENDING" {
                self.pending_file = true;
            }
            if line.starts_with("ACCEPTED_PARENT") {
                self.accepted = true;
            }
            if line.starts_with(prefix) {
                return line;
            }
        }
        panic!("missing {prefix}");
    }
    fn send(&mut self, line: &str) {
        let stdin = self.parent.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }
    fn path(&self, name: &str) -> std::path::PathBuf {
        self.root
            .path()
            .join(self.nonce.as_ref().unwrap())
            .join(name)
    }
    fn release(&mut self) {
        self.send("release");
        self.pending_file = false;
    }
    fn assert_no_observation(&self) {
        assert!(!self.path("observed-parent-exit.json").exists());
    }
    fn helper_returned(&self) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            if let Ok(bytes) = fs::read(self.path("fixture-helper-returned.json")) {
                if let Ok(value) = serde_json::from_slice(&bytes) {
                    return value;
                }
            }
            assert!(Instant::now() < deadline, "helper did not return");
            thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        if self.parent.try_wait().ok().flatten().is_none() {
            if let Some(stdin) = &mut self.parent.stdin {
                if self.pending_file {
                    let _ = writeln!(stdin, "release");
                }
                let _ = writeln!(stdin, "cancel");
                let _ = stdin.flush();
            }
        }
        let _ = self.parent.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
#[test]
fn real_fd_join_durable_ack_and_ordinary_parent_return_are_all_required() {
    let mut d = Driver::new("normal");
    d.until("ACTIVE_FILE_PENDING");
    assert!(!d.path("drain-accepted.json").exists());
    d.assert_no_observation();
    d.release();
    d.until("ACCEPTED_PARENT");
    let ack: serde_json::Value =
        serde_json::from_slice(&fs::read(d.path("drain-accepted.json")).unwrap()).unwrap();
    assert_eq!(ack["installAuthorized"], false);
    d.assert_no_observation();
    assert!(d.parent.try_wait().unwrap().is_none());
    d.send("exit");
    d.until("ORDINARY_MAIN_RETURN");
    assert!(d.parent.wait().unwrap().success());
    let deadline = Instant::now() + Duration::from_secs(4);
    let observed: serde_json::Value = loop {
        if let Ok(bytes) = fs::read(d.path("observed-parent-exit.json")) {
            if let Ok(value) = serde_json::from_slice(&bytes) {
                break value;
            }
        }
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(observed["installAuthorized"], false);
    assert_eq!(
        observed["binding"]["nonce"],
        d.nonce.as_ref().unwrap().as_str()
    );
    let returned = d.helper_returned();
    assert_eq!(returned["result"], "Ok(())");
    assert_eq!(returned["installAuthorized"], false);
    assert_eq!(
        fs::read(d.root.path().join("owned-active-file")).unwrap(),
        b"disposable handoff FD; preserve bytes"
    );
}
#[test]
fn explicit_cancel_after_accepted_waits_actual_child_and_worker_join() {
    let mut d = Driver::new("normal");
    d.until("ACTIVE_FILE_PENDING");
    d.release();
    d.until("ACCEPTED_PARENT");
    d.send("cancel");
    let report = d.until("CANCELLED Reaped");
    assert!(report.contains("install_authorized: false"));
    assert!(d.parent.wait().unwrap().success());
    d.assert_no_observation();
}
#[test]
fn dropped_api_waiter_retains_child_until_actual_wait_and_worker_join() {
    let mut d = Driver::new("drop-waiter");
    let report = d.until("DROPPED_WAITER Reaped");
    assert!(report.contains("install_authorized: false"));
    assert!(d.parent.wait().unwrap().success());
    assert!(d.path("claim.json").exists());
    d.assert_no_observation();
}
#[test]
fn real_parent_exit_without_drain_never_becomes_combined_observation() {
    let mut d = Driver::new("no-drain-exit");
    d.until("REGISTERED");
    assert!(d.parent.wait().unwrap().success());
    thread::sleep(Duration::from_millis(100));
    assert!(d.path("claim.json").exists());
    assert!(!d.path("drain-accepted.json").exists());
    d.assert_no_observation();
}
#[test]
fn closed_business_error_is_normal_quit_ready_but_has_no_strict_handoff_receipt() {
    let mut d = Driver::new("dirty");
    d.until("ACTIVE_FILE_PENDING");
    d.release();
    let report = d.until("DIRTY_NO_RECEIPT Reaped");
    assert!(report.contains("install_authorized: false"));
    assert!(d.parent.wait().unwrap().success());
    assert!(!d.path("drain-accepted.json").exists());
    d.assert_no_observation();
}
#[test]
fn genuine_coordinator_receipt_for_another_session_is_rejected() {
    let mut d = Driver::new("wrong-session");
    d.until("ACTIVE_FILE_PENDING");
    d.release();
    d.until("WRONG_SESSION_REJECTED Reaped");
    assert!(d.parent.wait().unwrap().success());
    assert!(!d.path("drain-accepted.json").exists());
    d.assert_no_observation();
}
#[test]
fn serialized_ready_and_install_flag_are_untrusted_diagnostics() {
    let mut d = Driver::new("fake-ready");
    let report = d.until("REJECTED Reaped");
    assert!(report.contains("install_authorized: false"));
    assert!(d.parent.wait().unwrap().success());
}

#[test]
fn helper_descriptor_validation_failure_closes_both_owned_fds_and_parent_reaps() {
    let mut d = Driver::new("fd-invalid");
    let report = d.until("REJECTED Reaped");
    assert!(report.contains("child_success: true"));
    assert!(report.contains("install_authorized: false"));
    assert!(d.parent.wait().unwrap().success());
}

#[test]
fn helper_restart_cannot_reuse_cancelled_or_failed_registration_claim() {
    for (mode, prefix) in [
        ("replay", "CANCELLED_SESSION_REPLAY_REJECTED"),
        ("failed-replay", "FAILED_REGISTRATION_REPLAY_REJECTED"),
    ] {
        let mut d = Driver::new(mode);
        d.until(prefix);
        assert!(d.parent.wait().unwrap().success());
        assert!(d.path("claim.json").exists());
        assert!(!d.path("drain-accepted.json").exists());
        d.assert_no_observation();
    }
}

#[test]
fn parent_cancel_before_consuming_pending_line_releases_real_file_and_reaps_helper() {
    let mut d = Driver::new("normal");
    d.until("REGISTERED");
    d.send("cancel");
    d.until("CANCELLED_BEFORE_RELEASE Reaped");
    assert!(d.parent.wait().unwrap().success());
    d.assert_no_observation();
}

#[test]
fn accepted_endpoint_drop_and_immediate_parent_return_never_commit_arm() {
    let mut d = Driver::new("accepted-drop");
    d.until("ACTIVE_FILE_PENDING");
    d.release();
    d.until("ACCEPTED_DROP_ORDINARY_RETURN");
    assert!(d.parent.wait().unwrap().success());
    let returned = d.helper_returned();
    assert_eq!(returned["installAuthorized"], false);
    assert!(!d.path("witness-armed.json").exists());
    d.assert_no_observation();
}

#[test]
fn real_parent_exit_cannot_bypass_truncated_cancel_or_unknown_tail_after_arm() {
    for mode in ["partial-exit", "cancel-exit", "unknown-exit"] {
        let mut d = Driver::new(mode);
        d.until("ADVERSARIAL_TAIL_THEN_ORDINARY_RETURN");
        assert!(d.parent.wait().unwrap().success());
        let returned = d.helper_returned();
        assert_eq!(returned["installAuthorized"], false);
        assert!(d.path("witness-armed.json").exists());
        d.assert_no_observation();
        if mode == "partial-exit" {
            assert_eq!(returned["result"], "Err(InvalidFrame)");
        }
    }
}
