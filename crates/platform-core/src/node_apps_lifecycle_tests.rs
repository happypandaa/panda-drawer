//! Disposable real subprocess/HTTP regressions using an artificial interpreter.
//! These prove startup ownership and cleanup, not actual Node compatibility.
use super::*;
use std::os::unix::fs::PermissionsExt;

struct Fixture {
    _temporary: tempfile::TempDir,
    source: PathBuf,
    data: PathBuf,
    manager: NodeApps,
}

impl Fixture {
    fn new(behavior: &str) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("disposable-lifecycle-source");
        let data = root.join("disposable-platform-data");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&data).unwrap();
        fs::create_dir(source.join("public")).unwrap();
        fs::create_dir(source.join("server")).unwrap();
        fs::write(
            source.join("public/index.html"),
            "<p>Artificial lifecycle fixture</p>",
        )
        .unwrap();
        fs::write(source.join("local-app.json"), serde_json::to_vec(&json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"test.disposable.lifecycle","name":"可丢弃生命周期测试","version":"1"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"protocolVersion":1,"root":"server","entry":"fixture.mjs","nodeMajor":24},
            "configuration":{"version":1,"fields":[{
                "name":"TRIAL_BEHAVIOR","label":"人工测试行为","type":"string","required":false,"secret":false,
                "description":"只控制可丢弃测试夹具","howToGet":["无需用户配置"],"default":behavior
            }]}
        })).unwrap()).unwrap();
        fs::write(source.join("server/fixture.mjs"), r#"import sys, os, json, threading,signal,time,subprocess
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
message = json.loads(sys.stdin.readline())
instance = message['instanceId']
token = message['token']
if os.environ['TRIAL_BEHAVIOR'] == 'leak-descendants':
    grand = "import os,signal,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);open(os.path.join(os.environ['APP_DATA_DIR'],'grandchild-pid'),'w').write(str(os.getpid()));time.sleep(60)"
    child = "import os,signal,subprocess,time;signal.signal(signal.SIGTERM,signal.SIG_IGN);open(os.path.join(os.environ['APP_DATA_DIR'],'child-pid'),'w').write(str(os.getpid()));subprocess.Popen(['/usr/bin/python3','-c'," + repr(grand) + "],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);time.sleep(60)"
    subprocess.Popen(['/usr/bin/python3','-c',child],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
    deadline=time.monotonic()+2
    while not os.path.isfile(os.path.join(os.environ['APP_DATA_DIR'],'grandchild-pid')):
        if time.monotonic()>deadline: raise RuntimeError('artificial descendants were not ready')
        time.sleep(.01)
source_manifest = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'local-app.json')
with open(os.path.join(os.environ['APP_DATA_DIR'], 'artificial-pid'), 'w') as output:
    output.write(str(os.getpid()))
class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *args): pass
    def do_GET(self):
        if self.headers.get('Authorization') != 'Bearer ' + token:
            self.send_response(401); self.send_header('Content-Length','0'); self.end_headers(); return
        if os.path.exists(os.path.join(os.environ['APP_DATA_DIR'],'force-health-failure')):
            self.send_response(503);self.send_header('Content-Length','0');self.end_headers();return
        if os.environ['TRIAL_BEHAVIOR'] == 'remove-manifest' and os.path.isfile(source_manifest):
            os.rename(source_manifest, source_manifest + '.artificial-hidden')
        body = json.dumps({'protocolVersion':1,'instanceId':instance,'status':'ok'}).encode()
        self.send_response(200); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
server = ThreadingHTTPServer(('127.0.0.1',0), Handler)
with open(os.path.join(os.environ['APP_DATA_DIR'], 'artificial-port'), 'w') as output:
    output.write(str(server.server_address[1]))
threading.Thread(target=server.serve_forever, daemon=True).start()
print(json.dumps({'protocolVersion':1,'type':'ready','instanceId':instance,'host':'127.0.0.1','port':server.server_address[1]}),flush=True)
def graceful_term(*args):
    print(json.dumps({'protocolVersion':1,'type':'stopping','instanceId':instance}),flush=True)
    server.shutdown();server.server_close();sys.exit(0)
if os.environ['TRIAL_BEHAVIOR'] == 'log-budget-graceful': signal.signal(signal.SIGTERM,graceful_term)
for line in sys.stdin:
    command = json.loads(line)
    if command['type'] == 'shutdown':
        if os.environ['TRIAL_BEHAVIOR'] == 'hang-stop':
            signal.signal(signal.SIGTERM,signal.SIG_IGN)
            while True: time.sleep(1)
        print(json.dumps({'protocolVersion':1,'type':'stopping','instanceId':instance}),flush=True)
        break
    if command['type'] == 'ping':
        print(json.dumps({'protocolVersion':1,'type':'pong','instanceId':instance,'requestId':command['requestId']}),flush=True)
        if os.environ['TRIAL_BEHAVIOR'] == 'log-budget-graceful': print('x'*65537,file=sys.stderr,flush=True)
server.shutdown(); server.server_close()
"#).unwrap();
        let node = root.join("artificial-node-interpreter");
        fs::write(&node, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'v24.22.0\\n'; exit 0; fi\nexec /usr/bin/python3 \"$@\"\n").unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
        let mut manager = NodeApps::new(data.clone(), node.clone()).unwrap();
        // Only this private test fixture trusts the artificial interpreter.
        // Production continues to use the independently pinned release digest.
        manager.digest = format!("{:x}", Sha256::digest(fs::read(node).unwrap()));
        Self {
            _temporary: temporary,
            source,
            data,
            manager,
        }
    }

    fn slot(&self, id: &str) -> PathBuf {
        self.data.join("node-apps").join(id)
    }

    fn pid(&self, id: &str) -> u32 {
        fs::read_to_string(self.slot(id).join("data/artificial-pid"))
            .unwrap()
            .parse()
            .unwrap()
    }

    fn port(&self, id: &str) -> u16 {
        fs::read_to_string(self.slot(id).join("data/artificial-port"))
            .unwrap()
            .parse()
            .unwrap()
    }
}

#[tokio::test]
async fn product_runtime_constructor_shares_both_launch_probes_with_service_shutdown() {
    let mut fixture = Fixture::new("normal");
    let (_temporary, paths) = crate::RuntimePaths::for_test();
    let lock = paths.acquire_lock().unwrap();
    let service = crate::RuntimeService::start(paths, lock).await.unwrap();
    let handle = service.handle();
    let mut apps = NodeApps::with_runtime(
        fixture.data.clone(),
        fixture.manager.node_path.clone(),
        &handle,
    )
    .unwrap();
    apps.digest = fixture.manager.digest.clone();
    assert!(apps.processes.shares_registry(&service.store().processes()));
    fixture.manager = apps;
    let registration = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registration["id"].as_str().unwrap();
    fixture.manager.start(id).await.unwrap();
    let records = service.store().processes().snapshot();
    assert_eq!(
        records
            .iter()
            .filter(|record| record.component == "node-runtime/version-probe")
            .count(),
        2
    );
    fixture.manager.stop_all().await.unwrap();
    let processes = service.store().processes();
    service.shutdown().await.unwrap();
    assert!(processes
        .snapshot()
        .iter()
        .all(|record| record.resource_closed()));
    assert_eq!(
        fs::read_to_string(fixture.source.join("public/index.html")).unwrap(),
        "<p>Artificial lifecycle fixture</p>"
    );
}

#[tokio::test]
async fn runtime_reader_failure_keeps_running_owner_and_marker_after_group_exit() {
    let fixture = Fixture::new("normal");
    let registration = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registration["id"].as_str().unwrap();
    fixture.manager.start(id).await.unwrap();
    fixture
        .manager
        .inner
        .lock()
        .await
        .running
        .get_mut(id)
        .unwrap()
        .runtime
        .fixture_reader_failure();
    assert!(fixture.manager.stop_all().await.is_err());
    assert_eq!(fixture.manager.owned_process_ids().await, vec![id]);
    assert!(fixture.slot(id).join("active.json").is_file());
    assert!(
        fixture.manager.stop_all().await.is_err(),
        "empty child/reader slots discarded a retained failure"
    );
    assert!(fixture.manager.start(id).await.is_err());
    assert_eq!(
        fs::read_to_string(fixture.source.join("public/index.html")).unwrap(),
        "<p>Artificial lifecycle fixture</p>"
    );
}

#[tokio::test]
async fn failed_final_detail_rolls_back_only_the_newly_started_backend() {
    let fixture = Fixture::new("remove-manifest");
    let registered = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registered["id"].as_str().unwrap();
    assert!(fixture.manager.start(id).await.is_err());
    let pid = fixture.pid(id);
    assert!(fixture.manager.running_ids().await.is_empty());
    assert!(!fixture.slot(id).join("active.json").exists());
    assert!(
        tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, fixture.port(id)))
            .await
            .is_err()
    );
    assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
    assert!(fixture
        .source
        .join("local-app.json.artificial-hidden")
        .is_file());
}

#[tokio::test]
async fn failed_detail_during_reuse_does_not_stop_the_already_running_instance() {
    let fixture = Fixture::new("keep-manifest");
    let registered = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registered["id"].as_str().unwrap();
    fixture.manager.start(id).await.unwrap();
    let original_pid = fixture.pid(id);
    fs::rename(
        fixture.source.join("local-app.json"),
        fixture.source.join("local-app.json.artificial-hidden"),
    )
    .unwrap();
    assert!(fixture.manager.start(id).await.is_err());
    assert_eq!(fixture.manager.running_ids().await, [id]);
    assert_eq!(unsafe { libc::kill(original_pid as i32, 0) }, 0);
    {
        let inner = fixture.manager.inner.lock().await;
        inner
            .running
            .get(id)
            .unwrap()
            .runtime
            .verify_health()
            .await
            .unwrap();
    }
    assert!(fixture.slot(id).join("active.json").exists());
    fixture.manager.stop_runtime(id).await.unwrap();
    assert!(!fixture.slot(id).join("active.json").exists());
}

fn process_is_live(pid: i32) -> bool {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info);
    let got = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size as i32,
        )
    };
    got == size as i32 && info.pbi_status != libc::SZOMB
}

fn copy_source(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    for name in ["public", "server"] {
        fs::create_dir(destination.join(name)).unwrap();
    }
    for name in ["local-app.json", "public/index.html", "server/fixture.mjs"] {
        fs::copy(source.join(name), destination.join(name)).unwrap();
    }
}

#[tokio::test]
async fn shutdown_cleans_every_owned_group_and_preserves_an_unrelated_process() {
    use tokio::process::Command;
    let fixture = Fixture::new("leak-descendants");
    let first = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let first_id = first["id"].as_str().unwrap();
    let second_source = fixture.source.with_file_name("second-disposable-source");
    copy_source(&fixture.source, &second_source);
    let path = second_source.join("local-app.json");
    let mut declaration: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    declaration["configuration"]["fields"][0]["default"] = json!("hang-stop");
    fs::write(path, serde_json::to_vec(&declaration).unwrap()).unwrap();
    let second = fixture.manager.register(second_source).await.unwrap();
    let second_id = second["id"].as_str().unwrap();
    fixture.manager.start(first_id).await.unwrap();
    fixture.manager.start(second_id).await.unwrap();
    let mut unrelated = Command::new("/usr/bin/python3")
        .args(["-c", "import time;time.sleep(60)"])
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let unrelated_pid = unrelated.id().unwrap() as i32;
    let pids = [
        fixture.pid(first_id) as i32,
        fixture.pid(second_id) as i32,
        fs::read_to_string(fixture.slot(first_id).join("data/child-pid"))
            .unwrap()
            .parse()
            .unwrap(),
        fs::read_to_string(fixture.slot(first_id).join("data/grandchild-pid"))
            .unwrap()
            .parse()
            .unwrap(),
    ];
    // Forced termination can preserve a business failure, but must not leave
    // any owned process or prevent cleanup of the other registered application.
    assert!(fixture.manager.stop_all().await.is_err());
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    for pid in pids {
        assert!(!process_is_live(pid));
    }
    for id in [first_id, second_id] {
        assert!(
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, fixture.port(id)))
                .await
                .is_err()
        );
        assert_eq!(
            fixture.manager.configuration(id).await.unwrap()["state"],
            "failed"
        );
    }
    assert!(process_is_live(unrelated_pid));
    unrelated.kill().await.unwrap();
    unrelated.wait().await.unwrap();
}

#[tokio::test]
async fn health_failure_and_missing_source_still_clean_the_whole_owned_group() {
    let fixture = Fixture::new("leak-descendants");
    let registered = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registered["id"].as_str().unwrap();
    fixture.manager.start(id).await.unwrap();
    let pids = [
        fixture.pid(id) as i32,
        fs::read_to_string(fixture.slot(id).join("data/child-pid"))
            .unwrap()
            .parse()
            .unwrap(),
        fs::read_to_string(fixture.slot(id).join("data/grandchild-pid"))
            .unwrap()
            .parse()
            .unwrap(),
    ];
    fs::rename(
        fixture.source.join("local-app.json"),
        fixture.source.join("local-app.json.artificial-hidden"),
    )
    .unwrap();
    fs::write(
        fixture.slot(id).join("data/force-health-failure"),
        b"artificial",
    )
    .unwrap();
    fixture.manager.monitor_tick().await;
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    for pid in pids {
        assert!(!process_is_live(pid));
    }
    assert!(fixture
        .manager
        .inner
        .lock()
        .await
        .errors
        .get(id)
        .unwrap()
        .contains("未完成正常关闭"));
    fixture.manager.stop_runtime(id).await.unwrap();
}

#[tokio::test]
async fn unconfirmed_ownership_retains_cleanup_handle_and_blocks_a_second_instance() {
    let fixture = Fixture::new("keep-manifest");
    let registered = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registered["id"].as_str().unwrap();
    fixture.manager.start(id).await.unwrap();
    let pid = fixture.pid(id) as i32;
    // Artificially steal this disposable child's reap to simulate an OS
    // ownership failure. No unrelated PID or process group is signalled.
    assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
    assert!(fixture.manager.stop_runtime(id).await.is_err());
    assert_eq!(fixture.manager.owned_process_ids().await, [id]);
    assert!(fixture.manager.running_ids().await.is_empty());
    fixture.manager.monitor_tick().await;
    assert_eq!(fixture.manager.owned_process_ids().await, [id]);
    assert!(
        fixture.manager.configuration(id).await.unwrap()["lastError"]
            .as_str()
            .unwrap()
            .contains("尚未确认全部关闭")
    );
    assert!(fixture.manager.start(id).await.is_err());
    assert!(fixture.manager.stop_all().await.is_err());
    assert_eq!(fixture.manager.owned_process_ids().await, [id]);
    assert_eq!(
        fixture.manager.configuration(id).await.unwrap()["state"],
        "failed"
    );
}

#[tokio::test]
async fn control_failure_with_graceful_exit_still_preserves_business_failure_marker() {
    let fixture = Fixture::new("log-budget-graceful");
    let registered = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registered["id"].as_str().unwrap();
    fixture.manager.start(id).await.unwrap();
    {
        let mut inner = fixture.manager.inner.lock().await;
        let runtime = &mut inner.running.get_mut(id).unwrap().runtime;
        runtime.ping().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while runtime.status().unwrap().state != "failed" {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let status = runtime.stop().await.unwrap();
        assert!(status.process_group_exited && status.stopping_observed);
        assert!(!status.forced);
        assert_eq!(status.exit_code, Some(0));
        assert_eq!(status.state, "failed");
    }
    assert!(fixture.manager.stop_runtime(id).await.is_err());
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    assert!(fixture.slot(id).join("active.json").exists());
    assert_eq!(
        fixture.manager.configuration(id).await.unwrap()["state"],
        "failed"
    );
}

async fn proxy_test_browser(manager: &NodeApps, id: &str) -> (reqwest::Client, String, String) {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let bootstrap = manager.open_url(id).await.unwrap();
    let url = url::Url::parse(&bootstrap).unwrap();
    let origin = format!(
        "{}://{}:{}",
        url.scheme(),
        url.host_str().unwrap(),
        url.port().unwrap()
    );
    let response = client.get(bootstrap).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
    let cookie = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    (client, origin, cookie)
}
async fn wait_artificial_process_gone(pid: u32) {
    let pid: i32 = pid.try_into().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        loop {
            if unsafe { libc::kill(pid, 0) } == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("owned artificial runtime should stop before the blocked proxy file is released");
}

#[tokio::test]
async fn proxy_drain_failure_retains_running_even_after_the_actual_runtime_exits() {
    let fixture = Fixture::new("normal");
    let registered = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registered["id"].as_str().unwrap();
    fixture.manager.start(id).await.unwrap();
    let marker = fixture.slot(id).join("active.json");
    let evidence = fixture.slot(id).join("data/artificial-preserved-value");
    fs::write(&evidence, b"artificial retained business bytes").unwrap();
    {
        let inner = fixture.manager.inner.lock().await;
        inner.running[id]
            .proxy
            .as_ref()
            .unwrap()
            .fixture_serve_panic();
    }
    assert_eq!(
        fixture.manager.stop_all().await.unwrap_err().code,
        "NODE_APP_PROXY_DRAIN_BLOCKED"
    );
    assert_eq!(fixture.manager.owned_process_ids().await, [id]);
    assert!(fixture.manager.running_ids().await.is_empty());
    {
        let mut inner = fixture.manager.inner.lock().await;
        let running = inner.running.get_mut(id).unwrap();
        assert!(running.runtime.status().unwrap().process_group_exited);
        assert!(running.proxy.is_some());
        assert!(running.cleanup_pending);
    }
    assert!(marker.exists());
    assert!(fs::read(evidence).unwrap() == b"artificial retained business bytes");
    assert!(fixture.manager.start(id).await.is_err());
    assert!(fixture.manager.stop_all().await.is_err());
}

#[tokio::test]
async fn one_blocked_proxy_does_not_prevent_the_other_real_runtime_from_stopping() {
    let fixture = Fixture::new("normal");
    let second = Fixture::new("normal");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(second.source.join("local-app.json")).unwrap()).unwrap();
    manifest["app"]["id"] = json!("test.disposable.lifecycle.second");
    fs::write(
        second.source.join("local-app.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    let first = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let second_registered = fixture
        .manager
        .register(second.source.clone())
        .await
        .unwrap();
    let mut ids = [
        first["id"].as_str().unwrap().to_owned(),
        second_registered["id"].as_str().unwrap().to_owned(),
    ];
    ids.sort();
    for id in &ids {
        fixture.manager.start(id).await.unwrap();
    }
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    {
        let inner = fixture.manager.inner.lock().await;
        inner.running[&ids[0]]
            .proxy
            .as_ref()
            .unwrap()
            .fixture_read_pause(entered, blocked);
    }
    let pids = ids.iter().map(|id| fixture.pid(id)).collect::<Vec<_>>();
    let (client, origin, cookie) = proxy_test_browser(&fixture.manager, &ids[0]).await;
    let request = tokio::spawn(
        client
            .get(format!("{origin}/"))
            .header(reqwest::header::COOKIE, cookie)
            .send(),
    );
    ready.await.unwrap();
    let manager = Arc::new(fixture.manager);
    let closing = manager.clone();
    let stop = tokio::spawn(async move { closing.stop_all().await });
    wait_artificial_process_gone(pids[1]).await;
    wait_artificial_process_gone(pids[0]).await;
    assert!(
        !stop.is_finished(),
        "first proxy remains busy despite both runtimes really exiting"
    );
    release.send(()).unwrap();
    if let Ok(Ok(response)) = request.await {
        drop(response);
    }
    tokio::time::timeout(std::time::Duration::from_secs(4), stop)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(manager.owned_process_ids().await.is_empty());
}

#[tokio::test]
async fn losing_node_stop_waiter_preserves_running_and_proxy_until_actual_file_completion() {
    let fixture = Fixture::new("normal");
    let registered = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registered["id"].as_str().unwrap().to_owned();
    fixture.manager.start(&id).await.unwrap();
    let pid = fixture.pid(&id);
    let marker = fixture.slot(&id).join("active.json");
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    {
        let inner = fixture.manager.inner.lock().await;
        inner.running[&id]
            .proxy
            .as_ref()
            .unwrap()
            .fixture_read_pause(entered, blocked);
    }
    let (client, origin, cookie) = proxy_test_browser(&fixture.manager, &id).await;
    let request = tokio::spawn(
        client
            .get(format!("{origin}/"))
            .header(reqwest::header::COOKIE, cookie)
            .send(),
    );
    ready.await.unwrap();
    let manager = Arc::new(fixture.manager);
    let closing = manager.clone();
    let stop = tokio::spawn(async move { closing.stop_all().await });
    wait_artificial_process_gone(pid).await;
    assert!(!stop.is_finished());
    stop.abort();
    assert!(stop.await.unwrap_err().is_cancelled());
    assert_eq!(manager.owned_process_ids().await, [id.as_str()]);
    assert!(marker.exists());
    assert!(manager.start(&id).await.is_err());
    release.send(()).unwrap();
    if let Ok(Ok(response)) = request.await {
        drop(response);
    }
    tokio::time::timeout(std::time::Duration::from_secs(4), manager.stop_all())
        .await
        .unwrap()
        .unwrap();
    assert!(manager.owned_process_ids().await.is_empty());
    assert!(!marker.exists());
}
