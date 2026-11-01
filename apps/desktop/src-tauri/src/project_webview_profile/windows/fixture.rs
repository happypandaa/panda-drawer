//! Interactive, exact test-executable subprocesses. These use only freshly
//! created disposable roots and a fixed fixture identity, never RuntimePaths.
use super::{binding, owner};
use crate::{business_window_lifetime::WindowLifetimes, project_webview_profile as profile};
use platform_core::{project_cleanup::ProjectKind, webview_storage::StoreLedger, ApiError};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager, Url, WebviewWindowBuilder};

mod crash {
    include!("crash_fixture.rs");
}

const FIXTURE_ID: &str = "dev.happypandaa.pandadrawer.project-profile-fixture";
const CHILD: &str =
    "project_webview_profile::windows::fixture::real_project_profile_fixture_process";
const ROOT_VARIABLE: &str = "PANDA_DISPOSABLE_PROJECT_UDF_FIXTURE";
const NODE_VARIABLE: &str = "PANDA_DISPOSABLE_NODE_UDF_FIXTURE";
const PURGE_VARIABLE: &str = "PANDA_DISPOSABLE_PROJECT_UDF_PURGE_FIXTURE";
const A: &str = "prj_disposable_profile_a";
const B: &str = "prj_disposable_profile_b";
const NODE_A: &str = "00000000-0000-4000-8000-000000000041";
const NODE_B: &str = "00000000-0000-4000-8000-000000000042";
const PAGE: &str = r#"<!doctype html><meta charset=utf-8><title>Disposable project profile fixture</title>
<script>
const project = new URL(location.href).searchParams.get('project');
function db() { return new Promise((resolve, reject) => {
  const request = indexedDB.open('shared-profile-database', 1);
  request.onupgradeneeded = () => request.result.createObjectStore('values');
  request.onsuccess = () => resolve(request.result);
  request.onerror = () => reject(request.error);
}); }
async function snapshot() {
  const database = await db();
  const indexed = await new Promise((resolve, reject) => {
    const request = database.transaction('values').objectStore('values').get('shared');
    request.onsuccess = () => resolve(request.result ?? null); request.onerror = () => reject(request.error);
  });
  database.close();
  return {local: localStorage.getItem('shared-profile-key'), indexed,
    cookie: document.cookie.split('; ').find(v => v.startsWith('shared-profile-cookie='))?.split('=')[1] ?? null};
}
window.reportProfile = async (phase, mark) => {
  let report;
  try {
    const before = await snapshot();
    if (mark !== null) {
      localStorage.setItem('shared-profile-key', mark);
      document.cookie = 'shared-profile-cookie=' + mark + '; Path=/; SameSite=Lax; Max-Age=3600';
      const database = await db();
      await new Promise((resolve, reject) => {
        const transaction = database.transaction('values', 'readwrite');
        transaction.objectStore('values').put(mark, 'shared');
        transaction.oncomplete = resolve; transaction.onerror = () => reject(transaction.error);
      });
      database.close();
    }
    report = {project, phase, before, after: await snapshot()};
  } catch (error) { report = {project, phase, error: String(error)}; }
  await fetch('/report', {method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(report)});
};
const query = new URL(location.href).searchParams;
reportProfile(query.get('phase'), query.get('mark'));
</script>"#;

fn failure(message: &str) -> ApiError {
    ApiError::new("PROJECT_PROFILE_FIXTURE_FAILED", message)
}
fn label(id: &str) -> String {
    match kind(id) {
        ProjectKind::Web => format!("project-{id}"),
        ProjectKind::Node => format!("node-app-{id}"),
    }
}
fn kind(id: &str) -> ProjectKind {
    match id {
        NODE_A | NODE_B => ProjectKind::Node,
        A | B => ProjectKind::Web,
        _ => panic!("fixture only uses its own explicit disposable IDs"),
    }
}

#[test]
#[ignore = "Opt-in real two-project WebView2 fixture requires an interactive desktop"]
fn real_same_origin_projects_keep_storage_isolated_and_wait_for_browser_exit() {
    run_profile_fixture(false);
}

#[test]
#[ignore = "Opt-in real Node-kind WebView2 profiles require an interactive desktop; no normal app data is read"]
fn real_node_profiles_keep_storage_isolated_and_wait_for_their_actual_browser_exit() {
    run_profile_fixture(true);
}

#[test]
#[ignore = "Opt-in actual WebView2 exit and precise Windows profile purge; disposable roots only"]
fn real_owned_profiles_purge_after_actual_exit_and_keep_the_peer_profile() {
    run_profile_fixture_mode(false, true);
    run_profile_fixture_mode(true, true);
}

fn run_profile_fixture(node: bool) {
    run_profile_fixture_mode(node, false);
}
fn run_profile_fixture_mode(node: bool, purge: bool) {
    for verbatim in [false, true] {
        let root = tempfile::Builder::new()
            .prefix(if node {
                "panda-disposable-node-udf-real-"
            } else {
                "panda-disposable-project-udf-real-"
            })
            .tempdir()
            .unwrap()
            .keep()
            .canonicalize()
            .unwrap();
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
            .env(NODE_VARIABLE, if node { "true" } else { "false" })
            .env(PURGE_VARIABLE, if purge { "true" } else { "false" })
            .env(
                "PANDA_DISPOSABLE_PROJECT_VERBATIM_UDF_FIXTURE",
                if verbatim { "true" } else { "false" },
            )
            .env_remove("WEBVIEW2_USER_DATA_FOLDER")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(120);
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
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        assert!(
            status.is_some_and(|status| status.success())
                && report
                    .as_ref()
                    .is_some_and(|report| report["success"] == true),
            "Real project profile fixture failed; retained {}: {status:?}; {report:?}; {}",
            root.display(),
            fs::read_to_string(root.join("child.log")).unwrap_or_default()
        );
        assert_eq!(
            report.as_ref().unwrap()["details"]["projectKind"],
            if node { "node" } else { "web" }
        );
        eprintln!(
            "Actual same-origin project profile fixture: {}",
            report.unwrap()
        );
        eprintln!(
            "Retained only disposable profile evidence: {}",
            root.display()
        );
    }
}

#[derive(Default)]
struct Observations {
    requests: Mutex<BTreeMap<String, usize>>,
    reports: Mutex<BTreeMap<(String, String), Value>>,
}
struct Page {
    origin: String,
    observations: Arc<Observations>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Page {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let observations = Arc::new(Observations::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (observed, stopping) = (observations.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if let Err(error) = serve(&mut stream, &observed) {
                            eprintln!("Disposable fixture HTTP: {error}");
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            origin,
            observations,
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
fn serve(stream: &mut TcpStream, observed: &Observations) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut input = vec![];
    let header_end = loop {
        if let Some(index) = input.windows(4).position(|v| v == b"\r\n\r\n") {
            break index + 4;
        }
        if input.len() > 65536 {
            return Err(std::io::Error::other("fixture header too large"));
        }
        let mut buffer = [0; 8192];
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Err(std::io::Error::other("incomplete fixture header"));
        }
        input.extend_from_slice(&buffer[..count]);
    };
    let header = String::from_utf8_lossy(&input[..header_end]);
    let target = header
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("")
        .to_owned();
    let content_length = header
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    if content_length > 65536 {
        return Err(std::io::Error::other("fixture body too large"));
    }
    while input.len() < header_end + content_length {
        let mut buffer = [0; 8192];
        let count = stream.read(&mut buffer)?;
        if count == 0 {
            return Err(std::io::Error::other("incomplete fixture body"));
        }
        input.extend_from_slice(&buffer[..count]);
    }
    *observed
        .requests
        .lock()
        .unwrap()
        .entry(target.clone())
        .or_default() += 1;
    if target == "/report" {
        let report: Value =
            serde_json::from_slice(&input[header_end..header_end + content_length])?;
        let key = (
            report["project"].as_str().unwrap_or("").to_owned(),
            report["phase"].as_str().unwrap_or("").to_owned(),
        );
        observed.reports.lock().unwrap().insert(key, report);
    }
    let body = if target.starts_with("/?") { PAGE } else { "ok" };
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}", body.len())
}

struct Release(Arc<AtomicUsize>);
impl Drop for Release {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}
struct Opened {
    generation: u64,
    pid: u32,
    actual: PathBuf,
    root_id: String,
    released: Arc<AtomicUsize>,
    requests_before_verification: usize,
}
struct PageRequest<'a> {
    id: &'a str,
    origin: &'a str,
    phase: &'a str,
    mark: &'a str,
}
fn open(
    app: &AppHandle,
    ledger: &StoreLedger,
    lifetimes: &Arc<WindowLifetimes>,
    page: PageRequest<'_>,
    observed: &Observations,
) -> Result<Opened, ApiError> {
    let PageRequest {
        id,
        origin,
        phase,
        mark,
    } = page;
    let identifier = ledger.record(kind(id), id)?;
    let profile = profile::Profile::prepare(app, ledger, kind(id), id, &label(id), identifier)?;
    let receipt = lifetimes
        .reserve(&label(id))
        .map_err(|_| failure("Fixture generation still owned"))?;
    profile.reserve(app, receipt.generation)?;
    let released = Arc::new(AtomicUsize::new(0));
    profile.hold_resource(&receipt, Release(released.clone()))?;
    let target = format!("/?project={id}&phase={phase}&mark={mark}");
    let launch = Url::parse(&format!("{origin}{target}")).unwrap();
    let navigation_profile = profile.clone();
    let admitted_origin = origin.to_owned();
    let window = profile
        .configure(WebviewWindowBuilder::new(
            app,
            label(id),
            profile.initial_url(launch.clone()),
        ))
        .title(format!("Disposable project profile {id}"))
        .on_navigation(move |url| {
            navigation_profile.navigation(|| url.origin().ascii_serialization() == admitted_origin)
        })
        .build()
        .map_err(|cause| failure(&cause.to_string()))?;
    let (app_owner, lifetime_owner, receipt_owner) =
        (app.clone(), lifetimes.clone(), receipt.clone());
    window.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            lifetime_owner.destroyed(&receipt_owner, || {});
            profile::window_destroyed(&app_owner, &receipt_owner.label, receipt_owner.generation);
        }
    });
    let before = observed
        .requests
        .lock()
        .unwrap()
        .get(&target)
        .copied()
        .unwrap_or(0);
    if before != 0 {
        return Err(failure(
            "First document loaded before actual-directory verification",
        ));
    }
    profile.bind_window(&window)?;
    let (actual, pid, private) = binding::fixture_facts(&window)?;
    eprintln!("Actual project profile: id={id}, generation={}, pid={pid}, IsInPrivateModeEnabled={private}, udf={}",receipt.generation,actual.display());
    if private {
        return Err(failure(
            "Actual fixture browser profile is unexpectedly InPrivate",
        ));
    }
    profile.activate(&window, launch, || Ok(()))?;
    let identity = platform_native::windows::files::open_no_reparse(&actual)?.identity;
    Ok(Opened {
        generation: receipt.generation,
        pid,
        actual,
        root_id: format!("{}:{:02x?}", identity.volume_serial, identity.file_id),
        released,
        requests_before_verification: before,
    })
}

async fn on_main<T: Send + 'static>(
    app: &AppHandle,
    work: impl FnOnce(&AppHandle) -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let captured = app.clone();
    app.run_on_main_thread(move || {
        let _ = send.send(work(&captured));
    })
    .map_err(|_| failure("Fixture UI dispatch failed"))?;
    receive
        .await
        .map_err(|_| failure("Fixture UI reply missing"))?
}
async fn close(
    app: &AppHandle,
    lifetimes: &Arc<WindowLifetimes>,
    id: &str,
    generation: u64,
) -> Result<(), ApiError> {
    let window_label = label(id);
    let receipt = lifetimes
        .receipt(&window_label)
        .ok_or_else(|| failure("Fixture destroy receipt missing"))?;
    let node = kind(id) == ProjectKind::Node;
    let selected = window_label.clone();
    let window = on_main(app, move |app| {
        let window = app
            .get_webview_window(&selected)
            .ok_or_else(|| failure("Fixture window missing before destroy"))?;
        if node {
            return Ok(Some(window));
        }
        profile::before_destroy(app, &window, generation)?;
        window
            .destroy()
            .map_err(|cause| failure(&cause.to_string()))?;
        Ok(None)
    })
    .await?;
    if let Some(window) = window {
        // Exercise the same UI-thread sealing adapter as Node's real close,
        // restart rollback and monitor paths, then require real Env5 exit.
        profile::destroy_node(app, window, Some(generation)).await?;
    }
    tokio::time::timeout(Duration::from_secs(5), receipt.wait())
        .await
        .map_err(|_| failure("Fixture native Destroyed missing"))?;
    let selected = window_label.clone();
    on_main(app, move |app| {
        if app.get_webview_window(&selected).is_none() {
            Ok(())
        } else {
            Err(failure("Fixture native registry retained window"))
        }
    })
    .await?;
    profile::wait_closed(app, &window_label, Some(generation)).await?;
    if !owner::current(app, &window_label, generation)?.closed_and_retired() {
        return Err(failure("Fixture browser owner not retired"));
    }
    Ok(())
}
async fn report(observed: &Observations, id: &str, phase: &str) -> Result<Value, ApiError> {
    let key = (id.to_owned(), phase.to_owned());
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if let Some(report) = observed.reports.lock().unwrap().get(&key).cloned() {
                return if report["error"].is_null() {
                    Ok(report)
                } else {
                    Err(failure(&report.to_string()))
                };
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| failure("Fixture storage report timed out"))?
}
fn expect(report: &Value, before: Option<&str>, after: &str) -> Result<(), ApiError> {
    for field in ["local", "indexed", "cookie"] {
        if report["before"][field] != json!(before) || report["after"][field] != json!(after) {
            return Err(failure(&format!("Storage isolation mismatch: {report}")));
        }
    }
    Ok(())
}
async fn evaluate(app: &AppHandle, id: &str, script: &str) -> Result<(), ApiError> {
    let (label, script) = (label(id), script.to_owned());
    on_main(app, move |app| {
        app.get_webview_window(&label)
            .ok_or_else(|| failure("Fixture live peer missing"))?
            .eval(&script)
            .map_err(|cause| failure(&cause.to_string()))
    })
    .await
}

async fn native_cookie_facts(app: &AppHandle, id: &str) -> Result<Value, ApiError> {
    let selected = label(id);
    let window = on_main(app, move |app| {
        app.get_webview_window(&selected)
            .ok_or_else(|| failure("Fixture cookie window missing"))
    })
    .await?;
    // Tauri documents that cookies() deadlocks from synchronous Windows UI
    // callbacks. Only an independent worker waits for its real native callback.
    tauri::async_runtime::spawn_blocking(move || {
        let cookies = window.cookies().map_err(|cause| failure(&cause.to_string()))?;
        Ok(Value::Array(cookies.into_iter().map(|cookie| json!({"name":cookie.name(),"value":cookie.value(),"path":cookie.path(),"domain":cookie.domain(),"expiresUnix":cookie.expires_datetime().map(|value|value.unix_timestamp())})).collect()))
    }).await.map_err(|_| failure("Fixture cookie worker failed"))?
}

#[test]
#[ignore = "Exact child fixture invoked only by the disposable real-engine test"]
fn real_project_profile_fixture_process() {
    let Some(root) = std::env::var_os(ROOT_VARIABLE) else {
        return;
    };
    let root = PathBuf::from(root).canonicalize().unwrap();
    let node = std::env::var(NODE_VARIABLE).as_deref() == Ok("true");
    let (a_id, b_id) = if node { (NODE_A, NODE_B) } else { (A, B) };
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with(if node {
            "panda-disposable-node-udf-real-"
        } else {
            "panda-disposable-project-udf-real-"
        }));
    let flags = std::env::var_os("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS");
    let text = flags
        .as_deref()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    fs::write(root.join("browser-flag-state.json"),serde_json::to_vec_pretty(&json!({"additionalArgumentsPresent":flags.is_some(),"inprivateFlag":text.contains("inprivate"),"incognitoFlag":text.contains("incognito"),"cookieFlag":text.contains("cookie"),"disableFeaturesFlag":text.contains("--disable-features")})).unwrap()).unwrap();
    let page = Page::start();
    let observations = page.observations.clone();
    let origin = page.origin.clone();
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
    let code = app.run_return(move |app,event| {
        if let tauri::RunEvent::ExitRequested { api, .. } = &event { if !permit_exit.load(Ordering::Acquire) { api.prevent_exit(); } }
        if !matches!(event, tauri::RunEvent::Ready) || started { return; } started = true;
        let lifetimes = Arc::new(WindowLifetimes::default());
        // Production default_root() preserves the normal DOS path from dirs;
        // canonicalize() above is only for the retained evidence location.
        let units: Vec<u16> = root.as_os_str().encode_wide().collect();
        assert!(units.starts_with(&[92,92,63,92]) && units.get(5) == Some(&58));
        let data_root = if std::env::var("PANDA_DISPOSABLE_PROJECT_VERBATIM_UDF_FIXTURE").as_deref() == Ok("true") { root.clone() } else { PathBuf::from(std::ffi::OsString::from_wide(&units[4..])) };
        fs::write(root.join("path-presentation.json"),serde_json::to_vec_pretty(&json!({"dataRootPassedToLedger":data_root})).unwrap()).unwrap();
        let ledger = StoreLedger::new(data_root);
        let a = open(app, &ledger, &lifetimes, PageRequest {id:a_id,origin:&origin,phase:"first-a",mark:"A1"}, &observations).unwrap();
        let (app, root, finished, observations, origin) = (app.clone(), root.clone(), finishing.clone(), observations.clone(), origin.clone());
        tauri::async_runtime::spawn(async move {
            let result = async {
                let first_a = report(&observations,a_id,"first-a").await?; expect(&first_a,None,"A1")?;
                let native_a = native_cookie_facts(&app,a_id).await?;
                fs::write(root.join("native-cookies-first-a.json"),serde_json::to_vec_pretty(&native_a).unwrap())?;
                if std::env::var(crash::CRASH_VARIABLE).as_deref() == Ok("true") {
                    let processes=crash::processes(&app,a_id).await?;
                    fs::write(root.join("crash-ready.tmp"),serde_json::to_vec(&json!({"kind":kind(a_id),"id":a_id,"directory":a.actual,"rootId":a.root_id,"processes":processes})).unwrap())?;
                    fs::rename(root.join("crash-ready.tmp"),root.join("crash-ready.json"))?;
                    return std::future::pending::<Result<Value,ApiError>>().await;
                }
                let (ledger_b,lifetimes_b,origin_b,observations_b) = (ledger.clone(),lifetimes.clone(),origin.clone(),observations.clone());
                let b = on_main(&app,move |app| open(app,&ledger_b,&lifetimes_b,PageRequest{id:b_id,origin:&origin_b,phase:"first-b",mark:"B1"},&observations_b)).await?;
                if a.pid == b.pid || a.actual == b.actual || a.root_id == b.root_id { return Err(failure("Two projects shared a browser/profile identity")); }
                let first_b = report(&observations,b_id,"first-b").await?; expect(&first_b,None,"B1")?;
                let native_b = native_cookie_facts(&app,b_id).await?;
                fs::write(root.join("native-cookies-first-b.json"),serde_json::to_vec_pretty(&native_b).unwrap())?;
                close(&app,&lifetimes,a_id,a.generation).await?;
                if a.released.load(Ordering::Acquire) != 1 || b.released.load(Ordering::Acquire) != 0 || !owner::current(&app,&label(b_id),b.generation)?.document_allowed() { return Err(failure("A close released its peer or retained its own lease")); }
                // The actual A leaf can be renamed only after its own browser
                // exit and all of this generation's pinned profile handles end.
                let renamed = root.join("webview-stores/disposable-closed-a-rename-check");
                fs::rename(&a.actual,&renamed)?; fs::rename(&renamed,&a.actual)?;
                evaluate(&app,b_id,"reportProfile('b-kept-open','B2')").await?;
                let kept_b = report(&observations,b_id,"b-kept-open").await?; expect(&kept_b,Some("B1"),"B2")?;
                let (ledger_a,lifetimes_a,origin_a,observations_a) = (ledger.clone(),lifetimes.clone(),origin.clone(),observations.clone());
                let reopened = on_main(&app,move |app| open(app,&ledger_a,&lifetimes_a,PageRequest{id:a_id,origin:&origin_a,phase:"reopen-a",mark:"A2"},&observations_a)).await?;
                if reopened.generation == a.generation || reopened.actual != a.actual || reopened.root_id != a.root_id { return Err(failure("A reopen changed profile identity or reused window generation")); }
                let reopened_a = report(&observations,a_id,"reopen-a").await?;
                let native_reopened = native_cookie_facts(&app,a_id).await?;
                fs::write(root.join("native-cookies-reopened-a.json"),serde_json::to_vec_pretty(&native_reopened).unwrap())?;
                expect(&reopened_a,Some("A1"),"A2")?;
                evaluate(&app,b_id,"reportProfile('b-after-reopen',null)").await?;
                let after_b = report(&observations,b_id,"b-after-reopen").await?; expect(&after_b,Some("B2"),"B2")?;
                close(&app,&lifetimes,a_id,reopened.generation).await?; close(&app,&lifetimes,b_id,b.generation).await?;
                if reopened.released.load(Ordering::Acquire) != 1 || b.released.load(Ordering::Acquire) != 1 { return Err(failure("Final real exits did not retire native leases")); }
                let mut purged = false;
                if std::env::var(PURGE_VARIABLE).as_deref() == Ok("true") {
                    let retained = ledger.bind_profile(kind(a_id),a_id)?;
                    if crate::native_stores::purge_project(&app,&ledger,kind(a_id),a_id).await.is_ok() {
                        return Err(failure("Held actual profile pin was reported purged"));
                    }
                    if !a.actual.join("store.json").is_file() || !b.actual.join("store.json").is_file() {
                        return Err(failure("Held-root preflight removed profile payload or peer"));
                    }
                    drop(retained);
                    crate::native_stores::purge_project(&app,&ledger,kind(a_id),a_id).await?;
                    if a.actual.try_exists()? || ledger.contains(kind(a_id),a_id)? {
                        return Err(failure("Actual purged A profile or receipt remained"));
                    }
                    let (ledger_b,lifetimes_b,origin_b,observations_b)=(ledger.clone(),lifetimes.clone(),origin.clone(),observations.clone());
                    let kept=on_main(&app,move |app| open(app,&ledger_b,&lifetimes_b,PageRequest{id:b_id,origin:&origin_b,phase:"b-after-a-purge",mark:"B3"},&observations_b)).await?;
                    let after=report(&observations,b_id,"b-after-a-purge").await?;
                    expect(&after,Some("B2"),"B3")?;
                    close(&app,&lifetimes,b_id,kept.generation).await?;
                    crate::native_stores::purge_project(&app,&ledger,kind(b_id),b_id).await?;
                    if b.actual.try_exists()? || !ledger.records()?.is_empty() {
                        return Err(failure("Final peer profile or ledger remained"));
                    }
                    purged=true;
                }
                Ok::<Value,ApiError>(json!({"projectKind":kind(a_id).as_str(),"origin":origin,"firstA":first_a,"firstB":first_b,"bWhileAClosed":kept_b,"reopenedA":reopened_a,"bAfterAReopen":after_b,
                    "actualUserDataFolders":[a.actual,b.actual],"fullRootIds":[a.root_id,b.root_id],"browserPids":[a.pid,b.pid,reopened.pid],
                    "nativeCookies":[native_a,native_b,native_reopened],"requestsBeforeActualVerification":[a.requests_before_verification,b.requests_before_verification,reopened.requests_before_verification],"allThreeBrowserGenerationsExitedAndRetired":true,"aLeaseReleasedBeforeBClosed":true,"exactProfilesPurgedAfterExit":purged}))
            }.await;
            let report = match result { Ok(details) => json!({"success":true,"details":details,"fixtureIdentifier":FIXTURE_ID}), Err(error) => json!({"success":false,"error":error,"fixtureIdentifier":FIXTURE_ID}) };
            fs::write(root.join("report.json"),serde_json::to_vec_pretty(&report).unwrap()).unwrap();
            finished.store(true,Ordering::Release); app.exit(if report["success"] == true {0} else {1});
        });
    });
    drop(page);
    assert_eq!(code, 0);
}
