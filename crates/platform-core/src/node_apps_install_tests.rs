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
        .is_err());
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
    assert_eq!(
        fixture
            .manager
            .configuration(&installed.app_id)
            .await
            .unwrap()["state"],
        "failed"
    );
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
