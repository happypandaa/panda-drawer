//! Disposable direct-source registration and configuration refresh verification.
use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    apps: NodeApps,
    source: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let (temporary, paths) = crate::RuntimePaths::for_test();
        let source = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-direct-manifest-refresh");
        fs::create_dir_all(source.join("public")).unwrap();
        fs::create_dir_all(source.join("server")).unwrap();
        fs::write(source.join("public/index.html"), "disposable").unwrap();
        fs::write(
            source.join("server/main.mjs"),
            "throw Error('must not execute');",
        )
        .unwrap();
        let value = json!({
            "manifestVersion":1,"runtime":"node",
            "app":{"name":"Disposable original","version":"1"},
            "web":{"root":"public","entry":"index.html"},
            "backend":{"root":"server","entry":"main.mjs","protocolVersion":1,"nodeMajor":24},
            "configuration":{"version":1,"fields":[
                {"name":"MODEL","label":"Model","type":"string","required":false,"secret":false,
                 "description":"Disposable model","howToGet":["Test input"]}
            ]}
        });
        fs::write(
            source.join("panda.json"),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        let apps = NodeApps::new(
            paths.data_root().into(),
            "/disposable-missing-runtime-node".into(),
        )
        .unwrap();
        Self {
            _temporary: temporary,
            apps,
            source,
        }
    }
    fn change(&self, edit: impl FnOnce(&mut Value)) {
        let path = self.source.join("panda.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        edit(&mut value);
        fs::write(path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    }
    fn request(detail: &Value, values: Value) -> SaveConfigurationRequest {
        serde_json::from_value(json!({
            "revision":detail["configuration"]["revision"],
            "manifestDigest":detail["manifestDigest"],"values":values,"secrets":{}
        }))
        .unwrap()
    }
}

#[tokio::test]
async fn agent_hint_tracks_valid_edits_and_survives_missing_source_without_rebinding() {
    let fixture = Fixture::new();
    fs::write(
        fixture.source.join("AGENTS.md"),
        "Disposable agent instructions",
    )
    .unwrap();
    fixture.change(|value| value["agent"] = json!({"instructions":"AGENTS.md"}));
    let original = fixture.apps.register(fixture.source.clone()).await.unwrap();
    let id = original["id"].as_str().unwrap();
    assert_eq!(original["isAgent"], true);
    assert_eq!(original["ownership"], "external");
    let slot = fixture.apps.root.join(id);
    let registration = slot.join("registration.json");
    let before = fs::read(&registration).unwrap();
    let launch_identity = node_registration::registration_digest(&slot).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&before).unwrap()["isAgent"],
        true
    );

    fixture.change(|value| {
        value.as_object_mut().unwrap().remove("agent");
    });
    let ordinary = fixture.apps.configuration(id).await.unwrap();
    assert_eq!(ordinary["isAgent"], false);
    assert_eq!(
        ordinary["configuration"]["revision"],
        original["configuration"]["revision"]
    );
    assert_eq!(fixture.apps.list().await.unwrap()[0]["isAgent"], false);
    assert_eq!(fs::read(&registration).unwrap(), before);
    assert_eq!(
        node_registration::registration_digest(&slot).unwrap(),
        launch_identity
    );
    let moved = fixture
        .source
        .with_file_name("disposable-moved-agent-source");
    fs::rename(&fixture.source, &moved).unwrap();
    let missing = fixture.apps.status(id).await.unwrap();
    assert_eq!(missing["isAgent"], false);
    assert_eq!(missing["ownership"], "external");

    fs::rename(&moved, &fixture.source).unwrap();
    fixture.change(|value| value["agent"] = json!({"instructions":"AGENTS.md"}));
    assert_eq!(fixture.apps.status(id).await.unwrap()["isAgent"], true);
    fs::remove_file(fixture.source.join("AGENTS.md")).unwrap();
    assert_eq!(fixture.apps.status(id).await.unwrap()["isAgent"], true);
    assert_eq!(fs::read(&registration).unwrap(), before);
    assert_eq!(
        node_registration::registration_digest(&slot).unwrap(),
        launch_identity
    );
}

#[tokio::test]
async fn changed_display_metadata_is_previewed_and_accepted_on_start() {
    let fixture = Fixture::new();
    let original = fixture.apps.register(fixture.source.clone()).await.unwrap();
    let id = original["id"].as_str().unwrap();
    let saved = fixture
        .apps
        .save(
            id,
            Fixture::request(&original, json!({"MODEL":"keep-saved-model"})),
        )
        .await
        .unwrap();
    let slot = fixture.apps.root.join(id);
    let registration_before = fs::read(slot.join("registration.json")).unwrap();
    let configuration_before = fs::read(slot.join("configuration/configuration.json")).unwrap();
    fixture.change(|value| {
        value["app"] = json!({"name":"Renamed author metadata","version":"2"});
    });
    let preview = fixture.apps.configuration(id).await.unwrap();
    assert_eq!(preview["id"], original["id"]);
    assert_eq!(preview["name"], "Renamed author metadata");
    assert_eq!(preview["version"], "2");
    assert_eq!(
        preview["configuration"]["values"]["MODEL"],
        "keep-saved-model"
    );
    assert_eq!(
        fixture.apps.status(id).await.unwrap()["manifestDigest"],
        preview["manifestDigest"]
    );
    assert_eq!(
        fixture.apps.list().await.unwrap()[0]["manifestDigest"],
        preview["manifestDigest"]
    );
    assert_eq!(
        fs::read(slot.join("registration.json")).unwrap(),
        registration_before
    );
    let stale = fixture
        .apps
        .save(
            id,
            Fixture::request(&saved, json!({"MODEL":"stale-overwrite"})),
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code, "NODE_MANIFEST_CHANGED");
    assert_eq!(
        fs::read(slot.join("configuration/configuration.json")).unwrap(),
        configuration_before
    );
    // The deliberately absent interpreter prevents any application execution.
    assert!(fixture.apps.start(id).await.is_err());
    let accepted: Value =
        serde_json::from_slice(&fs::read(slot.join("registration.json")).unwrap()).unwrap();
    assert_eq!(accepted["manifestDigest"], preview["manifestDigest"]);
    assert_eq!(accepted["id"], original["id"]);
    assert_eq!(
        fs::read(slot.join("configuration/configuration.json")).unwrap(),
        configuration_before
    );
    assert!(!slot.join("active.json").exists());
}

#[tokio::test]
async fn newly_required_configuration_is_editable_before_launch() {
    let fixture = Fixture::new();
    let original = fixture.apps.register(fixture.source.clone()).await.unwrap();
    let id = original["id"].as_str().unwrap();
    fixture.change(|value| {
        value["configuration"]["fields"][0]["required"] = json!(true);
    });
    let preview = fixture.apps.configuration(id).await.unwrap();
    assert_eq!(preview["configuration"]["canStart"], false);
    assert_ne!(preview["configuration"]["unavailable"], true);
    assert_eq!(preview["fields"][0]["name"], "MODEL");
    assert_eq!(
        fixture.apps.start(id).await.unwrap_err().code,
        "NODE_CONFIGURATION_REQUIRED"
    );
    let saved = fixture
        .apps
        .save(
            id,
            Fixture::request(&preview, json!({"MODEL":"new-required-value"})),
        )
        .await
        .unwrap();
    assert_eq!(saved["configuration"]["canStart"], true);
    assert_eq!(
        saved["configuration"]["values"]["MODEL"],
        "new-required-value"
    );
    assert!(!fixture.apps.root.join(id).join("active.json").exists());
}

#[tokio::test]
async fn invalid_or_missing_entry_does_not_replace_the_accepted_declaration() {
    let fixture = Fixture::new();
    let original = fixture.apps.register(fixture.source.clone()).await.unwrap();
    let id = original["id"].as_str().unwrap();
    let slot = fixture.apps.root.join(id);
    let before = fs::read(slot.join("registration.json")).unwrap();
    fixture.change(|value| value["backend"]["entry"] = json!("missing.mjs"));
    assert_eq!(
        fixture.apps.configuration(id).await.unwrap()["configuration"]["canStart"],
        false
    );
    assert!(fixture.apps.start(id).await.is_err());
    assert_eq!(fs::read(slot.join("registration.json")).unwrap(), before);
    fs::write(fixture.source.join("panda.json"), "{invalid").unwrap();
    assert!(fixture.apps.start(id).await.is_err());
    assert_eq!(fs::read(slot.join("registration.json")).unwrap(), before);
    assert!(!slot.join("active.json").exists());
}

#[tokio::test]
async fn running_snapshot_is_frozen_while_configuration_reads_preview_changes() {
    let fixture = Fixture::new();
    let original = fixture.apps.register(fixture.source.clone()).await.unwrap();
    let id = original["id"].as_str().unwrap();
    let manifest = Arc::new(fixture.apps.load_source(&fixture.source).unwrap());
    let entry = Arc::new(RuntimeEntry::new(
        0,
        "disposable-registration".into(),
        manifest.clone(),
    ));
    entry.set_phase(RUNTIME_RUNNING);
    fixture
        .apps
        .inner
        .lock()
        .await
        .running
        .insert(id.into(), entry.clone());
    fixture.change(|value| value["app"]["version"] = json!("next-version"));
    let preview = fixture.apps.configuration(id).await.unwrap();
    assert_eq!(preview["state"], "running");
    assert_eq!(preview["restartRequired"], true);
    assert_eq!(preview["version"], "next-version");
    assert_eq!(entry.manifest.app.version.as_deref(), Some("1"));
    assert_eq!(entry.manifest.source_digest, manifest.source_digest);
    assert_ne!(preview["manifestDigest"], entry.manifest.source_digest);
    fixture.apps.inner.lock().await.running.remove(id);
}

#[tokio::test]
async fn unsupported_author_fields_do_not_change_registration_or_data() {
    let fixture = Fixture::new();
    let original = fixture.apps.register(fixture.source.clone()).await.unwrap();
    let id = original["id"].as_str().unwrap();
    let registration = fixture.apps.root.join(id).join("registration.json");
    let before = fs::read(&registration).unwrap();
    fixture.change(|value| value["app"]["id"] = json!("not-an-instance-id"));
    assert!(
        !fixture.apps.configuration(id).await.unwrap()["configuration"]["canStart"]
            .as_bool()
            .unwrap()
    );
    assert!(fixture.apps.start(id).await.is_err());
    assert_eq!(fs::read(registration).unwrap(), before);
    assert!(!fixture.source.join("data").exists());
}

#[tokio::test]
async fn changing_runtime_refuses_start_without_rebinding_or_removing_file_data() {
    let fixture = Fixture::new();
    let original = fixture.apps.register(fixture.source.clone()).await.unwrap();
    let id = original["id"].as_str().unwrap();
    fs::create_dir(fixture.source.join("data")).unwrap();
    fs::write(fixture.source.join("data/keep"), "user data").unwrap();
    let registration = fixture.apps.root.join(id).join("registration.json");
    let before = fs::read(&registration).unwrap();
    fixture.change(|value| {
        value["runtime"] = json!("web");
        value.as_object_mut().unwrap().remove("backend");
    });
    let error = fixture.apps.start(id).await.unwrap_err();
    assert_eq!(error.code, "APP_RUNTIME_CHANGED");
    assert_eq!(fs::read(registration).unwrap(), before);
    assert_eq!(
        fs::read_to_string(fixture.source.join("data/keep")).unwrap(),
        "user data"
    );
}
