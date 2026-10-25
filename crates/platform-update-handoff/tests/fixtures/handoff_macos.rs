//! Real own-parent fixture. Includes the production pure coordinator, never Tauri.
#[path = "../../../../apps/desktop/src-tauri/src/desktop_shutdown.rs"]
mod desktop_shutdown;
use desktop_shutdown::*;
use platform_update_handoff::{
    helper_from_inherited_descriptors, owner_state, ParentEndpoint, SessionBinding,
    SessionDirectory,
};
use std::{
    fs::File,
    io::{BufRead, Read, Write},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::oneshot;

type Work = std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Issue>> + Send>>;
struct BackendFixture {
    release: Mutex<Option<oneshot::Receiver<()>>>,
    file: Mutex<Option<File>>,
    ready: AtomicBool,
    dirty: bool,
}
impl Backend for BackendFixture {
    fn start_core(&self) {}
    fn core_state(&self) -> CoreState {
        CoreState {
            closed: true,
            blocked: false,
            issues: vec![],
        }
    }
    fn confirm_core(&self) -> Work {
        Box::pin(async { Ok(()) })
    }
    fn windows(&self) -> Work {
        let wait = self.release.lock().unwrap().take().unwrap();
        let file = self.file.lock().unwrap().take().unwrap();
        Box::pin(async move {
            wait.await
                .map_err(|_| Issue::new("FIXTURE_RELEASE_LOST", false, false))?;
            drop(file);
            Ok(())
        })
    }
    fn nodes(&self) -> Work {
        let dirty = self.dirty;
        Box::pin(async move {
            if dirty {
                Err(Issue::new("FIXTURE_CLOSED_BUSINESS_ERROR", true, false))
            } else {
                Ok(())
            }
        })
    }
    fn sessions(&self) -> Work {
        Box::pin(async { Ok(()) })
    }
    fn changed(&self, _: &Report) {}
    fn ready(&self) {
        self.ready.store(true, Ordering::Release);
    }
}
fn line(text: &str) {
    println!("{text}");
    std::io::stdout().flush().unwrap();
}
// Test-only adversarial launcher; its wire statement is never a production receipt.
fn raw_launch(
    root: &str,
    binding: &SessionBinding,
    wrong_start: bool,
) -> (std::os::unix::net::UnixStream, std::process::Child) {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::{net::UnixStream, process::CommandExt},
    };
    let (mut pipe, helper_pipe) = UnixStream::pair().unwrap();
    let directory = File::open(std::path::Path::new(root).join(binding.nonce())).unwrap();
    let duplicate = |fd| {
        let fd = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 10) };
        assert!(fd >= 10);
        unsafe { File::from_raw_fd(fd) }
    };
    let socket_source = duplicate(helper_pipe.as_raw_fd());
    let directory_source = duplicate(directory.as_raw_fd());
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg("helper")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    unsafe {
        command.pre_exec(move || {
            for (source, target) in [
                (socket_source.as_raw_fd(), 3),
                (directory_source.as_raw_fd(), 4),
            ] {
                if libc::dup2(source, target) < 0 || libc::fcntl(target, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    drop(command);
    drop(helper_pipe);
    let parent = binding.parent();
    let hello = serde_json::json!({"kind":"Hello","binding":{"version":1,"nonce":binding.nonce(),"pid":parent.pid(),"start_seconds":parent.start_seconds()+u64::from(wrong_start),"start_microseconds":parent.start_microseconds(),"effective_uid":parent.effective_uid()}});
    let bytes = serde_json::to_vec(&hello).unwrap();
    pipe.write_all(&(bytes.len() as u32).to_be_bytes()).unwrap();
    pipe.write_all(&bytes).unwrap();
    (pipe, child)
}
fn raw_restart(root: &str, binding: &SessionBinding, wrong_start: bool) -> bool {
    let (_pipe, mut child) = raw_launch(root, binding, wrong_start);
    child.wait().unwrap().success()
}
fn raw_message(pipe: &mut std::os::unix::net::UnixStream) -> serde_json::Value {
    let mut prefix = [0; 4];
    pipe.read_exact(&mut prefix).unwrap();
    let size = u32::from_be_bytes(prefix) as usize;
    assert!(size <= 4096);
    let mut bytes = vec![0; size];
    pipe.read_exact(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
fn parent(root: &str, mode: &str) {
    let root_file = File::open(root).unwrap();
    let session = SessionDirectory::create_in(&root_file).unwrap();
    let binding = session.binding().clone();
    if mode == "partial-exit" || mode == "cancel-exit" || mode == "unknown-exit" {
        let (mut pipe, child) = raw_launch(root, &binding, false);
        pipe.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let registered = raw_message(&mut pipe);
        assert_eq!(registered["kind"], "Registered");
        let drain = serde_json::json!({"kind":"Drain","binding":registered["binding"]});
        let bytes = serde_json::to_vec(&drain).unwrap();
        pipe.write_all(&(bytes.len() as u32).to_be_bytes()).unwrap();
        pipe.write_all(&bytes).unwrap();
        assert_eq!(raw_message(&mut pipe)["kind"], "Accepted");
        let arm = serde_json::json!({"kind":"Arm","binding":registered["binding"]});
        let bytes = serde_json::to_vec(&arm).unwrap();
        pipe.write_all(&(bytes.len() as u32).to_be_bytes()).unwrap();
        pipe.write_all(&bytes).unwrap();
        assert_eq!(raw_message(&mut pipe)["kind"], "Armed");
        match mode {
            "partial-exit" => pipe.write_all(&[0, 0]).unwrap(),
            _ => {
                let bytes = if mode == "cancel-exit" {
                    br#"{"kind":"Cancel"}"#.as_slice()
                } else {
                    br#"{"kind":"ReadyForNativeExit"}"#.as_slice()
                };
                pipe.write_all(&(bytes.len() as u32).to_be_bytes()).unwrap();
                pipe.write_all(bytes).unwrap();
            }
        }
        line(&format!("REGISTERED {}", binding.nonce()));
        line("ADVERSARIAL_TAIL_THEN_ORDINARY_RETURN installAuthorized=false");
        // Deliberate parent exit: the test helper must reject the tail even if
        // its kqueue already reports NOTE_EXIT. No signals or stale PID wait.
        drop(pipe);
        drop(child);
        return;
    }
    if mode == "failed-replay" {
        assert!(!raw_restart(root, &binding, true));
        let path = std::path::Path::new(root)
            .join(binding.nonce())
            .join("claim.json");
        let claim = std::fs::read(&path).unwrap();
        assert!(!raw_restart(root, &binding, false));
        assert_eq!(claim, std::fs::read(path).unwrap());
        line(&format!("REGISTERED {}", binding.nonce()));
        line("FAILED_REGISTRATION_REPLAY_REJECTED installAuthorized=false");
        return;
    }
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .arg(match mode {
            "fake-ready" => "fake-ready",
            "fd-invalid" => "bad-descriptor-helper",
            _ => "helper",
        })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut endpoint = ParentEndpoint::<CleanDrainReceipt>::launch(session, command).unwrap();
    if mode == "fake-ready" || mode == "fd-invalid" {
        assert!(endpoint.wait_registered(Duration::from_secs(3)).is_err());
        line(&format!(
            "REJECTED {:?}",
            endpoint.cancel_and_wait(Duration::from_secs(3))
        ));
        return;
    }
    endpoint.wait_registered(Duration::from_secs(3)).unwrap();
    line(&format!("REGISTERED {}", binding.nonce()));
    if mode == "replay" {
        let state = endpoint.cancel_and_wait(Duration::from_secs(3));
        assert!(matches!(
            state,
            platform_update_handoff::OwnerState::Reaped { .. }
        ));
        let path = std::path::Path::new(root)
            .join(binding.nonce())
            .join("claim.json");
        let claim = std::fs::read(&path).unwrap();
        assert!(!raw_restart(root, &binding, false));
        assert_eq!(claim, std::fs::read(path).unwrap());
        line("CANCELLED_SESSION_REPLAY_REJECTED installAuthorized=false");
        return;
    }
    if mode == "no-drain-exit" {
        return;
    }
    if mode == "drop-waiter" {
        drop(endpoint);
        for _ in 0..600 {
            let state = owner_state(&binding);
            if state != platform_update_handoff::OwnerState::Retained {
                line(&format!("DROPPED_WAITER {state:?}"));
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("owner not reaped");
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let shutdown = Arc::new(DesktopShutdown::default());
    let arm_binding = if mode == "wrong-session" {
        SessionDirectory::create_in(&root_file)
            .unwrap()
            .binding()
            .clone()
    } else {
        binding.clone()
    };
    shutdown.arm_handoff(arm_binding).unwrap();
    let (release, wait) = oneshot::channel();
    let path = std::path::Path::new(root).join("owned-active-file");
    std::fs::write(&path, b"disposable handoff FD; preserve bytes").unwrap();
    let backend = Arc::new(BackendFixture {
        release: Mutex::new(Some(wait)),
        file: Mutex::new(Some(File::open(path).unwrap())),
        ready: AtomicBool::new(false),
        dirty: mode == "dirty",
    });
    shutdown.request(backend.clone(), Some(runtime.handle().clone()), || {
        Ok((Box::new(()), Box::new(())))
    });
    assert!(shutdown.take_clean_drain_receipt().is_none());
    line("ACTIVE_FILE_PENDING");
    let mut input = std::io::stdin().lock().lines();
    let cancelled = !matches!(input.next(), Some(Ok(command)) if command == "release");
    release.send(()).unwrap();
    for _ in 0..600 {
        if backend.ready.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(backend.ready.load(Ordering::Acquire));
    if cancelled {
        line(&format!(
            "CANCELLED_BEFORE_RELEASE {:?}",
            endpoint.cancel_and_wait(Duration::from_secs(3))
        ));
        return;
    }
    let mut receipt = shutdown.take_clean_drain_receipt();
    if mode != "dirty" {
        for _ in 0..600 {
            if receipt.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
            receipt = shutdown.take_clean_drain_receipt();
        }
    }
    if mode == "dirty" {
        assert!(receipt.is_none());
        line(&format!(
            "DIRTY_NO_RECEIPT {:?}",
            endpoint.cancel_and_wait(Duration::from_secs(3))
        ));
        return;
    }
    if mode == "wrong-session" {
        assert!(matches!(
            endpoint.acknowledge_drained(receipt.unwrap(), Duration::from_secs(3)),
            Err(platform_update_handoff::Error::BindingMismatch)
        ));
        line(&format!(
            "WRONG_SESSION_REJECTED {:?}",
            endpoint.cancel_and_wait(Duration::from_secs(3))
        ));
        return;
    }
    let observation = endpoint
        .acknowledge_drained(receipt.unwrap(), Duration::from_secs(3))
        .unwrap();
    assert!(!observation.install_authorized());
    line("ACCEPTED_PARENT_STILL_ALIVE installAuthorized=false");
    if mode == "accepted-drop" {
        drop(endpoint);
        line("ACCEPTED_DROP_ORDINARY_RETURN");
        return;
    }
    match input.next() {
        Some(Ok(command)) if command == "exit" => {
            let armed = endpoint
                .leave_witness_armed(observation, Duration::from_secs(3))
                .unwrap();
            assert!(!armed.install_authorized());
            line("ORDINARY_MAIN_RETURN");
        }
        _ => line(&format!(
            "CANCELLED {:?}",
            endpoint.cancel_and_wait(Duration::from_secs(3))
        )),
    }
}
pub(super) fn run() {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("bad-descriptor-helper") => {
            assert_eq!(unsafe { libc::dup2(4, 3) }, 3);
            assert!(matches!(
                unsafe { helper_from_inherited_descriptors() },
                Err(platform_update_handoff::Error::Protocol)
            ));
            for fd in [3, 4] {
                assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::EBADF)
                );
            }
        }
        Some("helper") => {
            use std::os::fd::{AsRawFd, FromRawFd};
            let duplicate = unsafe { libc::fcntl(4, libc::F_DUPFD_CLOEXEC, 10) };
            assert!(duplicate >= 10);
            let directory = unsafe { File::from_raw_fd(duplicate) };
            let result = unsafe { helper_from_inherited_descriptors() };
            let name = b"fixture-helper-returned.json\0";
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr().cast(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if fd >= 0 {
                let mut evidence = unsafe { File::from_raw_fd(fd) };
                let bytes = serde_json::to_vec(
                    &serde_json::json!({"result":format!("{result:?}"),"installAuthorized":false}),
                )
                .unwrap();
                evidence.write_all(&bytes).unwrap();
                evidence.sync_all().unwrap();
                directory.sync_all().unwrap();
            }
            if let Err(error) = result {
                eprintln!("helper rejected: {error:?}");
                std::process::exit(2)
            }
        }
        Some("fake-ready") => {
            use std::os::fd::FromRawFd;
            let mut pipe = unsafe { std::os::unix::net::UnixStream::from_raw_fd(3) };
            let payload = br#"{"kind":"ReadyForNativeExit","installAuthorized":true}"#;
            pipe.write_all(&(payload.len() as u32).to_be_bytes())
                .unwrap();
            pipe.write_all(payload).unwrap();
        }
        Some("parent") => parent(&args[2], &args[3]),
        _ => panic!("fixture mode required"),
    }
}
