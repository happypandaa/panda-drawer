//! Public API coverage using disposable source fixtures and ordinary values.
//! These tests never provide a runnable Node, save a secret, query Keychain or
//! call an external service. The source side effect is an execution sentinel.

use platform_core::node_app_configuration::SaveConfigurationRequest;
use platform_core::node_apps::NodeApps;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Fixture {
    _temporary: TempDir,
    source: PathBuf,
    data: PathBuf,
    node: PathBuf,
    executed: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        // /var is a macOS symlink; use the real disposable location to exercise
        // the same no-link path rules as manager-selected application folders.
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join(name);
        let data = root.join("platform-data");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&data).unwrap();
        fs::create_dir(source.join("public")).unwrap();
        fs::create_dir(source.join("server")).unwrap();
        fs::write(
            source.join("public/index.html"),
            "<p>Artificial fixture</p>",
        )
        .unwrap();
        let executed = root.join("source-was-executed");
        fs::write(
            source.join("server/managed.mjs"),
            format!(
                "import {{writeFileSync}} from 'node:fs';writeFileSync({},'unexpected execution');process.exit(77);",
                serde_json::to_string(&executed).unwrap()
            ),
        )
        .unwrap();
        fs::write(
            source.join(".env"),
            "ARTIFICIAL_SENTINEL=do-not-read-as-configuration\n",
        )
        .unwrap();
        fs::write(
            source.join("local-app.json"),
            serde_json::to_vec_pretty(&json!({
                "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
                "app":{"id":"test.disposable.node","name":name,"version":"0.1.0"},
                "web":{"root":"public","entry":"index.html","routing":"files"},
                "backend":{"protocolVersion":1,"root":"server","entry":"managed.mjs","nodeMajor":24},
                "configuration":{"version":1,"fields":[
                    {"name":"MODEL","label":"测试模型","type":"string","required":false,"secret":false,
                     "description":"仅供人工测试","howToGet":["此夹具不连接外部服务。"],"default":"default-fixture-model"},
                    {"name":"AI_KEY","label":"测试密钥","type":"string","required":true,"secret":true,
                     "description":"此测试不保存密钥","howToGet":["保留未填写状态用于预检。"]}
                ]}
            }))
            .unwrap(),
        )
        .unwrap();
        Self {
            _temporary: temporary,
            source,
            data,
            node: root.join("deliberately-missing-node"),
            executed,
        }
    }

    fn manager(&self) -> NodeApps {
        NodeApps::new(self.data.clone(), self.node.clone()).unwrap()
    }

    fn assert_not_executed(&self) {
        assert!(
            !self.executed.exists(),
            "registration must never execute application source"
        );
        assert_eq!(
            fs::read_to_string(self.source.join(".env")).unwrap(),
            "ARTIFICIAL_SENTINEL=do-not-read-as-configuration\n"
        );
    }
}

fn id(value: &Value) -> &str {
    value["id"].as_str().unwrap()
}

fn save(revision: u64, values: Value) -> SaveConfigurationRequest {
    serde_json::from_value(json!({"revision":revision,"values":values,"secrets":{}})).unwrap()
}

#[tokio::test]
async fn preview_and_registration_are_read_only_toward_source_and_do_not_execute_it() {
    let fixture = Fixture::new("read-only-node-fixture");
    let manager = fixture.manager();
    let before = fs::read(fixture.source.join("local-app.json")).unwrap();
    let preview = manager.inspect(&fixture.source).unwrap();
    assert_eq!(preview["inspectionKind"], "read-only-trial");
    let registrations = fixture.data.join("node-apps");
    assert!(
        !registrations.exists()
            || !registrations
                .read_dir()
                .unwrap()
                .any(|entry| { entry.unwrap().path().join("registration.json").exists() })
    );
    let detail = manager.register(fixture.source.clone()).await.unwrap();
    assert_eq!(detail["runtime"], "node-service");
    assert_eq!(detail["state"], "stopped");
    assert_eq!(detail["configuration"]["canStart"], false);
    assert_eq!(
        detail["configuration"]["missingRequired"],
        json!(["AI_KEY"])
    );
    assert_eq!(manager.list().await.unwrap().len(), 1);
    assert_eq!(
        fs::read(fixture.source.join("local-app.json")).unwrap(),
        before
    );
    fixture.assert_not_executed();
}

#[tokio::test]
async fn duplicate_registration_keeps_the_original_id_and_saved_configuration() {
    let fixture = Fixture::new("deduplicated-node-fixture");
    let manager = fixture.manager();
    let original = manager.register(fixture.source.clone()).await.unwrap();
    let original_id = id(&original);
    let configured = manager
        .save(
            original_id,
            save(0, json!({"MODEL":"artificial-user-choice"})),
        )
        .await
        .unwrap();
    assert_eq!(configured["configuration"]["revision"], 1);
    let duplicate = manager.register(fixture.source.clone()).await.unwrap();
    assert_eq!(id(&duplicate), original_id);
    assert_eq!(duplicate["configuration"]["revision"], 1);
    assert_eq!(
        duplicate["configuration"]["values"]["MODEL"],
        "artificial-user-choice"
    );
    assert_eq!(manager.list().await.unwrap().len(), 1);
    fixture.assert_not_executed();
}

#[tokio::test]
async fn recreating_the_manager_preserves_the_slot_and_saved_ordinary_values() {
    let fixture = Fixture::new("reopened-node-fixture");
    let original_id = {
        let manager = fixture.manager();
        let registered = manager.register(fixture.source.clone()).await.unwrap();
        let original_id = id(&registered).to_owned();
        manager
            .save(
                &original_id,
                save(0, json!({"MODEL":"persisted-artificial-model"})),
            )
            .await
            .unwrap();
        original_id
    };
    let manager = fixture.manager();
    let values = manager.list().await.unwrap();
    assert_eq!(values.len(), 1);
    assert_eq!(id(&values[0]), original_id);
    assert_eq!(values[0]["configuration"]["revision"], 1);
    assert_eq!(
        values[0]["configuration"]["values"]["MODEL"],
        "persisted-artificial-model"
    );
    assert_eq!(values[0]["state"], "stopped");
    fixture.assert_not_executed();
}

#[tokio::test]
async fn missing_required_credentials_block_before_creating_an_active_marker() {
    let fixture = Fixture::new("missing-key-node-fixture");
    let manager = fixture.manager();
    let registered = manager.register(fixture.source.clone()).await.unwrap();
    let original_id = id(&registered);
    assert!(manager.start(original_id).await.is_err());
    let slot = fixture.data.join("node-apps").join(original_id);
    assert!(!slot.join("active.json").exists());
    assert_eq!(slot.join("runs").read_dir().unwrap().count(), 0);
    assert_eq!(
        manager.configuration(original_id).await.unwrap()["state"],
        "stopped"
    );
    fixture.assert_not_executed();
}

#[tokio::test]
async fn a_missing_private_node_does_not_leave_a_false_active_process_marker() {
    let fixture = Fixture::new("missing-runtime-node-fixture");
    let manifest_path = fixture.source.join("local-app.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["configuration"]["fields"]
        .as_array_mut()
        .unwrap()
        .retain(|field| field["secret"] != true);
    fs::write(manifest_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    let manager = fixture.manager();
    let registered = manager.register(fixture.source.clone()).await.unwrap();
    assert_eq!(registered["configuration"]["canStart"], true);
    let original_id = id(&registered);
    assert!(manager.start(original_id).await.is_err());
    assert!(!fixture
        .data
        .join("node-apps")
        .join(original_id)
        .join("active.json")
        .exists());
    assert_eq!(
        manager.configuration(original_id).await.unwrap()["state"],
        "stopped"
    );
    fixture.assert_not_executed();
}

#[tokio::test]
async fn failed_or_changed_source_registration_preserves_the_existing_record() {
    let fixture = Fixture::new("protected-registration-node-fixture");
    let manager = fixture.manager();
    let registered = manager.register(fixture.source.clone()).await.unwrap();
    let original_id = id(&registered);
    manager
        .save(
            original_id,
            save(0, json!({"MODEL":"keep-this-user-value"})),
        )
        .await
        .unwrap();
    let slot = fixture.data.join("node-apps").join(original_id);
    let registration_before = fs::read(slot.join("registration.json")).unwrap();
    let configuration_before = fs::read(slot.join("configuration/configuration.json")).unwrap();
    let manifest_before = fs::read(fixture.source.join("local-app.json")).unwrap();
    let mut changed: Value = serde_json::from_slice(&manifest_before).unwrap();
    changed["app"]["name"] = json!("Changed declaration requires review");
    fs::write(
        fixture.source.join("local-app.json"),
        serde_json::to_vec_pretty(&changed).unwrap(),
    )
    .unwrap();
    assert!(manager.register(fixture.source.clone()).await.is_err());
    assert!(manager.start(original_id).await.is_err());
    assert!(!slot.join("active.json").exists());
    assert_eq!(
        fs::read(slot.join("registration.json")).unwrap(),
        registration_before
    );
    assert_eq!(
        fs::read(slot.join("configuration/configuration.json")).unwrap(),
        configuration_before
    );
    fs::write(fixture.source.join("local-app.json"), manifest_before).unwrap();
    assert_eq!(
        manager.configuration(original_id).await.unwrap()["configuration"]["values"]["MODEL"],
        "keep-this-user-value"
    );
    fixture.assert_not_executed();
}

#[tokio::test]
async fn one_changed_declaration_does_not_hide_the_other_registered_application() {
    let fixture = Fixture::new("first-isolated-node-fixture");
    let manager = fixture.manager();
    let first = manager.register(fixture.source.clone()).await.unwrap();
    let other_source = fixture
        .source
        .parent()
        .unwrap()
        .join("second-isolated-node-fixture");
    copy_fixture(&fixture.source, &other_source);
    let second = manager.register(other_source).await.unwrap();
    fs::write(
        fixture.source.join("local-app.json"),
        "{broken artificial fixture}",
    )
    .unwrap();
    let listed = manager.list().await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(
        listed.iter().find(|value| id(value) == id(&first)).unwrap()["state"],
        "failed"
    );
    assert_eq!(
        listed
            .iter()
            .find(|value| id(value) == id(&second))
            .unwrap()["state"],
        "stopped"
    );
    fixture.assert_not_executed();
}

fn copy_fixture(source: &Path, destination: &Path) {
    fs::create_dir(destination).unwrap();
    fs::create_dir(destination.join("public")).unwrap();
    fs::create_dir(destination.join("server")).unwrap();
    for relative in ["local-app.json", "public/index.html", "server/managed.mjs"] {
        fs::copy(source.join(relative), destination.join(relative)).unwrap();
    }
}

#[tokio::test]
async fn data_root_and_its_ancestors_cannot_be_registered_as_application_sources() {
    let fixture = Fixture::new("separate-source-node-fixture");
    let manager = fixture.manager();
    assert!(manager.inspect(&fixture.data).is_err());
    assert!(manager.register(fixture.data.clone()).await.is_err());
    assert!(manager.inspect(fixture.data.parent().unwrap()).is_err());
    assert!(manager
        .register(fixture.data.parent().unwrap().to_path_buf())
        .await
        .is_err());
    assert!(manager.list().await.unwrap().is_empty());
    fixture.assert_not_executed();
}

#[cfg(unix)]
#[tokio::test]
async fn linked_source_paths_are_rejected_without_registering_or_executing() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new("unlinked-node-fixture");
    let manager = fixture.manager();
    let linked = fixture.source.parent().unwrap().join("linked-node-fixture");
    symlink(&fixture.source, &linked).unwrap();
    assert!(manager.inspect(&linked).is_err());
    assert!(manager.register(linked).await.is_err());
    assert!(manager.list().await.unwrap().is_empty());
    fixture.assert_not_executed();
}
