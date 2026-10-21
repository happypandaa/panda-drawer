//! Private disposable Node slots and the actual pinned Windows Node binary.
//! No existing application root, user credential or external service is used.
use super::*;
use crate::node_app_configuration::SecretAction;
use platform_host::{WindowsSourceDirectory, WindowsWorkspace};
use reqwest::{header, StatusCode};
use std::fs;
use std::time::Duration;

const SCRIPT: &str = r#"const fs=require('fs'),path=require('path'),http=require('http'),readline=require('readline');
let server,token,instance;
const lines=readline.createInterface({input:process.stdin});
(async()=>{for await(const line of lines){const message=JSON.parse(line);
 if(message.type==='initialize'){
  token=message.token;instance=message.instanceId;
  const marker=path.join(process.env.APP_DATA_DIR,'starts');
  const starts=fs.existsSync(marker)?Number(fs.readFileSync(marker,'utf8'))+1:1;
  fs.writeFileSync(marker,String(starts));
  server=http.createServer((req,res)=>{
   if(req.headers.authorization!==`Bearer ${token}`){res.writeHead(401);return res.end();}
   if(req.url==='/_platform/health'){
    const body=JSON.stringify({protocolVersion:1,instanceId:instance,status:'ok'});
    res.writeHead(200,{'content-type':'application/json','content-length':Buffer.byteLength(body)});return res.end(body);
   }
   if(req.url==='/api/check'){
    const body=JSON.stringify({configured:process.env.API_KEY==='artificial-private-key',label:process.env.TEST_LABEL,starts,cookieReceived:!!req.headers.cookie});
    res.writeHead(200,{'content-type':'application/json','content-length':Buffer.byteLength(body)});return res.end(body);
   }
   if(req.url==='/api/events'){res.writeHead(200,{'content-type':'text/event-stream'});res.write('data: windows-node-app\n\n');return;}
   if(req.url==='/tts/fixture.mp3'&&req.headers.range==='bytes=0-2'){
    res.writeHead(206,{'content-type':'audio/mpeg','content-range':'bytes 0-2/6','accept-ranges':'bytes','set-cookie':'backend-secret=must-not-forward'});return res.end('abc');
   }
   res.writeHead(404);res.end();
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  process.stdout.write(JSON.stringify({protocolVersion:1,type:'ready',instanceId:instance,host:'127.0.0.1',port:server.address().port})+'\n');
 }else if(message.type==='ping')process.stdout.write(JSON.stringify({protocolVersion:1,type:'pong',instanceId:instance,requestId:message.requestId})+'\n');
 else if(message.type==='shutdown'){
  process.stdout.write(JSON.stringify({protocolVersion:1,type:'stopping',instanceId:instance})+'\n');
  server.closeAllConnections();await new Promise(resolve=>server.close(resolve));lines.close();break;
 }
}})().catch(()=>process.exit(19));
"#;

struct Fixture {
    workspace: WindowsWorkspace,
    parent: WindowsSourceDirectory,
}
impl Fixture {
    fn new() -> Self {
        let artifacts = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join(".test-artifacts");
        fs::create_dir_all(&artifacts).unwrap();
        let path = artifacts.join(format!("windows-node-disposable-apps-{}", Uuid::new_v4()));
        let workspace = platform_host::disposable_node_workspace(&path).unwrap();
        let parent = WindowsSourceDirectory::open_private(&path).unwrap();
        Self { workspace, parent }
    }
    fn source(&self, name: &str) -> PathBuf {
        let source = self.parent.create_private_directory(name).unwrap();
        let web = source.create_private_directory("public").unwrap();
        web.write_new(
            "index.html",
            b"<!doctype html><title>Disposable Windows Node</title>",
        )
        .unwrap();
        source
            .create_private_directory("server")
            .unwrap()
            .write_new("managed.cjs", SCRIPT.as_bytes())
            .unwrap();
        source.write_new("local-app.json", &serde_json::to_vec(&json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":format!("dev.disposable.{}",name.replace(' ',"-")),"name":name,"version":"1"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"root":"server","entry":"managed.cjs","nodeMajor":24,"protocolVersion":1},
            "configuration":{"version":1,"fields":[
                {"name":"TEST_LABEL","label":"测试名称","type":"string","required":true,"secret":false,"description":"只用于可丢弃样本","howToGet":["填写测试名称"]},
                {"name":"API_KEY","label":"人工密钥","type":"string","required":true,"secret":true,"description":"非用户密钥","howToGet":["仅填写人工测试值"]}
            ]}
        })).unwrap()).unwrap();
        source.path().to_owned()
    }
    fn apps(&self, node: PathBuf) -> NodeApps {
        NodeApps::new_windows(self.workspace.root(), node).unwrap()
    }
}
fn configuration(revision: u64, label: &str) -> SaveConfigurationRequest {
    SaveConfigurationRequest {
        revision,
        values: BTreeMap::from([("TEST_LABEL".into(), json!(label))]),
        secrets: BTreeMap::from([(
            "API_KEY".into(),
            SecretAction::Replace {
                value: "artificial-private-key".into(),
            },
        )]),
    }
}

#[tokio::test]
async fn source_aliases_reuse_exact_slot_and_saved_configuration_by_windows_identity() {
    let fixture = Fixture::new();
    let source = fixture.source("SourceOne");
    let apps = fixture.apps(PathBuf::from("not-executed.exe"));
    let first = apps.register(source.clone()).await.unwrap();
    let id = first["id"].as_str().unwrap();
    apps.save(id, configuration(0, "retained")).await.unwrap();
    for alias in [
        source.to_string_lossy().replace('\\', "/"),
        source.to_string_lossy().to_lowercase(),
        source
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    ] {
        let again = apps.register(PathBuf::from(alias)).await.unwrap();
        assert_eq!(again["id"], first["id"]);
        assert_eq!(again["configuration"]["revision"], 1);
        assert_eq!(again["configuration"]["values"]["TEST_LABEL"], "retained");
    }
    assert_eq!(apps.list().await.unwrap().len(), 1);
    assert!(!fixture
        .workspace
        .path()
        .join("node-apps")
        .join(id)
        .join("active.json")
        .exists());
}

#[tokio::test]
async fn missing_credentials_and_unknown_slots_are_retained_without_recreation_or_adoption() {
    let fixture = Fixture::new();
    let apps = fixture.apps(PathBuf::from("not-executed.exe"));
    assert!(apps.list().await.unwrap().is_empty());
    assert!(!fixture.workspace.path().join("node-apps").exists());
    let app = apps.register(fixture.source("SourceTwo")).await.unwrap();
    let id = app["id"].as_str().unwrap();
    apps.save(id, configuration(0, "retained")).await.unwrap();
    let credentials = fixture
        .workspace
        .path()
        .join("node-apps")
        .join(id)
        .join("configuration/credentials");
    let retained = credentials.with_file_name("credentials-retained-evidence");
    fs::rename(&credentials, &retained).unwrap();
    let files = fs::read_dir(&retained).unwrap().count();
    assert!(apps.list().await.is_err());
    assert!(apps.configuration(id).await.is_err());
    assert!(!credentials.exists());
    assert_eq!(fs::read_dir(&retained).unwrap().count(), files);
    let second = Fixture::new();
    let other = second.apps(PathBuf::from("not-executed.exe"));
    let root =
        platform_host::windows_node_root::node_apps_directory(&second.workspace.root(), true)
            .unwrap()
            .unwrap();
    root.create_directory("unknown-preserved-directory")
        .unwrap();
    assert!(other.list().await.is_err());
    assert!(root.path().join("unknown-preserved-directory").is_dir());
}

fn official_node() -> PathBuf {
    let path = std::env::var_os("NODE_RUNTIME_TRIAL_TEST_NODE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.test-artifacts/node-official-win24/node-v24.22.0-win-x64/node.exe")
        });
    assert_eq!(
        format!(
            "{:x}",
            Sha256::digest(
                fs::read(&path).expect("prepare verified official Windows Node fixture")
            )
        ),
        "ba93f9a70bd86fce3ebbb39ec758501458453467aa92a34a0a3602d1e3fc2b9c"
    );
    path.canonicalize().unwrap()
}
async fn connect(apps: &NodeApps, id: &str, client: &reqwest::Client) -> (String, String) {
    let url = apps.open_url(id).await.unwrap();
    let response = client.get(&url).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let origin = reqwest::Url::parse(&url)
        .unwrap()
        .origin()
        .ascii_serialization();
    (origin, cookie)
}

#[tokio::test]
#[ignore = "requires independently verified official private Windows Node fixture"]
async fn official_node_registration_private_configuration_proxy_streams_sessions_and_shutdown() {
    let fixture = Fixture::new();
    let apps = fixture.apps(official_node());
    let first = apps.register(fixture.source("RadioFirst")).await.unwrap();
    let second = apps.register(fixture.source("RadioSecond")).await.unwrap();
    let id = first["id"].as_str().unwrap();
    let other = second["id"].as_str().unwrap();
    assert!(apps.start(id).await.is_err());
    assert!(apps.owned_process_ids().await.is_empty());
    for (id, label) in [(id, "first"), (other, "second")] {
        let saved = apps.save(id, configuration(0, label)).await.unwrap();
        assert_eq!(saved["state"], "stopped");
        assert!(!serde_json::to_string(&saved)
            .unwrap()
            .contains("artificial-private-key"));
        let running = apps.start(id).await.unwrap();
        assert_eq!(running["state"], "running");
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let (origin, cookie) = connect(&apps, id, &client).await;
    let (second_origin, second_cookie) = connect(&apps, other, &client).await;
    let response = client
        .get(format!("{origin}/"))
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response
        .text()
        .await
        .unwrap()
        .contains("Disposable Windows Node"));
    let check: Value = client
        .get(format!("{origin}/api/check"))
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        check,
        json!({"configured":true,"label":"first","starts":1,"cookieReceived":false})
    );
    let saved = apps
        .save(id, configuration(1, "first updated"))
        .await
        .unwrap();
    assert_eq!(saved["state"], "running");
    assert_eq!(saved["configuration"]["revision"], 2);
    assert_eq!(saved["runningRevision"], 1);
    assert_eq!(saved["restartRequired"], true);
    let unchanged: Value = client
        .get(format!("{origin}/api/check"))
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(unchanged["label"], "first");
    assert_eq!(
        unchanged["starts"], 1,
        "saving configuration must not restart the application"
    );
    let old_cookie = cookie;
    assert_eq!(apps.restart(id).await.unwrap()["runningRevision"], 2);
    let (origin, cookie) = connect(&apps, id, &client).await;
    let updated: Value = client
        .get(format!("{origin}/api/check"))
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["label"], "first updated");
    assert_eq!(updated["starts"], 2);
    assert_eq!(
        client
            .get(format!("{origin}/api/check"))
            .header(header::COOKIE, &old_cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client
            .get(format!("{second_origin}/api/check"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let second_check: Value = client
        .get(format!("{second_origin}/api/check"))
        .header(header::COOKIE, &second_cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(second_check["label"], "second");
    let range = client
        .get(format!("{origin}/tts/fixture.mp3"))
        .header(header::COOKIE, &cookie)
        .header(header::RANGE, "bytes=0-2")
        .send()
        .await
        .unwrap();
    assert_eq!(range.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(range.headers()[header::CONTENT_RANGE], "bytes 0-2/6");
    assert!(!range.headers().contains_key(header::SET_COOKIE));
    assert_eq!(range.text().await.unwrap(), "abc");
    let mut events = client
        .get(format!("{origin}/api/events"))
        .header(header::COOKIE, &cookie)
        .send()
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(3), events.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(event.as_ref(), b"data: windows-node-app\n\n");
    apps.stop(id).await.unwrap();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), events.chunk())
            .await
            .unwrap(),
        Ok(None) | Err(_)
    ));
    assert!(
        tokio::net::TcpStream::connect(origin.trim_start_matches("http://"))
            .await
            .is_err()
    );
    assert_eq!(apps.status(other).await.unwrap()["state"], "running");
    assert!(!fixture
        .workspace
        .path()
        .join("node-apps")
        .join(id)
        .join("active.json")
        .exists());
    apps.stop_all().await.unwrap();
    assert!(apps.owned_process_ids().await.is_empty());
    assert!(
        tokio::net::TcpStream::connect(second_origin.trim_start_matches("http://"))
            .await
            .is_err()
    );
    let reopened = fixture.apps(official_node());
    assert_eq!(
        reopened.configuration(id).await.unwrap()["configuration"]["revision"],
        2
    );
    assert_eq!(reopened.start(id).await.unwrap()["state"], "running");
    let (new_origin, new_cookie) = connect(&reopened, id, &client).await;
    let retained: Value = client
        .get(format!("{new_origin}/api/check"))
        .header(header::COOKIE, &new_cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(retained["starts"], 3);
    assert_eq!(retained["label"], "first updated");
    reopened.stop_all().await.unwrap();
}
