#![cfg(all(target_os = "macos", feature = "test-process-fixture"))]
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::Duration,
};

struct Driver {
    parent: Child,
    lines: Receiver<String>,
    reader: Option<JoinHandle<()>>,
    seen: Vec<String>,
}
impl Driver {
    fn start(mode: &str) -> Self {
        let mut parent = Command::new(env!("CARGO_BIN_EXE_update-witness-test-fixture"))
            .args(["parent", mode])
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
            parent,
            lines,
            reader: Some(reader),
            seen: Vec::new(),
        }
    }
    fn send(&mut self, command: &str) {
        let stdin = self.parent.stdin.as_mut().unwrap();
        writeln!(stdin, "{command}").unwrap();
        stdin.flush().unwrap();
    }
    fn until(&mut self, prefix: &str) -> String {
        for _ in 0..20 {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(3))
                .unwrap_or_else(|error| panic!("missing {prefix}: {error}; seen {:?}", self.seen));
            self.seen.push(line.clone());
            if line.starts_with(prefix) {
                return line;
            }
        }
        panic!("too many fixture messages: {:?}", self.seen);
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        if self.parent.try_wait().ok().flatten().is_none() {
            if let Some(stdin) = &mut self.parent.stdin {
                let _ = writeln!(stdin, "exit");
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
fn actual_parent_liveness_eof_and_ready_text_remain_pending_then_kernel_exit_yields_one_receipt() {
    let mut driver = Driver::start("normal");
    let ready = driver.until("READY ");
    assert!(ready.contains(&"a".repeat(64)));
    assert!(ready.ends_with(&driver.parent.id().to_string()));
    driver.until("ALIVE_PENDING");
    assert!(driver.parent.try_wait().unwrap().is_none());
    driver.send("eof");
    driver.until("EOF_AND_READY_TEXT_PENDING");
    assert!(driver.parent.try_wait().unwrap().is_none());
    driver.send("exit");
    assert!(driver.parent.wait().unwrap().success());
    let exited = driver.until("EXITED ");
    assert!(exited.ends_with(&driver.parent.id().to_string()));
    driver.until("EXIT_EVIDENCE_AND_NONCE_REPLAY_REJECTED");
}

#[test]
fn actual_helper_refuses_wrong_parent_start_identity_and_burns_failed_nonce() {
    let mut driver = Driver::start("mismatch");
    driver.until("REJECTED ProcessIdentityChanged");
    driver.until("FAILED_NONCE_REPLAY_REJECTED");
    assert!(driver.parent.try_wait().unwrap().is_none());
}

#[test]
fn actual_helper_refuses_a_pid_that_is_not_its_parent() {
    let mut driver = Driver::start("nonparent");
    driver.until("REJECTED NotActualParent");
    driver.until("FAILED_NONCE_REPLAY_REJECTED");
    assert!(driver.parent.try_wait().unwrap().is_none());
}

#[test]
fn parent_disappearance_before_registration_never_becomes_exit_evidence() {
    let mut driver = Driver::start("late");
    driver.until("UNREGISTERED");
    driver.send("exit");
    assert!(driver.parent.wait().unwrap().success());
    driver.until("REJECTED ");
    driver.until("FAILED_NONCE_REPLAY_REJECTED");
    assert!(!driver
        .seen
        .iter()
        .any(|line| line.starts_with("READY ") || line.starts_with("EXITED ")));
}
