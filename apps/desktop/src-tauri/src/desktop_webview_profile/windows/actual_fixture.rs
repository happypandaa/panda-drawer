//! Exact test-executable subprocesses use a fixed fixture identity and a newly
//! created disposable directory. They never call RuntimePaths or production setup.
use super::*;
use serde_json::json;
use std::{
    cell::RefCell,
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::{Command, Stdio},
    sync::atomic::{AtomicU32, AtomicUsize},
    time::{Duration, Instant},
};
use tokio::sync::Notify;
use webview2_com::{
    BrowserProcessExitedEventHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Environment5, COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL,
    },
};

const FIXTURE_ID: &str = "dev.happypandaa.pandadrawer.manager-profile-fixture";
const FIXTURE_TEST: &str =
    "desktop_webview_profile::windows::actual::fixture::real_engine_fixture_process";
const ROOT_VARIABLE: &str = "PANDA_DISPOSABLE_MANAGER_UDF_FIXTURE";
const OVERRIDE_VARIABLE: &str = "PANDA_DISPOSABLE_MANAGER_UDF_OVERRIDE_CASE";
const PURGE_VARIABLE: &str = "PANDA_DISPOSABLE_MANAGER_UDF_PURGE_CASE";

#[test]
#[ignore = "Opt-in real WebView2 fixture requires an interactive desktop and unused port 1420"]
fn real_engine_readback_accepts_expected_profile_and_rejects_udf_override_before_navigation() {
    run_manager_fixture(false);
}

#[test]
#[ignore = "Opt-in actual manager Env5 exit, released UDF pins and exact original-root cleanup"]
fn real_manager_cleanup_releases_the_engine_and_preserves_root_until_final_plan_removal() {
    run_manager_fixture(true);
}

fn run_manager_fixture(purge: bool) {
    let cases = if purge {
        vec![false]
    } else {
        vec![false, true]
    };
    for overridden in cases {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-manager-udf-real-")
            .tempdir()
            .unwrap();
        // Keep both dedicated roots as local evidence from the outset. No
        // panic or unknown engine outcome may delete an active/overridden UDF.
        let root = temporary.keep().canonicalize().unwrap();
        let foreign = root.join("external-fixture-profile");
        fs::create_dir(&foreign).unwrap();
        fs::write(
            foreign.join("sentinel"),
            b"retain external fixture contents",
        )
        .unwrap();
        let log = fs::File::create(root.join("child.log")).unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                FIXTURE_TEST,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(ROOT_VARIABLE, &root)
            .env(OVERRIDE_VARIABLE, if overridden { "true" } else { "false" })
            .env(PURGE_VARIABLE, if purge { "true" } else { "false" })
            .env_remove("WEBVIEW2_USER_DATA_FOLDER")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        if overridden {
            command.env("WEBVIEW2_USER_DATA_FOLDER", &foreign);
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let report = fs::read(root.join("report.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
        if !status.is_some_and(|status| status.success())
            || report
                .as_ref()
                .is_none_or(|report| report["success"] != true || report["browserExited"] != true)
        {
            panic!("Real manager UDF fixture failed; retained {}: status={status:?}, report={report:?}, log={}", root.display(), fs::read_to_string(root.join("child.log")).unwrap_or_default());
        }
        let report = report.unwrap();
        if purge {
            assert_eq!(report["managerRootPreservedUntilFinalRemoval"], true);
            assert_eq!(report["managerProfileActuallyAbsent"], true);
        }
        assert_eq!(report["overridden"], overridden);
        assert_eq!(report["requestsBeforeVerification"], 0);
        assert_eq!(report["pageLoaded"], !overridden);
        assert_eq!(
            fs::read(foreign.join("sentinel")).unwrap(),
            b"retain external fixture contents"
        );
        assert_eq!(
            report["verificationCode"],
            if overridden {
                "MANAGER_PROFILE_ACTUAL_MISMATCH"
            } else {
                "verified"
            }
        );
        eprintln!("Actual isolated manager UDF fixture: {report}");
        // A mismatched runtime can have written metadata before readback. Keep
        // this externally selected disposable directory intact as evidence.
        eprintln!("Retained only disposable UDF evidence: {}", root.display());
    }
}

struct Page {
    requests: Arc<AtomicUsize>,
    loaded: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Page {
    fn start() -> Self {
        // The production development manager admits exactly this origin. A
        // busy port fails this disposable test; no existing listener is stopped.
        let listener = TcpListener::bind(("127.0.0.1", 1420)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let loaded = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (count, marked, stopping) = (requests.clone(), loaded.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut input = [0u8; 8192];
                        let length = stream.read(&mut input).unwrap_or(0);
                        count.fetch_add(1, Ordering::AcqRel);
                        let loaded_request = input[..length].starts_with(b"GET /loaded ");
                        if loaded_request {
                            marked.store(true, Ordering::Release);
                        }
                        let body = if loaded_request {
                            "ok"
                        } else {
                            "<!doctype html><meta charset=utf-8><title>Disposable manager UDF fixture</title><script>localStorage.setItem('disposable-profile','verified');fetch('/loaded');</script>"
                        };
                        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            requests,
            loaded,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Page {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

#[derive(Default)]
struct ExitReceipt {
    pid: AtomicU32,
    exited: AtomicBool,
    changed: Notify,
}
thread_local! {
    static OBSERVER: RefCell<Option<(ICoreWebView2Environment5, i64)>> = const { RefCell::new(None) };
}
fn attach_exit_observer(window: &WebviewWindow) -> Result<(PathBuf, Arc<ExitReceipt>), ApiError> {
    let receipt = Arc::new(ExitReceipt::default());
    let target = receipt.clone();
    let result = Arc::new(Mutex::new(None));
    let reply = result.clone();
    window
        .with_webview(move |webview| {
            let checked = (|| {
                let path = actual_directory(&webview)?;
                let environment = webview
                    .environment()
                    .cast::<ICoreWebView2Environment5>()
                    .map_err(|_| unknown("Fixture browser exit interface missing"))?;
                let mut pid = 0;
                unsafe {
                    webview
                        .controller()
                        .CoreWebView2()
                        .and_then(|view| view.BrowserProcessId(&mut pid))
                }
                .map_err(|_| unknown("Fixture browser PID unknown"))?;
                if pid == 0 {
                    return Err(unknown("Fixture browser PID empty"));
                }
                target.pid.store(pid, Ordering::Release);
                let exited = target.clone();
                let handler = BrowserProcessExitedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut actual_pid = 0;
                    let mut kind = COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL;
                    unsafe {
                        args.BrowserProcessId(&mut actual_pid)?;
                        args.BrowserProcessExitKind(&mut kind)?;
                    }
                    if actual_pid == exited.pid.load(Ordering::Acquire)
                        && kind == COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL
                    {
                        exited.exited.store(true, Ordering::Release);
                        exited.changed.notify_waiters();
                    }
                    Ok(())
                }));
                let mut token = 0;
                unsafe { environment.add_BrowserProcessExited(&handler, &mut token) }
                    .map_err(|_| unknown("Fixture browser exit subscription failed"))?;
                OBSERVER.with(|observer| *observer.borrow_mut() = Some((environment, token)));
                Ok(path)
            })();
            *reply.lock().unwrap_or_else(|p| p.into_inner()) = Some(checked);
        })
        .map_err(|_| unknown("Fixture actual WebView dispatch failed"))?;
    let path = result
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take()
        .ok_or_else(|| unknown("Fixture actual WebView callback missing"))??;
    Ok((path, receipt))
}
async fn on_main(
    app: &AppHandle,
    work: impl FnOnce() -> Result<(), ApiError> + Send + 'static,
) -> Result<(), ApiError> {
    let (send, receive) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = send.send(work());
    })
    .map_err(|_| unknown("Fixture main dispatch failed"))?;
    receive
        .await
        .map_err(|_| unknown("Fixture main result missing"))?
}

#[test]
#[ignore = "Exact child fixture invoked only by the disposable real-engine test"]
fn real_engine_fixture_process() {
    let Some(root) = std::env::var_os(ROOT_VARIABLE) else {
        return;
    };
    let root = PathBuf::from(root).canonicalize().unwrap();
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("panda-disposable-manager-udf-real-"));
    assert!(root.join("external-fixture-profile/sentinel").is_file());
    let overridden = std::env::var(OVERRIDE_VARIABLE).unwrap() == "true";
    let page = Page::start();
    let page_loaded = page.loaded.clone();
    let request_count = page.requests.clone();
    let finishing = Arc::new(AtomicBool::new(false));
    let permit_exit = finishing.clone();
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = FIXTURE_ID.into();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default()
        .any_thread()
        .build(context)
        .unwrap();
    let mut started = false;
    let code = app.run_return(move |app, event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = &event {
            if !permit_exit.load(Ordering::Acquire) { api.prevent_exit(); }
        }
        if !matches!(event, tauri::RunEvent::Ready) || started { return; }
        started = true;
        super::super::initialize(app, &root).unwrap();
        let navigation = Arc::new(Navigation::default());
        let guard = navigation.clone();
        let config = tauri::utils::config::WindowConfig {
            label: "manager".into(),
            url: tauri::WebviewUrl::External(Url::parse("http://127.0.0.1:1420/").unwrap()),
            ..Default::default()
        };
        let builder = builder_from_config(app, &config).unwrap();
        let window = super::super::manager_builder(app, builder).unwrap()
            .visible(false).on_navigation(move |url| guard.allows(url)).build().unwrap();
        let (actual, receipt) = attach_exit_observer(&window).unwrap();
        let before = request_count.load(Ordering::Acquire);
        assert_eq!(before, 0, "Manager page navigated before actual UDF proof");
        let checked = verify_on_main(app, &window);
        let verification_code = checked.as_ref().map(|()| "verified".to_owned()).unwrap_or_else(|error| error.code.clone());
        if overridden {
            assert_eq!(verification_code, "MANAGER_PROFILE_ACTUAL_MISMATCH");
            window.destroy().unwrap();
        } else {
            checked.unwrap();
            assert!(app.manage(VerifiedManager { window: window.clone(), navigation, target: app.state::<ManagerTarget>().0.clone() }));
            activate(app).unwrap();
        }
        let app = app.clone();
        let root = root.clone();
        let loaded = page_loaded.clone();
        let finished = finishing.clone();
        tauri::async_runtime::spawn(async move {
            let purge = std::env::var(PURGE_VARIABLE).as_deref() == Ok("true");
            let mut root_preserved = false;
            let mut profile_absent = false;
            let result = async {
                if !overridden {
                    tokio::time::timeout(Duration::from_secs(15), async {
                        while !loaded.load(Ordering::Acquire) { tokio::time::sleep(Duration::from_millis(10)).await; }
                    }).await.map_err(|_| unknown("Verified fixture page did not load"))?;
                    if purge {
                        let identity = platform_native::windows::files::open_no_reparse(&actual)?.identity;
                        let check_app = app.clone();
                        let check_window = window.clone();
                        on_main(&app, move || {
                            super::super::exit::seal(&check_app,&check_window)?;
                            // Reproduce the real tray/reopen window: cleanup is
                            // sealed, but the original directory pin is valid
                            // and the old browser has not been destroyed yet.
                            check_app.state::<profile::ManagerProfile>().verify()?;
                            let candidate = tauri::WebviewWindowBuilder::new(
                                &check_app,"disposable-manager-reopen-probe",
                                tauri::WebviewUrl::External(Url::parse("about:blank").unwrap()),
                            );
                            if !matches!(super::super::manager_builder(&check_app,candidate),Err(issue) if issue.code=="MANAGER_PROFILE_RETIRED") {
                                return Err(unknown("Sealed manager unexpectedly admitted a second browser builder"));
                            }
                            Ok(())
                        }).await?;
                        super::super::close_and_clear_manager(&app).await?;
                        root_preserved = identity.same_object(&platform_native::windows::files::open_no_reparse(&actual)?.identity)
                            && fs::read_dir(&actual)?.next().transpose()?.is_none();
                        if !root_preserved { return Err(unknown("Manager cleanup replaced the reviewed root or retained contents")); }
                        // Represents the original full cleanup plan's final
                        // bound-root removal, after the native phase kept its ID.
                        platform_native::windows::removal::remove(&actual,&identity)?;
                        profile_absent = matches!(fs::symlink_metadata(&actual),Err(error) if error.kind()==std::io::ErrorKind::NotFound);
                        if !profile_absent { return Err(unknown("Manager UDF removal was not an actual absence receipt")); }
                    } else {
                        on_main(&app, move || window.destroy().map_err(|_| unknown("Fixture window destroy failed"))).await?;
                    }
                }
                tokio::time::timeout(Duration::from_secs(20), async {
                    loop {
                        let changed = receipt.changed.notified();
                        if receipt.exited.load(Ordering::Acquire) { break; }
                        changed.await;
                    }
                }).await.map_err(|_| unknown("Fixture browser process family release unconfirmed"))?;
                on_main(&app, || {
                    OBSERVER.with(|observer| {
                        if let Some((environment, token)) = observer.borrow_mut().take() {
                            unsafe { environment.remove_BrowserProcessExited(token) }
                                .map_err(|_| unknown("Fixture observer retirement failed"))?;
                        }
                        Ok(())
                    })
                }).await?;
                Ok::<(), ApiError>(())
            }.await;
            let report = json!({"success":result.is_ok(),"error":result.err(),"overridden":overridden,
                "actualUserDataFolder":actual,"requestsBeforeVerification":before,"pageLoaded":loaded.load(Ordering::Acquire),
                "verificationCode":verification_code,"browserProcessId":receipt.pid.load(Ordering::Acquire),"browserExited":receipt.exited.load(Ordering::Acquire),
                "managerRootPreservedUntilFinalRemoval":root_preserved,"managerProfileActuallyAbsent":profile_absent,
                "fixtureIdentifier":FIXTURE_ID});
            fs::write(root.join("report.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
            finished.store(true, Ordering::Release);
            app.exit(if report["success"] == true { 0 } else { 1 });
        });
    });
    drop(page);
    assert_eq!(code, 0);
}
