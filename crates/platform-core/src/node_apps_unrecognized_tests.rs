//! A registration this version cannot read (older format or damaged) must not
//! block listing, adding or deleting other applications, and must stay deletable.
use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    apps: NodeApps,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let (temporary, paths) = crate::RuntimePaths::for_test();
        let root = temporary.path().canonicalize().unwrap();
        let apps = NodeApps::new(
            paths.data_root().into(),
            "/disposable-missing-runtime-node".into(),
        )
        .unwrap();
        Self {
            _temporary: temporary,
            apps,
            root,
        }
    }
    fn source(&self, name: &str) -> PathBuf {
        let source = self.root.join(name);
        fs::create_dir_all(source.join("public")).unwrap();
        fs::create_dir_all(source.join("server")).unwrap();
        fs::write(source.join("public/index.html"), "disposable").unwrap();
        fs::write(
            source.join("server/main.mjs"),
            "throw Error('must not execute');",
        )
        .unwrap();
        fs::write(
            source.join("panda.json"),
            serde_json::to_vec(&json!({
                "manifestVersion":1,"runtime":"node","app":{"name":name},
                "web":{"root":"public","entry":"index.html"},
                "backend":{"root":"server","entry":"main.mjs","protocolVersion":1,"nodeMajor":24}
            }))
            .unwrap(),
        )
        .unwrap();
        source
    }
    /// An application registered before the fixed-address format: version 3,
    /// no address and no address reservation.
    fn outdated(&self, source: &Path) -> String {
        let id = Uuid::new_v4().to_string();
        let slot = self.apps.root.join(&id);
        fs::create_dir_all(&slot).unwrap();
        fs::write(
            slot.join("registration.json"),
            serde_json::to_vec(
                &json!({"version":3,"id":id,"sourcePath":source,"manifestDigest":"a".repeat(64)}),
            )
            .unwrap(),
        )
        .unwrap();
        id
    }
}

#[tokio::test]
async fn an_outdated_registration_is_listed_separately_and_never_blocks_other_apps() {
    let fixture = Fixture::new();
    let old_source = fixture.source("disposable-outdated");
    let old_id = fixture.outdated(&old_source);
    let damaged = fixture.apps.root.join(Uuid::new_v4().to_string());
    fs::create_dir_all(&damaged).unwrap();
    fs::write(damaged.join("registration.json"), b"{ not json").unwrap();

    let list = fixture.apps.list().await.unwrap();
    assert_eq!(list.len(), 2);
    let outdated = list.iter().find(|item| item["id"] == old_id).unwrap();
    assert_eq!(outdated["registrationUnrecognized"], true);
    assert_eq!(
        outdated["sourcePath"],
        json!(old_source.display().to_string())
    );
    assert_eq!(outdated["configuration"]["canStart"], false);
    assert!(outdated["lastError"]
        .as_str()
        .unwrap()
        .contains("请删除后重新添加"));

    let status = fixture.apps.status(&old_id).await.unwrap();
    assert_eq!(status["registrationUnrecognized"], true);
    assert!(fixture.apps.start(&old_id).await.is_err());

    let fresh = fixture.source("disposable-fresh");
    let added = fixture.apps.register(fresh.clone()).await.unwrap();
    assert_eq!(added["sourcePath"], json!(fresh));
    assert_eq!(fixture.apps.list().await.unwrap().len(), 3);
}

#[tokio::test]
async fn the_folder_of_an_outdated_registration_is_not_added_twice() {
    let fixture = Fixture::new();
    let source = fixture.source("disposable-reused");
    fixture.outdated(&source);
    assert_eq!(
        fixture.apps.register(source).await.unwrap_err().code,
        "NODE_REGISTRATION_UNRECOGNIZED"
    );
}

#[tokio::test]
async fn an_outdated_registration_can_be_previewed_for_deletion_preserving_the_user_folder() {
    let fixture = Fixture::new();
    let source = fixture.source("disposable-outdated-delete");
    let id = fixture.outdated(&source);
    let preview = fixture.apps.delete_preview(&id).await.unwrap();
    assert_eq!(preview["ownership"], "external");
    assert_eq!(preview["preservedPaths"], json!([source]));
    assert_eq!(
        preview["deletePaths"][0],
        json!(fixture.apps.root.join(&id))
    );
}

#[tokio::test]
async fn an_ownership_index_with_retired_fields_does_not_block_deletion() {
    let fixture = Fixture::new();
    let source = fixture.source("disposable-retired-ownership");
    let id = fixture.outdated(&source);
    let owner = fixture
        .apps
        .data_root
        .join("node-project-ownership")
        .join(&id);
    fs::create_dir_all(&owner).unwrap();
    fs::write(
        owner.join("ownership.json"),
        serde_json::to_vec(&json!({
            "version":1,"appId":id,"artifacts":{},"tasks":[],"generatedSource":null,"retainedMaterials":[],
            "sourceRestores":[{"sourceRoot":source,"sourceIdentity":{},"directories":{}}]
        }))
        .unwrap(),
    )
    .unwrap();
    let preview = fixture.apps.delete_preview(&id).await.unwrap();
    assert_eq!(preview["ownership"], "external");
    assert_eq!(preview["preservedPaths"], json!([source]));
}
