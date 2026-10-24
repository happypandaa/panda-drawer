use super::*;
use std::os::unix::fs::symlink;

struct Fixture {
    _temporary: tempfile::TempDir,
    source: PathBuf,
    apps: NodeApps,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("disposable-single-author-tree");
        fs::create_dir_all(source.join("public")).unwrap();
        fs::create_dir_all(source.join("server")).unwrap();
        fs::write(source.join("public/index.html"), "disposable").unwrap();
        fs::write(
            source.join("server/main.mjs"),
            "throw Error('not executed')",
        )
        .unwrap();
        crate::builds::atomic_json(
            &source.join("local-app.json"),
            &json!({
                "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
                "dataContract":{"id":"disposable.node-development-data","version":1},
                "app":{"id":"disposable.one-author-tree","name":"可丢弃源码绑定","version":"1.0.0"},
                "web":{"root":"public","entry":"index.html","routing":"files"},
                "backend":{"protocolVersion":1,"root":"server","entry":"main.mjs","nodeMajor":24}
            }),
        )
        .unwrap();
        let data_root = root.join("disposable-platform-data");
        fs::create_dir(&data_root).unwrap();
        let apps = NodeApps::new(data_root, "/nonexistent-disposable-node".into()).unwrap();
        Self {
            _temporary: temporary,
            source,
            apps,
        }
    }
    async fn register(&self) -> String {
        self.apps.register(self.source.clone()).await.unwrap()["id"]
            .as_str()
            .unwrap()
            .into()
    }
}

#[tokio::test]
async fn source_operation_does_not_hold_global_mutex_and_blocks_instance_mutations() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let lease = fixture.apps.development_source_lease(&id).await.unwrap();
    assert_eq!(lease.source_root(), fixture.source);
    assert!(lease.source().runtime_is_source);
    assert!(
        fixture.apps.inner.try_lock().is_ok(),
        "a long build cannot block all runtime management"
    );
    assert_eq!(
        fixture.apps.start(&id).await.unwrap_err().code,
        "NODE_DEVELOPMENT_BUSY"
    );
    assert_eq!(
        fixture
            .apps
            .install_target_snapshot(Some(&id))
            .await
            .unwrap_err()
            .code,
        "NODE_DEVELOPMENT_BUSY"
    );
    assert!(fixture.apps.development_source_lease(&id).await.is_err());
    assert_eq!(
        fixture.apps.delete_preview(&id).await.unwrap_err().code,
        "NODE_DEVELOPMENT_BUSY"
    );
    assert_eq!(fixture.apps.list().await.unwrap().len(), 1);
    fixture.apps.stop_runtime(&id).await.unwrap();
    lease.validate().unwrap();
    drop(lease);
    fixture
        .apps
        .install_target_snapshot(Some(&id))
        .await
        .unwrap();
}

#[tokio::test]
async fn source_binding_rejects_replacement_and_another_application_identity() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let lease = fixture.apps.development_source_lease(&id).await.unwrap();
    let original = lease.source().clone();
    drop(lease);
    let retained = fixture.source.with_extension("retained-original");
    fs::rename(&fixture.source, &retained).unwrap();
    symlink(&retained, &fixture.source).unwrap();
    assert!(original.validate().is_err());
    assert!(fixture.apps.development_source_lease(&id).await.is_err());
    fs::remove_file(&fixture.source).unwrap();
    fs::rename(&retained, &fixture.source).unwrap();
    let path = fixture.source.join("local-app.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["app"]["id"] = json!("disposable.other-app");
    crate::builds::atomic_json(&path, &manifest).unwrap();
    assert_eq!(
        original.validate().unwrap_err().code,
        "NODE_DEVELOPMENT_SOURCE_CHANGED"
    );
    assert!(fixture
        .apps
        .install_target_snapshot(Some(&id))
        .await
        .is_err());
}

#[tokio::test]
async fn external_target_retains_registered_manifest_while_source_version_changes() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    drop(fixture.apps.development_source_lease(&id).await.unwrap());
    let prior = fs::read(fixture.apps.root.join(&id).join("registration.json")).unwrap();
    let path = fixture.source.join("local-app.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["app"]["version"] = json!("2.0.0");
    crate::builds::atomic_json(&path, &manifest).unwrap();
    let target = fixture
        .apps
        .install_target_snapshot(Some(&id))
        .await
        .unwrap();
    target.validate().unwrap();
    assert!(target.generation.is_none());
    assert_eq!(target.manifest.as_ref().unwrap()["app"]["version"], "1.0.0");
    assert_eq!(
        target.external_registration,
        Some(serde_json::from_slice(&prior).unwrap())
    );
    assert_eq!(
        target.development_source.as_ref().unwrap().source_root,
        fixture.source
    );
    node_app_install::compatible(&target, &manifest).unwrap();
    fixture
        .apps
        .development_source_lease(&id)
        .await
        .unwrap()
        .validate()
        .unwrap();
}

#[tokio::test]
async fn build_source_lease_checks_stopped_state_atomically_and_releases_on_drop() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let marker = fixture.apps.root.join(&id).join("active.json");
    fs::write(&marker, b"retained disposable interrupted run").unwrap();
    assert_eq!(
        fixture
            .apps
            .development_build_source_lease(&id)
            .await
            .err()
            .unwrap()
            .code,
        "NODE_INSTALL_BUSY"
    );
    let read = fixture.apps.development_source_lease(&id).await.unwrap();
    read.validate().unwrap();
    drop(read);
    fs::remove_file(marker).unwrap();
    let build = fixture
        .apps
        .development_build_source_lease(&id)
        .await
        .unwrap();
    assert_eq!(
        fixture.apps.start(&id).await.unwrap_err().code,
        "NODE_DEVELOPMENT_BUSY"
    );
    drop(build);
    fixture
        .apps
        .install_target_snapshot(Some(&id))
        .await
        .unwrap();
}

#[tokio::test]
async fn ordinary_detail_and_configuration_reads_never_create_a_development_binding() {
    let fixture = Fixture::new();
    let registered = fixture.apps.register(fixture.source.clone()).await.unwrap();
    let id = registered["id"].as_str().unwrap();
    let path = fixture.apps.root.join(id).join("development-source.json");
    assert!(!path.exists());
    fixture.apps.list().await.unwrap();
    fixture.apps.configuration(id).await.unwrap();
    assert!(!path.exists());
    drop(fixture.apps.development_source_lease(id).await.unwrap());
    assert!(path.is_file());
}

#[tokio::test]
async fn source_restore_requires_a_separate_runtime_and_releases_its_failed_lease() {
    let fixture = Fixture::new();
    let registered = fixture.apps.register(fixture.source.clone()).await.unwrap();
    let id = registered["id"].as_str().unwrap();
    assert_eq!(
        fixture
            .apps
            .development_restore_source_lease(id)
            .await
            .err()
            .unwrap()
            .code,
        "NODE_SOURCE_RESTORE_RUNTIME_ACTIVE"
    );
    drop(fixture.apps.development_source_lease(id).await.unwrap());
}
