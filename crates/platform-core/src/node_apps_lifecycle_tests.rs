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
        fs::write(source.join("server/fixture.mjs"), r#"import sys, os, json, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
message = json.loads(sys.stdin.readline())
instance = message['instanceId']
token = message['token']
source_manifest = os.path.join(os.path.dirname(os.path.dirname(__file__)), 'local-app.json')
with open(os.path.join(os.environ['APP_DATA_DIR'], 'artificial-pid'), 'w') as output:
    output.write(str(os.getpid()))
class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'
    def log_message(self, *args): pass
    def do_GET(self):
        if self.headers.get('Authorization') != 'Bearer ' + token:
            self.send_response(401); self.send_header('Content-Length','0'); self.end_headers(); return
        if os.environ['TRIAL_BEHAVIOR'] == 'remove-manifest' and os.path.isfile(source_manifest):
            os.rename(source_manifest, source_manifest + '.artificial-hidden')
        body = json.dumps({'protocolVersion':1,'instanceId':instance,'status':'ok'}).encode()
        self.send_response(200); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
server = ThreadingHTTPServer(('127.0.0.1',0), Handler)
with open(os.path.join(os.environ['APP_DATA_DIR'], 'artificial-port'), 'w') as output:
    output.write(str(server.server_address[1]))
threading.Thread(target=server.serve_forever, daemon=True).start()
print(json.dumps({'protocolVersion':1,'type':'ready','instanceId':instance,'host':'127.0.0.1','port':server.server_address[1]}),flush=True)
for line in sys.stdin:
    command = json.loads(line)
    if command['type'] == 'shutdown':
        print(json.dumps({'protocolVersion':1,'type':'stopping','instanceId':instance}),flush=True)
        break
    if command['type'] == 'ping':
        print(json.dumps({'protocolVersion':1,'type':'pong','instanceId':instance,'requestId':command['requestId']}),flush=True)
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
