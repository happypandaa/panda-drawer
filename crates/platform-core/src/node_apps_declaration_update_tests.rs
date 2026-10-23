//! Declaration re-trust only uses disposable external registrations. Artificial
//! credential contents are never sent to any upstream or used to start code.
use super::*;
use crate::node_app_configuration::SecretAction;

struct Fixture {
    _temporary: tempfile::TempDir,
    source: PathBuf,
    data: PathBuf,
    manager: NodeApps,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("disposable-declaration-source");
        let data = root.join("disposable-platform-data");
        fs::create_dir_all(source.join("public")).unwrap();
        fs::create_dir_all(source.join("server")).unwrap();
        fs::create_dir(&data).unwrap();
        fs::write(
            source.join("public/index.html"),
            "disposable declaration fixture",
        )
        .unwrap();
        fs::write(
            source.join("server/main.mjs"),
            "throw Error('must not run during declaration updates');",
        )
        .unwrap();
        let manager = NodeApps::new(data.clone(), root.join("nonexistent-node")).unwrap();
        let fixture = Self {
            _temporary: temporary,
            source,
            data,
            manager,
        };
        fixture.write_manifest(&json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"disposable.declaration","name":"可丢弃声明更新","version":"1.0.0"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"protocolVersion":1,"root":"server","entry":"main.mjs","nodeMajor":24},
            "configuration":{"version":1,"fields":[
                {"name":"BASE_URL","label":"人工服务地址","type":"url","required":true,"secret":false,
                    "description":"可丢弃测试地址","howToGet":["仅供测试"],"default":"https://old.example.invalid"},
                {"name":"LABEL","label":"人工标签","type":"string","required":false,"secret":false,
                    "description":"可丢弃测试标签","howToGet":["仅供测试"]},
                {"name":"API_KEY","label":"人工密钥","type":"string","required":true,"secret":true,
                    "description":"仅人工测试占位","howToGet":["仅供测试"],"endpointField":"BASE_URL"}
            ]}
        }));
        fixture
    }

    fn manifest(&self) -> Value {
        serde_json::from_slice(&fs::read(self.source.join("local-app.json")).unwrap()).unwrap()
    }

    fn write_manifest(&self, value: &Value) {
        fs::write(
            self.source.join("local-app.json"),
            serde_json::to_vec_pretty(value).unwrap(),
        )
        .unwrap();
    }

    fn extend(&self) {
        let mut manifest = self.manifest();
        manifest["app"]["version"] = json!("2.0.0");
        manifest["configuration"]["fields"].as_array_mut().unwrap().push(json!({
            "name":"TTS_PROVIDER","label":"主持语音服务","type":"select","required":false,"secret":false,
            "description":"选用自己的语音服务","howToGet":["选择已经开通的服务"],"default":"fish",
            "options":[{"value":"fish","label":"原语音"},{"value":"gemini","label":"新语音"}]
        }));
        self.write_manifest(&manifest);
    }

    fn slot(&self, id: &str) -> PathBuf {
        self.data.join("node-apps").join(id)
    }

    async fn register_and_configure(&self) -> String {
        let detail = self.manager.register(self.source.clone()).await.unwrap();
        let id = detail["id"].as_str().unwrap().to_owned();
        self.manager
            .save(
                &id,
                SaveConfigurationRequest {
                    revision: 0,
                    values: BTreeMap::from([("LABEL".into(), json!("preserved ordinary value"))]),
                    secrets: BTreeMap::from([(
                        "API_KEY".into(),
                        SecretAction::Replace {
                            value: "artificial-declaration-secret".into(),
                        },
                    )]),
                },
            )
            .await
            .unwrap();
        fs::write(
            self.slot(&id).join("data/user-content.bin"),
            b"preserve artificial business data\0\xff",
        )
        .unwrap();
        id
    }
}

fn request(preview: &Value) -> DeclarationUpdateRequest {
    serde_json::from_value(json!({
        "expectedRegisteredDigest":preview["expectedRegisteredDigest"],
        "expectedManifestDigest":preview["expectedManifestDigest"],
        "configurationRevision":preview["configurationRevision"],
        "confirmed":true
    }))
    .unwrap()
}

fn files_below(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), result);
            } else {
                result.insert(
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(path, path, &mut result);
    result
}

#[tokio::test]
async fn explicit_retrust_restores_changed_external_declaration_without_touching_data_or_credentials(
) {
    let fixture = Fixture::new();
    let id = fixture.register_and_configure().await;
    fixture.extend();
    let before = files_below(&fixture.slot(&id));
    let failed = fixture.manager.configuration(&id).await.unwrap();
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["fields"], json!([]));
    assert_eq!(failed["declarationUpdateAvailable"], true);
    assert!(fixture.manager.start(&id).await.is_err());
    let preview = fixture
        .manager
        .declaration_update_preview(&id)
        .await
        .unwrap();
    assert_eq!(preview["appId"], id);
    assert_eq!(preview["app"]["version"], "2.0.0");
    assert_eq!(preview["backend"]["entry"], "main.mjs");
    assert_eq!(preview["fields"].as_array().unwrap().len(), 4);
    assert_eq!(preview["configurationRevision"], 1);
    assert_eq!(preview["requiresRetrust"], true);
    assert!(preview["notice"]
        .as_str()
        .unwrap()
        .contains("没有可供比较的旧声明"));
    assert_eq!(preview["configuration"]["secretStatus"]["API_KEY"], "set");
    assert!(!preview
        .to_string()
        .contains("artificial-declaration-secret"));
    assert_eq!(
        files_below(&fixture.slot(&id)),
        before,
        "preview must not rewrite saved files"
    );
    let applied = fixture
        .manager
        .declaration_update_apply(&id, request(&preview))
        .await
        .unwrap();
    assert_eq!(applied["id"], id);
    assert_eq!(applied["state"], "stopped");
    assert_eq!(applied["configuration"]["canStart"], true);
    assert_eq!(applied["configuration"]["revision"], 1);
    assert_eq!(
        applied["configuration"]["values"]["LABEL"],
        "preserved ordinary value"
    );
    assert!(!applied
        .to_string()
        .contains("artificial-declaration-secret"));
    let mut after = files_below(&fixture.slot(&id));
    let registration = after.remove(Path::new("registration.json")).unwrap();
    let mut expected = before;
    expected.remove(Path::new("registration.json"));
    assert_eq!(after, expected, "only registration may change");
    let registered: Value = serde_json::from_slice(&registration).unwrap();
    assert_eq!(registered.as_object().unwrap().len(), 4);
    assert_eq!(registered["version"], 1);
    assert_eq!(
        registered["manifestDigest"],
        preview["expectedManifestDigest"]
    );
    assert!(fixture.manager.running_ids().await.is_empty());
    assert!(!fixture.slot(&id).join("active.json").exists());
    assert_eq!(
        fixture.manager.configuration(&id).await.unwrap()["state"],
        "stopped"
    );
}

#[tokio::test]
async fn retrust_requires_confirmation_and_rejects_stale_registration_manifest_or_configuration() {
    for change in ["confirmation", "registration", "manifest", "configuration"] {
        let fixture = Fixture::new();
        let id = fixture.register_and_configure().await;
        let preview = fixture
            .manager
            .declaration_update_preview(&id)
            .await
            .unwrap();
        let mut input = request(&preview);
        match change {
            "confirmation" => input.confirmed = false,
            "registration" => {
                let path = fixture.slot(&id).join("registration.json");
                let mut bytes = fs::read(&path).unwrap();
                bytes.push(b'\n');
                fs::write(path, bytes).unwrap();
            }
            "manifest" => fixture.extend(),
            "configuration" => {
                fixture
                    .manager
                    .save(
                        &id,
                        SaveConfigurationRequest {
                            revision: 1,
                            values: BTreeMap::from([(
                                "LABEL".into(),
                                json!("newer saved revision"),
                            )]),
                            secrets: BTreeMap::new(),
                        },
                    )
                    .await
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let before = files_below(&fixture.slot(&id));
        let error = fixture
            .manager
            .declaration_update_apply(&id, input)
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            if change == "confirmation" {
                "NODE_DECLARATION_UPDATE_CONFIRMATION_REQUIRED"
            } else {
                "NODE_DECLARATION_UPDATE_CHANGED"
            }
        );
        assert_eq!(files_below(&fixture.slot(&id)), before);
    }
}

#[tokio::test]
async fn retrust_preserves_endpoint_rebinding_and_new_required_configuration_requirements() {
    let fixture = Fixture::new();
    let id = fixture.register_and_configure().await;
    let mut manifest = fixture.manifest();
    manifest["configuration"]["fields"][0]["default"] = json!("https://new.example.invalid");
    manifest["configuration"]["fields"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "name":"NEW_REQUIRED","label":"新增必填","type":"string","required":true,"secret":false,
            "description":"明确要求用户补全","howToGet":["填写自己的配置"]
        }));
    fixture.write_manifest(&manifest);
    let before = fs::read(fixture.slot(&id).join("configuration/configuration.json")).unwrap();
    let preview = fixture
        .manager
        .declaration_update_preview(&id)
        .await
        .unwrap();
    assert_eq!(
        preview["configuration"]["secretStatus"]["API_KEY"],
        "endpoint-changed"
    );
    assert_eq!(preview["configuration"]["canStart"], false);
    assert!(preview["configuration"]["missingRequired"]
        .as_array()
        .unwrap()
        .contains(&json!("NEW_REQUIRED")));
    let detail = fixture
        .manager
        .declaration_update_apply(&id, request(&preview))
        .await
        .unwrap();
    assert_eq!(detail["configuration"]["canStart"], false);
    assert_eq!(
        detail["configuration"]["secretStatus"]["API_KEY"],
        "endpoint-changed"
    );
    assert_eq!(
        fs::read(fixture.slot(&id).join("configuration/configuration.json")).unwrap(),
        before
    );
}

#[tokio::test]
async fn incompatible_saved_field_types_or_secret_downgrades_are_rejected_without_changes() {
    for change in ["ordinary-type", "secret-downgrade", "removed-field"] {
        let fixture = Fixture::new();
        let id = fixture.register_and_configure().await;
        let mut manifest = fixture.manifest();
        match change {
            "ordinary-type" => manifest["configuration"]["fields"][1]["type"] = json!("integer"),
            "secret-downgrade" => {
                manifest["configuration"]["fields"][2]["secret"] = json!(false);
                manifest["configuration"]["fields"][2]
                    .as_object_mut()
                    .unwrap()
                    .remove("endpointField");
            }
            "removed-field" => {
                manifest["configuration"]["fields"]
                    .as_array_mut()
                    .unwrap()
                    .remove(1);
            }
            _ => unreachable!(),
        }
        fixture.write_manifest(&manifest);
        let before = files_below(&fixture.slot(&id));
        assert!(fixture
            .manager
            .declaration_update_preview(&id)
            .await
            .is_err());
        assert_eq!(files_below(&fixture.slot(&id)), before);
    }
}

#[tokio::test]
async fn active_markers_shutdown_and_managed_registration_reject_preview_and_apply() {
    for change in ["active", "shutdown", "managed"] {
        let fixture = Fixture::new();
        let id = fixture.register_and_configure().await;
        let preview = fixture
            .manager
            .declaration_update_preview(&id)
            .await
            .unwrap();
        match change {
            "active" => fs::write(
                fixture.slot(&id).join("active.json"),
                b"preserved artificial active evidence",
            )
            .unwrap(),
            "shutdown" => fixture.manager.inner.lock().await.shutting_down = true,
            "managed" => fs::write(
                fixture.slot(&id).join("registration.json"),
                b"{\"version\":2}",
            )
            .unwrap(),
            _ => unreachable!(),
        }
        let before = files_below(&fixture.slot(&id));
        let expected = if change == "managed" {
            "NODE_DECLARATION_UPDATE_MANAGED"
        } else {
            "NODE_DECLARATION_UPDATE_BUSY"
        };
        assert_eq!(
            fixture
                .manager
                .declaration_update_preview(&id)
                .await
                .unwrap_err()
                .code,
            expected
        );
        assert_eq!(
            fixture
                .manager
                .declaration_update_apply(&id, request(&preview))
                .await
                .unwrap_err()
                .code,
            expected
        );
        assert_eq!(files_below(&fixture.slot(&id)), before);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn linked_registration_manifest_backend_or_marker_is_never_accepted() {
    use std::os::unix::fs::symlink;
    for change in ["registration", "manifest", "backend", "active", "hardlink"] {
        let fixture = Fixture::new();
        let id = fixture.register_and_configure().await;
        let preview = fixture
            .manager
            .declaration_update_preview(&id)
            .await
            .unwrap();
        let original = match change {
            "registration" => fixture.slot(&id).join("registration.json"),
            "manifest" | "hardlink" => fixture.source.join("local-app.json"),
            "backend" => fixture.source.join("server/main.mjs"),
            "active" => fixture.slot(&id).join("active.json"),
            _ => unreachable!(),
        };
        let target = fixture.source.join("preserved-link-target");
        if change == "active" {
            fs::write(&target, b"artificial active evidence").unwrap();
        } else if change == "hardlink" {
            fs::hard_link(&original, &target).unwrap();
        } else {
            fs::rename(&original, &target).unwrap();
        }
        if change != "hardlink" {
            symlink(&target, &original).unwrap();
        }
        let original_registration = fs::read(fixture.slot(&id).join("registration.json")).unwrap();
        assert!(
            fixture
                .manager
                .declaration_update_preview(&id)
                .await
                .is_err(),
            "{change}"
        );
        assert!(
            fixture
                .manager
                .declaration_update_apply(&id, request(&preview))
                .await
                .is_err(),
            "{change}"
        );
        assert_eq!(
            fs::read(fixture.slot(&id).join("registration.json")).unwrap(),
            original_registration
        );
    }
}

#[tokio::test]
async fn declaration_update_checks_credential_metadata_without_decoding_credential_contents() {
    let fixture = Fixture::new();
    let id = fixture.register_and_configure().await;
    let directory = fixture.slot(&id).join("configuration/credentials");
    let entry = fs::read_dir(&directory)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    // A metadata-only view can inspect this artificial file, whereas reading a
    // launch credential would reject its invalid UTF-8 and control characters.
    fs::write(&entry, [0xff, 0, 0xfe, 1]).unwrap();
    fixture.extend();
    let before = files_below(&fixture.slot(&id));
    let preview = fixture
        .manager
        .declaration_update_preview(&id)
        .await
        .unwrap();
    assert_eq!(preview["configuration"]["secretStatus"]["API_KEY"], "set");
    fixture
        .manager
        .declaration_update_apply(&id, request(&preview))
        .await
        .unwrap();
    let mut after = files_below(&fixture.slot(&id));
    let mut expected = before;
    after.remove(Path::new("registration.json"));
    expected.remove(Path::new("registration.json"));
    assert_eq!(after, expected);
}

#[test]
fn declaration_update_request_requires_complete_confirmation_and_rejects_unknown_fields() {
    let value = json!({"expectedRegisteredDigest":"a".repeat(64),"expectedManifestDigest":"b".repeat(64),"configurationRevision":1,"confirmed":true});
    for omitted in [
        "expectedRegisteredDigest",
        "expectedManifestDigest",
        "configurationRevision",
        "confirmed",
    ] {
        let mut invalid = value.clone();
        invalid.as_object_mut().unwrap().remove(omitted);
        assert!(serde_json::from_value::<DeclarationUpdateRequest>(invalid).is_err());
    }
    let mut invalid = value;
    invalid["credential"] = json!("must never be accepted");
    assert!(serde_json::from_value::<DeclarationUpdateRequest>(invalid).is_err());
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[tokio::test]
async fn running_external_service_cannot_preview_or_apply_a_declaration_update() {
    use std::os::unix::fs::PermissionsExt;
    let mut fixture = Fixture::new();
    let id = fixture.register_and_configure().await;
    let preview = fixture
        .manager
        .declaration_update_preview(&id)
        .await
        .unwrap();
    fs::write(fixture.source.join("server/main.mjs"), r#"import sys,json,threading
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
message=json.loads(sys.stdin.readline())
instance=message['instanceId']
token=message['token']
class Handler(BaseHTTPRequestHandler):
    def log_message(self,*args): pass
    def do_GET(self):
        if self.headers.get('Authorization')!='Bearer '+token:
            self.send_response(401);self.end_headers();return
        body=json.dumps({'protocolVersion':1,'instanceId':instance,'status':'ok'}).encode()
        self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
threading.Thread(target=server.serve_forever,daemon=True).start()
print(json.dumps({'protocolVersion':1,'type':'ready','instanceId':instance,'host':'127.0.0.1','port':server.server_address[1]}),flush=True)
for line in sys.stdin:
    command=json.loads(line)
    if command['type']=='shutdown':
        print(json.dumps({'protocolVersion':1,'type':'stopping','instanceId':instance}),flush=True)
        break
    if command['type']=='ping':
        print(json.dumps({'protocolVersion':1,'type':'pong','instanceId':instance,'requestId':command['requestId']}),flush=True)
server.shutdown();server.server_close()
"#).unwrap();
    let interpreter = fixture
        .source
        .parent()
        .unwrap()
        .join("artificial-declaration-node");
    fs::write(&interpreter, "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then printf 'v24.22.0\\n'; exit 0; fi\nexec /usr/bin/python3 \"$@\"\n").unwrap();
    fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o700)).unwrap();
    fixture.manager.node_path = interpreter.clone();
    fixture.manager.digest = format!("{:x}", Sha256::digest(fs::read(&interpreter).unwrap()));
    fixture.manager.start(&id).await.unwrap();
    let registration_before = fs::read(fixture.slot(&id).join("registration.json")).unwrap();
    let preview_result = fixture.manager.declaration_update_preview(&id).await;
    let apply_result = fixture
        .manager
        .declaration_update_apply(&id, request(&preview))
        .await;
    let running = fixture.manager.running_ids().await;
    // Always finish the artificial subprocess before asserting failures.
    fixture.manager.stop(&id).await.unwrap();
    assert_eq!(
        preview_result.unwrap_err().code,
        "NODE_DECLARATION_UPDATE_BUSY"
    );
    assert_eq!(
        apply_result.unwrap_err().code,
        "NODE_DECLARATION_UPDATE_BUSY"
    );
    assert_eq!(running, vec![id.clone()]);
    assert_eq!(
        fs::read(fixture.slot(&id).join("registration.json")).unwrap(),
        registration_before
    );
    assert!(!fixture.slot(&id).join("active.json").exists());
}
