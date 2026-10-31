//! Real engine events, original production stage/ticket code, disposable UDF.
//! Core publisher tests separately exercise real project admission/deletion.
//! This does not automate or claim proof of the interactive native save dialog.
use super::*;
use platform_native::windows::files as native;
use serde_json::json;
use std::{
    cell::RefCell,
    ffi::OsString,
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, AtomicU32, AtomicUsize},
    time::{Duration, Instant},
};
use tauri::{Manager, WebviewWindow};
use tokio::sync::Notify;
use webview2_com::{
    BrowserProcessExitedEventHandler, CoTaskMemPWSTR, DownloadStartingEventHandler,
    Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Environment5, ICoreWebView2Environment7, ICoreWebView2_4,
        COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL,
    },
};
use windows::core::{Interface, PWSTR};
const ROOT_VARIABLE: &str = "PANDA_DISPOSABLE_DOWNLOAD_ENGINE_FIXTURE";
const MODE_VARIABLE: &str = "PANDA_DISPOSABLE_DOWNLOAD_ENGINE_CANCEL";
const CHILD: &str = "browser_download::windows::fixture::actual_download_engine_fixture_process";
const PAYLOAD: &[u8] = b"disposable real WebView2 download bytes";

#[test]
#[ignore = "Opt-in actual WebView2 downloads require an interactive desktop; disposable roots only"]
fn actual_webview2_requested_finished_and_native_cancel_use_original_stages() {
    for cancel in [false, true] {
        let root = tempfile::Builder::new()
            .prefix("panda-disposable-download-engine-")
            .tempdir()
            .unwrap()
            .keep();
        let log = fs::File::create(root.join("child.log")).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                CHILD,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(ROOT_VARIABLE, &root)
            .env(MODE_VARIABLE, if cancel { "true" } else { "false" })
            .env_remove("TERM")
            .env_remove("WEBVIEW2_USER_DATA_FOLDER")
            .env_remove("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(80);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!(
                    "Real download fixture timed out; preserved {}",
                    root.display()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("report.json")).unwrap_or_default())
                .unwrap_or_else(|error| {
                    panic!(
                        "Fixture report absent: {error}; preserved {}",
                        root.display()
                    )
                });
        assert!(status.success(), "{report}; preserved {}", root.display());
        assert_eq!(report["requested"], 1);
        assert_eq!(report["finished"], 1);
        assert_eq!(report["engineSuccess"], !cancel);
        assert_eq!(report["stageActuallyAbsent"], true);
        assert_eq!(report["browserExited"], true);
        assert_eq!(report["actualUdfMatched"], true);
        assert_eq!(report["payloadMatched"], !cancel);
        // Keep the small report/log and disposable UDF as local evidence. No
        // recursively computed cleanup path is used by this physical fixture.
    }
}

#[derive(Default)]
struct Observations {
    requested: AtomicUsize,
    finished: AtomicUsize,
    success: AtomicBool,
    payload: AtomicBool,
    absent: AtomicBool,
    pid: AtomicU32,
    exited: AtomicBool,
    destroyed: AtomicBool,
    actual_udf: AtomicBool,
    changed: Notify,
    error: Mutex<Option<String>>,
}
thread_local! {
    static OBSERVERS:RefCell<Option<(ICoreWebView2Environment5,i64)>>=const {RefCell::new(None)};
    static DOWNLOAD:RefCell<Option<(ICoreWebView2_4,i64)>>=const {RefCell::new(None)};
}
fn native_observers(
    window: &WebviewWindow,
    root: PathBuf,
    cancel: bool,
    observed: Arc<Observations>,
) {
    window
        .with_webview(move |webview| {
            let result = (|| -> windows::core::Result<()> {
                let environment = webview.environment();
                let environment7 = environment.cast::<ICoreWebView2Environment7>()?;
                let mut path = PWSTR::null();
                let result = unsafe { environment7.UserDataFolder(&mut path) };
                let _allocation = CoTaskMemPWSTR::from(path);
                result?;
                if path.is_null() {
                    return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                        0x80004005u32 as i32,
                    )));
                }
                let actual = PathBuf::from(OsString::from_wide(unsafe { path.as_wide() }));
                let wanted = native::open_no_reparse(&root).unwrap().identity;
                observed.actual_udf.store(
                    wanted.same_object(&native::open_no_reparse(&actual).unwrap().identity),
                    Ordering::Release,
                );
                let view = unsafe { webview.controller().CoreWebView2() }?;
                let mut pid = 0;
                unsafe { view.BrowserProcessId(&mut pid) }?;
                assert_ne!(pid, 0);
                observed.pid.store(pid, Ordering::Release);
                let environment5 = environment.cast::<ICoreWebView2Environment5>()?;
                let target = observed.clone();
                let exit = BrowserProcessExitedEventHandler::create(Box::new(move |_, args| {
                    if let Some(args) = args {
                        let mut pid = 0;
                        let mut kind = COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL;
                        unsafe {
                            args.BrowserProcessId(&mut pid)?;
                            args.BrowserProcessExitKind(&mut kind)?;
                        }
                        if pid == target.pid.load(Ordering::Acquire)
                            && kind == COREWEBVIEW2_BROWSER_PROCESS_EXIT_KIND_NORMAL
                        {
                            target.exited.store(true, Ordering::Release);
                            target.changed.notify_waiters();
                        }
                    }
                    Ok(())
                }));
                let mut exit_token = 0;
                unsafe { environment5.add_BrowserProcessExited(&exit, &mut exit_token) }?;
                let view4 = view.cast::<ICoreWebView2_4>()?;
                let starting = DownloadStartingEventHandler::create(Box::new(move |_, args| {
                    if cancel {
                        if let Some(args) = args {
                            unsafe { args.DownloadOperation()?.Cancel() }?;
                        }
                    }
                    Ok(())
                }));
                let mut download_token = 0;
                unsafe { view4.add_DownloadStarting(&starting, &mut download_token) }?;
                OBSERVERS.with(|slot| *slot.borrow_mut() = Some((environment5, exit_token)));
                DOWNLOAD.with(|slot| *slot.borrow_mut() = Some((view4, download_token)));
                Ok(())
            })();
            if let Err(error) = result {
                *observed.error.lock().unwrap() = Some(error.to_string());
            }
        })
        .unwrap();
}

#[test]
#[ignore = "Exact child process invoked only by the disposable interactive download fixture"]
fn actual_download_engine_fixture_process() {
    let Some(root) = std::env::var_os(ROOT_VARIABLE) else {
        return;
    };
    let root = PathBuf::from(root).canonicalize().unwrap();
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("panda-disposable-download-engine-"));
    let cancel = std::env::var(MODE_VARIABLE).as_deref() == Ok("true");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = Url::parse(&format!(
        "http://{}/p/prj_disposable/payload",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = stop.clone();
    let server = std::thread::spawn(move || {
        while !server_stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    stream
                        .set_write_timeout(Some(Duration::from_secs(3)))
                        .unwrap();
                    let mut request = [0; 8192];
                    let _ = stream.read(&mut request);
                    let length = if cancel {
                        16 * 1024 * 1024
                    } else {
                        PAYLOAD.len()
                    };
                    let header=format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=disposable.txt\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n");
                    if stream.write_all(header.as_bytes()).is_ok() {
                        if cancel {
                            let chunk = [42; 8192];
                            for _ in 0..length / chunk.len() {
                                if server_stop.load(Ordering::Acquire)
                                    || stream.write_all(&chunk).is_err()
                                {
                                    break;
                                }
                            }
                        } else {
                            let _ = stream.write_all(PAYLOAD);
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(_) => break,
            }
        }
    });
    let observed = Arc::new(Observations::default());
    let engines = Arc::new(super::super::super::engine_downloads::EngineDownloads::default());
    let io = platform_core::io_lifetime::IoLifetime::default();
    let pending = Arc::new(Mutex::new(None::<EngineDownload>));
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = "dev.happypandaa.pandadrawer.download-fixture".into();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default()
        .any_thread()
        .build(context)
        .unwrap();
    let allow_exit = Arc::new(AtomicBool::new(false));
    let exit = allow_exit.clone();
    let mut started = false;
    let code=app.run_return(move |app,event| {
        if let tauri::RunEvent::ExitRequested {api,..}=&event {if !exit.load(Ordering::Acquire){api.prevent_exit();}}
        if !matches!(event,tauri::RunEvent::Ready)||started{return;}started=true;
        let profile=root.join("isolated profile");native::create_private_directory(&profile).unwrap();
        let parent=root.join("下载 空间");fs::create_dir(&parent).unwrap();
        let events=observed.clone();let downloads=pending.clone();let owner=engines.clone();let lifetime=io.clone();
        let window=tauri::WebviewWindowBuilder::new(app,"disposable-download",tauri::WebviewUrl::External(Url::parse("about:blank").unwrap()))
            .data_directory(profile.clone()).visible(false).on_download(move |_,event|match event {
                DownloadEvent::Requested {destination,..}=>{
                    let (lease,project,mut ticket)=owner.admit_project("prj_disposable",||lifetime.admit()).unwrap();
                    let stage=PendingDownload::create_at(&parent.join("native-downloads"),"人工 中文 下载.txt").unwrap();
                    *destination=stage.file.clone();stage.engine_accepted.store(true,Ordering::Release);ticket.started();
                    *downloads.lock().unwrap()=Some(EngineDownload {resource:Some(project.hold(lease.hold(stage))),ticket:Some(ticket)});
                    events.requested.fetch_add(1,Ordering::AcqRel);events.changed.notify_waiters();true
                },
                DownloadEvent::Finished {success,..}=>{
                    let Some(engine)=downloads.lock().unwrap().take() else{return false;};let stage=engine.finished();let file=pending_stage(&stage).file.clone();
                    if success {match pending_stage(&stage).open_file().and_then(|mut file|{let mut bytes=Vec::new();file.read_to_end(&mut bytes)?;Ok(bytes)}) {
                        Ok(bytes)=>events.payload.store(bytes==PAYLOAD,Ordering::Release),Err(error)=>*events.error.lock().unwrap()=Some(error.message),
                    }}
                    drop(stage);events.absent.store(!file.parent().unwrap().exists(),Ordering::Release);events.success.store(success,Ordering::Release);
                    events.finished.fetch_add(1,Ordering::AcqRel);events.changed.notify_waiters();true
                },_=>false,
            }).build().unwrap();
        let destroyed=observed.clone();window.on_window_event(move |event|if matches!(event,tauri::WindowEvent::Destroyed){destroyed.destroyed.store(true,Ordering::Release);destroyed.changed.notify_waiters();});
        native_observers(&window,profile,cancel,observed.clone());assert!(observed.actual_udf.load(Ordering::Acquire));
        window.navigate(url.clone()).unwrap();let app=app.clone();let report_root=root.clone();let state=observed.clone();let done=exit.clone();
        let owner=engines.clone();let lifetime=io.clone();
        tauri::async_runtime::spawn(async move {
            let result=tokio::time::timeout(Duration::from_secs(35),async {
                loop {let change=state.changed.notified();if state.finished.load(Ordering::Acquire)>0{break;}change.await;}
                owner.close();lifetime.close();lifetime.seal();owner.wait_for_finished().await;lifetime.drain().await;
                let (send,receive)=tokio::sync::oneshot::channel();let close=window.clone();app.run_on_main_thread(move ||{
                    DOWNLOAD.with(|slot|if let Some((view,token))=slot.borrow_mut().take(){unsafe {view.remove_DownloadStarting(token).unwrap();}});
                    let _=send.send(close.destroy());
                }).unwrap();receive.await.unwrap().unwrap();
                loop {let change=state.changed.notified();if state.exited.load(Ordering::Acquire)&&state.destroyed.load(Ordering::Acquire){break;}change.await;}
            }).await;
            let (send,receive)=tokio::sync::oneshot::channel();app.run_on_main_thread(move ||{
                OBSERVERS.with(|slot|if let Some((environment,exit))=slot.borrow_mut().take(){unsafe {environment.remove_BrowserProcessExited(exit).unwrap();}});let _=send.send(());
            }).unwrap();receive.await.unwrap();
            let report=json!({"boundedCompletion":result.is_ok(),"requested":state.requested.load(Ordering::Acquire),"finished":state.finished.load(Ordering::Acquire),
                "engineSuccess":state.success.load(Ordering::Acquire),"stageActuallyAbsent":state.absent.load(Ordering::Acquire),"payloadMatched":state.payload.load(Ordering::Acquire),
                "browserExited":state.exited.load(Ordering::Acquire),"actualUdfMatched":state.actual_udf.load(Ordering::Acquire),"error":*state.error.lock().unwrap()});
            fs::write(report_root.join("report.json"),serde_json::to_vec_pretty(&report).unwrap()).unwrap();
            done.store(true,Ordering::Release);app.exit(if result.is_ok()&&report["error"].is_null(){0}else{1});
        });
    });
    stop.store(true, Ordering::Release);
    server.join().unwrap();
    assert_eq!(code, 0);
}
