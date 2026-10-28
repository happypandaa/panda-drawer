use super::*;
use std::os::unix::fs::symlink;

struct Fixture {
    _temporary: tempfile::TempDir,
    source: PathBuf,
    apps: NodeApps,
}

#[tokio::test]
async fn owned_node_source_token_outlives_borrowed_lease_without_opening_instance_mutations() {
    let fixture = Fixture::new();
    let id = fixture.register().await;
    let lease = fixture
        .apps
        .development_build_source_lease(&id)
        .await
        .unwrap();
    let owner = lease.keep_alive();
    drop(lease);
    assert_eq!(
        fixture.apps.start(&id).await.unwrap_err().code,
        "NODE_DEVELOPMENT_BUSY"
    );
    assert!(fixture.apps.delete_preview(&id).await.unwrap()["blocker"].is_null());
    assert!(fixture
        .apps
        .development_build_source_lease(&id)
        .await
        .is_err());
    drop(owner);
    fixture
        .apps
        .install_target_snapshot(Some(&id))
        .await
        .unwrap();
    fixture
        .apps
        .development_build_source_lease(&id)
        .await
        .unwrap()
        .validate()
        .unwrap();
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
    assert!(fixture.apps.delete_preview(&id).await.unwrap()["blocker"].is_null());
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
async fn deletion_waits_for_real_source_owner_and_preserves_admitted_work_and_other_instance() {
    use std::time::Duration;

    let fixture = Fixture::new();
    let id = fixture.register().await;
    let sentinel = fixture.source.join("disposable-author-sentinel");
    fs::write(&sentinel, b"external source survives stop and delete").unwrap();
    let source_before = fs::read(fixture.source.join("server/main.mjs")).unwrap();
    let lease = fixture
        .apps
        .development_build_source_lease(&id)
        .await
        .unwrap();
    let worker = lease.keep_alive();
    let preview = fixture.apps.delete_preview(&id).await.unwrap();
    let registration = fixture.apps.root.join(&id).join("registration.json");
    let registration_before = fs::read(&registration).unwrap();
    let deletion = fixture.apps.delete(
        &id,
        DeleteNodeAppInput {
            expected_revision: preview["expectedRevision"].as_str().unwrap().into(),
        },
    );
    tokio::pin!(deletion);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut deletion)
            .await
            .is_err()
    );
    let status = fixture.apps.deletion_operation_status(&id).unwrap();
    assert_eq!(status["active"], true);
    assert_eq!(status["phase"], "waiting");
    assert_eq!(status["waitingFor"], json!(["Git 或源码操作"]));
    assert!(
        fixture.apps.inner.try_lock().is_ok(),
        "waiting for a source owner must release the global table"
    );

    // The existing capability stays valid while new capabilities are sealed.
    lease.validate().unwrap();
    assert_eq!(
        fixture.apps.start(&id).await.unwrap_err().code,
        "NODE_APP_DELETING"
    );
    assert_eq!(
        fixture.apps.restart(&id).await.unwrap_err().code,
        "NODE_APP_DELETING"
    );
    assert_eq!(
        fixture
            .apps
            .development_source_lease(&id)
            .await
            .err()
            .unwrap()
            .code,
        "NODE_APP_DELETING"
    );
    assert_eq!(
        fixture
            .apps
            .development_build_source_lease(&id)
            .await
            .err()
            .unwrap()
            .code,
        "NODE_APP_DELETING"
    );
    assert_eq!(
        fixture
            .apps
            .development_restore_source_lease(&id)
            .await
            .err()
            .unwrap()
            .code,
        "NODE_APP_DELETING"
    );
    assert_eq!(
        fixture
            .apps
            .install_target_snapshot(Some(&id))
            .await
            .unwrap_err()
            .code,
        "NODE_APP_DELETING"
    );
    assert_eq!(
        fixture
            .apps
            .bind_development_source(&id, fixture.source.clone())
            .await
            .unwrap_err()
            .code,
        "NODE_APP_DELETING"
    );
    assert_eq!(
        fixture
            .apps
            .save(
                &id,
                SaveConfigurationRequest {
                    revision: 1,
                    values: BTreeMap::new(),
                    secrets: BTreeMap::new()
                }
            )
            .await
            .unwrap_err()
            .code,
        "NODE_APP_DELETING"
    );
    let mut opened = false;
    assert_eq!(
        fixture
            .apps
            .with_operation_admission(&id, "打开窗口", || {
                opened = true;
                Ok(())
            })
            .unwrap_err()
            .code,
        "NODE_APP_DELETING"
    );
    assert!(!opened, "a sealed instance must not create a late window");
    assert_eq!(
        fixture.apps.begin_delete_operation(&id).err().unwrap().code,
        "NODE_APP_DELETING"
    );
    let detail = fixture.apps.configuration(&id).await.unwrap();
    assert_eq!(detail["configuration"]["canStart"], false);
    assert!(detail["lastError"].as_str().unwrap().contains("等待"));
    assert_eq!(fs::read(&registration).unwrap(), registration_before);

    let other_source = fixture
        .source
        .with_file_name("disposable-other-author-tree");
    copy_artificial_migration_tree(&fixture.source, &other_source);
    let other_manifest = other_source.join("local-app.json");
    let mut declaration: Value =
        serde_json::from_slice(&fs::read(&other_manifest).unwrap()).unwrap();
    declaration["app"]["id"] = json!("disposable.other-author-tree");
    crate::builds::atomic_json(&other_manifest, &declaration).unwrap();
    let other = tokio::time::timeout(
        Duration::from_secs(1),
        fixture.apps.register(other_source.clone()),
    )
    .await
    .unwrap()
    .unwrap();
    let other_id = other["id"].as_str().unwrap();
    let other_registration = fixture.apps.root.join(other_id).join("registration.json");
    let other_before = fs::read(&other_registration).unwrap();
    let other_lease = tokio::time::timeout(
        Duration::from_secs(1),
        fixture.apps.development_source_lease(other_id),
    )
    .await
    .unwrap()
    .unwrap();
    other_lease.validate().unwrap();
    drop(other_lease);

    // Dropping a borrowed request cannot release the actual worker's token.
    drop(lease);
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut deletion)
            .await
            .is_err()
    );
    assert_eq!(fs::read(&registration).unwrap(), registration_before);
    assert_eq!(
        fs::read(&sentinel).unwrap(),
        b"external source survives stop and delete"
    );
    drop(worker);
    let result = tokio::time::timeout(Duration::from_secs(3), &mut deletion)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result["operationOutcome"], "deleted");
    assert!(!registration.exists());
    assert_eq!(
        fixture.apps.deletion_operation_status(&id).unwrap()["active"],
        false
    );
    assert_eq!(
        fs::read(&sentinel).unwrap(),
        b"external source survives stop and delete"
    );
    assert_eq!(
        fs::read(fixture.source.join("server/main.mjs")).unwrap(),
        source_before
    );
    assert_eq!(fs::read(&other_registration).unwrap(), other_before);
    assert!(other_source.join("disposable-author-sentinel").is_file());
    fixture
        .apps
        .development_source_lease(other_id)
        .await
        .unwrap()
        .validate()
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

struct MigratedAuthorFixture {
    fixture: Fixture,
    old: PathBuf,
    new: PathBuf,
    old_source: PathBuf,
    new_source: PathBuf,
    binding: PathBuf,
    receipt: PathBuf,
    external_binding: PathBuf,
}
fn copy_artificial_migration_tree(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let metadata = fs::symlink_metadata(entry.path()).unwrap();
        assert!(!metadata.file_type().is_symlink());
        if metadata.is_dir() {
            copy_artificial_migration_tree(&entry.path(), &target.join(entry.file_name()));
        } else {
            assert!(metadata.is_file());
            fs::copy(entry.path(), target.join(entry.file_name())).unwrap();
        }
    }
}
impl MigratedAuthorFixture {
    async fn new() -> Self {
        let fixture = Fixture::new();
        let external_id = fixture.register().await;
        drop(
            fixture
                .apps
                .development_source_lease(&external_id)
                .await
                .unwrap(),
        );
        let external_binding = fixture
            .apps
            .root
            .join(&external_id)
            .join("development-source.json");
        let base = fixture.source.parent().unwrap();
        let old = base.join("disposable-old-workspace");
        let new = base.join("disposable-new-workspace");
        let old_source = old.join("project-sources/disposable-author");
        let new_source = new.join("project-sources/disposable-author");
        copy_artificial_migration_tree(&fixture.source, &old_source);
        copy_artificial_migration_tree(&old_source, &new_source);
        let id = Uuid::new_v4().to_string();
        let binding = fixture.apps.root.join(&id).join("development-source.json");
        fs::create_dir_all(binding.parent().unwrap()).unwrap();
        let text = fs::read_to_string(old_source.join("local-app.json")).unwrap();
        let manifest: Value = serde_json::from_str(&text).unwrap();
        let record = NodeDevelopmentBinding {
            source_root: old_source.clone(),
            manifest_id: manifest["app"]["id"].as_str().unwrap().into(),
            root_identity: PersistedIdentity::capture(&old_source).unwrap(),
            registered_manifest_digest: format!("{:x}", Sha256::digest(text.as_bytes())),
            registered_manifest_text: text,
        };
        crate::builds::atomic_json(&binding, &record).unwrap();
        let receipt = new
            .join(".platform/source-publications")
            .join(&id)
            .join("publication.json");
        fs::create_dir_all(receipt.parent().unwrap()).unwrap();
        crate::builds::atomic_json(&receipt, &json!({
            "schemaVersion":1,"sourceId":format!("src_{}",Uuid::new_v4().simple()),
            "sourceTaskId":format!("task_{}",Uuid::new_v4().simple()),"sourceDigest":"a".repeat(64),
            "authorDigest":"b".repeat(64),"appId":id,"directory":"disposable-author","phase":"published",
            "payload":"payload_disposable","payloadIdentity":record.root_identity,"retainedPayloads":[]
        })).unwrap();
        Self {
            fixture,
            old,
            new,
            old_source,
            new_source,
            binding,
            receipt,
            external_binding,
        }
    }
    fn rebind(&self) -> Result<()> {
        rebind_published_after_migration(&self.fixture.apps.data_root, &self.old, &self.new)
    }
    fn binding(&self) -> NodeDevelopmentBinding {
        serde_json::from_slice(&fs::read(&self.binding).unwrap()).unwrap()
    }
    fn receipt(&self) -> Value {
        serde_json::from_slice(&fs::read(&self.receipt).unwrap()).unwrap()
    }
}

#[tokio::test]
async fn public_migration_rebinds_only_published_author_sources_and_retry_is_idempotent() {
    let fixture = MigratedAuthorFixture::new().await;
    let before = fixture.binding();
    let external_binding = fs::read(&fixture.external_binding).unwrap();
    let external_manifest = fs::read(fixture.fixture.source.join("local-app.json")).unwrap();
    let old_manifest = fs::read(fixture.old_source.join("local-app.json")).unwrap();
    assert!(before
        .root_identity
        .matches_path(&fixture.old_source)
        .unwrap());
    assert!(!before
        .root_identity
        .matches_path(&fixture.new_source)
        .unwrap());

    fixture.rebind().unwrap();
    let rebound = fixture.binding();
    assert_eq!(rebound.source_root, fixture.new_source);
    assert_eq!(rebound.manifest_id, before.manifest_id);
    assert_eq!(
        rebound.registered_manifest_digest,
        before.registered_manifest_digest
    );
    assert_eq!(
        rebound.registered_manifest_text,
        before.registered_manifest_text
    );
    rebound.validate_record().unwrap();
    assert!(rebound
        .root_identity
        .matches_path(&fixture.new_source)
        .unwrap());
    let identity: PersistedIdentity =
        serde_json::from_value(fixture.receipt()["payloadIdentity"].clone()).unwrap();
    assert!(identity.matches_path(&fixture.new_source).unwrap());
    assert_eq!(
        fs::read(&fixture.external_binding).unwrap(),
        external_binding
    );
    assert_eq!(
        fs::read(fixture.fixture.source.join("local-app.json")).unwrap(),
        external_manifest
    );
    assert_eq!(
        fs::read(fixture.old_source.join("local-app.json")).unwrap(),
        old_manifest
    );

    let binding = fs::read(&fixture.binding).unwrap();
    let receipt = fs::read(&fixture.receipt).unwrap();
    fixture.rebind().unwrap();
    assert_eq!(fs::read(&fixture.binding).unwrap(), binding);
    assert_eq!(fs::read(&fixture.receipt).unwrap(), receipt);
    assert_eq!(
        fs::read(&fixture.external_binding).unwrap(),
        external_binding
    );
}

#[tokio::test]
async fn migration_rebind_rejects_binding_receipt_and_target_identity_mismatches_without_writes() {
    for mismatch in [
        "binding-identity",
        "receipt-identity",
        "receipt-app",
        "manifest-digest",
        "target-app",
    ] {
        let fixture = MigratedAuthorFixture::new().await;
        if mismatch == "binding-identity" || mismatch == "manifest-digest" {
            let mut binding = fixture.binding();
            if mismatch == "binding-identity" {
                binding.root_identity =
                    PersistedIdentity::capture(&fixture.fixture.source).unwrap();
            } else {
                binding.registered_manifest_digest = "0".repeat(64);
            }
            crate::builds::atomic_json(&fixture.binding, &binding).unwrap();
        } else if mismatch.starts_with("receipt") {
            let mut receipt = fixture.receipt();
            if mismatch == "receipt-identity" {
                receipt["payloadIdentity"] = serde_json::to_value(
                    PersistedIdentity::capture(&fixture.fixture.source).unwrap(),
                )
                .unwrap();
            } else {
                receipt["appId"] = json!(Uuid::new_v4().to_string());
            }
            crate::builds::atomic_json(&fixture.receipt, &receipt).unwrap();
        } else {
            let path = fixture.new_source.join("local-app.json");
            let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            manifest["app"]["id"] = json!("disposable.other-migration-app");
            crate::builds::atomic_json(&path, &manifest).unwrap();
        }
        let binding = fs::read(&fixture.binding).unwrap();
        let receipt = fs::read(&fixture.receipt).unwrap();
        let external = fs::read(&fixture.external_binding).unwrap();
        assert!(
            fixture.rebind().is_err(),
            "{mismatch} must not authorize a source rebind"
        );
        assert_eq!(
            fs::read(&fixture.binding).unwrap(),
            binding,
            "{mismatch} changed the binding"
        );
        assert_eq!(
            fs::read(&fixture.receipt).unwrap(),
            receipt,
            "{mismatch} changed the receipt"
        );
        assert_eq!(fs::read(&fixture.external_binding).unwrap(), external);
    }
}

#[tokio::test]
async fn migration_rebind_retry_finishes_an_old_receipt_but_rejects_an_unrelated_identity() {
    let fixture = MigratedAuthorFixture::new().await;
    let original_receipt = fixture.receipt();
    fixture.rebind().unwrap();
    // The public migration writes the association first. An interruption at
    // that boundary leaves the copied receipt with the old author identity.
    crate::builds::atomic_json(&fixture.receipt, &original_receipt).unwrap();
    let rebound_bytes = fs::read(&fixture.binding).unwrap();
    fixture.rebind().unwrap();
    assert_eq!(fs::read(&fixture.binding).unwrap(), rebound_bytes);
    let expected: PersistedIdentity =
        serde_json::from_value(fixture.receipt()["payloadIdentity"].clone()).unwrap();
    assert!(expected.matches_path(&fixture.new_source).unwrap());

    let mut unrelated = fixture.receipt();
    unrelated["payloadIdentity"] =
        serde_json::to_value(PersistedIdentity::capture(&fixture.fixture.source).unwrap()).unwrap();
    crate::builds::atomic_json(&fixture.receipt, &unrelated).unwrap();
    let receipt = fs::read(&fixture.receipt).unwrap();
    assert!(
        fixture.rebind().is_err(),
        "a retry must not accept a third directory's unrelated receipt identity"
    );
    assert_eq!(fs::read(&fixture.binding).unwrap(), rebound_bytes);
    assert_eq!(fs::read(&fixture.receipt).unwrap(), receipt);
}
