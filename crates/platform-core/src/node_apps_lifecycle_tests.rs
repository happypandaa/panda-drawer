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
ready_instance = instance + '-wrong' if os.environ['TRIAL_BEHAVIOR'].startswith('bad-ready') else instance
print(json.dumps({'protocolVersion':1,'type':'ready','instanceId':ready_instance,'host':'127.0.0.1','port':server.server_address[1]}),flush=True)
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
        if os.environ['TRIAL_BEHAVIOR'] != 'bad-ready-without-stopping':
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
        .fixture_running(id)
        .await
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
    fixture
        .manager
        .fixture_running(id)
        .await
        .runtime
        .verify_health()
        .await
        .unwrap();
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
    // Forced termination remains diagnostic information. Once both groups and
    // their owned I/O are closed, it must not prevent complete shutdown.
    fixture.manager.stop_all().await.unwrap();
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
            "stopped"
        );
        assert!(!fixture.slot(id).join("active.json").exists());
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
        .contains("确认后台进程已停止"));
    assert!(!fixture.slot(id).join("active.json").exists());
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
async fn control_failure_with_closed_group_and_io_does_not_preserve_a_running_marker() {
    let fixture = Fixture::new("log-budget-graceful");
    let registered = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registered["id"].as_str().unwrap();
    fixture.manager.start(id).await.unwrap();
    {
        let mut running = fixture.manager.fixture_running(id).await;
        let runtime = &mut running.runtime;
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
    fixture.manager.stop_runtime(id).await.unwrap();
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    assert!(!fixture.slot(id).join("active.json").exists());
    assert_eq!(
        fixture.manager.configuration(id).await.unwrap()["state"],
        "stopped"
    );
    let preview = fixture.manager.delete_preview(id).await.unwrap();
    assert!(preview["blocker"].is_null());
}

#[tokio::test]
async fn deleting_a_forced_stop_preserves_external_source_and_another_running_instance() {
    let fixture = Fixture::new("hang-stop");
    let author_sentinel = fixture.source.join("author-kept.txt");
    fs::write(
        &author_sentinel,
        b"disposable author source must survive deletion",
    )
    .unwrap();
    let first = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let first_id = first["id"].as_str().unwrap();

    let second_source = fixture
        .source
        .with_file_name("other-disposable-running-source");
    copy_source(&fixture.source, &second_source);
    let declaration_path = second_source.join("local-app.json");
    let mut declaration: Value =
        serde_json::from_slice(&fs::read(&declaration_path).unwrap()).unwrap();
    declaration["app"]["id"] = json!("test.disposable.lifecycle.unrelated");
    declaration["configuration"]["fields"][0]["default"] = json!("normal");
    fs::write(&declaration_path, serde_json::to_vec(&declaration).unwrap()).unwrap();
    let second = fixture
        .manager
        .register(second_source.clone())
        .await
        .unwrap();
    let second_id = second["id"].as_str().unwrap();

    fixture.manager.start(first_id).await.unwrap();
    fixture.manager.start(second_id).await.unwrap();
    let first_pid = fixture.pid(first_id) as i32;
    let second_pid = fixture.pid(second_id) as i32;
    let other_sentinel = fixture.slot(second_id).join("data/other-app-kept.txt");
    fs::write(
        &other_sentinel,
        b"another app remains running and keeps its data",
    )
    .unwrap();
    let registration_before = fs::read(fixture.slot(second_id).join("registration.json")).unwrap();
    let source_before = fs::read(fixture.source.join("server/fixture.mjs")).unwrap();
    let preview = fixture.manager.delete_preview(first_id).await.unwrap();
    assert!(preview["blocker"].is_null());
    let result = fixture
        .manager
        .delete(
            first_id,
            DeleteNodeAppInput {
                expected_revision: preview["expectedRevision"].as_str().unwrap().into(),
            },
        )
        .await
        .unwrap();

    assert_eq!(result["operationOutcome"], "deleted");
    assert_eq!(result["ownership"], "external");
    assert_eq!(result["sourcePreserved"], true);
    assert!(!process_is_live(first_pid));
    assert!(!fixture.slot(first_id).join("registration.json").exists());
    assert!(!fixture.slot(first_id).join("configuration").exists());
    assert!(!fixture.slot(first_id).join("data").exists());
    assert!(!fixture.slot(first_id).join("cache").exists());
    assert!(!fixture.slot(first_id).join("active.json").exists());
    assert_eq!(
        fs::read(&author_sentinel).unwrap(),
        b"disposable author source must survive deletion"
    );
    assert_eq!(
        fs::read(fixture.source.join("server/fixture.mjs")).unwrap(),
        source_before
    );
    assert!(second_source.join("local-app.json").is_file());
    assert!(process_is_live(second_pid));
    assert_eq!(fixture.manager.running_ids().await, [second_id]);
    assert_eq!(
        fs::read(&other_sentinel).unwrap(),
        b"another app remains running and keeps its data"
    );
    assert_eq!(
        fs::read(fixture.slot(second_id).join("registration.json")).unwrap(),
        registration_before
    );
    fixture
        .manager
        .fixture_running(second_id)
        .await
        .runtime
        .ping()
        .await
        .unwrap();
    fixture.manager.stop_runtime(second_id).await.unwrap();
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
    fixture
        .manager
        .fixture_running(id)
        .await
        .proxy
        .as_ref()
        .unwrap()
        .fixture_serve_panic();
    assert_eq!(
        fixture.manager.stop_all().await.unwrap_err().code,
        "NODE_APP_PROXY_DRAIN_BLOCKED"
    );
    assert_eq!(fixture.manager.owned_process_ids().await, [id]);
    assert!(fixture.manager.running_ids().await.is_empty());
    {
        let mut running = fixture.manager.fixture_running(id).await;
        assert!(running.runtime.status().unwrap().process_group_exited);
        assert!(running.proxy.is_some());
    }
    assert!(fixture.manager.inner.lock().await.running[id].cleanup_pending());
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
    fixture
        .manager
        .fixture_running(&ids[0])
        .await
        .proxy
        .as_ref()
        .unwrap()
        .fixture_read_pause(entered, blocked);
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
    fixture
        .manager
        .fixture_running(&id)
        .await
        .proxy
        .as_ref()
        .unwrap()
        .fixture_read_pause(entered, blocked);
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

#[tokio::test]
async fn blocked_instance_stop_keeps_status_and_another_instance_available_after_waiter_loss() {
    let fixture = Fixture::new("normal");
    let other = Fixture::new("normal");
    let first = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let second = fixture
        .manager
        .register(other.source.clone())
        .await
        .unwrap();
    let id = first["id"].as_str().unwrap().to_owned();
    let other_id = second["id"].as_str().unwrap().to_owned();
    fixture.manager.start(&id).await.unwrap();
    fixture.manager.start(&other_id).await.unwrap();
    let pid = fixture.pid(&id);
    let other_pid = fixture.pid(&other_id);
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    fixture
        .manager
        .fixture_running(&id)
        .await
        .proxy
        .as_ref()
        .unwrap()
        .fixture_read_pause(entered, blocked);
    let (client, origin, cookie) = proxy_test_browser(&fixture.manager, &id).await;
    let request = tokio::spawn(
        client
            .get(format!("{origin}/"))
            .header(reqwest::header::COOKIE, cookie)
            .send(),
    );
    ready.await.unwrap();
    let manager = fixture.manager.clone();
    let stop_id = id.clone();
    let stopping = tokio::spawn(async move { manager.stop_runtime(&stop_id).await });
    wait_artificial_process_gone(pid).await;
    assert!(!stopping.is_finished());
    let snapshot = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        fixture.manager.status(&id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(snapshot["state"], "stopping");
    assert_eq!(snapshot["configuration"]["canStart"], false);
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.manager.start(&other_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(process_is_live(other_pid as i32));
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        fixture.manager.stop_runtime(&other_id),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!process_is_live(other_pid as i32));
    stopping.abort();
    assert!(stopping.await.unwrap_err().is_cancelled());
    assert!(fixture.slot(&id).join("active.json").is_file());
    release.send(()).unwrap();
    if let Ok(Ok(response)) = request.await {
        drop(response);
    }
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        while !fixture.manager.owned_process_ids().await.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!fixture.slot(&id).join("active.json").exists());
    assert!(fixture.source.join("server/fixture.mjs").is_file());
    assert!(other.source.join("server/fixture.mjs").is_file());
}

#[tokio::test]
async fn lost_start_waiter_remains_owned_without_blocking_another_instance_or_shutdown() {
    let fixture = Fixture::new("normal");
    let other = Fixture::new("normal");
    let first = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = first["id"].as_str().unwrap().to_owned();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = tokio::sync::oneshot::channel();
    *fixture.manager.start_intent_pause.lock().unwrap() = Some((entered, blocked));
    let manager = fixture.manager.clone();
    let start_id = id.clone();
    let starting = tokio::spawn(async move { manager.start(&start_id).await });
    ready.await.unwrap();
    let snapshot = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        fixture.manager.status(&id),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(snapshot["state"], "starting");
    assert_eq!(snapshot["configuration"]["canStart"], false);
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        fixture.manager.register(other.source.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    let other_id = second["id"].as_str().unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        fixture.manager.start(other_id),
    )
    .await
    .unwrap()
    .unwrap();
    fixture.manager.stop_runtime(other_id).await.unwrap();
    starting.abort();
    assert!(starting.await.unwrap_err().is_cancelled());
    assert_eq!(fixture.manager.owned_process_ids().await, [id.as_str()]);
    let manager = fixture.manager.clone();
    let shutdown = tokio::spawn(async move { manager.stop_all().await });
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !fixture.manager.inner.lock().await.shutting_down {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!shutdown.is_finished());
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(8), shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    assert!(!fixture.slot(&id).join("active.json").exists());
    assert!(fixture
        .manager
        .processes
        .snapshot()
        .iter()
        .all(|record| record.resource_closed()));
    assert!(fixture.source.join("server/fixture.mjs").is_file());
}

#[tokio::test]
async fn marker_retirement_failure_keeps_closed_runtime_blocked_and_preserves_replacement() {
    use std::os::unix::fs::MetadataExt;
    for replace in [false, true] {
        let fixture = Fixture::new("normal");
        let registered = fixture
            .manager
            .register(fixture.source.clone())
            .await
            .unwrap();
        let id = registered["id"].as_str().unwrap();
        fixture.manager.start(id).await.unwrap();
        let pid = fixture.pid(id);
        let marker = fixture.slot(id).join("active.json");
        let displaced = fixture.slot(id).join("displaced-owned-active.json");
        let original = fs::read(&marker).unwrap();
        let sentinel = fixture.slot(id).join("data/retained-after-retire-failure");
        fs::write(&sentinel, b"business data must survive failed retirement").unwrap();
        let (entered, ready) = tokio::sync::oneshot::channel();
        let (release, blocked) = std::sync::mpsc::channel();
        fixture
            .manager
            .fixture_running(id)
            .await
            .proxy
            .as_ref()
            .unwrap()
            .fixture_read_pause(entered, blocked);
        let (client, origin, cookie) = proxy_test_browser(&fixture.manager, id).await;
        let request = tokio::spawn(
            client
                .get(format!("{origin}/"))
                .header(reqwest::header::COOKIE, cookie)
                .send(),
        );
        ready.await.unwrap();
        let replacement = b"foreign marker must never be retired";
        let mut replacement_inode = None;
        let (stopped, ()) = tokio::join!(fixture.manager.stop_runtime(id), async {
            // The real child exits while a real proxy file read still owns the
            // drain. Replace the active name before retirement can begin.
            wait_artificial_process_gone(pid).await;
            fs::rename(&marker, &displaced).unwrap();
            if replace {
                fs::write(&marker, replacement).unwrap();
                replacement_inode = Some(fs::symlink_metadata(&marker).unwrap().ino());
            }
            release.send(()).unwrap();
            if let Ok(Ok(response)) = request.await {
                drop(response);
            }
        });
        assert!(stopped.is_err());
        assert!(!process_is_live(pid as i32));
        assert_eq!(fixture.manager.owned_process_ids().await, [id]);
        assert!(fixture.manager.running_ids().await.is_empty());
        assert!(
            fixture
                .manager
                .fixture_running(id)
                .await
                .runtime
                .status()
                .unwrap()
                .process_group_exited
        );
        assert!(fixture.manager.inner.lock().await.markers.contains_key(id));
        for detail in [
            fixture.manager.configuration(id).await.unwrap(),
            fixture.manager.status(id).await.unwrap(),
        ] {
            assert_eq!(detail["state"], "failed", "replacement={replace}");
            assert_eq!(detail["configuration"]["canStart"], false);
            assert!(detail["lastError"].as_str().unwrap().contains("运行标记"));
        }
        assert!(fixture.manager.start(id).await.is_err());
        let preview = fixture.manager.delete_preview(id).await.unwrap();
        assert!(fixture
            .manager
            .delete(
                id,
                DeleteNodeAppInput {
                    expected_revision: preview["expectedRevision"].as_str().unwrap().into(),
                },
            )
            .await
            .is_err());
        assert_eq!(fs::read(&displaced).unwrap(), original);
        assert_eq!(
            fs::read(&sentinel).unwrap(),
            b"business data must survive failed retirement"
        );
        if let Some(inode) = replacement_inode {
            assert_eq!(fs::read(&marker).unwrap(), replacement);
            assert_eq!(fs::symlink_metadata(&marker).unwrap().ino(), inode);
        } else {
            assert!(!marker.exists());
        }
        assert_eq!(fixture.manager.owned_process_ids().await, [id]);
    }
}

#[tokio::test]
async fn sealed_after_start_intent_confirms_no_spawn_and_allows_reopen() {
    let fixture = Fixture::new("normal");
    let registration = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registration["id"].as_str().unwrap();
    let marker = fixture.slot(id).join("active.json");
    let sentinel = fixture.slot(id).join("data/preserved-startup-record");
    fs::write(&sentinel, "人工数据保持 🌱").unwrap();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let (release, resume) = tokio::sync::oneshot::channel();
    *fixture.manager.start_intent_pause.lock().unwrap() = Some((entered, resume));
    let (result, ()) = tokio::join!(fixture.manager.start(id), async {
        observed.await.unwrap();
        assert!(marker.is_file());
        assert!(!fixture.slot(id).join("data/artificial-pid").exists());
        fixture.manager.processes.seal();
        release.send(()).unwrap();
    });
    let error = result.unwrap_err();
    assert_eq!(error.details["launchCauseCode"], "SERVICE_STOPPING");
    assert_eq!(error.details["launchCleanupConfirmed"], true);
    assert!(!marker.exists());
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    assert!(fixture
        .manager
        .processes
        .snapshot()
        .iter()
        .all(|p| p.resource_closed()));
    assert_eq!(fs::read_to_string(sentinel).unwrap(), "人工数据保持 🌱");
    let mut reopened =
        NodeApps::new(fixture.data.clone(), fixture.manager.node_path.clone()).unwrap();
    reopened.digest = fixture.manager.digest.clone();
    reopened.start(id).await.unwrap();
    reopened.stop_runtime(id).await.unwrap();
    assert!(!marker.exists());
}

#[tokio::test]
async fn cancelled_admitted_second_probe_joins_real_resources_before_clearing_intent() {
    let mut fixture = Fixture::new("normal");
    let counter = fixture.manager.node_path.with_file_name("probe-count");
    let admitted = fixture.manager.node_path.with_file_name("second-probe-pid");
    let probe = fixture
        .manager
        .node_path
        .with_file_name("disposable-version-probe.py");
    fs::write(
        &probe,
        format!(
            r#"import os,time,pathlib
counter=pathlib.Path({counter:?})
count=int(counter.read_text())+1 if counter.exists() else 1
counter.write_text(str(count))
if count==2:
    pathlib.Path({admitted:?}).write_text(str(os.getpid()))
    time.sleep(60)
print('v24.22.0',flush=True)
"#,
            counter = counter.to_str().unwrap(),
            admitted = admitted.to_str().unwrap()
        ),
    )
    .unwrap();
    fs::write(&fixture.manager.node_path, format!("#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exec /usr/bin/python3 '{}' ; fi\nexec /usr/bin/python3 \"$@\"\n", probe.display())).unwrap();
    fixture.manager.digest = format!(
        "{:x}",
        Sha256::digest(fs::read(&fixture.manager.node_path).unwrap())
    );
    let registration = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registration["id"].as_str().unwrap();
    let marker = fixture.slot(id).join("active.json");
    let (result, ()) = tokio::join!(fixture.manager.start(id), async {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while !admitted.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "actual second probe must start"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(marker.is_file());
        fixture.manager.processes.seal();
    });
    let error = result.unwrap_err();
    assert_eq!(error.details["launchCauseCode"], "PROCESS_CANCELLED");
    assert_eq!(error.details["launchCleanupConfirmed"], true);
    let records = fixture.manager.processes.snapshot();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|p| p.resource_closed()));
    let pid: i32 = fs::read_to_string(admitted).unwrap().parse().unwrap();
    assert!(!process_is_live(pid));
    assert!(!fixture.slot(id).join("data/artificial-pid").exists());
    assert!(!marker.exists());
    assert!(fixture.manager.processes.shutdown().await.is_empty());
}

#[tokio::test]
async fn failed_ready_with_verified_normal_stop_clears_only_this_launch_intent() {
    let fixture = Fixture::new("bad-ready");
    let registration = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registration["id"].as_str().unwrap();
    let marker = fixture.slot(id).join("active.json");
    for _ in 0..2 {
        let error = fixture.manager.start(id).await.unwrap_err();
        assert_eq!(error.details["launchCauseCode"], "TRIAL_READY_INVALID");
        assert_eq!(error.details["launchCleanupConfirmed"], true);
        assert!(!process_is_live(fixture.pid(id) as i32));
        assert!(!marker.exists());
        assert!(fixture.manager.owned_process_ids().await.is_empty());
    }
}

#[tokio::test]
async fn failed_ready_without_normal_handshake_clears_intent_only_after_verified_resource_stop() {
    let fixture = Fixture::new("bad-ready-without-stopping");
    let registration = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registration["id"].as_str().unwrap();
    let marker = fixture.slot(id).join("active.json");
    for _ in 0..2 {
        let error = fixture.manager.start(id).await.unwrap_err();
        assert_eq!(error.details["launchCauseCode"], "TRIAL_READY_INVALID");
        assert_eq!(error.details["launchCleanupConfirmed"], true);
        assert!(!process_is_live(fixture.pid(id) as i32));
        assert!(fixture.manager.owned_process_ids().await.is_empty());
        assert!(
            !marker.exists(),
            "the local runtime's completed group and I/O stop, rather than a normal protocol exit, authorizes retiring this owned intent"
        );
    }
}

#[tokio::test]
async fn confirmed_no_spawn_never_removes_replaced_modified_or_retyped_intent() {
    use std::os::unix::fs::{symlink, MetadataExt};
    for replacement in [
        "same-content-new-inode",
        "modified-in-place",
        "hard-link",
        "symlink",
        "directory",
    ] {
        let fixture = Fixture::new("normal");
        let registration = fixture
            .manager
            .register(fixture.source.clone())
            .await
            .unwrap();
        let id = registration["id"].as_str().unwrap();
        let marker = fixture.slot(id).join("active.json");
        let retained = marker.with_file_name("preserved-original-intent.json");
        let (entered, observed) = tokio::sync::oneshot::channel();
        let (release, resume) = tokio::sync::oneshot::channel();
        *fixture.manager.start_intent_pause.lock().unwrap() = Some((entered, resume));
        let (result, identity) = tokio::join!(fixture.manager.start(id), async {
            observed.await.unwrap();
            let contents = fs::read(&marker).unwrap();
            if replacement == "modified-in-place" {
                fs::write(&marker, b"foreign-intent").unwrap();
            } else if replacement == "hard-link" {
                fs::hard_link(&marker, &retained).unwrap();
            } else {
                fs::rename(&marker, &retained).unwrap();
                match replacement {
                    "same-content-new-inode" => fs::write(&marker, contents).unwrap(),
                    "symlink" => symlink(&retained, &marker).unwrap(),
                    "directory" => fs::create_dir(&marker).unwrap(),
                    _ => unreachable!(),
                }
            }
            let identity = fs::symlink_metadata(&marker).unwrap().ino();
            fixture.manager.processes.seal();
            release.send(()).unwrap();
            identity
        });
        let error = result.unwrap_err();
        assert!(error.message.contains("归属"), "{replacement}: {error}");
        assert_eq!(error.details["launchCauseCode"], "SERVICE_STOPPING");
        assert_eq!(fs::symlink_metadata(&marker).unwrap().ino(), identity);
        if replacement == "modified-in-place" {
            assert_eq!(fs::read(&marker).unwrap(), b"foreign-intent");
        }
        assert!(!fixture.slot(id).join("data/artificial-pid").exists());
    }
}

#[tokio::test]
async fn late_foreign_intent_is_atomically_captured_and_never_deleted_or_overwritten() {
    use std::os::unix::fs::MetadataExt;
    for race in [
        "before-capture",
        "before-and-after-capture",
        "after-owned-capture",
    ] {
        let fixture = Fixture::new("normal");
        let registration = fixture
            .manager
            .register(fixture.source.clone())
            .await
            .unwrap();
        let id = registration["id"].as_str().unwrap();
        let marker = fixture.slot(id).join("active.json");
        let preserved = marker.with_file_name("preserved-original-intent.json");
        let intent = StartIntentOwnership::create(&marker, &json!({"disposable":"owned"})).unwrap();
        let mut first_foreign = None;
        let mut new_active = None;
        let error = intent
            .remove_if_owned_with(&marker, |point| match point {
                IntentCleanupPoint::BeforeCapture if race != "after-owned-capture" => {
                    // Deterministic replacement after the final old-path check.
                    fs::rename(&marker, &preserved).unwrap();
                    fs::write(&marker, b"foreign-captured").unwrap();
                    first_foreign = Some(fs::symlink_metadata(&marker).unwrap().ino());
                }
                IntentCleanupPoint::Captured if race != "before-capture" => {
                    fs::write(&marker, b"new-foreign-active").unwrap();
                    new_active = Some(fs::symlink_metadata(&marker).unwrap().ino());
                }
                _ => (),
            })
            .unwrap_err();
        assert!(error.message.contains("归属"), "{race}: {error}");
        let cleanup: Vec<_> = fs::read_dir(fixture.slot(id))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(START_INTENT_CLEANUP_PREFIX)
            })
            .collect();
        assert_eq!(
            cleanup.len(),
            1,
            "failed cleanup retains its persistent gate"
        );
        let captured = cleanup[0].join("captured-intent");
        if race == "before-capture" {
            assert_eq!(fs::read(&marker).unwrap(), b"foreign-captured");
            assert_eq!(
                fs::symlink_metadata(&marker).unwrap().ino(),
                first_foreign.unwrap()
            );
            assert!(
                !captured.exists(),
                "foreign intent was restored with no overwrite"
            );
        } else {
            assert_eq!(fs::read(&marker).unwrap(), b"new-foreign-active");
            assert_eq!(
                fs::symlink_metadata(&marker).unwrap().ino(),
                new_active.unwrap()
            );
            if race == "before-and-after-capture" {
                assert_eq!(fs::read(&captured).unwrap(), b"foreign-captured");
                assert_eq!(
                    fs::symlink_metadata(&captured).unwrap().ino(),
                    first_foreign.unwrap()
                );
            } else {
                assert!(
                    !captured.exists(),
                    "only the positively identified owned intent was removed"
                );
            }
        }
        assert!(fixture
            .manager
            .start(id)
            .await
            .unwrap_err()
            .message
            .contains("清理尚未确认"));
        assert!(
            fixture.manager.processes.snapshot().is_empty(),
            "gate rejects before running any probe"
        );
    }
}

#[tokio::test]
async fn residual_intent_cleanup_gate_blocks_reopen_even_without_an_active_marker() {
    use std::os::unix::fs::symlink;
    for kind in ["directory", "file", "symlink"] {
        let fixture = Fixture::new("normal");
        let registration = fixture
            .manager
            .register(fixture.source.clone())
            .await
            .unwrap();
        let id = registration["id"].as_str().unwrap();
        let preview = fixture.manager.delete_preview(id).await.unwrap();
        let sentinel = fixture.slot(id).join("data/preserved-cleanup-record");
        fs::write(&sentinel, b"preserve-runtime-data").unwrap();
        let before = fixture.manager.configuration(id).await.unwrap();
        assert_eq!(before["state"], "stopped");
        assert_eq!(before["configuration"]["canStart"], true);
        let gate = fixture
            .slot(id)
            .join(format!("{START_INTENT_CLEANUP_PREFIX}disposable-{kind}"));
        let outside = fixture.source.join("preserved-outside-cleanup-gate");
        fs::write(&outside, b"keep-external-evidence").unwrap();
        match kind {
            "directory" => fs::create_dir(&gate).unwrap(),
            "file" => fs::write(&gate, b"unknown-cleanup-evidence").unwrap(),
            "symlink" => symlink(&outside, &gate).unwrap(),
            _ => unreachable!(),
        }
        assert!(!fixture.slot(id).join("active.json").exists());
        let detail = fixture.manager.configuration(id).await.unwrap();
        assert_eq!(detail["state"], "failed");
        assert_eq!(detail["configuration"]["canStart"], false);
        assert!(detail["lastError"].as_str().unwrap().contains("重启电脑"));
        let listed = fixture.manager.list().await.unwrap();
        assert_eq!(listed[0]["state"], "failed");
        assert_eq!(listed[0]["configuration"]["canStart"], false);
        assert_eq!(
            fixture
                .manager
                .install_target_snapshot(Some(id))
                .await
                .unwrap_err()
                .code,
            "NODE_INSTALL_BUSY"
        );
        assert_eq!(
            fixture
                .manager
                .development_build_source_lease(id)
                .await
                .err()
                .unwrap()
                .code,
            "NODE_INSTALL_BUSY"
        );
        assert_eq!(
            fixture
                .manager
                .bind_development_source(id, fixture.source.clone())
                .await
                .unwrap_err()
                .code,
            "NODE_INSTALL_BUSY"
        );
        let delete_input =
            serde_json::from_value(json!({"expectedRevision":preview["expectedRevision"]}))
                .unwrap();
        assert_eq!(
            fixture
                .manager
                .delete(id, delete_input)
                .await
                .unwrap_err()
                .code,
            "NODE_APP_RUN_RECOVERY_REQUIRED"
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve-runtime-data");
        assert_eq!(
            fixture.manager.start(id).await.unwrap_err().code,
            "NODE_APP_DELETING"
        );
        let task = fixture
            .manager
            .deletion_tasks()
            .unwrap()
            .into_iter()
            .find(|task| task["appId"] == id)
            .unwrap();
        let task_id = task["taskId"].as_str().unwrap();
        fixture
            .manager
            .abandon_deletion(task_id, id, task["journalRevision"].as_u64().unwrap())
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if fixture.manager.deletion_task_status(task_id, id).unwrap()["phase"]
                    == "abandoned"
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(fixture
            .manager
            .start(id)
            .await
            .unwrap_err()
            .message
            .contains("清理尚未确认"));
        assert!(fs::symlink_metadata(&gate).is_ok());
        assert_eq!(fs::read(outside).unwrap(), b"keep-external-evidence");
        assert!(fixture.manager.processes.snapshot().is_empty());
    }
}

#[tokio::test]
async fn cleanup_gate_keeps_an_actual_running_backend_visible_and_blocks_new_start() {
    let fixture = Fixture::new("normal");
    let registration = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = registration["id"].as_str().unwrap();
    fixture.manager.start(id).await.unwrap();
    let gate = fixture
        .slot(id)
        .join(format!("{START_INTENT_CLEANUP_PREFIX}disposable-running"));
    fs::write(&gate, b"unconfirmed-cleanup-evidence").unwrap();
    let detail = fixture.manager.status(id).await.unwrap();
    assert_eq!(detail["state"], "running");
    assert_eq!(detail["configuration"]["canStart"], false);
    assert!(detail["lastError"].as_str().unwrap().contains("仍在运行"));
    assert!(process_is_live(fixture.pid(id) as i32));
    fixture.manager.stop_runtime(id).await.unwrap();
    assert_eq!(fs::read(gate).unwrap(), b"unconfirmed-cleanup-evidence");
    let stopped = fixture.manager.configuration(id).await.unwrap();
    assert_eq!(stopped["state"], "failed");
    assert_eq!(stopped["configuration"]["canStart"], false);
}
