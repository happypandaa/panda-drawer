//! Disposable direct-source registration and configuration refresh verification.
use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    apps: NodeApps,
    source: PathBuf,
    paths: crate::RuntimePaths,
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
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"disposable.original","name":"Disposable original","version":"1"},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"root":"server","entry":"main.mjs","protocolVersion":1,"nodeMajor":24},
            "configuration":{"version":1,"fields":[
                {"name":"MODEL","label":"Model","type":"string","required":false,"secret":false,
                 "description":"Disposable model","howToGet":["Test input"]}
            ]}
        });
        fs::write(
            source.join("local-app.json"),
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
            paths,
        }
    }
    fn change(&self, edit: impl FnOnce(&mut Value)) {
        let path = self.source.join("local-app.json");
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
async fn changed_author_identity_and_declaration_are_previewed_and_accepted_on_start() {
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
        value["app"] =
            json!({"id":"disposable.renamed","name":"Renamed author metadata","version":"2"});
        value["development"] = json!({"retired":"ignored by runtime"});
        value["dataContract"] = json!({"retired":"ignored by runtime"});
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
    fs::write(fixture.source.join("local-app.json"), "{invalid").unwrap();
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
    assert_eq!(entry.manifest.app.version, "1");
    assert_eq!(entry.manifest.source_digest, manifest.source_digest);
    assert_ne!(preview["manifestDigest"], entry.manifest.source_digest);
    fixture.apps.inner.lock().await.running.remove(id);
}

#[tokio::test]
async fn retired_registration_is_unavailable_but_all_owned_materials_are_deletable() {
    let fixture = Fixture::new();
    let store = Arc::new(crate::ProjectStore::new(fixture.paths.workspace_root().into()).unwrap());
    let tasks = crate::builds::BuildManager::new(
        fixture.paths.clone(),
        store.clone(),
        "http://127.0.0.1:1".into(),
    )
    .unwrap();
    fixture.apps.set_deletion_store(store.clone()).unwrap();
    fixture.apps.set_import_task_manager(tasks.clone()).unwrap();
    let id = Uuid::new_v4().to_string();
    let generation = format!("gen_{}", Uuid::new_v4().simple());
    let source_id = format!("src_{}", Uuid::new_v4().simple());
    let candidate_id = format!("install_{}", Uuid::new_v4().simple());
    let task_id = format!("task_{}", Uuid::new_v4().simple());
    let slot = fixture.apps.root.join(&id);
    let program = slot.join("generations").join(&generation);
    for path in [
        program.join("node_modules"),
        slot.join("data"),
        slot.join("configuration"),
        slot.join("runs"),
    ] {
        fs::create_dir_all(path).unwrap();
    }
    fs::write(
        program.join("local-app.json"),
        "broken retired program declaration",
    )
    .unwrap();
    fs::write(program.join("node_modules/dependency"), "retired code").unwrap();
    fs::write(
        slot.join("data/business.sqlite"),
        "disposable business data",
    )
    .unwrap();
    fs::write(
        slot.join("configuration/private-value"),
        "disposable saved configuration",
    )
    .unwrap();
    fs::write(slot.join("runs/runtime.log"), "disposable runtime log").unwrap();
    let registration = json!({
        "version":2,"kind":"managed","id":id,"generation":generation,
        "manifestDigest":"a".repeat(64),
        "provenance":{"sourceId":source_id,"sourceDigest":"b".repeat(64)}
    });
    fs::write(
        slot.join("registration.json"),
        serde_json::to_vec(&registration).unwrap(),
    )
    .unwrap();
    let ownership = fixture.apps.project_ownership();
    ownership.initialize(&id).unwrap();
    let owned_source = fixture
        .apps
        .deletion_workspace()
        .join("project-sources/disposable-retired-owned-source");
    let download = fixture
        .apps
        .deletion_workspace()
        .join(".platform/sources")
        .join(&source_id);
    let candidate = fixture.apps.root.join(".installations").join(&candidate_id);
    let task = fixture
        .apps
        .deletion_workspace()
        .join(".platform/tasks")
        .join(&task_id);
    for path in [&owned_source, &download, &candidate, &task] {
        fs::create_dir_all(path).unwrap();
        fs::write(path.join("disposable-payload"), "owned material").unwrap();
    }
    ownership
        .record_generated_source(&id, &owned_source)
        .unwrap();
    ownership.record_source(&id, &source_id).unwrap();
    ownership.record_installation(&id, &candidate_id).unwrap();
    ownership.record_task(&id, &task_id).unwrap();
    let listed = fixture.apps.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0]["lastError"],
        node_registration::RETIRED_NODE_MESSAGE
    );
    assert_eq!(listed[0]["configuration"]["canStart"], false);
    assert_eq!(
        fixture.apps.start(&id).await.unwrap_err().code,
        "NODE_REGISTRATION_RETIRED"
    );
    let preview = fixture.apps.delete_preview(&id).await.unwrap();
    let input: DeleteNodeAppInput =
        serde_json::from_value(json!({"expectedRevision":preview["expectedRevision"]})).unwrap();
    fixture.apps.delete(&id, input).await.unwrap();
    for path in [
        &slot,
        &program,
        &owned_source,
        &download,
        &candidate,
        &task,
        &ownership.directory(&id).unwrap(),
    ] {
        assert!(
            !path.exists(),
            "retired project residue: {}",
            path.display()
        );
    }
    assert!(
        fixture.source.join("local-app.json").is_file(),
        "unowned original source must remain"
    );
    assert!(fixture.apps.list().await.unwrap().is_empty());
    assert!(tasks.shutdown().await.is_empty());
    assert!(store.shutdown().await.is_empty());
}

#[tokio::test]
async fn damaged_retired_display_metadata_does_not_block_its_deletion_or_other_projects() {
    let fixture = Fixture::new();
    let store = Arc::new(crate::ProjectStore::new(fixture.paths.workspace_root().into()).unwrap());
    let tasks = crate::builds::BuildManager::new(
        fixture.paths.clone(),
        store.clone(),
        "http://127.0.0.1:1".into(),
    )
    .unwrap();
    fixture.apps.set_deletion_store(store.clone()).unwrap();
    fixture.apps.set_import_task_manager(tasks.clone()).unwrap();
    let normal = fixture.apps.register(fixture.source.clone()).await.unwrap();
    let normal_id = normal["id"].as_str().unwrap();
    let id = Uuid::new_v4().to_string();
    let slot = fixture.apps.root.join(&id);
    fs::create_dir_all(slot.join("generations/disposable-program")).unwrap();
    fs::create_dir_all(slot.join("data")).unwrap();
    fs::write(
        slot.join("generations/disposable-program/code.js"),
        b"retired program",
    )
    .unwrap();
    fs::write(slot.join("data/business"), b"retired data").unwrap();
    fixture.apps.project_ownership().initialize(&id).unwrap();
    fs::write(
        slot.join("registration.json"),
        serde_json::to_vec(&json!({
            "version":2,"id":id,"kind":null,"generation":"/outside/the/owned/slot",
            "manifestDigest":null,"provenance":{"sourceId":"damaged","sourceDigest":false},
            "sourcePath":fixture.source
        }))
        .unwrap(),
    )
    .unwrap();
    let listed = fixture.apps.list().await.unwrap();
    assert_eq!(listed.len(), 2);
    let retired = listed.iter().find(|app| app["id"] == id).unwrap();
    assert_eq!(
        retired["lastError"],
        node_registration::RETIRED_NODE_MESSAGE
    );
    assert_eq!(retired["sourcePath"], json!(slot));
    assert_eq!(retired["configuration"]["canStart"], false);
    assert_eq!(
        fixture.apps.configuration(normal_id).await.unwrap()["id"],
        normal_id
    );
    let preview = fixture.apps.delete_preview(&id).await.unwrap();
    fixture
        .apps
        .delete(
            &id,
            serde_json::from_value(json!({"expectedRevision":preview["expectedRevision"]}))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(!slot.exists());
    assert!(fixture.source.join("local-app.json").is_file());
    assert_eq!(fixture.apps.list().await.unwrap().len(), 1);
    assert_eq!(
        fixture.apps.configuration(normal_id).await.unwrap()["id"],
        normal_id
    );
    assert!(tasks.shutdown().await.is_empty());
    assert!(store.shutdown().await.is_empty());
}
