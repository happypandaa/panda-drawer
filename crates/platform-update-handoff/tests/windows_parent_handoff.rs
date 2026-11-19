#![cfg(all(windows, feature = "test-process-fixture"))]

use platform_native::windows::files;
use platform_update_handoff::cleanup_sessions;
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn wait_until(ready: impl Fn() -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(
            Instant::now() < until,
            "timed out waiting for update fixture"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn exercise(executable: &Path, root: &Path, mode: &str) {
    let mut parent = Command::new(executable)
        .arg(mode)
        .arg(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut name = String::new();
    BufReader::new(parent.stdout.take().unwrap())
        .read_line(&mut name)
        .unwrap();
    assert!(name.starts_with("handoff-"));
    let session = root.join(name.trim());
    let receipt = session.join("observed");
    assert!(parent.try_wait().unwrap().is_none());
    assert!(
        !receipt.exists(),
        "helper advanced before parent process exit"
    );
    parent.stdin.take().unwrap().write_all(b"X").unwrap();
    assert!(parent.wait().unwrap().success());
    if mode == "commit" {
        wait_until(|| {
            fs::read(&receipt)
                .as_deref()
                .is_ok_and(|bytes| bytes == b"exited")
        });
        fs::remove_file(&receipt).unwrap();
    }
    // On cancellation no observation may be created even after parent exit.
    // A clean session can retire only once the real helper's image is no longer in use.
    let pin = files::directory_file(root).unwrap();
    wait_until(|| {
        cleanup_sessions(&pin).unwrap();
        !session.exists()
    });
}

#[test]
fn committed_handoff_waits_for_actual_exit_and_cancellation_never_advances() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("updates");
    files::create_private_directory(&root).unwrap();
    // Cargo hard-links target/debug to target/debug/deps. NSIS installs an
    // independent executable, so reproduce that layout for the real child.
    let installed = temp.path().join("fixture.exe");
    fs::copy(
        env!("CARGO_BIN_EXE_update-handoff-test-fixture"),
        &installed,
    )
    .unwrap();
    exercise(&installed, &root, "commit");
    exercise(&installed, &root, "cancel");
}
