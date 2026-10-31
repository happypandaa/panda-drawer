//! Public NodeApps integration with the independently pinned official Node.
//! Every source/control/data file belongs to a new named disposable directory.
//! The abrupt-host fixture is a real process boundary, not a Task Manager or
//! normal desktop/WebView acceptance test.
use super::*;
use crate::node_app_configuration::SecretAction;
use platform_native::windows::{files as native_files, process as native_process};
use std::{
    process::{Child, Command, Stdio},
    time::Duration,
};

mod configuration_concurrency {
    include!("windows_configuration_concurrency_tests.rs");
}

const HOST: &str = "node_apps::windows_runtime_tests::disposable_nodeapps_host";
const HOST_ROOT: &str = "PANDA_DISPOSABLE_W3_NODEAPPS_HOST_ROOT";
const FIRST_SECRET: &str = "artificial-nodeapps-secret-first";
const SECOND_SECRET: &str = "artificial-nodeapps-secret-second";
const PAGE: &str = "<!doctype html><meta charset=utf-8><p>Disposable public NodeApps page</p>";
const BACKEND: &str = r#"
const fs = require('node:fs'), path = require('node:path'), os = require('node:os');
const http = require('node:http'), readline = require('node:readline');
const crypto = require('node:crypto'), { spawn } = require('node:child_process');
const input = readline.createInterface({input:process.stdin, terminal:false});
let session, server;
function send(value) { process.stdout.write(JSON.stringify(value)+'\n'); }
const grandCode = "require('node:fs').writeFileSync(require('node:path').join(process.env.APP_DATA_DIR,'grandchild-pid'),String(process.pid));setInterval(()=>{},1000)";
const childCode = "const fs=require('node:fs'),path=require('node:path');require('node:child_process').spawn(process.execPath,['-e',"+JSON.stringify(grandCode)+"],{stdio:'ignore'});fs.writeFileSync(path.join(process.env.APP_DATA_DIR,'child-pid'),String(process.pid));setInterval(()=>{},1000)";
input.on('line', line => {
  const command = JSON.parse(line);
  if (command.type === 'initialize') {
    session = command;
    const counterFile = path.join(process.env.APP_DATA_DIR,'launch-count');
    const counter = fs.existsSync(counterFile) ? Number(fs.readFileSync(counterFile,'utf8'))+1 : 1;
    fs.writeFileSync(counterFile,String(counter));
    if (process.env.MODE === 'descendants') spawn(process.execPath,['-e',childCode],{stdio:'ignore'});
    server = http.createServer((request,response) => {
      if (request.headers.authorization !== 'Bearer '+session.token) { response.writeHead(401,{'Content-Length':0});response.end();return; }
      const value = request.url === '/_platform/health'
        ? {protocolVersion:1,instanceId:session.instanceId,status:'ok'}
        : {counter,greeting:process.env.GREETING,secretHash:crypto.createHash('sha256').update(process.env.API_KEY).digest('hex'),
           pid:process.pid,dataDirectory:process.env.APP_DATA_DIR,temporary:os.tmpdir(),declaredTemporary:process.env.APP_TEMP_DIR,
           inherited:process.env.PANDA_W3_MUST_NOT_INHERIT??null};
      const body=Buffer.from(JSON.stringify(value));
      response.writeHead(200,{'Content-Type':'application/json','Content-Length':body.length});response.end(body);
    });
    server.listen(0,'127.0.0.1',()=> {
      send({protocolVersion:1,type:'ready',instanceId:session.instanceId,host:'127.0.0.1',port:server.address().port});
      process.stderr.write(process.env.API_KEY+' '+session.token+' '+process.env.PANDA_DRAWER_RUN_IDENTITY+'\n');
      for(let index=0;index<96;index++) process.stderr.write(index+' '+process.env.API_KEY+' '+ 'x'.repeat(1024)+'\n');
    });
  } else if (command.type === 'ping') {
    send({protocolVersion:1,type:'pong',instanceId:session.instanceId,requestId:command.requestId});
  } else if (command.type === 'shutdown') {
    send({protocolVersion:1,type:'stopping',instanceId:session.instanceId});input.close();
    server.close(()=>process.exit(0));server.closeAllConnections();
  }
});
"#;

fn official_node() -> PathBuf {
    let node = PathBuf::from(
        std::env::var_os("PANDA_W3_OFFICIAL_NODE").expect("explicit pinned official Node required"),
    );
    assert!(node.is_absolute());
    let release: Value = serde_json::from_str(crate::node_tool_layout::RUNTIME_RELEASE).unwrap();
    assert_eq!(release["version"], "24.22.0");
    assert_eq!(
        format!("{:x}", Sha256::digest(fs::read(&node).unwrap())),
        release["binary"]["sha256"].as_str().unwrap()
    );
    // NodeApps keeps its normal release-selected digest; no test override.
    node
}

struct Fixture {
    source: PathBuf,
    data: PathBuf,
    node: PathBuf,
}
impl Fixture {
    fn create(root: &Path, mode: &str) -> Self {
        assert!(root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("panda-disposable-w3-nodeapps-"));
        let source = root.join("external-original-source 中文 space");
        let data = root.join("disposable-platform-data 中文 space");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&data).unwrap();
        fs::create_dir(source.join("public")).unwrap();
        fs::create_dir(source.join("server")).unwrap();
        fs::write(source.join("public/index.html"), PAGE).unwrap();
        fs::write(source.join("server/backend.cjs"), BACKEND).unwrap();
        let fields = json!([
            {"name":"GREETING","label":"Disposable greeting","type":"string","required":true,"secret":false,"description":"Fixture ordinary configuration","howToGet":["Use artificial test text"],"default":"first"},
            {"name":"API_KEY","label":"Disposable secret","type":"string","required":true,"secret":true,"description":"Artificial test credential only","howToGet":["Use the fixture artificial key"]},
            {"name":"MODE","label":"Disposable process mode","type":"string","required":false,"secret":false,"description":"Fixture process tree behavior","howToGet":["The fixture chooses its own mode"],"default":mode}
        ]);
        fs::write(source.join("local-app.json"),serde_json::to_vec(&json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"dev.disposable.windows.nodeapps","name":"可丢弃 NodeApps 集成","version":"1"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"protocolVersion":1,"root":"server","entry":"backend.cjs","nodeMajor":24},
            "configuration":{"version":1,"fields":fields}
        })).unwrap()).unwrap();
        Self {
            source,
            data,
            node: official_node(),
        }
    }
    fn manager(&self) -> NodeApps {
        NodeApps::new(self.data.clone(), self.node.clone()).unwrap()
    }
    fn slot(&self, id: &str) -> PathBuf {
        self.data.join("node-apps").join(id)
    }
}

async fn configure(apps: &NodeApps, id: &str, greeting: &str, secret: &str) -> Value {
    let view = apps.configuration(id).await.unwrap();
    let result = apps
        .save(
            id,
            SaveConfigurationRequest {
                revision: view["configuration"]["revision"].as_u64().unwrap(),
                manifest_digest: Some(view["manifestDigest"].as_str().unwrap().into()),
                values: BTreeMap::from([("GREETING".into(), json!(greeting))]),
                secrets: BTreeMap::from([(
                    "API_KEY".into(),
                    SecretAction::Replace {
                        value: secret.into(),
                    },
                )]),
            },
        )
        .await
        .unwrap();
    assert!(!result.to_string().contains(secret));
    assert_eq!(result["configuration"]["secretStatus"]["API_KEY"], "set");
    result
}

struct Browser {
    client: reqwest::Client,
    origin: String,
    cookie: String,
}
impl Browser {
    async fn open(apps: &NodeApps, id: &str) -> Self {
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let bootstrap = apps.open_url(id).await.unwrap();
        let url = url::Url::parse(&bootstrap).unwrap();
        let origin = url.origin().ascii_serialization();
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
        Self {
            client,
            origin,
            cookie,
        }
    }
    async fn state(&self) -> Value {
        let response = self
            .client
            .get(format!("{}/api/state", self.origin))
            .header("cookie", &self.cookie)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        serde_json::from_str(&response.text().await.unwrap()).unwrap()
    }
    async fn page(&self) -> String {
        let response = self
            .client
            .get(format!("{}/", self.origin))
            .header("cookie", &self.cookie)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        response.text().await.unwrap()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapturedProcess {
    pid: u32,
    creation_100ns: u64,
    executable: PathBuf,
}
impl CapturedProcess {
    fn capture(pid: u32) -> Self {
        let value = native_process::capture_live(pid)
            .unwrap()
            .expect("fixture process is actually live");
        Self {
            pid: value.pid,
            creation_100ns: value.creation_100ns,
            executable: value.executable,
        }
    }
    fn native(&self) -> native_process::ProcessIdentity {
        native_process::ProcessIdentity {
            pid: self.pid,
            creation_100ns: self.creation_100ns,
            executable: self.executable.clone(),
        }
    }
}
async fn stopped(processes: &[CapturedProcess]) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if processes.iter().all(|identity| {
                matches!(
                    native_process::observation(&identity.native()).unwrap(),
                    native_process::ProcessObservation::Stopped
                )
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("all captured original full-precision process identities must actually stop");
}
fn absent(path: &Path) {
    assert_eq!(
        fs::symlink_metadata(path).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
}
fn no_cleanup(slot: &Path) {
    assert!(fs::read_dir(slot).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(START_INTENT_CLEANUP_PREFIX)));
}

#[tokio::test]
#[ignore = "Explicit PANDA_W3_OFFICIAL_NODE required; public NodeApps with only new disposable source/data"]
async fn official_nodeapps_public_configuration_restart_proxy_and_persistent_data() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w3-nodeapps-public-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let fixture = Fixture::create(&root, "normal");
    let apps = fixture.manager();
    let registration = apps.register(fixture.source.clone()).await.unwrap();
    let id = registration["id"].as_str().unwrap().to_owned();
    assert_eq!(apps.list().await.unwrap().len(), 1);
    assert_eq!(registration["configuration"]["canStart"], false);
    assert_eq!(
        apps.start(&id).await.unwrap_err().code,
        "NODE_CONFIGURATION_REQUIRED"
    );
    absent(&fixture.slot(&id).join("active.json"));
    configure(&apps, &id, "first", FIRST_SECRET).await;
    assert_eq!(apps.start(&id).await.unwrap()["state"], "running");
    let browser = Browser::open(&apps, &id).await;
    assert_eq!(browser.page().await, PAGE);
    let first = browser.state().await;
    assert_eq!(first["counter"], 1);
    assert_eq!(first["greeting"], "first");
    assert_eq!(
        first["secretHash"],
        format!("{:x}", Sha256::digest(FIRST_SECRET.as_bytes()))
    );
    assert_eq!(first["inherited"], Value::Null);
    assert_eq!(first["temporary"], first["declaredTemporary"]);
    let first_process =
        CapturedProcess::capture(first["pid"].as_u64().unwrap().try_into().unwrap());
    let log = fixture.slot(&id).join("logs/current.log");
    tokio::time::timeout(Duration::from_secs(5), async {
        while fs::metadata(&log).unwrap().len() <= 64 * 1024 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    native_files::verify_private(&native_files::open_no_reparse(&log).unwrap().file).unwrap();
    assert!(!fs::read_to_string(&log).unwrap().contains(FIRST_SECRET));
    assert!(fs::read_to_string(&log).unwrap().contains("***"));
    let updated = configure(&apps, &id, "second", SECOND_SECRET).await;
    assert_eq!(updated["restartRequired"], true);
    assert_eq!(browser.state().await["greeting"], "first");
    let restarted = apps.restart(&id).await.unwrap();
    assert_eq!(restarted["id"], id);
    assert_eq!(restarted["restartRequired"], false);
    stopped(&[first_process]).await;
    let second = Browser::open(&apps, &id).await.state().await;
    assert_eq!(second["counter"], 2);
    assert_eq!(second["greeting"], "second");
    assert_eq!(
        second["secretHash"],
        format!("{:x}", Sha256::digest(SECOND_SECRET.as_bytes()))
    );
    assert_eq!(second["dataDirectory"], first["dataDirectory"]);
    let second_process =
        CapturedProcess::capture(second["pid"].as_u64().unwrap().try_into().unwrap());
    assert_eq!(apps.stop(&id).await.unwrap()["state"], "stopped");
    stopped(&[second_process]).await;
    assert!(apps.owned_process_ids().await.is_empty());
    absent(&fixture.slot(&id).join("active.json"));
    no_cleanup(&fixture.slot(&id));
    assert!(fixture.slot(&id).join("logs/previous.log").is_file());
    let configuration =
        fs::read_to_string(fixture.slot(&id).join("configuration/configuration.json")).unwrap();
    assert!(!configuration.contains(FIRST_SECRET) && !configuration.contains(SECOND_SECRET));
    drop(apps);
    let reopened = fixture.manager();
    assert_eq!(
        reopened.reconcile_run_markers().await.unwrap()["unconfirmed"],
        0
    );
    let status = reopened.status(&id).await.unwrap();
    assert_eq!(status["state"], "stopped");
    assert_eq!(status["configuration"]["canStart"], true);
    assert_eq!(status["configuration"]["secretStatus"]["API_KEY"], "set");
    assert_eq!(reopened.start(&id).await.unwrap()["id"], id);
    let third = Browser::open(&reopened, &id).await.state().await;
    assert_eq!(third["counter"], 3);
    assert_eq!(third["greeting"], "second");
    assert_eq!(third["secretHash"], second["secretHash"]);
    reopened.stop_all().await.unwrap();
    absent(&fixture.slot(&id).join("active.json"));
    no_cleanup(&fixture.slot(&id));
    assert_eq!(
        fs::read_to_string(fixture.slot(&id).join("data/launch-count")).unwrap(),
        "3"
    );
    assert_eq!(
        fs::read_to_string(fixture.source.join("server/backend.cjs")).unwrap(),
        BACKEND
    );
    assert_eq!(
        fs::read_to_string(fixture.source.join("public/index.html")).unwrap(),
        PAGE
    );
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HostReady {
    id: String,
    processes: Vec<CapturedProcess>,
}

#[tokio::test]
#[ignore = "Exact child host only; requires explicit new disposable root and pinned official Node"]
async fn disposable_nodeapps_host() {
    let root = PathBuf::from(std::env::var_os(HOST_ROOT).expect("explicit disposable host root"));
    assert!(root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("panda-disposable-w3-nodeapps-host-"));
    assert_eq!(
        std::env::var("PANDA_W3_MUST_NOT_INHERIT").unwrap(),
        "artificial-host-only"
    );
    let fixture = Fixture::create(&root, "descendants");
    let apps = fixture.manager();
    let registration = apps.register(fixture.source.clone()).await.unwrap();
    let id = registration["id"].as_str().unwrap().to_owned();
    configure(&apps, &id, "crash host", FIRST_SECRET).await;
    apps.start(&id).await.unwrap();
    let state = Browser::open(&apps, &id).await.state().await;
    assert_eq!(state["inherited"], Value::Null);
    let mut processes = vec![CapturedProcess::capture(
        state["pid"].as_u64().unwrap().try_into().unwrap(),
    )];
    for leaf in ["child-pid", "grandchild-pid"] {
        let path = fixture.slot(&id).join("data").join(leaf);
        let pid = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(pid) = fs::read_to_string(&path)
                    .ok()
                    .and_then(|value| value.parse::<u32>().ok())
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        processes.push(CapturedProcess::capture(pid));
    }
    fs::write(
        root.join("host-ready.json"),
        serde_json::to_vec(&HostReady { id, processes }).unwrap(),
    )
    .unwrap();
    // Keep the real NodeApps/runtime/Job/pump owners alive until the parent
    // terminates this original host Child HANDLE, without Rust Drop cleanup.
    std::future::pending::<()>().await;
    drop(apps);
}

struct RescueHost(Child);
impl Drop for RescueHost {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

#[tokio::test]
#[ignore = "Pinned official Node and a disposable abrupt host subprocess; not a desktop/Task Manager acceptance test"]
async fn abrupt_host_exit_kills_the_actual_node_job_and_new_manager_retries_marker_retirement() {
    let node = official_node();
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-w3-nodeapps-host-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let log = fs::File::create(root.join("host-fixture.log")).unwrap();
    let mut host = RescueHost(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                HOST,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(HOST_ROOT, &root)
            .env("PANDA_W3_OFFICIAL_NODE", &node)
            .env("PANDA_W3_MUST_NOT_INHERIT", "artificial-host-only")
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let ready = tokio::time::timeout(Duration::from_secs(25), async {
        loop {
            if let Some(ready) = fs::read(root.join("host-ready.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<HostReady>(&bytes).ok())
            {
                break ready;
            }
            assert!(
                host.0.try_wait().unwrap().is_none(),
                "host exited before readiness: {}",
                fs::read_to_string(root.join("host-fixture.log")).unwrap()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("real NodeApps host must become ready");
    assert_eq!(ready.processes.len(), 3);
    assert!(ready
        .processes
        .iter()
        .all(|identity| identity.creation_100ns > 0));
    for identity in &ready.processes {
        assert!(matches!(
            native_process::observation(&identity.native()).unwrap(),
            native_process::ProcessObservation::Matching
        ));
    }
    let data = root.join("disposable-platform-data 中文 space");
    let slot = data.join("node-apps").join(&ready.id);
    let reader = fs::File::open(slot.join("active.json")).unwrap();
    host.0.kill().unwrap(); // Uses the original retained std::process::Child HANDLE.
    assert!(!host.0.wait().unwrap().success());
    stopped(&ready.processes).await;
    let manager = NodeApps::new(data, node).unwrap();
    assert_eq!(
        manager.reconcile_run_markers().await.unwrap()["unconfirmed"],
        1
    );
    assert_eq!(manager.status(&ready.id).await.unwrap()["state"], "failed");
    assert!(manager.stop(&ready.id).await.is_err());
    drop(reader);
    let stopped = manager.stop(&ready.id).await.unwrap();
    assert_eq!(stopped["state"], "stopped");
    assert_eq!(stopped["configuration"]["canStart"], true);
    absent(&slot.join("active.json"));
    no_cleanup(&slot);
    assert_eq!(
        manager.reconcile_run_markers().await.unwrap()["unconfirmed"],
        0
    );
    assert!(manager.owned_process_ids().await.is_empty());
    assert_eq!(
        fs::read_to_string(slot.join("data/launch-count")).unwrap(),
        "1"
    );
    assert_eq!(
        fs::read_to_string(root.join("external-original-source 中文 space/server/backend.cjs"))
            .unwrap(),
        BACKEND
    );
}
