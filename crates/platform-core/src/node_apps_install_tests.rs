//! Disposable installation-to-NodeApps integration. The interpreter path is
//! intentionally nonexistent: snapshot, configuration and publication never run it.
use super::*;
use crate::{
    builds::{atomic_json, BuildManager},
    environment::{ExecutableInfo, Toolchain},
    node_app_install::{
        InstallControl, InstalledNodeGeneration, NodeAppInstaller, PreparedNodeInstall,
        VerifiedNodeSource,
    },
    ProjectStore, RuntimePaths,
};
use std::os::unix::fs::PermissionsExt;
use tokio::sync::watch;

struct Fixture {
    _temp: tempfile::TempDir,
    paths: RuntimePaths,
    source: PathBuf,
    manager: NodeApps,
    installer: NodeAppInstaller,
    tasks: Arc<BuildManager>,
    store: Arc<ProjectStore>,
    tools: Toolchain,
}
impl Fixture {
    fn new() -> Self {
        let (temp, paths) = RuntimePaths::for_test();
        let source = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-managed-node-source");
        fs::create_dir_all(source.join("public")).unwrap();
        fs::create_dir_all(source.join("server")).unwrap();
        fs::write(
            source.join("public/index.html"),
            "disposable first generation",
        )
        .unwrap();
        fs::write(
            source.join("server/main.mjs"),
            "throw Error('preparation must not start application');",
        )
        .unwrap();
        atomic_json(&source.join("local-app.json"), &json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"disposable.managed.node","name":"可丢弃受管安装","version":"1.0.0"},
            "dataContract":{"id":"disposable.managed.node.data","version":1},
            "web":{"root":"public","entry":"index.html","routing":"files"},
            "backend":{"protocolVersion":1,"root":"server","entry":"main.mjs","nodeMajor":24},
            "configuration":{"version":1,"fields":[
                {"name":"LABEL","label":"人工标签","type":"string","required":false,"secret":false,"description":"测试普通配置","howToGet":["测试值"]},
                {"name":"API_KEY","label":"人工密钥","type":"string","required":true,"secret":true,"description":"测试密钥","howToGet":["仅人工测试占位"]}
            ]}
        })).unwrap();
        atomic_json(
            &source.join("package.json"),
            &json!({"name":"disposable-managed-node","version":"1.0.0"}),
        )
        .unwrap();
        atomic_json(&source.join("package-lock.json"), &json!({"name":"disposable-managed-node","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"disposable-managed-node","version":"1.0.0"}}})).unwrap();
        let npm = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-npm.sh");
        fs::write(
            &npm,
            "set -eu\nmkdir -p node_modules\nprintf '{}' > node_modules/.package-lock.json\n",
        )
        .unwrap();
        let tools = Toolchain {
            node: ExecutableInfo {
                path: "/bin/sh".into(),
                version: "artificial installer interpreter".into(),
            },
            npm: ExecutableInfo {
                path: npm,
                version: "artificial npm".into(),
            },
            path: "/usr/bin:/bin".into(),
        };
        let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1".into()).unwrap();
        let manager = NodeApps::new(
            paths.data_root().to_path_buf(),
            "/disposable-nonexistent-fixed-node".into(),
        )
        .unwrap();
        let installer = NodeAppInstaller::new(paths.data_root().to_path_buf()).unwrap();
        Self {
            _temp: temp,
            paths,
            source,
            manager,
            installer,
            tasks,
            store,
            tools,
        }
    }
    async fn prepare(&self, id: Option<&str>) -> (InstallTargetSnapshot, PreparedNodeInstall) {
        let target = self.manager.install_target_snapshot(id).await.unwrap();
        let task = self
            .tasks
            .create_workspace_operation("nodeInstallPrepare")
            .unwrap();
        let (_stop, _cancel, control) = control();
        let source = VerifiedNodeSource::new(
            format!("src_{}", "a".repeat(32)),
            self.source.clone(),
            "b".repeat(64),
            None,
        )
        .unwrap();
        let prepared = self
            .installer
            .prepare(
                &task.task_id,
                source,
                target.clone(),
                self.tools.clone(),
                control,
                &self.tasks,
            )
            .await
            .unwrap();
        assert_eq!(
            self.installer.target(&prepared.candidate_id).unwrap(),
            target
        );
        (target, prepared)
    }
    async fn apply(
        &self,
        target: &InstallTargetSnapshot,
        prepared: &PreparedNodeInstall,
    ) -> Result<InstalledNodeGeneration> {
        let task = self
            .tasks
            .create_workspace_operation("nodeInstallApply")
            .unwrap();
        let (_stop, _cancel, control) = control();
        self.manager
            .with_install_commit(target, &prepared.manifest, |fresh| {
                assert!(
                    self.manager.inner.try_lock().is_err(),
                    "start/save share the held commit lock"
                );
                self.installer.commit_stopped(
                    &prepared.candidate_id,
                    fresh,
                    &task.task_id,
                    &control,
                    &self.tasks,
                )
            })
            .await
    }
    fn slot(&self, id: &str) -> PathBuf {
        self.paths.data_root().join("node-apps").join(id)
    }
    fn program(&self, installed: &InstalledNodeGeneration) -> PathBuf {
        self.slot(&installed.app_id)
            .join("generations")
            .join(&installed.generation)
    }
    async fn install(&self) -> InstalledNodeGeneration {
        let (target, prepared) = self.prepare(None).await;
        self.apply(&target, &prepared).await.unwrap()
    }
    async fn shutdown(self) {
        self.manager.stop_all().await.unwrap();
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
}
fn control() -> (watch::Sender<bool>, watch::Sender<bool>, InstallControl) {
    let (stop, stopped) = watch::channel(false);
    let (cancel, cancelled) = watch::channel(false);
    (stop, cancel, InstallControl::new(stopped, cancelled))
}
fn save_request(revision: u64, label: &str, replace_secret: bool) -> SaveConfigurationRequest {
    serde_json::from_value(json!({"revision":revision,"values":{"LABEL":label},"secrets":{"API_KEY":if replace_secret { json!({"action":"replace","value":"artificial-secret-not-used"}) } else { json!({"action":"keep"}) }}})).unwrap()
}

async fn delete_input(manager: &NodeApps, id: &str) -> DeleteNodeAppInput {
    let preview = manager.delete_preview(id).await.unwrap();
    serde_json::from_value(json!({"expectedRevision":preview["expectedRevision"]})).unwrap()
}

#[tokio::test]
async fn external_delete_removes_only_private_data_and_keeps_source_other_apps_and_evidence() {
    let fixture = Fixture::new();
    let external = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = external["id"].as_str().unwrap();
    let installed = fixture.install().await;
    let other_registration =
        fs::read(fixture.slot(&installed.app_id).join("registration.json")).unwrap();
    let source = fs::read(fixture.source.join("local-app.json")).unwrap();
    let slot = fixture.slot(id);
    fs::write(
        slot.join("data/private.sqlite"),
        b"disposable private database",
    )
    .unwrap();
    fs::write(
        slot.join("runs/retained-recovery.txt"),
        b"keep recovery evidence",
    )
    .unwrap();
    fixture
        .manager
        .save(id, save_request(0, "disposable-private-config", true))
        .await
        .unwrap();
    let preview = fixture.manager.delete_preview(id).await.unwrap();
    assert_eq!(preview["ownership"], "external");
    let result = fixture
        .manager
        .delete(id, delete_input(&fixture.manager, id).await)
        .await
        .unwrap();
    assert_eq!(result["operationOutcome"], "deleted");
    for relative in ["data", "configuration", "cache", "registration.json"] {
        assert!(!slot.join(relative).exists(), "{relative} was retained");
    }
    assert_eq!(
        fs::read(slot.join("runs/retained-recovery.txt")).unwrap(),
        b"keep recovery evidence"
    );
    assert_eq!(
        fs::read(fixture.source.join("local-app.json")).unwrap(),
        source
    );
    assert_eq!(
        fs::read(fixture.slot(&installed.app_id).join("registration.json")).unwrap(),
        other_registration
    );
    let list = fixture.manager.list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], installed.app_id);
    let replacement = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    assert_ne!(replacement["id"], id);
    assert_eq!(
        replacement["configuration"]["secretStatus"]["API_KEY"],
        "missing"
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn managed_delete_removes_program_data_and_config_but_preserves_installation_history() {
    let fixture = Fixture::new();
    let installed = fixture.install().await;
    let id = &installed.app_id;
    let slot = fixture.slot(id);
    let history = fixture.paths.data_root().join("node-apps/.installations");
    fs::write(
        history.join("retained-recovery.txt"),
        b"old installation evidence",
    )
    .unwrap();
    fs::write(
        slot.join("data/private.sqlite"),
        b"disposable installed database",
    )
    .unwrap();
    let source = fs::read(fixture.source.join("local-app.json")).unwrap();
    assert_eq!(
        fixture.manager.delete_preview(id).await.unwrap()["ownership"],
        "managed"
    );
    fixture
        .manager
        .delete(id, delete_input(&fixture.manager, id).await)
        .await
        .unwrap();
    assert!(!slot.join("generations").exists());
    assert!(!slot.join("data").exists());
    assert!(!slot.join("configuration").exists());
    assert_eq!(
        fs::read(history.join("retained-recovery.txt")).unwrap(),
        b"old installation evidence"
    );
    assert_eq!(
        fs::read(fixture.source.join("local-app.json")).unwrap(),
        source
    );
    assert!(fixture.manager.list().await.unwrap().is_empty());
    fixture.shutdown().await;
}

#[tokio::test]
async fn delete_refuses_replaced_reviewed_directory_and_changed_registration() {
    let fixture = Fixture::new();
    let external = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = external["id"].as_str().unwrap();
    let slot = fixture.slot(id);
    let input = delete_input(&fixture.manager, id).await;
    let registration = fs::read(slot.join("registration.json")).unwrap();
    let mut changed = registration.clone();
    changed.push(b' ');
    fs::write(slot.join("registration.json"), changed).unwrap();
    assert_eq!(
        fixture.manager.delete(id, input).await.unwrap_err().code,
        "NODE_APP_DELETE_CHANGED"
    );
    let input = delete_input(&fixture.manager, id).await;
    let displaced = fixture.paths.data_root().join("disposable-displaced-app");
    fs::rename(&slot, &displaced).unwrap();
    fs::create_dir(&slot).unwrap();
    fs::copy(
        displaced.join("registration.json"),
        slot.join("registration.json"),
    )
    .unwrap();
    assert_eq!(
        fixture.manager.delete(id, input).await.unwrap_err().code,
        "NODE_APP_DELETE_CHANGED"
    );
    assert!(displaced.join("data").is_dir());
    assert!(slot.join("registration.json").is_file());
    fixture.shutdown().await;
}

#[tokio::test]
async fn partial_delete_blocks_config_recreation_and_restart_never_replays_removal() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let external = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = external["id"].as_str().unwrap();
    let slot = fixture.slot(id);
    let protected = fixture.source.join("original-data");
    fs::create_dir(&protected).unwrap();
    fs::write(
        protected.join("original.sqlite"),
        b"external original database",
    )
    .unwrap();
    fs::remove_dir(slot.join("data")).unwrap();
    symlink(&protected, slot.join("data")).unwrap();
    let failure = fixture
        .manager
        .delete(id, delete_input(&fixture.manager, id).await)
        .await
        .unwrap_err();
    assert_eq!(failure.details["operationOutcome"], "delete_incomplete");
    assert!(failure.message.contains("部分文件可能已永久删除"));
    assert_eq!(failure.details["platformRoot"], json!(slot));
    assert!(failure.details["deletionRecord"].as_str().is_some());
    assert!(!slot.join("configuration").exists());
    let restarted = NodeApps::new(
        fixture.paths.data_root().into(),
        "/disposable-missing-node".into(),
    )
    .unwrap();
    let state = restarted.configuration(id).await.unwrap();
    assert_eq!(state["configuration"]["canStart"], false);
    assert!(!slot.join("configuration").exists());
    assert!(restarted.start(id).await.is_err());
    assert_eq!(
        fs::read(protected.join("original.sqlite")).unwrap(),
        b"external original database"
    );
    // Stable volume identities must permit a reviewed retry after reboot even
    // when the saved device number no longer matches the current mount number.
    #[cfg(target_os = "macos")]
    {
        let receipt_path = fixture
            .paths
            .data_root()
            .join("node-apps/.deletions")
            .join(format!("{id}.json"));
        let mut receipt: Value = serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
        assert!(receipt["rootIdentity"]["volumeUuid"].is_string());
        receipt["rootIdentity"]["device"] =
            json!(receipt["rootIdentity"]["device"].as_u64().unwrap() + 100);
        atomic_json(&receipt_path, &receipt).unwrap();
    }
    fs::remove_file(slot.join("data")).unwrap();
    fs::create_dir(slot.join("data")).unwrap();
    restarted
        .delete(id, delete_input(&restarted, id).await)
        .await
        .unwrap();
    assert!(restarted.list().await.unwrap().is_empty());
    assert_eq!(
        fs::read(protected.join("original.sqlite")).unwrap(),
        b"external original database"
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn unconfirmed_runtime_marker_prevents_permanent_deletion_without_touching_data() {
    let fixture = Fixture::new();
    let external = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = external["id"].as_str().unwrap();
    let slot = fixture.slot(id);
    fs::write(slot.join("active.json"), b"retained ownership evidence").unwrap();
    fs::write(slot.join("data/private.sqlite"), b"keep data").unwrap();
    assert!(fixture
        .manager
        .delete(id, delete_input(&fixture.manager, id).await)
        .await
        .is_err());
    assert_eq!(
        fs::read(slot.join("data/private.sqlite")).unwrap(),
        b"keep data"
    );
    assert!(!fixture
        .paths
        .data_root()
        .join("node-apps/.deletions")
        .exists());
    fixture.shutdown().await;
}

#[tokio::test]
async fn unavailable_external_source_still_allows_removing_only_platform_data() {
    let fixture = Fixture::new();
    let external = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = external["id"].as_str().unwrap();
    let hidden = fixture
        .source
        .with_file_name("disposable-source-moved-by-user");
    fs::rename(&fixture.source, &hidden).unwrap();
    let original = fs::read(hidden.join("local-app.json")).unwrap();
    fixture
        .manager
        .delete(id, delete_input(&fixture.manager, id).await)
        .await
        .unwrap();
    assert!(!fixture.slot(id).join("configuration").exists());
    assert!(!fixture.slot(id).join("data").exists());
    assert_eq!(fs::read(hidden.join("local-app.json")).unwrap(), original);
    assert!(fixture.manager.list().await.unwrap().is_empty());
    fixture.shutdown().await;
}

#[tokio::test]
async fn managed_and_external_registrations_coexist_without_author_directory_dependency() {
    let fixture = Fixture::new();
    let external = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let external_id = external["id"].as_str().unwrap();
    let old_registration = fs::read(fixture.slot(external_id).join("registration.json")).unwrap();
    let installed = fixture.install().await;
    assert_ne!(installed.app_id, external_id);
    assert_eq!(
        fs::read(fixture.slot(external_id).join("registration.json")).unwrap(),
        old_registration
    );
    assert!(fixture
        .manager
        .install_target_snapshot(Some(external_id))
        .await
        .is_ok());
    assert!(
        fixture
            .manager
            .register(fixture.program(&installed))
            .await
            .is_err(),
        "external overlap rule stays intact"
    );
    fs::rename(
        &fixture.source,
        fixture.source.with_extension("retained-away"),
    )
    .unwrap();
    let apps = fixture.manager.list().await.unwrap();
    assert_eq!(apps.len(), 2);
    let managed = apps
        .iter()
        .find(|app| app["id"] == installed.app_id)
        .unwrap();
    assert_eq!(managed["state"], "stopped");
    assert_eq!(
        managed["dataContract"],
        json!({"id":"disposable.managed.node.data","version":1})
    );
    assert_eq!(managed["sourcePath"], json!(fixture.program(&installed)));
    assert_eq!(managed["installation"]["kind"], "managed");
    assert_eq!(managed["objectLayout"]["runtime"]["external"], false);
    assert_eq!(managed["configuration"]["canStart"], false);
    assert_eq!(
        managed["configuration"]["missingRequired"],
        json!(["API_KEY"])
    );
    assert_eq!(
        fixture.manager.node_path,
        PathBuf::from("/disposable-nonexistent-fixed-node")
    );
    assert_eq!(
        apps.iter().find(|app| app["id"] == external_id).unwrap()["state"],
        "failed"
    );
    assert!(!fixture.slot(&installed.app_id).join("active.json").exists());
    fixture.shutdown().await;
}

#[tokio::test]
async fn program_bytes_and_executable_mode_are_verified_before_start_or_update() {
    let fixture = Fixture::new();
    let installed = fixture.install().await;
    let entry = fixture.program(&installed).join("public/index.html");
    let original = fs::read(&entry).unwrap();
    fs::write(&entry, "artificial changed program").unwrap();
    let unavailable = fixture
        .manager
        .configuration(&installed.app_id)
        .await
        .unwrap();
    assert_eq!(unavailable["state"], "failed");
    assert!(unavailable
        .as_object()
        .unwrap()
        .contains_key("dataContract"));
    assert!(unavailable["dataContract"].is_null());
    assert!(fixture.manager.start(&installed.app_id).await.is_err());
    assert!(fixture
        .manager
        .install_target_snapshot(Some(&installed.app_id))
        .await
        .is_err());
    fs::write(&entry, original).unwrap();
    fs::set_permissions(&entry, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        fixture
            .manager
            .configuration(&installed.app_id)
            .await
            .unwrap()["state"],
        "failed"
    );
    assert!(!fixture.slot(&installed.app_id).join("active.json").exists());
    fixture.shutdown().await;
}

#[tokio::test]
async fn compatible_update_preserves_configuration_data_and_never_reads_secret_contents() {
    let fixture = Fixture::new();
    let first = fixture.install().await;
    fixture
        .manager
        .save(&first.app_id, save_request(0, "keep-this-label", true))
        .await
        .unwrap();
    let slot = fixture.slot(&first.app_id);
    let credential = fs::read_dir(slot.join("configuration/credentials"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    // Deliberately invalid UTF-8 would fail SecretVault::get. view/commit may
    // inspect only the entry's metadata; no actual credential is used here.
    fs::write(&credential, [0xff, 0xfe, 0xfd]).unwrap();
    let configuration = fs::read(slot.join("configuration/configuration.json")).unwrap();
    let data = slot.join("data/disposable-retained.sqlite");
    fs::write(&data, "artificial data retained").unwrap();
    fs::write(
        fixture.source.join("public/index.html"),
        "second generation",
    )
    .unwrap();
    let (target, candidate) = fixture.prepare(Some(&first.app_id)).await;
    assert_eq!(target.configuration_revision, 1);
    let encoded = serde_json::to_string(&target).unwrap();
    assert!(!encoded.contains("keep-this-label"));
    assert!(!encoded.contains("artificial-secret"));
    let second = fixture.apply(&target, &candidate).await.unwrap();
    assert_eq!(first.app_id, second.app_id);
    assert_ne!(first.generation, second.generation);
    assert_eq!(
        fs::read(slot.join("configuration/configuration.json")).unwrap(),
        configuration
    );
    assert_eq!(fs::read(credential).unwrap(), [0xff, 0xfe, 0xfd]);
    assert_eq!(
        fs::read_to_string(data).unwrap(),
        "artificial data retained"
    );
    assert!(fixture.program(&first).exists());
    assert_eq!(
        fixture.manager.configuration(&first.app_id).await.unwrap()["configuration"]["revision"],
        1
    );
    assert!(fixture
        .installer
        .committed(&candidate.candidate_id)
        .unwrap()
        .is_some());
    fixture.shutdown().await;
}

#[tokio::test]
async fn legacy_managed_application_keeps_configuration_and_read_access_but_cannot_update() {
    let fixture = Fixture::new();
    let manifest_path = fixture.source.join("local-app.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest.as_object_mut().unwrap().remove("dataContract");
    atomic_json(&manifest_path, &manifest).unwrap();
    let installed = fixture.install().await;
    let slot = fixture.slot(&installed.app_id);
    let registration = fs::read(slot.join("registration.json")).unwrap();
    let program_manifest = fixture.program(&installed).join("local-app.json");
    let original_manifest = fs::read(&program_manifest).unwrap();
    let data = slot.join("data/legacy-artificial.sqlite");
    fs::write(&data, "legacy data must not be inspected or rewritten").unwrap();
    fixture
        .manager
        .save(
            &installed.app_id,
            save_request(0, "legacy still configurable", true),
        )
        .await
        .unwrap();
    let detail = fixture
        .manager
        .configuration(&installed.app_id)
        .await
        .unwrap();
    assert_eq!(detail["state"], "stopped");
    assert_eq!(detail["configuration"]["revision"], 1);
    assert_eq!(detail["configuration"]["canStart"], true);
    assert!(detail.as_object().unwrap().contains_key("dataContract"));
    assert!(detail["dataContract"].is_null());
    let configuration = fs::read(slot.join("configuration/configuration.json")).unwrap();
    let target = fixture
        .manager
        .install_target_snapshot(Some(&installed.app_id))
        .await
        .unwrap();
    let error = fixture
        .manager
        .with_install_commit::<()>(&target, &manifest, |_| {
            panic!("unknown data compatibility must block before the publication closure")
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_INSTALL_DATA_COMPATIBILITY_UNKNOWN");
    assert_eq!(
        fs::read(slot.join("registration.json")).unwrap(),
        registration
    );
    assert_eq!(fs::read(&program_manifest).unwrap(), original_manifest);
    assert_eq!(
        fs::read(slot.join("configuration/configuration.json")).unwrap(),
        configuration
    );
    assert_eq!(
        fs::read_to_string(data).unwrap(),
        "legacy data must not be inspected or rewritten"
    );
    assert!(!slot.join("active.json").exists());
    fixture.shutdown().await;
}

#[tokio::test]
async fn native_commit_lock_checks_data_declaration_before_publication() {
    let fixture = Fixture::new();
    let installed = fixture.install().await;
    let target = fixture
        .manager
        .install_target_snapshot(Some(&installed.app_id))
        .await
        .unwrap();
    let mut changed = target.manifest.clone().unwrap();
    changed["dataContract"]["version"] = json!(2);
    let registration = fs::read(fixture.slot(&installed.app_id).join("registration.json")).unwrap();
    let error = fixture
        .manager
        .with_install_commit::<()>(&target, &changed, |_| {
            panic!("mismatched declaration must block under the native commit lock")
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_INSTALL_DATA_INCOMPATIBLE");
    assert_eq!(
        fs::read(fixture.slot(&installed.app_id).join("registration.json")).unwrap(),
        registration
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn changed_configuration_revision_rejects_stale_commit_without_publishing() {
    let fixture = Fixture::new();
    let first = fixture.install().await;
    let (target, candidate) = fixture.prepare(Some(&first.app_id)).await;
    fixture
        .manager
        .save(
            &first.app_id,
            save_request(0, "changed-after-preparation", false),
        )
        .await
        .unwrap();
    let before = fs::read(fixture.slot(&first.app_id).join("registration.json")).unwrap();
    assert_eq!(
        fixture.apply(&target, &candidate).await.unwrap_err().code,
        "NODE_INSTALL_CHANGED"
    );
    assert_eq!(
        fs::read(fixture.slot(&first.app_id).join("registration.json")).unwrap(),
        before
    );
    assert!(!fixture
        .program(&InstalledNodeGeneration {
            app_id: first.app_id.clone(),
            generation: candidate.generation.clone(),
            candidate_id: candidate.candidate_id.clone(),
            operation_outcome: "not-committed".into()
        })
        .exists());
    fixture.shutdown().await;
}

#[tokio::test]
async fn active_marker_and_shutdown_prevent_publication_without_running_candidate_code() {
    let fixture = Fixture::new();
    let first = fixture.install().await;
    let (target, candidate) = fixture.prepare(Some(&first.app_id)).await;
    let marker = fixture.slot(&first.app_id).join("active.json");
    fs::write(&marker, "artificial interrupted runtime evidence").unwrap();
    assert_eq!(
        fixture.apply(&target, &candidate).await.unwrap_err().code,
        "NODE_INSTALL_BUSY"
    );
    assert!(marker.exists());
    fs::remove_file(marker).unwrap();
    fixture.manager.stop_all().await.unwrap();
    assert_eq!(
        fixture.apply(&target, &candidate).await.unwrap_err().code,
        "NODE_INSTALL_BUSY"
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn managed_registration_cannot_supply_escape_generation_or_source_path() {
    let fixture = Fixture::new();
    let installed = fixture.install().await;
    let path = fixture.slot(&installed.app_id).join("registration.json");
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut escaped = original.clone();
    escaped["generation"] = json!("../../disposable-source");
    atomic_json(&path, &escaped).unwrap();
    assert!(fixture.manager.list().await.is_err());
    let mut absolute = original;
    absolute["sourcePath"] = json!(fixture.source);
    atomic_json(&path, &absolute).unwrap();
    assert!(fixture.manager.list().await.is_err());
    assert!(!fixture.slot(&installed.app_id).join("active.json").exists());
    fixture.shutdown().await;
}

#[tokio::test]
async fn external_source_promotes_to_managed_generation_without_moving_author_git_or_data() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.source.join(".git")).unwrap();
    fs::write(
        fixture.source.join(".git/author-history-evidence"),
        b"one original repository",
    )
    .unwrap();
    let external = fixture
        .manager
        .register(fixture.source.clone())
        .await
        .unwrap();
    let id = external["id"].as_str().unwrap();
    // Explicitly establish the original source declaration before editing it.
    drop(fixture.manager.development_source_lease(id).await.unwrap());
    fixture
        .manager
        .save(id, save_request(0, "retained local label", true))
        .await
        .unwrap();
    let registration = fs::read(fixture.slot(id).join("registration.json")).unwrap();
    fs::write(
        fixture.slot(id).join("data/disposable.sqlite"),
        b"retained business data",
    )
    .unwrap();
    let configuration =
        fs::read(fixture.slot(id).join("configuration/configuration.json")).unwrap();
    let mut changed: Value =
        serde_json::from_slice(&fs::read(fixture.source.join("local-app.json")).unwrap()).unwrap();
    changed["app"]["version"] = json!("1.1.0");
    atomic_json(&fixture.source.join("local-app.json"), &changed).unwrap();
    let edited_source = fixture.manager.configuration(id).await.unwrap();
    assert_eq!(edited_source["state"], "failed");
    assert_eq!(
        edited_source["configuration"]["values"]["LABEL"],
        "retained local label"
    );
    assert_eq!(edited_source["fields"].as_array().unwrap().len(), 2);
    assert_eq!(edited_source["configuration"]["canStart"], false);
    assert!(edited_source["lastError"]
        .as_str()
        .unwrap()
        .contains("构建并更新"));
    let (target, prepared) = fixture.prepare(Some(id)).await;
    assert_eq!(
        target.external_registration,
        Some(serde_json::from_slice(&registration).unwrap())
    );
    let installed = fixture.apply(&target, &prepared).await.unwrap();
    assert_eq!(installed.app_id, id);
    assert_eq!(
        fs::read(fixture.slot(id).join("data/disposable.sqlite")).unwrap(),
        b"retained business data"
    );
    assert_eq!(
        fs::read(fixture.slot(id).join("configuration/configuration.json")).unwrap(),
        configuration
    );
    assert_eq!(
        fs::read(fixture.source.join(".git/author-history-evidence")).unwrap(),
        b"one original repository"
    );
    assert!(!fixture.program(&installed).join(".git").exists());
    let detail = fixture.manager.configuration(id).await.unwrap();
    assert_eq!(detail["installation"]["kind"], "managed");
    assert_eq!(
        detail["configuration"]["values"]["LABEL"],
        "retained local label"
    );
    assert_eq!(detail["development"]["sourcePath"], json!(fixture.source));
    assert_eq!(detail["objectLayout"]["source"]["state"], "available");
    assert_eq!(detail["objectLayout"]["build"]["state"], "platform-build");
    assert_eq!(detail["objectLayout"]["versioning"]["state"], "source-git");
    let lease = fixture.manager.development_source_lease(id).await.unwrap();
    assert_eq!(lease.source_root(), fixture.source);
    assert!(!lease.source().runtime_is_source);
    drop(lease);
    fs::rename(
        &fixture.source,
        fixture.source.with_extension("retained-away"),
    )
    .unwrap();
    let missing_source = fixture.manager.configuration(id).await.unwrap();
    assert_eq!(missing_source["state"], "stopped");
    assert_eq!(missing_source["objectLayout"]["source"]["state"], "missing");
    assert_eq!(
        missing_source["objectLayout"]["build"]["state"],
        "unavailable"
    );
    assert!(fixture.manager.development_source_lease(id).await.is_err());
    fixture.shutdown().await;
}

#[tokio::test]
async fn managed_install_binds_an_explicit_matching_author_tree_and_rejects_runtime_copy() {
    let fixture = Fixture::new();
    let installed = fixture.install().await;
    let id = &installed.app_id;
    let registration = fs::read(fixture.slot(id).join("registration.json")).unwrap();
    assert!(fixture.manager.development_source_lease(id).await.is_err());
    assert!(fixture
        .manager
        .bind_development_source(id, fixture.program(&installed))
        .await
        .is_err());
    let detail = fixture
        .manager
        .bind_development_source(id, fixture.source.clone())
        .await
        .unwrap();
    assert_eq!(detail["state"], "stopped");
    assert_eq!(detail["development"]["sourcePath"], json!(fixture.source));
    assert_eq!(detail["objectLayout"]["build"]["state"], "platform-build");
    assert_eq!(
        fs::read(fixture.slot(id).join("registration.json")).unwrap(),
        registration
    );
    let lease = fixture.manager.development_source_lease(id).await.unwrap();
    assert_eq!(lease.source_root(), fixture.source);
    assert!(!lease.source().runtime_is_source);
    drop(lease);
    fixture.shutdown().await;
}

#[tokio::test]
async fn retained_node_source_restore_blocks_new_build_binding_installation_and_deletion_after_reload(
) {
    let fixture = Fixture::new();
    let installed = fixture.install().await;
    let id = &installed.app_id;
    fixture
        .manager
        .bind_development_source(id, fixture.source.clone())
        .await
        .unwrap();
    let registration = fs::read(fixture.slot(id).join("registration.json")).unwrap();
    let active = fixture.slot(id).join("active.json");
    fs::write(&active, b"retained artificial interrupted runtime").unwrap();
    assert_eq!(
        fixture
            .manager
            .development_restore_source_lease(id)
            .await
            .err()
            .unwrap()
            .code,
        "NODE_SOURCE_RESTORE_RUNTIME_ACTIVE"
    );
    fs::remove_file(active).unwrap();
    let lease = fixture
        .manager
        .development_restore_source_lease(id)
        .await
        .unwrap();
    assert_eq!(
        fixture.manager.start(id).await.unwrap_err().code,
        "NODE_DEVELOPMENT_BUSY"
    );
    drop(lease);
    let interrupted = fixture
        .source
        .join(".git/local-projects-node-restores")
        .join(format!("restore_{}", Uuid::new_v4()));
    fs::create_dir_all(&interrupted).unwrap();
    fs::write(
        interrupted.join("journal.json"),
        b"retained artificial unreadable source-restore evidence",
    )
    .unwrap();
    let reloaded = NodeApps::new(
        fixture.paths.data_root().into(),
        fixture.manager.node_path.clone(),
    )
    .unwrap();
    assert!(reloaded.development_build_source_lease(id).await.is_err());
    assert!(reloaded
        .bind_development_source(id, fixture.source.clone())
        .await
        .is_err());
    assert!(reloaded.install_target_snapshot(Some(id)).await.is_err());
    assert!(reloaded.delete_preview(id).await.is_err());
    let control = fixture.slot(id).join("source-restore.json");
    fs::write(&control, b"retained artificial unreadable instance control").unwrap();
    let held_git = fixture.source.with_extension("retained-git-evidence");
    fs::rename(fixture.source.join(".git"), &held_git).unwrap();
    assert!(reloaded.development_build_source_lease(id).await.is_err());
    assert!(reloaded
        .bind_development_source(id, fixture.source.clone())
        .await
        .is_err());
    assert!(reloaded.install_target_snapshot(Some(id)).await.is_err());
    assert!(reloaded.delete_preview(id).await.is_err());
    assert!(control.is_file());
    fs::rename(held_git, fixture.source.join(".git")).unwrap();
    // Recovery keeps its own guarded entry; ordinary read-only source information
    // and stopping remain available without clearing or guessing the bad journal.
    drop(reloaded.development_restore_source_lease(id).await.unwrap());
    reloaded.stop_runtime(id).await.unwrap();
    assert_eq!(
        fs::read(fixture.slot(id).join("registration.json")).unwrap(),
        registration
    );
    assert!(interrupted.join("journal.json").is_file());
    fixture.shutdown().await;
}
