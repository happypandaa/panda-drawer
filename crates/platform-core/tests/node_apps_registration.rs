//! Public unified application admission uses disposable, non-executed sources.
use platform_core::{
    node_app_configuration::SaveConfigurationRequest, panda_manifest::PandaManifest, ProjectStore,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tempfile::TempDir;
struct Fixture {
    _temporary: TempDir,
    source: PathBuf,
    workspace: PathBuf,
    data: PathBuf,
    executed: PathBuf,
}
impl Fixture {
    fn new(name: &str) -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join(name);
        let data = root.join("platform-data");
        let workspace = data.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(source.join("public")).unwrap();
        fs::create_dir_all(source.join("server")).unwrap();
        fs::write(
            source.join("public/index.html"),
            "<p>Disposable application</p>",
        )
        .unwrap();
        let executed = root.join("source-was-executed");
        fs::write(source.join("server/managed.mjs"),format!("import{{writeFileSync}}from'node:fs';writeFileSync({},'unexpected execution');process.exit(77);",serde_json::to_string(&executed).unwrap())).unwrap();
        fs::write(
            source.join(".env"),
            "ARTIFICIAL_SENTINEL=do-not-read-as-configuration\n",
        )
        .unwrap();
        fs::write(source.join("panda.json"),serde_json::to_vec_pretty(&json!({"manifestVersion":2,"app":{"name":name,"version":"0.1.0"},"web":{"root":"public","entry":"index.html"},"backend":{"protocolVersion":1,"root":"server","entry":"managed.mjs","nodeMajor":24},"configuration":{"version":1,"fields":[{"name":"MODEL","label":"测试模型","type":"string","required":false,"secret":false,"description":"仅供人工测试","howToGet":["不连接外部服务。"],"default":"default-fixture-model"},{"name":"AI_KEY","label":"测试密钥","type":"string","required":true,"secret":true,"description":"此测试不保存密钥","howToGet":["保留未填写状态用于预检。"]}]}})).unwrap()).unwrap();
        Self {
            _temporary: temporary,
            source,
            workspace,
            data,
            executed,
        }
    }
    fn store(&self) -> ProjectStore {
        ProjectStore::new(self.workspace.clone()).unwrap()
    }
    fn untouched(&self) {
        assert!(!self.executed.exists());
        assert_eq!(
            fs::read_to_string(self.source.join(".env")).unwrap(),
            "ARTIFICIAL_SENTINEL=do-not-read-as-configuration\n"
        );
    }
}
fn save(revision: u64, values: Value, digest: &str) -> SaveConfigurationRequest {
    serde_json::from_value(
        json!({"revision":revision,"manifestDigest":digest,"values":values,"secrets":{}}),
    )
    .unwrap()
}
#[tokio::test]
async fn preview_and_registration_do_not_execute_or_modify_sources() {
    let f = Fixture::new("read-only-backend");
    let store = f.store();
    let before = fs::read(f.source.join("panda.json")).unwrap();
    let preview = PandaManifest::load(&f.source)
        .unwrap()
        .inspect_local(&f.source)
        .unwrap();
    assert_eq!(preview["inspectionKind"], "read-only");
    assert!(fs::read_dir(f.data.join("apps")).unwrap().next().is_none());
    let app = store.register_local(&f.source).await.unwrap();
    assert!(app.has_backend);
    assert_eq!(
        app.ownership,
        platform_core::projects::ProjectOwnership::User
    );
    assert_eq!(app.state, "needsConfiguration");
    assert!(uuid::Uuid::parse_str(&app.id).is_ok());
    let handle = store.project(&app.id).unwrap();
    assert_eq!(handle.private_dir, f.data.join("apps").join(&app.id));
    let config = store.web_configuration(&app.id).unwrap();
    assert!(!config.configuration.can_start);
    assert_eq!(config.configuration.missing_required, vec!["AI_KEY"]);
    assert_eq!(store.list().await.unwrap().len(), 1);
    assert_eq!(fs::read(f.source.join("panda.json")).unwrap(), before);
    assert!(!f.source.join("data").exists());
    f.untouched();
    store.shutdown().await;
}
#[tokio::test]
async fn duplicate_source_never_creates_second_identity_or_discards_configuration() {
    let f = Fixture::new("duplicate-backend");
    let store = f.store();
    let app = store.register_local(&f.source).await.unwrap();
    let config = store.web_configuration(&app.id).unwrap();
    store
        .save_web_configuration(
            &app.id,
            save(0, json!({"MODEL":"user-choice"}), &config.manifest_digest),
        )
        .unwrap();
    assert!(store.register_local(&f.source).await.is_err());
    assert_eq!(store.list().await.unwrap().len(), 1);
    let actual = store.web_configuration(&app.id).unwrap();
    assert_eq!(actual.configuration.revision, 1);
    assert_eq!(actual.configuration.values["MODEL"], "user-choice");
    f.untouched();
    store.shutdown().await;
}
#[tokio::test]
async fn reopening_store_preserves_identity_and_public_configuration() {
    let f = Fixture::new("reopen-backend");
    let id;
    {
        let store = f.store();
        let app = store.register_local(&f.source).await.unwrap();
        id = app.id;
        let config = store.web_configuration(&id).unwrap();
        store
            .save_web_configuration(
                &id,
                save(
                    0,
                    json!({"MODEL":"persisted-model"}),
                    &config.manifest_digest,
                ),
            )
            .unwrap();
        store.shutdown().await;
    }
    let store = f.store();
    let apps = store.list().await.unwrap();
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].id, id);
    assert_eq!(
        store.web_configuration(&id).unwrap().configuration.values["MODEL"],
        "persisted-model"
    );
    f.untouched();
    store.shutdown().await;
}
#[tokio::test]
async fn missing_required_secret_is_detected_without_run_or_process_markers() {
    let f = Fixture::new("missing-required-secret");
    let store = f.store();
    let app = store.register_local(&f.source).await.unwrap();
    let handle = store.project(&app.id).unwrap();
    assert!(store.require_web_configuration_ready(&handle).is_err());
    assert!(!handle.private_dir.join("active.json").exists());
    assert!(!handle.private_dir.join("runs").exists());
    f.untouched();
    store.shutdown().await;
}
#[tokio::test]
async fn edited_manifest_is_previewed_without_committing_or_losing_configuration() {
    let f = Fixture::new("edit-preview-backend");
    let store = f.store();
    let app = store.register_local(&f.source).await.unwrap();
    let config = store.web_configuration(&app.id).unwrap();
    store
        .save_web_configuration(
            &app.id,
            save(
                0,
                json!({"MODEL":"keep-user-value"}),
                &config.manifest_digest,
            ),
        )
        .unwrap();
    let slot = store.project(&app.id).unwrap().private_dir;
    let before = fs::read(slot.join("registration.json")).unwrap();
    let mut value: Value =
        serde_json::from_slice(&fs::read(f.source.join("panda.json")).unwrap()).unwrap();
    value["app"]["name"] = json!("Edited declaration");
    fs::write(
        f.source.join("panda.json"),
        serde_json::to_vec_pretty(&value).unwrap(),
    )
    .unwrap();
    let preview = store.info(&app.id).await.unwrap();
    assert_eq!(preview.name, "Edited declaration");
    assert_eq!(
        store
            .web_configuration(&app.id)
            .unwrap()
            .configuration
            .values["MODEL"],
        "keep-user-value"
    );
    assert_eq!(fs::read(slot.join("registration.json")).unwrap(), before);
    f.untouched();
    store.shutdown().await;
}
#[tokio::test]
async fn missing_or_broken_source_does_not_hide_other_applications() {
    let f = Fixture::new("isolation-backend");
    let store = f.store();
    let first = store.register_local(&f.source).await.unwrap();
    let other = f.source.with_extension("second");
    copy_fixture(&f.source, &other);
    let second = store.register_local(&other).await.unwrap();
    fs::write(
        f.source.join("panda.json"),
        "{broken disposable declaration}",
    )
    .unwrap();
    let apps = store.list().await.unwrap();
    assert_eq!(apps.len(), 2);
    assert_eq!(
        apps.iter().find(|app| app.id == first.id).unwrap().state,
        "error"
    );
    assert_eq!(
        apps.iter().find(|app| app.id == second.id).unwrap().state,
        "needsConfiguration"
    );
    f.untouched();
    store.shutdown().await;
}
fn copy_fixture(source: &Path, destination: &Path) {
    fs::create_dir_all(destination.join("public")).unwrap();
    fs::create_dir_all(destination.join("server")).unwrap();
    for relative in ["panda.json", "public/index.html", "server/managed.mjs"] {
        fs::copy(source.join(relative), destination.join(relative)).unwrap();
    }
}
#[tokio::test]
async fn platform_data_and_ancestors_are_not_adopted_as_sources() {
    let f = Fixture::new("separate-backend");
    let store = f.store();
    assert!(store.register_local(&f.data).await.is_err());
    assert!(store
        .register_local(f.data.parent().unwrap())
        .await
        .is_err());
    assert!(store.list().await.unwrap().is_empty());
    f.untouched();
    store.shutdown().await;
}
#[cfg(unix)]
#[tokio::test]
async fn linked_sources_are_rejected_without_execution() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new("unlinked-backend");
    let store = f.store();
    let linked = f.source.with_extension("linked");
    symlink(&f.source, &linked).unwrap();
    assert!(store.register_local(&linked).await.is_err());
    assert!(store.list().await.unwrap().is_empty());
    f.untouched();
    store.shutdown().await;
}
