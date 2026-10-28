//! Explicit opt-in real WebKit fixture. It never initializes the platform's
//! runtime or resolves the platform data root.
#[cfg(target_os = "macos")]
#[path = "../native_stores.rs"]
mod native_stores;

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("The native store fixture requires macOS.");
    std::process::exit(2);
}

#[cfg(target_os = "macos")]
fn main() {
    fixture::run();
}

#[cfg(target_os = "macos")]
mod fixture {
    use super::native_stores;
    use objc2_foundation::{NSBundle, NSUUID};
    use platform_core::ApiError;
    use serde_json::{json, Value};
    use std::{
        collections::BTreeMap,
        fs,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Duration,
    };
    use tauri::Manager;
    use tokio::sync::{mpsc, oneshot, Notify};

    type Result<T> = std::result::Result<T, ApiError>;
    const PREFIX: &str = "dev.happypandaa.pandadrawer.webkit-fixture.";
    const WAIT: Duration = Duration::from_secs(30);

    fn failure(message: impl Into<String>) -> ApiError {
        ApiError::new("NATIVE_STORE_FIXTURE_FAILED", message)
    }

    pub(super) fn run() {
        let arguments = std::env::args().collect::<Vec<_>>();
        if arguments.len() != 3 {
            eprintln!("usage: native-store-fixture <fixture-id> <temporary-report.json>");
            std::process::exit(2);
        }
        let id = &arguments[1];
        let actual = NSBundle::mainBundle()
            .bundleIdentifier()
            .map(|id| id.to_string());
        let suffix = id.strip_prefix(PREFIX).unwrap_or("");
        if suffix.len() != 32
            || !suffix
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || actual.as_deref() != Some(id)
        {
            eprintln!(
                "Refusing WebKit access: the real NSBundle must have the unique fixture identifier"
            );
            std::process::exit(2);
        }
        let report = PathBuf::from(&arguments[2]);
        if !report.is_absolute()
            || report.exists()
            || !report.parent().is_some_and(|parent| parent.is_dir())
        {
            std::process::exit(2);
        }
        let mut context = tauri::test::mock_context(tauri::test::noop_assets());
        context.config_mut().identifier = id.clone();
        let app = tauri::Builder::default()
            .build(context)
            .expect("fixture Tauri app");
        let mut started = false;
        let identity = id.clone();
        app.run(move |app, event| {
            if matches!(event, tauri::RunEvent::Ready) && !started {
                started = true;
                let app = app.clone();
                let report = report.clone();
                let identity = identity.clone();
                tauri::async_runtime::spawn(async move {
                    let result = scenario(&app).await;
                    let value = match &result {
                        Ok(checks) => {
                            json!({"ok":true,"bundleIdentifier":identity,"checks":checks})
                        }
                        Err(error) => json!({"ok":false,"bundleIdentifier":identity,"error":error}),
                    };
                    let _ = fs::write(report, serde_json::to_vec_pretty(&value).unwrap());
                    app.exit(if result.is_ok() { 0 } else { 1 });
                });
            }
        });
    }

    struct Server {
        origin: String,
        reports: mpsc::UnboundedReceiver<Value>,
        stop: Arc<AtomicBool>,
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
        }
    }
    impl Server {
        fn start() -> Result<Self> {
            let listener =
                TcpListener::bind(("127.0.0.1", 0)).map_err(|error| failure(error.to_string()))?;
            listener
                .set_nonblocking(true)
                .map_err(|error| failure(error.to_string()))?;
            let origin = format!("http://{}", listener.local_addr().unwrap());
            let (send, reports) = mpsc::unbounded_channel();
            let stop = Arc::new(AtomicBool::new(false));
            let ended = stop.clone();
            std::thread::spawn(move || {
                while !ended.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = respond(stream, &send);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5))
                        }
                        Err(_) => break,
                    }
                }
            });
            Ok(Self {
                origin,
                reports,
                stop,
            })
        }
        async fn report(&mut self, tag: &str) -> Result<Value> {
            tokio::time::timeout(WAIT, async {
                loop {
                    let value = self
                        .reports
                        .recv()
                        .await
                        .ok_or_else(|| failure("fixture HTTP reporter closed"))?;
                    if value["error"].is_string() || value["data"]["error"].is_string() {
                        return Err(failure(value.to_string()));
                    }
                    if value["tag"] == tag {
                        return Ok(value["data"].clone());
                    }
                }
            })
            .await
            .map_err(|_| failure(format!("missing WebKit report: {tag}")))?
        }
    }
    fn respond(
        mut stream: TcpStream,
        reports: &mpsc::UnboundedSender<Value>,
    ) -> std::io::Result<()> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut bytes = vec![];
        let mut chunk = [0; 8192];
        let mut head = None;
        let mut length = 0;
        loop {
            let count = stream.read(&mut chunk)?;
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.len() > 1024 * 1024 {
                return Ok(());
            }
            if head.is_none() {
                if let Some(position) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    head = Some(position + 4);
                    for line in String::from_utf8_lossy(&bytes[..position]).lines() {
                        if let Some(value) =
                            line.to_ascii_lowercase().strip_prefix("content-length:")
                        {
                            length = value.trim().parse().unwrap_or(0);
                        }
                    }
                }
            }
            if head.is_some_and(|position| bytes.len() >= position + length) {
                break;
            }
        }
        let post = bytes.starts_with(b"POST /report ");
        if post {
            if let Some(position) = head {
                if let Ok(value) = serde_json::from_slice(&bytes[position..]) {
                    let _ = reports.send(value);
                }
            }
        }
        let body = if post { "ok" } else { PAGE };
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}", body.len(), body)
    }

    #[derive(Default)]
    struct Destroyed {
        done: AtomicBool,
        changed: Notify,
    }
    async fn ui<T: Send + 'static>(
        app: &tauri::AppHandle,
        work: impl FnOnce(&tauri::AppHandle) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (send, receive) = oneshot::channel();
        let owner = app.clone();
        app.run_on_main_thread(move || {
            let _ = send.send(work(&owner));
        })
        .map_err(|error| failure(error.to_string()))?;
        tokio::time::timeout(WAIT, receive)
            .await
            .map_err(|_| failure("native UI timeout"))?
            .map_err(|_| failure("native UI reply lost"))?
    }
    async fn create(
        app: &tauri::AppHandle,
        server: &mut Server,
        label: &str,
        id: Option<[u8; 16]>,
        ephemeral: bool,
    ) -> Result<Arc<Destroyed>> {
        let owned_label = label.to_owned();
        let url = format!("{}/page?who={label}", server.origin)
            .parse()
            .unwrap();
        let receipt = Arc::new(Destroyed::default());
        let signal = receipt.clone();
        ui(app, move |app| {
            let mut builder = tauri::WebviewWindowBuilder::new(
                app,
                &owned_label,
                tauri::WebviewUrl::External(url),
            )
            .visible(false)
            .incognito(ephemeral)
            .on_download(|_, _| false)
            .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny);
            if let Some(id) = id {
                builder = builder.data_store_identifier(id);
            }
            let window = builder
                .build()
                .map_err(|error| failure(error.to_string()))?;
            window.on_window_event(move |event| {
                if matches!(event, tauri::WindowEvent::Destroyed) {
                    signal.done.store(true, Ordering::Release);
                    signal.changed.notify_waiters();
                }
            });
            Ok(())
        })
        .await?;
        server.report(&format!("loaded:{label}")).await?;
        Ok(receipt)
    }
    async fn close(app: &tauri::AppHandle, label: &str, receipt: &Destroyed) -> Result<()> {
        let label = label.to_string();
        ui(app, move |app| {
            app.get_webview_window(&label)
                .ok_or_else(|| failure("fixture window disappeared"))?
                .destroy()
                .map_err(|error| failure(error.to_string()))
        })
        .await?;
        tokio::time::timeout(WAIT, async {
            loop {
                let changed = receipt.changed.notified();
                if receipt.done.load(Ordering::Acquire) {
                    break;
                }
                changed.await;
            }
        })
        .await
        .map_err(|_| failure("missing native Destroyed receipt"))?;
        // Drain an additional main-loop turn after native destruction.
        ui(app, |_| Ok(())).await
    }
    async fn javascript(app: &tauri::AppHandle, label: &str, script: String) -> Result<()> {
        let label = label.to_string();
        ui(app, move |app| {
            app.get_webview_window(&label)
                .ok_or_else(|| failure("fixture window absent"))?
                .eval(script)
                .map_err(|error| failure(error.to_string()))
        })
        .await
    }
    async fn write(
        app: &tauri::AppHandle,
        server: &mut Server,
        label: &str,
        value: &str,
    ) -> Result<Value> {
        javascript(
            app,
            label,
            format!(
                "fixtureWrite({}, {})",
                json!(value),
                json!(format!("write:{label}"))
            ),
        )
        .await?;
        server.report(&format!("write:{label}")).await
    }
    async fn read(app: &tauri::AppHandle, server: &mut Server, label: &str) -> Result<Value> {
        javascript(
            app,
            label,
            format!("fixtureRead({})", json!(format!("read:{label}"))),
        )
        .await?;
        server.report(&format!("read:{label}")).await
    }
    fn check_data(data: &Value, value: &str) -> Result<()> {
        if data["local"] != value || data["cookie"] != value || data["indexed"] != value {
            return Err(failure(format!("unexpected isolated data: {data}")));
        }
        Ok(())
    }

    async fn scenario(app: &tauri::AppHandle) -> Result<Vec<&'static str>> {
        let mut server = Server::start()?;
        let a = NSUUID::UUID().as_bytes();
        let b = NSUUID::UUID().as_bytes();
        let manager = NSUUID::UUID().as_bytes();
        let mut receipts = BTreeMap::new();
        let mut phase = "create stores";
        let result = async {
            receipts.insert(
                "keeper",
                create(app, &mut server, "keeper", None, true).await?,
            );
            receipts.insert("a", create(app, &mut server, "a", Some(a), false).await?);
            receipts.insert("b", create(app, &mut server, "b", Some(b), false).await?);
            receipts.insert(
                "manager",
                create(app, &mut server, "manager", Some(manager), false).await?,
            );
            check_data(&write(app, &mut server, "a", "web-A").await?, "web-A")?;
            check_data(&write(app, &mut server, "b", "node-B").await?, "node-B")?;
            check_data(
                &write(app, &mut server, "manager", "global-manager").await?,
                "global-manager",
            )?;
            check_data(&read(app, &mut server, "a").await?, "web-A")?;
            check_data(&read(app, &mut server, "b").await?, "node-B")?;
            phase = "destroy A";
            close(app, "a", receipts.get("a").unwrap()).await?;
            receipts.remove("a");
            phase = "purge A";
            eprintln!(
                "fixture phase={phase}; windows={:?}",
                app.webview_windows().keys().collect::<Vec<_>>()
            );
            native_stores::purge_identifier(app, a).await?;
            native_stores::clear_identifier_and_verify(app, a).await?;
            if native_stores::identifiers(app).await?.contains(&a) {
                return Err(failure("clearing an absent ID recreated a store"));
            }
            if native_stores::identifiers(app).await?.contains(&a) {
                return Err(failure("removed store remains in system enumeration"));
            }
            native_stores::purge_identifier(app, a).await?;
            check_data(&read(app, &mut server, "b").await?, "node-B")?;
            check_data(&read(app, &mut server, "manager").await?, "global-manager")?;
            receipts.insert("a", create(app, &mut server, "a", Some(a), false).await?);
            phase = "recreate A empty";
            let fresh = read(app, &mut server, "a").await?;
            if fresh["local"] != Value::Null
                || fresh["cookie"] != Value::Null
                || fresh["indexed"] != Value::Null
            {
                return Err(failure(format!("recreated store has old data: {fresh}")));
            }
            receipts.insert(
                "legacy",
                create(app, &mut server, "legacy", None, false).await?,
            );
            phase = "legacy default write";
            check_data(
                &write(app, &mut server, "legacy", "legacy-shared").await?,
                "legacy-shared",
            )?;
            close(app, "legacy", receipts.get("legacy").unwrap()).await?;
            receipts.remove("legacy");
            phase = "legacy default clear";
            native_stores::clear_default_and_verify(app).await?;
            receipts.insert(
                "legacy",
                create(app, &mut server, "legacy", None, false).await?,
            );
            let clean = read(app, &mut server, "legacy").await?;
            if clean["local"] != Value::Null
                || clean["cookie"] != Value::Null
                || clean["indexed"] != Value::Null
            {
                return Err(failure(format!(
                    "default store retains legacy data: {clean}"
                )));
            }
            check_data(&read(app, &mut server, "manager").await?, "global-manager")?;
            phase = "live manager clear";
            native_stores::clear_identifier_and_verify(app, manager).await?;
            if !native_stores::identifiers(app).await?.contains(&manager)
                || receipts
                    .get("manager")
                    .unwrap()
                    .done
                    .load(Ordering::Acquire)
            {
                return Err(failure("clearing live manager removed its store or window"));
            }
            let manager_clean = read(app, &mut server, "manager").await?;
            if manager_clean["local"] != Value::Null
                || manager_clean["cookie"] != Value::Null
                || manager_clean["indexed"] != Value::Null
            {
                return Err(failure(format!(
                    "live manager retains data after clear: {manager_clean}"
                )));
            }
            check_data(&read(app, &mut server, "b").await?, "node-B")?;
            Ok(vec![
                "three-same-origin-stores-isolated",
                "destroyed-receipt-before-purge",
                "removed-id-absent",
                "absent-delete-idempotent",
                "absent-clear-does-not-create-store",
                "recreated-A-empty",
                "Node-B-preserved",
                "manager-preserved",
                "default-callback-and-empty-verification",
                "live-manager-cleared-without-removing-store",
                "Node-B-preserved-after-manager-clear",
            ])
        }
        .await;
        let result = result.map_err(|error| error.detail("phase", json!(phase)));
        let mut cleanup_error = None;
        for (label, receipt) in receipts.iter().filter(|(label, _)| **label != "keeper") {
            if let Err(error) = close(app, label, receipt).await {
                cleanup_error = Some(error);
            }
        }
        for id in [a, b, manager] {
            if let Err(error) = native_stores::purge_identifier(app, id).await {
                cleanup_error = Some(error);
            }
        }
        if let Err(error) = native_stores::clear_default_and_verify(app).await {
            cleanup_error = Some(error);
        }
        if let Some(error) = cleanup_error {
            if let Err(primary) = result {
                return Err(primary.detail("cleanupError", json!(error)));
            }
            return Err(error.detail("phase", json!("fixture final cleanup")));
        }
        result
    }

    const PAGE: &str = r#"<!doctype html><meta charset="utf-8"><title>Disposable WebKit store fixture</title><script>
const tag=new URLSearchParams(location.search).get('who');
async function db(){return new Promise((resolve,reject)=>{const r=indexedDB.open('fixture',1);r.onupgradeneeded=()=>r.result.createObjectStore('kv');r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error);});}
async function indexed(value,write){const d=await db();try{return await new Promise((resolve,reject)=>{const t=d.transaction('kv',write?'readwrite':'readonly');const r=write?t.objectStore('kv').put(value,'probe'):t.objectStore('kv').get('probe');let result;r.onsuccess=()=>{result=r.result;};t.oncomplete=()=>resolve(write?value:(result??null));t.onerror=()=>reject(t.error);});}finally{d.close();}}
async function report(tag,data){await fetch('/report',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({tag,data})});}
async function state(){return {local:localStorage.getItem('probe'),cookie:(document.cookie.split('; ').find(x=>x.startsWith('probe='))?.slice(6)??null),indexed:await indexed(null,false)};}
window.fixtureWrite=async(value,tag)=>{try{localStorage.setItem('probe',value);document.cookie='probe='+value+'; Path=/; SameSite=Lax';await indexed(value,true);await report(tag,await state());}catch(e){await report(tag,{error:String(e)});}};
window.fixtureRead=async(tag)=>{try{await report(tag,await state());}catch(e){await report(tag,{error:String(e)});}};
report('loaded:'+tag,{});
</script>"#;
}
