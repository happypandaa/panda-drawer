use super::*;
use platform_native::windows::process as native_process;
use windows::core::Interface;

pub(super) const CRASH_VARIABLE: &str = "PANDA_DISPOSABLE_PROFILE_CRASH_HOST";
const RECOVERY_HOST: &str =
    "project_webview_profile::windows::fixture::crash::real_disposable_profile_recovery_host";

pub(super) async fn processes(app: &AppHandle, id: &str) -> Result<Value, ApiError> {
    let label = label(id);
    on_main(app,move |app| {
        let window=app.get_webview_window(&label).ok_or_else(||failure("Crash fixture window missing"))?;
        let result=Arc::new(Mutex::new(None)); let reply=result.clone();
        window.with_webview(move |webview| {
            let checked=(|| {
                let environment=webview.environment().cast::<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Environment8>()
                    .map_err(|_|failure("Fixture process inventory interface unavailable"))?;
                let mut processes=vec![];
                unsafe {
                    let collection=environment.GetProcessInfos().map_err(|_|failure("Fixture process inventory failed"))?;
                    let mut count=0; collection.Count(&mut count).map_err(|_|failure("Fixture process count failed"))?;
                    for index in 0..count {
                        let entry=collection.GetValueAtIndex(index).map_err(|_|failure("Fixture process entry failed"))?;
                        let mut pid=0; entry.ProcessId(&mut pid).map_err(|_|failure("Fixture process PID unavailable"))?;
                        if let Some(identity)=native_process::capture_live(pid.try_into().map_err(|_|failure("Invalid fixture PID"))?)? {
                            processes.push(json!({"pid":identity.pid,"creation100ns":identity.creation_100ns,"executable":identity.executable}));
                        }
                    }
                }
                if processes.is_empty() {return Err(failure("Actual browser process inventory was empty"));}
                Ok(Value::Array(processes))
            })();
            *reply.lock().unwrap_or_else(|p|p.into_inner())=Some(checked);
        }).map_err(|_|failure("Crash fixture process inventory dispatch failed"))?;
        let checked=result.lock().unwrap_or_else(|p|p.into_inner()).take();
        checked.ok_or_else(||failure("Crash fixture process inventory missing"))?
    }).await
}

fn spawn_host(root: &std::path::Path, alternate: bool, recovery: bool) -> std::process::Child {
    let log = fs::File::create(root.join(if recovery {
        "recovery-host.log"
    } else {
        "crashed-host.log"
    }))
    .unwrap();
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            if recovery { RECOVERY_HOST } else { CHILD },
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROOT_VARIABLE, root)
        .env(ALTERNATE_VARIABLE, if alternate { "true" } else { "false" })
        .env(CRASH_VARIABLE, if recovery { "false" } else { "true" })
        .env("PANDA_DISPOSABLE_PROJECT_VERBATIM_UDF_FIXTURE", "false")
        .env_remove("WEBVIEW2_USER_DATA_FOLDER")
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap()
}

#[test]
#[ignore = "Opt-in actual abrupt host termination and same-boot WebView2 profile cleanup; disposable roots only"]
fn real_same_boot_crashed_profile_owner_recovers_through_native_blank_exit() {
    for alternate in [false, true] {
        let root = tempfile::Builder::new()
            .prefix(if alternate {
                "panda-disposable-app-alternate-udf-real-crash-"
            } else {
                "panda-disposable-project-udf-real-crash-"
            })
            .tempdir()
            .unwrap()
            .keep()
            .canonicalize()
            .unwrap();
        let boot = platform_native::windows::boot_session::current_boot_id().unwrap();
        let mut host = spawn_host(&root, alternate, false);
        let deadline = Instant::now() + Duration::from_secs(45);
        while !root.join("crash-ready.json").is_file() {
            if host.try_wait().unwrap().is_some() || Instant::now() >= deadline {
                let _ = host.kill();
                let _ = host.wait();
                panic!(
                    "Disposable crash host did not become ready: {}",
                    root.display()
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let ready: Value =
            serde_json::from_slice(&fs::read(root.join("crash-ready.json")).unwrap()).unwrap();
        let identities = ready["processes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| native_process::ProcessIdentity {
                pid: value["pid"].as_u64().unwrap().try_into().unwrap(),
                creation_100ns: value["creation100ns"].as_u64().unwrap(),
                executable: PathBuf::from(value["executable"].as_str().unwrap()),
            })
            .collect::<Vec<_>>();
        // std::process::Child retains the actual original host process HANDLE;
        // this does not signal a PID reconstructed from the persisted JSON.
        host.kill().unwrap();
        host.wait().unwrap();
        let mut recovery = spawn_host(&root, alternate, true);
        let deadline = Instant::now() + Duration::from_secs(75);
        let status = loop {
            if let Some(status) = recovery.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                recovery.kill().unwrap();
                recovery.wait().unwrap();
                panic!("Profile recovery host timed out: {}", root.display());
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let report: Value =
            serde_json::from_slice(&fs::read(root.join("recovery-report.json")).unwrap()).unwrap();
        assert!(
            status.success() && report["success"] == true,
            "Same-boot profile recovery failed: {}; {report}",
            root.display()
        );
        assert_eq!(
            boot,
            platform_native::windows::boot_session::current_boot_id().unwrap()
        );
        for identity in identities {
            assert!(
                !matches!(
                    native_process::observation(&identity).unwrap(),
                    native_process::ProcessObservation::Matching
                ),
                "Original browser family identity remained alive"
            );
        }
        let directory = PathBuf::from(ready["directory"].as_str().unwrap());
        assert!(!directory.exists());
        assert!(StoreLedger::new(root.clone()).records().unwrap().is_empty());
        eprintln!(
            "Actual same-boot profile crash and native exit cleanup: {report}; evidence={}",
            root.display()
        );
    }
}

#[test]
#[ignore = "Exact recovery host invoked only by the fresh profile crash parent"]
fn real_disposable_profile_recovery_host() {
    let root = PathBuf::from(std::env::var_os(ROOT_VARIABLE).unwrap())
        .canonicalize()
        .unwrap();
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("panda-disposable-"));
    let ready: Value =
        serde_json::from_slice(&fs::read(root.join("crash-ready.json")).unwrap()).unwrap();
    let alternate = std::env::var(ALTERNATE_VARIABLE).as_deref() == Ok("true");
    let id = if alternate { ALT_A } else { A };
    assert_eq!(ready["id"], id);
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = FIXTURE_ID.into();
    context.config_mut().app.windows.clear();
    let app = tauri::Builder::default()
        .any_thread()
        .build(context)
        .unwrap();
    let finished = Arc::new(AtomicBool::new(false));
    let exit = finished.clone();
    let mut started = false;
    let code=app.run_return(move |app,event| {
        if let tauri::RunEvent::ExitRequested{api,..}=&event {if !exit.load(Ordering::Acquire) {api.prevent_exit();}}
        if !matches!(event,tauri::RunEvent::Ready)||started {return;}started=true;
        let app=app.clone();let root=root.clone();let finished=finished.clone();
        tauri::async_runtime::spawn(async move {
            let ledger=StoreLedger::new(root.clone());
            let result=crate::native_stores::purge_project(&app,&ledger,kind(id),id).await;
            let report=json!({"success":result.is_ok(),"error":result.err(),"sameBootRecovery":true,"projectKind":kind(id).as_str(),"actualProfileAbsent":!ledger.project_directory(kind(id),id).unwrap().exists()});
            fs::write(root.join("recovery-report.json"),serde_json::to_vec_pretty(&report).unwrap()).unwrap();
            finished.store(true,Ordering::Release);app.exit(if report["success"]==true {0} else {1});
        });
    });
    assert_eq!(code, 0);
}
