//! Real loopback lifecycle tests; all author content and private slots are disposable.
use super::*;
use std::fs;
async fn fixture(
    name: &str,
    backend: bool,
) -> (
    tempfile::TempDir,
    RuntimePaths,
    RuntimeService,
    String,
    PathBuf,
) {
    let (temp, paths) = RuntimePaths::for_test();
    let service = RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
        .await
        .unwrap();
    let source = temp.path().canonicalize().unwrap().join(name);
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("index.html"),
        "<!doctype html><html><body>Disposable application</body></html>",
    )
    .unwrap();
    let mut manifest = json!({"manifestVersion":2,"app":{"name":name},"web":{"root":".","entry":"index.html"},"platform":{"sdkApiVersion":1}});
    if backend {
        fs::create_dir(source.join("backend")).unwrap();
        fs::write(
            source.join("backend/server.mjs"),
            "throw Error('must not execute without installed resources');",
        )
        .unwrap();
        manifest["backend"] =
            json!({"protocolVersion":1,"root":"backend","entry":"server.mjs","nodeMajor":24});
    }
    fs::write(
        source.join("panda.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    let app = service.store().register_local(&source).await.unwrap();
    (temp, paths, service, app.id, source)
}
fn change(source: &std::path::Path, mutate: impl FnOnce(&mut Value)) {
    let path = source.join("panda.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    mutate(&mut value);
    fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
}
#[tokio::test]
async fn reload_opens_closed_app_and_preserves_origin_data_and_identity_on_next_reload() {
    let (_temp, _paths, service, id, source) = fixture("reload-static", false).await;
    let handle = service.handle();
    let first = handle.reload_app(&id).await.unwrap();
    assert_eq!(first["state"], "opened");
    let origin = first["launch"]["url"].clone();
    let epoch = first["launch"]["documentEpoch"].clone();
    fs::create_dir(source.join("data")).unwrap();
    fs::write(source.join("data/keep.txt"), "user data").unwrap();
    let second = handle.reload_app(&id).await.unwrap();
    assert_eq!(second["state"], "reloaded");
    assert_eq!(second["launch"]["url"], origin);
    assert_ne!(second["launch"]["documentEpoch"], epoch);
    assert_eq!(
        fs::read_to_string(source.join("data/keep.txt")).unwrap(),
        "user data"
    );
    assert_eq!(handle.store().list().await.unwrap().len(), 1);
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn invalid_manifest_and_missing_configuration_leave_old_running_epoch_and_registration() {
    let (_temp, paths, service, id, source) = fixture("reload-preflight", false).await;
    let handle = service.handle();
    let first = handle.reload_app(&id).await.unwrap();
    let epoch = first["launch"]["documentEpoch"].clone();
    let registration = paths
        .data_root()
        .join("apps")
        .join(&id)
        .join("registration.json");
    let before = fs::read(&registration).unwrap();
    change(&source, |value| value["manifestVersion"] = json!(1));
    let invalid = handle.reload_app(&id).await.unwrap();
    assert_eq!(invalid["state"], "invalidManifest");
    assert!(
        handle.app_logs(&id).is_ok(),
        "logs remain readable while the author manifest is broken"
    );
    assert_eq!(handle.web_status(&id)["state"], "running");
    assert_eq!(fs::read(&registration).unwrap(), before);
    change(&source, |value| {
        value["manifestVersion"] = json!(2);
        value["configuration"] = json!({"version":1,"fields":[{"name":"REQUIRED_VALUE","type":"string","label":"Required","required":true,"secret":false,"description":"Disposable required configuration","howToGet":["Enter a value in this disposable test."]}]});
    });
    let missing = handle.reload_app(&id).await.unwrap();
    assert_eq!(missing["state"], "needsConfiguration");
    assert_eq!(handle.web_status(&id)["state"], "running");
    assert_eq!(
        handle.store().project(&id).unwrap().document_epoch,
        epoch.as_str().unwrap()
    );
    assert_eq!(fs::read(&registration).unwrap(), before);
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn startup_failure_keeps_accepted_manifest_and_stored_secrets() {
    let (_temp, paths, service, id, source) = fixture("reload-start-failure", false).await;
    let handle = service.handle();
    handle.reload_app(&id).await.unwrap();
    let slot = paths.data_root().join("apps").join(&id);
    let before = fs::read(slot.join("registration.json")).unwrap();
    fs::create_dir(source.join("backend")).unwrap();
    fs::write(
        source.join("backend/server.mjs"),
        "throw Error('must not run');",
    )
    .unwrap();
    change(
        &source,
        |value| {
            value["backend"] =
                json!({"protocolVersion":1,"root":"backend","entry":"server.mjs","nodeMajor":24})
        },
    );
    let error = handle.reload_app(&id).await.unwrap_err();
    assert_eq!(error.code, "INSTALLATION_RESOURCES_MISSING");
    assert_eq!(fs::read(slot.join("registration.json")).unwrap(), before);
    assert_eq!(handle.web_status(&id)["state"], "failed");
    assert!(!slot.join("active.json").exists());
    assert_eq!(
        error.details["logPath"],
        json!(slot.join("logs/current.log"))
    );
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn removing_backend_clears_current_and_replaced_secret_files_only_after_success() {
    let (_temp, paths, service, id, source) = fixture("reload-remove-secrets", true).await;
    let handle = service.handle();
    change(
        &source,
        |value| value["configuration"] = json!({"version":1,"fields":[{"name":"API_KEY","type":"string","label":"Key","required":true,"secret":true,"description":"Artificial test key","howToGet":["Artificial fixture only."]}]}),
    );
    let config = handle.web_configuration(&id).await.unwrap();
    let first=handle.save_web_configuration(&id,json!({"revision":0,"manifestDigest":config["manifestDigest"],"values":{},"secrets":{"API_KEY":{"action":"replace","value":"artificial-first-secret"}}})).await.unwrap();
    handle.save_web_configuration(&id,json!({"revision":first["configuration"]["revision"],"manifestDigest":config["manifestDigest"],"values":{},"secrets":{"API_KEY":{"action":"replace","value":"artificial-second-secret"}}})).await.unwrap();
    let vault = paths.data_root().join("apps").join(&id).join("secrets");
    assert_eq!(fs::read_dir(&vault).unwrap().count(), 2);
    change(&source, |value| {
        value.as_object_mut().unwrap().remove("backend");
        value.as_object_mut().unwrap().remove("configuration");
        value["web"]["entry"] = json!("missing-entry.html");
    });
    let registration = paths
        .data_root()
        .join("apps")
        .join(&id)
        .join("registration.json");
    let before = fs::read(&registration).unwrap();
    assert!(handle.reload_app(&id).await.is_err());
    assert_eq!(fs::read(&registration).unwrap(), before);
    assert_eq!(fs::read_dir(&vault).unwrap().count(), 2);
    change(&source, |value| value["web"]["entry"] = json!("index.html"));
    let result = handle.reload_app(&id).await.unwrap();
    assert_eq!(result["state"], "opened");
    assert_eq!(result["removedSecrets"], json!(["API_KEY"]));
    assert_eq!(fs::read_dir(&vault).unwrap().count(), 0);
    assert!(!handle.store().project(&id).unwrap().manifest.has_backend());
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn successful_reload_rejects_old_browser_session_on_the_same_origin() {
    let (_temp, _paths, service, id, _source) = fixture("reload-session-epoch", false).await;
    let handle = service.handle();
    let first = handle.reload_app(&id).await.unwrap();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let response = client
        .get(first["launch"]["launchUrl"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let url = first["launch"]["url"].as_str().unwrap();
    assert_eq!(
        client
            .get(url)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    let next = handle.reload_app(&id).await.unwrap();
    assert_eq!(next["launch"]["url"], first["launch"]["url"]);
    assert_eq!(
        client
            .get(url)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn reservation_identity_is_required_and_expiry_only_cleans_empty_owned_roots() {
    let (_temp, paths, service, _id, _source) = fixture("reservation-fixture", false).await;
    let store = service.store();
    let empty = store.reserve_app(Some("empty reserved")).unwrap();
    let retained = store.reserve_app(Some("nonempty reserved")).unwrap();
    let empty_id = empty["reservation"].as_str().unwrap();
    let retained_id = retained["reservation"].as_str().unwrap();
    let empty_root = PathBuf::from(empty["path"].as_str().unwrap());
    let retained_root = PathBuf::from(retained["path"].as_str().unwrap());
    fs::write(retained_root.join("preserve.txt"), "author work").unwrap();
    for id in [empty_id, retained_id] {
        let marker = paths
            .data_root()
            .join("apps")
            .join(id)
            .join("ai-reservation.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
        value["createdAt"] = json!(0);
        fs::write(marker, serde_json::to_vec(&value).unwrap()).unwrap();
    }
    let preserved = store.cleanup_expired_app_reservations().unwrap();
    assert!(!empty_root.exists());
    assert_eq!(
        fs::read_to_string(retained_root.join("preserve.txt")).unwrap(),
        "author work"
    );
    assert!(preserved
        .iter()
        .any(|item| item["reservation"] == retained_id));
    let held = retained_root.with_extension("original");
    fs::rename(&retained_root, &held).unwrap();
    fs::create_dir(&retained_root).unwrap();
    assert!(store.add_reserved_app(retained_id).await.is_err());
    assert!(held.join("preserve.txt").exists());
    assert!(retained_root.exists());
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn management_mutation_waits_for_native_confirmation_and_decline_does_not_execute() {
    let (_temp, _paths, service, _id, _source) = fixture("management-confirmation", false).await;
    let handle = service.handle();
    let mut requests = handle.subscribe_management_requests();
    let request_handle = handle.clone();
    let pending = tokio::spawn(async move {
        request_handle
            .confirm_management(
                "Delete disposable application",
                "Artificial impact only".into(),
            )
            .await
    });
    let event = requests.recv().await.unwrap();
    assert!(event.expires_at >= millis() + 119_000);
    assert!(!pending.is_finished());
    assert!(handle
        .resolve_management_request("made-up-request", true)
        .is_err());
    handle
        .resolve_management_request(&event.request_id, false)
        .unwrap();
    assert_eq!(
        pending.await.unwrap().unwrap().unwrap()["state"],
        "declined"
    );
    assert!(handle
        .resolve_management_request(&event.request_id, true)
        .is_err());
    service.shutdown().await.unwrap();
}

async fn browser_cookie(client: &reqwest::Client, launch: &Value) -> String {
    let response = client
        .get(launch["launchUrl"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}
async fn browser_sql(client: &reqwest::Client, launch: &Value, cookie: &str, sql: &str) -> Value {
    let origin = launch["url"].as_str().unwrap().trim_end_matches('/');
    let value: Value = client
        .post(format!("{origin}/_platform/api/v1/db/execute"))
        .header("cookie", cookie)
        .header("origin", origin)
        .header(
            "x-platform-document-epoch",
            launch["documentEpoch"].as_str().unwrap(),
        )
        .json(&json!({"sql":sql}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(value["ok"], true, "{value}");
    value
}
#[tokio::test]
#[ignore = "Requires explicit read-only installed resources and bundled CLI paths"]
async fn actual_backend_and_sdk_share_one_origin_and_database_across_both_conversions() {
    let resources = PathBuf::from(
        std::env::var_os("PANDA_UNIFIED_TEST_RESOURCES").expect("explicit installed resources"),
    );
    let cli =
        PathBuf::from(std::env::var_os("PANDA_UNIFIED_TEST_CLI").expect("explicit bundled CLI"));
    let (temp, paths) = RuntimePaths::for_test();
    let service = RuntimeService::start_with_resources(
        paths.clone(),
        paths.acquire_lock().unwrap(),
        resources,
        cli,
    )
    .await
    .unwrap();
    let source = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-real-backend-sdk");
    fs::create_dir(&source).unwrap();
    fs::write(
        source.join("index.html"),
        "<!doctype html><html><body>Disposable backend + SDK</body></html>",
    )
    .unwrap();
    let public = json!({"name":"MODEL","type":"string","label":"Model","required":false,"secret":false,"description":"Disposable model selection","howToGet":["Artificial fixture only."],"default":"default-model"});
    fs::write(source.join("panda.json"),serde_json::to_vec_pretty(&json!({"manifestVersion":2,"app":{"name":"Disposable backend SDK conversion"},"web":{"root":".","entry":"index.html"},"platform":{"sdkApiVersion":1},"configuration":{"version":1,"fields":[public]}})).unwrap()).unwrap();
    let id = service.store().register_local(&source).await.unwrap().id;
    let handle = service.handle();
    let config = handle.web_configuration(&id).await.unwrap();
    handle.save_web_configuration(&id,json!({"revision":0,"manifestDigest":config["manifestDigest"],"values":{"MODEL":"persisted-model"},"secrets":{}})).await.unwrap();
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let first = handle.reload_app(&id).await.unwrap();
    let cookie = browser_cookie(&client, &first["launch"]).await;
    browser_sql(
        &client,
        &first["launch"],
        &cookie,
        "CREATE TABLE shared_notes(id INTEGER PRIMARY KEY,value TEXT NOT NULL)",
    )
    .await;
    browser_sql(
        &client,
        &first["launch"],
        &cookie,
        "INSERT INTO shared_notes(value) VALUES ('before-backend')",
    )
    .await;
    fs::write(
        source.join("data/preserve.txt"),
        "author data survives conversion",
    )
    .unwrap();
    fs::create_dir(source.join("backend")).unwrap();
    fs::write(source.join("backend/server.mjs"),r#"import http from 'node:http';
import readline from 'node:readline';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
let server,db,token,instanceId;
const emit=value=>process.stdout.write(JSON.stringify(value)+'\n');
const input=readline.createInterface({input:process.stdin,crlfDelay:Infinity});
input.on('line',line=>{
 const message=JSON.parse(line);
 if(message.type==='initialize'){
  ({token,instanceId}=message);
  db=new DatabaseSync(path.join(process.env.APP_DATA_DIR,'main.sqlite'));
  db.exec('PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;');
  server=http.createServer(async(req,res)=>{
   if(req.headers.authorization!==`Bearer ${token}`){res.writeHead(401);res.end();return;}
   res.setHeader('content-type','application/json');
   if(req.url==='/_platform/health'){res.end(JSON.stringify({protocolVersion:1,instanceId,status:'ok'}));return;}
   if(req.url==='/api/exit'&&req.method==='POST'){res.end(JSON.stringify({exiting:true}));setTimeout(()=>process.exit(9),50);return;}
   if(req.url==='/api/notes'&&req.method==='GET'){
    res.end(JSON.stringify({notes:db.prepare('SELECT value FROM shared_notes ORDER BY id').all(),model:process.env.MODEL,hasSecret:Boolean(process.env.API_KEY)}));return;
   }
   if(req.url==='/api/write'&&req.method==='POST'){
    let body='';for await(const chunk of req)body+=chunk;const value=JSON.parse(body).value;
    db.prepare('INSERT INTO shared_notes(value) VALUES (?)').run(value);res.end(JSON.stringify({saved:true}));return;
   }
   res.writeHead(404);res.end('{}');
  });
  server.listen(0,'127.0.0.1',()=>emit({protocolVersion:1,type:'ready',instanceId,host:'127.0.0.1',port:server.address().port}));
 } else if(message.type==='ping')emit({protocolVersion:1,type:'pong',instanceId,requestId:message.requestId});
 else if(message.type==='shutdown'){
  emit({protocolVersion:1,type:'stopping',instanceId});
  server.close(()=>{db.close();input.close();process.exit(0);});
 }
});
"#).unwrap();
    change(&source, |value| {
        value["backend"] = json!({"protocolVersion":1,"root":"backend","entry":"server.mjs","nodeMajor":24,"apiPrefixes":["/api"]});
        value["configuration"]["fields"].as_array_mut().unwrap().push(json!({"name":"API_KEY","type":"string","label":"Key","required":true,"secret":true,"description":"Artificial test secret","howToGet":["Artificial fixture only."]}));
    });
    let config = handle.web_configuration(&id).await.unwrap();
    handle.save_web_configuration(&id,json!({"revision":config["configuration"]["revision"],"manifestDigest":config["manifestDigest"],"values":{"MODEL":"persisted-model"},"secrets":{"API_KEY":{"action":"replace","value":"artificial-conversion-secret"}}})).await.unwrap();
    let second = handle.reload_app(&id).await.unwrap();
    assert_eq!(second["state"], "reloaded");
    assert_eq!(second["launch"]["url"], first["launch"]["url"]);
    let cookie = browser_cookie(&client, &second["launch"]).await;
    let origin = second["launch"]["url"]
        .as_str()
        .unwrap()
        .trim_end_matches('/');
    let notes: Value = client
        .get(format!("{origin}/api/notes"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(notes["notes"][0]["value"], "before-backend");
    assert_eq!(notes["model"], "persisted-model");
    assert_eq!(notes["hasSecret"], true);
    browser_sql(
        &client,
        &second["launch"],
        &cookie,
        "INSERT INTO shared_notes(value) VALUES ('sdk-with-backend')",
    )
    .await;
    let saved: Value = client
        .post(format!("{origin}/api/write"))
        .header("cookie", &cookie)
        .header("origin", origin)
        .json(&json!({"value":"backend-with-sdk"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(saved["saved"], true);
    let count = browser_sql(
        &client,
        &second["launch"],
        &cookie,
        "SELECT COUNT(*) AS count FROM shared_notes",
    )
    .await;
    assert_eq!(count["data"]["rows"][0]["count"], 3);
    assert_eq!(
        client
            .get(format!("{origin}/_platform/sdk.js"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .status(),
        reqwest::StatusCode::OK
    );
    let exposed: Value = client
        .get(format!("{origin}/_platform/config.json"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(exposed["MODEL"], "persisted-model");
    assert!(exposed.get("API_KEY").is_none());
    let slot = paths.data_root().join("apps").join(&id);
    assert!(slot.join("active.json").exists());
    assert!(!handle
        .app_logs(&id)
        .unwrap()
        .to_string()
        .contains("artificial-conversion-secret"));
    let exiting: Value = client
        .post(format!("{origin}/api/exit"))
        .header("cookie", &cookie)
        .header("origin", origin)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(exiting["exiting"], true);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = handle.web_status(&id);
            if status["state"] == "failed" && status["resourceClosed"] == true {
                assert_eq!(status["error"]["code"], "APP_BACKEND_EXITED");
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let restarted = handle.reload_app(&id).await.unwrap();
    assert_eq!(restarted["state"], "opened");
    assert_eq!(restarted["launch"]["url"], first["launch"]["url"]);
    let resumed_cookie = browser_cookie(&client, &restarted["launch"]).await;
    let count = browser_sql(
        &client,
        &restarted["launch"],
        &resumed_cookie,
        "SELECT COUNT(*) AS count FROM shared_notes",
    )
    .await;
    assert_eq!(count["data"]["rows"][0]["count"], 3);
    change(&source, |value| {
        value.as_object_mut().unwrap().remove("backend");
        value["configuration"]["fields"]
            .as_array_mut()
            .unwrap()
            .retain(|field| field["secret"] != true);
    });
    let third = handle.reload_app(&id).await.unwrap();
    assert_eq!(third["state"], "reloaded");
    assert_eq!(third["launch"]["url"], first["launch"]["url"]);
    assert_eq!(third["removedSecrets"], json!(["API_KEY"]));
    let cookie = browser_cookie(&client, &third["launch"]).await;
    let count = browser_sql(
        &client,
        &third["launch"],
        &cookie,
        "SELECT COUNT(*) AS count FROM shared_notes",
    )
    .await;
    assert_eq!(count["data"]["rows"][0]["count"], 3);
    assert_eq!(
        fs::read_to_string(source.join("data/preserve.txt")).unwrap(),
        "author data survives conversion"
    );
    assert!(!slot.join("active.json").exists());
    assert_eq!(fs::read_dir(slot.join("secrets")).unwrap().count(), 0);
    assert_eq!(
        handle.web_configuration(&id).await.unwrap()["configuration"]["values"]["MODEL"],
        "persisted-model"
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn secret_cleanup_failure_can_retry_after_accepted_backend_flag_is_already_false() {
    let (_temp, paths, service, id, source) = fixture("retry-secret-retirement", true).await;
    let handle = service.handle();
    change(
        &source,
        |value| value["configuration"] = json!({"version":1,"fields":[{"name":"API_KEY","type":"string","label":"Key","required":true,"secret":true,"description":"Artificial test secret","howToGet":["Artificial fixture only."]}]}),
    );
    let config = handle.web_configuration(&id).await.unwrap();
    handle.save_web_configuration(&id,json!({"revision":0,"manifestDigest":config["manifestDigest"],"values":{},"secrets":{"API_KEY":{"action":"replace","value":"artificial-retry-secret"}}})).await.unwrap();
    let vault = paths.data_root().join("apps").join(&id).join("secrets");
    let unrecognized = vault.join("disposable-unrecognized-entry");
    fs::write(&unrecognized, "preserve unknown material").unwrap();
    change(&source, |value| {
        value.as_object_mut().unwrap().remove("backend");
        value.as_object_mut().unwrap().remove("configuration");
    });
    let failed = handle.reload_app(&id).await.unwrap_err();
    assert_eq!(failed.code, "APP_SECRET_CLEANUP_FAILED");
    assert!(!handle.store().accepted_has_backend(&id).unwrap());
    assert_eq!(handle.web_status(&id)["state"], "failed");
    assert_eq!(handle.web_status(&id)["resourceClosed"], true);
    assert_eq!(
        fs::read_to_string(&unrecognized).unwrap(),
        "preserve unknown material"
    );
    fs::remove_file(unrecognized).unwrap();
    let retried = handle.reload_app(&id).await.unwrap();
    assert_eq!(retried["removedSecrets"], json!(["API_KEY"]));
    assert_eq!(fs::read_dir(&vault).unwrap().count(), 0);
    let repeated = handle.reload_app(&id).await.unwrap();
    assert_eq!(repeated["removedSecrets"], json!([]));
    assert_eq!(repeated["message"], "");
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn native_ready_failure_stops_new_service_and_preserves_a_retryable_failed_state() {
    let (_temp, _paths, service, id, _source) = fixture("native-ready-rejected", false).await;
    let handle = service.handle();
    handle.set_app_reload_hook(Arc::new(|notice| {
        Box::pin(async move {
            if notice["phase"] == "ready" {
                Err(ApiError::new(
                    "DISPOSABLE_NATIVE_READY_REJECTED",
                    "Disposable window closed before navigation",
                ))
            } else {
                Ok(())
            }
        })
    }));
    let error = handle.reload_app(&id).await.unwrap_err();
    assert_eq!(error.code, "DISPOSABLE_NATIVE_READY_REJECTED");
    assert_eq!(handle.web_status(&id)["state"], "failed");
    assert_eq!(handle.web_status(&id)["resourceClosed"], true);
    let address = handle.store().project(&id).unwrap().address.origin();
    assert!(reqwest::get(&address).await.is_err());
    handle.set_app_reload_hook(Arc::new(|_| Box::pin(async { Ok(()) })));
    assert_eq!(handle.reload_app(&id).await.unwrap()["state"], "opened");
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn initialize_empty_user_folder_returns_a_registered_sdk_application() {
    let (temp, _paths, service, _id, _source) = fixture("initialize-user-folder", false).await;
    let handle = service.handle();
    let source = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-user-starter");
    fs::create_dir(&source).unwrap();
    let inspected = handle
        .check_directory(source.display().to_string())
        .await
        .unwrap();
    assert_eq!(inspected["state"], "empty");
    let created=handle.initialize_directory(source.display().to_string(),json!({"expectedRevision":inspected["expectedRevision"],"name":"Disposable initialized user app"})).await.unwrap();
    assert!(created["id"]
        .as_str()
        .is_some_and(|id| Uuid::parse_str(id).is_ok()));
    assert_eq!(created["ownership"], "user");
    assert_eq!(created["hasBackend"], false);
    assert_eq!(created["manifest"]["platform"]["sdkApiVersion"], 1);
    assert_eq!(created["projectRoot"], json!(source));
    assert_eq!(handle.store().list().await.unwrap().len(), 2);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn expired_confirmation_cannot_execute_the_real_http_delete_request() {
    let (_temp, paths, service, id, source) = fixture("expired-confirmed-http-delete", false).await;
    let handle = service.handle();
    let mut confirmations = handle.subscribe_management_requests();
    let epoch = handle.store().project(&id).unwrap().document_epoch;
    let endpoint = format!("{}/api/v1/apps/{id}/deletions", service.info().base_url);
    let token = paths.read_control_token().unwrap();
    let response = tokio::spawn(async move {
        reqwest::Client::new()
            .post(endpoint)
            .bearer_auth(token)
            .json(&json!({"expectedDocumentEpoch":epoch}))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    });
    let request = confirmations.recv().await.unwrap();
    assert!(!response.is_finished());
    handle
        .shared
        .management_pending
        .lock()
        .unwrap()
        .get_mut(&request.request_id)
        .unwrap()
        .expires = Instant::now();
    assert_eq!(
        handle
            .resolve_management_request(&request.request_id, true)
            .unwrap_err()
            .code,
        "MANAGEMENT_CONFIRMATION_EXPIRED"
    );
    let result = response.await.unwrap();
    assert_eq!(result["data"]["state"], "timeout", "{result}");
    assert!(handle.store().project(&id).is_ok());
    assert!(source.join("index.html").is_file());
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn approved_http_delete_uses_the_single_deletion_flow_and_preserves_user_source() {
    let (_temp, paths, service, id, source) =
        fixture("approved-confirmed-http-delete", false).await;
    let handle = service.handle();
    let mut confirmations = handle.subscribe_management_requests();
    let epoch = handle.store().project(&id).unwrap().document_epoch;
    fs::create_dir(source.join("data")).unwrap();
    fs::write(
        source.join("data/preserve.txt"),
        "user-owned database and files survive",
    )
    .unwrap();
    let endpoint = format!("{}/api/v1/apps/{id}/deletions", service.info().base_url);
    let token = paths.read_control_token().unwrap();
    let response = tokio::spawn(async move {
        reqwest::Client::new()
            .post(endpoint)
            .bearer_auth(token)
            .json(&json!({"expectedDocumentEpoch":epoch}))
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    });
    let request = confirmations.recv().await.unwrap();
    assert!(request.impact.contains("保留用户文件夹"));
    assert!(handle.store().project(&id).is_ok());
    handle
        .resolve_management_request(&request.request_id, true)
        .unwrap();
    let result = response.await.unwrap();
    assert_eq!(result["ok"], true, "{result}");
    let task_id = result["data"]["taskId"].as_str().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let task = handle
                .task_for_project(task_id, &id, "delete")
                .await
                .unwrap();
            if task["status"] == "succeeded" {
                break;
            }
            assert!(task["status"] != "failed", "{task}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        fs::read_to_string(source.join("data/preserve.txt")).unwrap(),
        "user-owned database and files survive"
    );
    assert!(source.join("index.html").is_file());
    assert!(!paths.data_root().join("apps").join(&id).exists());
    service.shutdown().await.unwrap();
}
