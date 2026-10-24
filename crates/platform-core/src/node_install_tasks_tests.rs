//! Coordinator coverage uses disposable local capture and artificial npm only.
//! No downloaded repository, user application or real credential is accessed.
use super::*;
use crate::{
    builds::atomic_json,
    git_environment::GitEnvironmentManager,
    source_acquisition::{CaptureLocalSourceInput, SourceCandidate},
};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf, time::Duration};

struct Fixture {
    _temp: tempfile::TempDir,
    paths: RuntimePaths,
    source: PathBuf,
    npm: PathBuf,
    store: Arc<ProjectStore>,
    builds: Arc<BuildManager>,
    sources: Arc<SourceAcquisitionManager>,
    nodes: Arc<NodeApps>,
    installs: Arc<NodeInstallTasks>,
    stop: watch::Sender<bool>,
}
impl Fixture {
    fn new(npm_body: &str) -> Self {
        let (temp, paths) = RuntimePaths::for_test();
        let base = temp.path().canonicalize().unwrap();
        let source = base.join("disposable-coordinator-source");
        fs::create_dir_all(source.join("public")).unwrap();
        fs::create_dir_all(source.join("server")).unwrap();
        fs::write(
            source.join("public/index.html"),
            "disposable coordinator program",
        )
        .unwrap();
        fs::write(
            source.join("server/main.mjs"),
            "throw Error('must not start during installation');",
        )
        .unwrap();
        atomic_json(
            &source.join("local-app.json"),
            &json!({
                "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
                "app":{"id":"disposable.coordinator","name":"可丢弃任务协调样本","version":"1.0.0"},
                "dataContract":{"id":"disposable.coordinator.data","version":1},
                "web":{"root":"public","entry":"index.html","routing":"files"},
                "backend":{"root":"server","entry":"main.mjs","nodeMajor":24,"protocolVersion":1}
            }),
        )
        .unwrap();
        atomic_json(
            &source.join("package.json"),
            &json!({"name":"disposable-coordinator","version":"1.0.0"}),
        )
        .unwrap();
        atomic_json(&source.join("package-lock.json"), &json!({"name":"disposable-coordinator","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"disposable-coordinator","version":"1.0.0"}}})).unwrap();
        let bin = base.join("disposable-toolchain/bin");
        fs::create_dir_all(&bin).unwrap();
        let node = bin.join("node");
        fs::write(&node, "#!/bin/sh\nif [ \"$1\" = --version ]; then printf 'v24.22.0\\n'; exit 0; fi\nexec /bin/sh \"$@\"\n").unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
        let npm = bin.join("npm");
        fs::write(&npm, script(npm_body)).unwrap();
        atomic_json(
            &paths.data_root().join("toolchain.json"),
            &json!({"schemaVersion":1,"nodePath":node}),
        )
        .unwrap();
        let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
        let builds =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1".into()).unwrap();
        let sources = SourceAcquisitionManager::new(
            paths.clone(),
            store.clone(),
            builds.clone(),
            GitEnvironmentManager::new(paths.data_root().to_path_buf(), None),
        )
        .unwrap();
        let nodes = Arc::new(
            NodeApps::new(
                paths.data_root().to_path_buf(),
                "/disposable-must-not-start-fixed-node".into(),
            )
            .unwrap(),
        );
        let installs = NodeInstallTasks::new(
            paths.clone(),
            store.clone(),
            builds.clone(),
            Ok(sources.clone()),
        );
        let (stop, _) = watch::channel(false);
        Self {
            _temp: temp,
            paths,
            source,
            npm,
            store,
            builds,
            sources,
            nodes,
            installs,
            stop,
        }
    }
    async fn capture(&self) -> SourceCandidate {
        let task = self
            .sources
            .capture(
                CaptureLocalSourceInput {
                    source_path: self.source.clone(),
                    name: Some("disposable coordinator capture".into()),
                },
                self.stop.subscribe(),
            )
            .await
            .unwrap();
        let task = self.wait_task(&task.task_id).await;
        assert!(task.error.is_none(), "{:?}", task.error);
        let id = task.result.unwrap()["sourceId"]
            .as_str()
            .unwrap()
            .to_owned();
        self.sources.info(&id).unwrap()
    }
    async fn submit(&self, source: &SourceCandidate, target: Option<String>) -> TaskRecord {
        self.submit_build(source, target, false).await
    }
    async fn submit_build(
        &self,
        source: &SourceCandidate,
        target: Option<String>,
        build_source: bool,
    ) -> TaskRecord {
        self.installs
            .prepare(
                self.nodes.clone(),
                PrepareNodeInstallInput {
                    build_source,
                    source_id: source.source_id.clone(),
                    expected_source_digest: source.content_digest.clone().unwrap(),
                    target_app_id: target,
                },
                self.stop.subscribe(),
            )
            .await
            .unwrap()
    }
    async fn wait_task(&self, id: &str) -> TaskRecord {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let task = self.builds.task(id).unwrap();
                if task.status.is_final() && self.installs.active.lock().unwrap().is_empty() {
                    return task;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn ready(&self, source: &SourceCandidate) -> String {
        let task = self.submit(source, None).await;
        let task = self.wait_task(&task.task_id).await;
        assert!(task.error.is_none(), "{:?}", task.error);
        task.result.unwrap()["candidate"]["candidateId"]
            .as_str()
            .unwrap()
            .into()
    }
    async fn marker(&self) -> PathBuf {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(entries) =
                    fs::read_dir(self.paths.data_root().join("node-apps/.installations"))
                {
                    for entry in entries.flatten() {
                        let path = entry.path().join("program/node_modules/disposable-pid");
                        if fs::read_to_string(&path).is_ok_and(|value| value.parse::<i32>().is_ok())
                        {
                            return path;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn shutdown(self) {
        let _ = self.stop.send(true);
        self.installs.shutdown().await;
        self.sources.shutdown().await;
        self.nodes.stop_all().await.unwrap();
        self.builds.shutdown().await;
        self.store.shutdown().await;
    }
}
fn script(body: &str) -> String {
    format!("set -eu\nif [ \"$1\" = --version ]; then printf '11.9.0\\n'; exit 0; fi\nmkdir -p node_modules\nprintf '{{}}' > node_modules/.package-lock.json\n{body}\n")
}
const SLOW_NPM: &str = "printf '%s' \"$$\" > node_modules/disposable-pid; /bin/sleep 60 & wait";

fn declare_build(fixture: &Fixture) {
    let path = fixture.source.join("local-app.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["development"] =
        json!({"sourceDir":".","build":{"kind":"npm","script":"build"},"outputDir":"release"});
    atomic_json(&path, &manifest).unwrap();
    atomic_json(&fixture.source.join("package.json"),&json!({"name":"disposable-coordinator","version":"1.0.0","scripts":{"build":"node build.mjs"}})).unwrap();
    fs::write(fixture.source.join("build.mjs"),"import fs from 'node:fs';\nfs.mkdirSync('release');\nfor(const path of ['local-app.json','package.json','package-lock.json'])fs.copyFileSync(path,'release/'+path);\nfor(const path of ['public','server'])fs.cpSync(path,'release/'+path,{recursive:true});\nfs.writeFileSync('release/public/disposable-build-proof.txt','explicit build completed');\n").unwrap();
}

const COPY_BUILD:&str="if [ \"$1\" = run ]; then mkdir release; cp local-app.json package.json package-lock.json release/; cp -R public server release/; printf 'explicit build completed' > release/public/disposable-build-proof.txt; fi";

#[tokio::test]
async fn source_build_requires_explicit_authority_before_creating_any_build_or_install_candidate() {
    let fixture = Fixture::new("exit 77");
    declare_build(&fixture);
    let source = fixture.capture().await;
    assert!(source.inspection.as_ref().unwrap().requires_build);
    let task = fixture.submit(&source, None).await;
    let done = fixture.wait_task(&task.task_id).await;
    assert_eq!(done.error.unwrap().code, "NODE_INSTALL_BUILD_REQUIRED");
    assert!(!fixture
        .paths
        .data_root()
        .join("node-development-builds")
        .exists());
    assert!(fixture.installs.list().unwrap()["candidates"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(fixture.nodes.list().await.unwrap().is_empty());
    assert_eq!(
        fixture
            .sources
            .verify_ready(&source.source_id, source.content_digest.as_ref().unwrap())
            .unwrap()
            .content_digest,
        source.content_digest
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn incompatible_data_contract_never_runs_an_explicit_source_build() {
    let fixture = Fixture::new("exit 79");
    let app = fixture
        .nodes
        .register(fixture.source.clone())
        .await
        .unwrap();
    let app_id = app["id"].as_str().unwrap().to_owned();
    drop(
        fixture
            .nodes
            .development_source_lease(&app_id)
            .await
            .unwrap(),
    );
    let registration = slot(&fixture, &app_id).join("registration.json");
    let previous = fs::read(&registration).unwrap();
    declare_build(&fixture);
    let path = fixture.source.join("local-app.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["dataContract"]["version"] = json!(2);
    atomic_json(&path, &manifest).unwrap();
    let source = fixture.capture().await;
    let task = fixture.submit_build(&source, Some(app_id), true).await;
    let done = fixture.wait_task(&task.task_id).await;
    assert_eq!(done.error.unwrap().code, "NODE_INSTALL_DATA_INCOMPATIBLE");
    assert!(!fixture
        .paths
        .data_root()
        .join("node-development-builds")
        .exists());
    assert!(fixture.installs.list().unwrap()["candidates"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(fs::read(registration).unwrap(), previous);
    fixture.shutdown().await;
}

#[tokio::test]
async fn old_source_inspection_derives_build_requirement_without_rewriting_records_or_running_code()
{
    let fixture = Fixture::new("exit 77");
    declare_build(&fixture);
    let source = fixture.capture().await;
    let path = source.evidence_path.join("source.json");
    let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["inspection"]
        .as_object_mut()
        .unwrap()
        .remove("requiresBuild");
    atomic_json(&path, &record).unwrap();
    let unchanged = fs::read(&path).unwrap();
    let refreshed = fixture.sources.info(&source.source_id).unwrap();
    assert!(refreshed.inspection.unwrap().requires_build);
    assert_eq!(fs::read(&path).unwrap(), unchanged);
    assert!(!fixture
        .paths
        .data_root()
        .join("node-development-builds")
        .exists());
    let verified = fixture
        .sources
        .verify_ready(&source.source_id, source.content_digest.as_ref().unwrap())
        .unwrap();
    assert!(verified.inspection.unwrap().requires_build);
    fs::write(source.source_path.unwrap().join("local-app.json"), b"{}").unwrap();
    assert!(
        fixture.sources.info(&source.source_id).is_err(),
        "an edited legacy payload must not supply build authority"
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn explicit_source_build_prepares_output_but_keeps_selected_source_identity_and_digest() {
    let fixture = Fixture::new(COPY_BUILD);
    declare_build(&fixture);
    let source = fixture.capture().await;
    let task = fixture.submit_build(&source, None, true).await;
    let done = fixture.wait_task(&task.task_id).await;
    assert!(done.error.is_none(), "{done:?}");
    let result = done.result.unwrap();
    let candidate = &result["candidate"];
    assert_eq!(candidate["provenance"]["sourceId"], source.source_id);
    assert_eq!(
        candidate["provenance"]["sourceDigest"],
        source.content_digest.as_deref().unwrap()
    );
    assert!(candidate["provenance"]["buildSourceDigest"]
        .as_str()
        .is_some_and(|value| value.len() == 64));
    let program = fixture
        .paths
        .data_root()
        .join("node-apps/.installations")
        .join(candidate["candidateId"].as_str().unwrap())
        .join("program");
    assert_eq!(
        fs::read_to_string(program.join("public/disposable-build-proof.txt")).unwrap(),
        "explicit build completed"
    );
    assert!(!fixture.source.join("release").exists());
    assert!(!source
        .source_path
        .as_ref()
        .unwrap()
        .join("release")
        .exists());
    assert!(fixture.nodes.list().await.unwrap().is_empty());
    assert_eq!(
        fixture.sources.list().unwrap().len(),
        1,
        "build must not create a second source identity"
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn failed_explicit_source_build_retains_its_operation_and_never_prepares_an_installed_generation(
) {
    let fixture=Fixture::new("if [ \"$1\" = run ]; then printf 'failure evidence' > disposable-failed-build.txt; exit 17; fi");
    declare_build(&fixture);
    let app = fixture
        .nodes
        .register(fixture.source.clone())
        .await
        .unwrap();
    let app_id = app["id"].as_str().unwrap().to_owned();
    let registration = slot(&fixture, &app_id).join("registration.json");
    let previous = fs::read(&registration).unwrap();
    let data = slot(&fixture, &app_id).join("data/disposable-existing-data.txt");
    fs::write(&data, "preserved existing application data").unwrap();
    let source = fixture.capture().await;
    let task = fixture
        .submit_build(&source, Some(app_id.clone()), true)
        .await;
    let done = fixture.wait_task(&task.task_id).await;
    assert!(done.error.is_some());
    let operation = fixture
        .paths
        .data_root()
        .join("node-development-builds")
        .join(&task.task_id);
    assert_eq!(
        fs::read_to_string(operation.join("repository/disposable-failed-build.txt")).unwrap(),
        "failure evidence"
    );
    assert!(fixture.installs.list().unwrap()["candidates"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(fixture.nodes.list().await.unwrap()[0]["id"], app_id);
    assert_eq!(fs::read(&registration).unwrap(), previous);
    assert_eq!(
        fs::read_to_string(data).unwrap(),
        "preserved existing application data"
    );
    assert!(!fixture.store.has_active_tasks());
    fixture
        .sources
        .verify_ready(&source.source_id, source.content_digest.as_ref().unwrap())
        .unwrap();
    fixture.shutdown().await;
}

#[tokio::test]
async fn cancelling_explicit_source_build_cleans_the_process_and_preserves_source_lease_until_settled(
) {
    let fixture = Fixture::new(":");
    declare_build(&fixture);
    let marker = fixture.paths.data_root().join("disposable-build-pid");
    fs::write(
        &fixture.npm,
        script(&format!(
            "if [ \"$1\" = run ]; then printf '%s' \"$$\" > '{}'; exec /bin/sleep 60; fi",
            marker.display()
        )),
    )
    .unwrap();
    let source = fixture.capture().await;
    let task = fixture.submit_build(&source, None, true).await;
    let pid: i32 = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(pid) = fs::read_to_string(&marker)
                .ok()
                .and_then(|value| value.parse::<i32>().ok())
            {
                break pid;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(fixture.store.has_active_tasks());
    fixture.installs.cancel(&task.task_id).unwrap();
    let done = fixture.wait_task(&task.task_id).await;
    assert_eq!(done.error.unwrap().code, "NODE_INSTALL_CANCELLED");
    assert_ne!(unsafe { libc::kill(-pid, 0) }, 0);
    assert!(fixture
        .paths
        .data_root()
        .join("node-development-builds")
        .join(&task.task_id)
        .exists());
    assert!(fixture.installs.list().unwrap()["candidates"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!fixture.store.has_active_tasks());
    fixture.shutdown().await;
}

#[tokio::test]
async fn a_trusted_prebuilt_source_never_runs_its_development_script_again_even_with_build_flag() {
    let fixture = Fixture::new(COPY_BUILD);
    declare_build(&fixture);
    let tools = fixture.builds.environment().resolve().await.unwrap();
    let operation = fixture
        .source
        .parent()
        .unwrap()
        .join("disposable-prebuilt-operation");
    let (cancel, _) = watch::channel(false);
    let output = crate::node_development_build::build_repository(
        &fixture.source,
        &operation,
        &tools,
        fixture.stop.subscribe(),
        cancel.subscribe(),
    )
    .await
    .unwrap();
    let task = fixture
        .sources
        .capture_built_output(
            output,
            Some("disposable trusted prebuilt source".into()),
            None,
            fixture.stop.subscribe(),
        )
        .await
        .unwrap();
    let done = fixture.wait_task(&task.task_id).await;
    let source = fixture
        .sources
        .info(done.result.unwrap()["sourceId"].as_str().unwrap())
        .unwrap();
    assert!(source.build_source_digest.is_some());
    assert!(!source.inspection.as_ref().unwrap().requires_build);
    fs::write(
        &fixture.npm,
        script("if [ \"$1\" = run ]; then exit 99; fi"),
    )
    .unwrap();
    let task = fixture.submit_build(&source, None, true).await;
    let done = fixture.wait_task(&task.task_id).await;
    assert!(done.error.is_none(), "{done:?}");
    assert_eq!(
        done.result.unwrap()["candidate"]["provenance"]["sourceId"],
        source.source_id
    );
    assert!(!fixture
        .paths
        .data_root()
        .join("node-development-builds")
        .exists());
    fixture.shutdown().await;
}

#[tokio::test]
#[ignore = "explicit bundled Node/npm offline source-build integration; requires desktop resources"]
async fn real_node_source_build_prepares_without_network_and_preserves_authoritative_source_identity(
) {
    let fixture = Fixture::new(":");
    declare_build(&fixture);
    let node = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src-tauri/resources/development-node/bin/node")
        .canonicalize()
        .unwrap();
    fixture
        .builds
        .environment()
        .configure_node(Some(node))
        .await
        .unwrap();
    let source = fixture.capture().await;
    let task = fixture.submit_build(&source, None, true).await;
    let done = fixture.wait_task(&task.task_id).await;
    assert!(done.error.is_none(), "{done:?}");
    let result = done.result.unwrap();
    assert_eq!(
        result["candidate"]["provenance"]["sourceId"],
        source.source_id
    );
    let program = fixture
        .paths
        .data_root()
        .join("node-apps/.installations")
        .join(result["candidate"]["candidateId"].as_str().unwrap())
        .join("program");
    assert_eq!(
        fs::read_to_string(program.join("public/disposable-build-proof.txt")).unwrap(),
        "explicit build completed"
    );
    assert_eq!(fixture.sources.list().unwrap().len(), 1);
    fixture.shutdown().await;
}
fn candidate_id(task: &TaskRecord) -> String {
    task.result.as_ref().unwrap()["candidateId"]
        .as_str()
        .unwrap()
        .into()
}
fn slot(fixture: &Fixture, id: &str) -> PathBuf {
    fixture.paths.data_root().join("node-apps").join(id)
}

#[test]
fn public_prepare_input_rejects_arbitrary_source_paths_and_unknown_fields() {
    assert!(serde_json::from_value::<PrepareNodeInstallInput>(json!({"sourceId":"src_example","expectedSourceDigest":"hash","sourcePath":"/private/other"})).is_err());
    assert!(serde_json::from_value::<PrepareNodeInstallInput>(
        json!({"sourceId":"src_example","expectedSourceDigest":"hash"})
    )
    .is_ok());
}

#[tokio::test]
async fn captured_source_prepares_and_applies_without_retaining_source_runtime_dependency() {
    let fixture = Fixture::new(":");
    let source = fixture.capture().await;
    let candidate = fixture.ready(&source).await;
    assert!(!fixture.store.has_active_tasks());
    let app_id = fixture.installs.target_app_id(&candidate).unwrap();
    assert!(!slot(&fixture, &app_id).exists());
    let captured = source.source_path.unwrap();
    fs::rename(&captured, captured.with_extension("retained-after-prepare")).unwrap();
    fs::rename(
        &fixture.source,
        fixture.source.with_extension("retained-author"),
    )
    .unwrap();
    let applied = fixture
        .installs
        .apply(fixture.nodes.clone(), &candidate, fixture.stop.subscribe())
        .await
        .unwrap();
    assert!(applied.error.is_none(), "{:?}", applied.error);
    assert_eq!(
        applied.result.as_ref().unwrap()["operationOutcome"],
        "committed"
    );
    assert_eq!(
        fixture.nodes.configuration(&app_id).await.unwrap()["state"],
        "stopped"
    );
    let pointer = fs::read(slot(&fixture, &app_id).join("registration.json")).unwrap();
    let again = fixture
        .installs
        .apply(fixture.nodes.clone(), &candidate, fixture.stop.subscribe())
        .await
        .unwrap();
    assert!(again.error.is_none());
    assert_eq!(
        fs::read(slot(&fixture, &app_id).join("registration.json")).unwrap(),
        pointer
    );
    assert_eq!(
        fixture.installs.candidate(&candidate).unwrap()["state"],
        "committed"
    );
    assert!(fixture.installs.issues().is_empty());
    fixture.shutdown().await;
}

#[tokio::test]
async fn cancellation_waits_for_npm_cleanup_and_releases_the_source_and_migration_lease() {
    let fixture = Fixture::new(SLOW_NPM);
    let source = fixture.capture().await;
    let task = fixture.submit(&source, None).await;
    let marker = fixture.marker().await;
    let pid: i32 = fs::read_to_string(&marker).unwrap().parse().unwrap();
    assert_eq!(
        fixture
            .store
            .active_task(&format!("source:{}", source.source_id))
            .as_deref(),
        Some(task.task_id.as_str())
    );
    fixture.store.begin_workspace_migration().unwrap();
    assert!(
        fixture.store.has_active_tasks(),
        "migration must wait for the installation lease"
    );
    fixture.installs.cancel(&task.task_id).unwrap();
    let done = fixture.wait_task(&task.task_id).await;
    assert_eq!(done.error.unwrap().code, "NODE_INSTALL_CANCELLED");
    assert_ne!(unsafe { libc::kill(-pid, 0) }, 0);
    assert!(!fixture.store.has_active_tasks());
    assert!(marker.exists());
    fixture.store.resume_workspace();
    fixture.shutdown().await;
}

#[tokio::test]
async fn source_mutation_during_npm_is_rejected_before_candidate_becomes_prepared() {
    let fixture = Fixture::new(":");
    let release = fixture.npm.parent().unwrap().join("disposable-release");
    fs::write(&fixture.npm, script(&format!("printf '%s' \"$$\" > node_modules/disposable-pid; while [ ! -f '{}' ]; do /bin/sleep .02; done", release.display()))).unwrap();
    let source = fixture.capture().await;
    let task = fixture.submit(&source, None).await;
    fixture.marker().await;
    fs::write(
        source
            .source_path
            .as_ref()
            .unwrap()
            .join("public/index.html"),
        "artificial post-capture change",
    )
    .unwrap();
    fs::write(&release, "release artificial npm").unwrap();
    let done = fixture.wait_task(&task.task_id).await;
    assert_eq!(done.error.as_ref().unwrap().code, "SOURCE_CHANGED");
    let candidate = candidate_id(&done);
    assert!(fixture.installs.target_app_id(&candidate).is_err());
    assert!(fixture.nodes.list().await.unwrap().is_empty());
    assert!(!fixture.store.has_active_tasks());
    fixture.shutdown().await;
}

#[tokio::test]
async fn shutdown_drains_prepare_and_rejects_subsequent_tasks() {
    let fixture = Fixture::new(SLOW_NPM);
    let source = fixture.capture().await;
    let task = fixture.submit(&source, None).await;
    let marker = fixture.marker().await;
    let pid: i32 = fs::read_to_string(marker).unwrap().parse().unwrap();
    fixture.stop.send(true).unwrap();
    fixture.installs.shutdown().await;
    let interrupted = fixture.builds.task(&task.task_id).unwrap();
    assert_eq!(interrupted.status, crate::TaskStatus::Interrupted);
    assert_eq!(interrupted.phase, "interrupted");
    assert_eq!(serde_json::to_value(&interrupted).unwrap()["final"], true);
    assert_eq!(
        fixture
            .builds
            .task(&task.task_id)
            .unwrap()
            .error
            .unwrap()
            .code,
        "NODE_INSTALL_INTERRUPTED"
    );
    assert!(!fixture.store.has_active_tasks());
    assert_ne!(unsafe { libc::kill(-pid, 0) }, 0);
    assert!(fixture
        .installs
        .prepare(
            fixture.nodes.clone(),
            PrepareNodeInstallInput {
                build_source: false,
                source_id: source.source_id,
                expected_source_digest: source.content_digest.unwrap(),
                target_app_id: None
            },
            fixture.stop.subscribe()
        )
        .await
        .is_err());
    fixture.shutdown().await;
}

#[tokio::test]
async fn missing_explicit_tools_fail_persistently_without_using_detected_tools() {
    let fixture = Fixture::new(":");
    let source = fixture.capture().await;
    atomic_json(
        &fixture.paths.data_root().join("toolchain.json"),
        &json!({"schemaVersion":1,"nodePath":"/disposable-missing-explicit-node"}),
    )
    .unwrap();
    let task = fixture.submit(&source, None).await;
    let done = fixture.wait_task(&task.task_id).await;
    assert_eq!(done.error.unwrap().code, "NODE_NOT_FOUND");
    assert!(fixture.nodes.list().await.unwrap().is_empty());
    assert!(!fixture.store.has_active_tasks());
    fixture.shutdown().await;
}

#[tokio::test]
async fn nodeapps_precommit_rejection_finishes_apply_task_and_preserves_pointer() {
    let fixture = Fixture::new(":");
    let source = fixture.capture().await;
    let candidate = fixture.ready(&source).await;
    let app_id = fixture.installs.target_app_id(&candidate).unwrap();
    // A preserved unregistered slot cannot be silently adopted by a fresh task.
    fs::create_dir_all(slot(&fixture, &app_id)).unwrap();
    fs::write(
        slot(&fixture, &app_id).join("disposable-evidence"),
        "retain",
    )
    .unwrap();
    let applied = fixture
        .installs
        .apply(fixture.nodes.clone(), &candidate, fixture.stop.subscribe())
        .await
        .unwrap();
    assert!(applied.status.is_final());
    assert!(applied.error.is_some());
    assert_eq!(applied.result.unwrap()["operationOutcome"], "not-committed");
    assert!(!slot(&fixture, &app_id).join("registration.json").exists());
    assert!(slot(&fixture, &app_id).join("disposable-evidence").exists());
    assert!(!fixture.store.has_active_tasks());
    fixture.shutdown().await;
}

#[tokio::test]
async fn damaged_install_area_reports_issues_without_hiding_external_registrations() {
    let fixture = Fixture::new(":");
    let external = fixture
        .nodes
        .register(fixture.source.clone())
        .await
        .unwrap();
    let install_root = fixture.paths.data_root().join("node-apps/.installations");
    fs::write(&install_root, "artificial corrupt install area").unwrap();
    let broken = NodeInstallTasks::new(
        fixture.paths.clone(),
        fixture.store.clone(),
        fixture.builds.clone(),
        Err(ApiError::new(
            "DISPOSABLE_SOURCE_UNAVAILABLE",
            "artificial source diagnostic",
        )),
    );
    assert!(!broken.issues().is_empty());
    let list = fixture.nodes.list().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["id"], external["id"]);
    assert_eq!(
        fs::read_to_string(&install_root).unwrap(),
        "artificial corrupt install area"
    );
    broken.shutdown().await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn restart_list_restores_new_and_update_candidates_while_isolating_a_bad_record() {
    let fixture = Fixture::new(":");
    let source = fixture.capture().await;
    let original = fixture.ready(&source).await;
    let original_app = fixture.installs.target_app_id(&original).unwrap();
    fixture
        .installs
        .apply(fixture.nodes.clone(), &original, fixture.stop.subscribe())
        .await
        .unwrap();
    let update_task = fixture.submit(&source, Some(original_app.clone())).await;
    let update_task = fixture.wait_task(&update_task.task_id).await;
    assert!(update_task.error.is_none());
    let update = update_task.result.unwrap()["candidate"]["candidateId"]
        .as_str()
        .unwrap()
        .to_owned();
    let fresh = fixture.ready(&source).await;
    let broken = format!("install_{}", "c".repeat(32));
    let evidence = fixture
        .paths
        .data_root()
        .join("node-apps/.installations")
        .join(&broken);
    fs::create_dir(&evidence).unwrap();
    fs::write(
        evidence.join("install.json"),
        "artificial malformed operation",
    )
    .unwrap();
    fixture.installs.shutdown().await;
    let restored = NodeInstallTasks::new(
        fixture.paths.clone(),
        fixture.store.clone(),
        fixture.builds.clone(),
        Ok(fixture.sources.clone()),
    );
    let listed = restored.list().unwrap();
    let rows = listed["candidates"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    let row = |id: &str| rows.iter().find(|row| row["candidateId"] == id).unwrap();
    assert_eq!(row(&update)["state"], "prepared");
    assert_eq!(row(&update)["targetAppId"], original_app);
    assert_eq!(row(&update)["prepareTaskId"], update_task.task_id);
    assert_eq!(row(&fresh)["state"], "prepared");
    assert!(row(&fresh)["targetAppId"].is_null());
    assert_eq!(row(&original)["state"], "committed");
    assert!(row(&original)["targetAppId"].is_null());
    assert_eq!(row(&broken)["state"], "unavailable");
    assert!(row(&broken)["appId"].is_null());
    assert!(!row(&broken)["issues"].as_array().unwrap().is_empty());
    assert!(!listed["issues"].as_array().unwrap().is_empty());
    assert_eq!(
        fs::read_to_string(evidence.join("install.json")).unwrap(),
        "artificial malformed operation"
    );
    let applied = restored
        .apply(fixture.nodes.clone(), &fresh, fixture.stop.subscribe())
        .await
        .unwrap();
    assert!(applied.error.is_none(), "{:?}", applied.error);
    restored.shutdown().await;
    fixture.shutdown().await;
}

#[tokio::test]
async fn interruption_status_mapping_is_narrow_and_keeps_partial_results() {
    let fixture = Fixture::new(":");
    let node = fixture
        .builds
        .create_workspace_operation("nodeInstallPrepare")
        .unwrap();
    fixture
        .builds
        .finish_partial(
            &node.task_id,
            json!({"partialFilesRetained":true}),
            interrupted(),
        )
        .unwrap();
    let node = fixture.builds.task(&node.task_id).unwrap();
    assert_eq!(node.status, crate::TaskStatus::Interrupted);
    assert_eq!(node.phase, "interrupted");
    assert_eq!(node.result.unwrap()["partialFilesRetained"], true);
    let ordinary = fixture
        .builds
        .create_workspace_operation("disposableOther")
        .unwrap();
    fixture
        .builds
        .finish_partial(
            &ordinary.task_id,
            json!({"kept":true}),
            ApiError::new(
                "SOURCE_ACQUISITION_INTERRUPTED",
                "unchanged historical partial-error mapping",
            ),
        )
        .unwrap();
    assert_eq!(
        fixture.builds.task(&ordinary.task_id).unwrap().status,
        crate::TaskStatus::Failed
    );
    let nonpartial = fixture
        .builds
        .create_workspace_operation("nodeInstallPrepare")
        .unwrap();
    fixture
        .builds
        .finish(&nonpartial.task_id, Err(interrupted()))
        .unwrap();
    assert_eq!(
        fixture.builds.task(&nonpartial.task_id).unwrap().status,
        crate::TaskStatus::Interrupted
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn capture_marked_for_adaptation_cannot_be_silently_installed() {
    let fixture = Fixture::new(":");
    fs::create_dir(fixture.source.join("config")).unwrap();
    fs::write(
        fixture.source.join("config/disposable.json"),
        "{} accounts for an unknown configuration format",
    )
    .unwrap();
    let source = fixture.capture().await;
    assert_eq!(source.inspection.as_ref().unwrap().kind, "needs-adaptation");
    let error = fixture
        .installs
        .prepare(
            fixture.nodes.clone(),
            PrepareNodeInstallInput {
                build_source: false,
                source_id: source.source_id,
                expected_source_digest: source.content_digest.unwrap(),
                target_app_id: None,
            },
            fixture.stop.subscribe(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_INSTALL_SOURCE_NEEDS_ADAPTATION");
    assert!(fixture.nodes.list().await.unwrap().is_empty());
    fixture.shutdown().await;
}
