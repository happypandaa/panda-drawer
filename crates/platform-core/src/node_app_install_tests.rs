use super::*;
use crate::{environment::EnvironmentManager, ProjectStore, RuntimePaths};
use std::sync::Arc;

mod owned_process {
    include!("node_install_owned_process_tests.rs");
}

struct Fixture {
    _temp: tempfile::TempDir,
    paths: RuntimePaths,
    store: Arc<ProjectStore>,
    tasks: Arc<BuildManager>,
    installer: NodeAppInstaller,
    source: PathBuf,
    tools: Toolchain,
}
impl Fixture {
    fn new(npm_body: &str) -> Self {
        let (temp, paths) = RuntimePaths::for_test();
        let base = temp.path().canonicalize().unwrap();
        let source = base.join("可丢弃 Node 安装源码");
        private_dir(&source.join("server")).unwrap();
        private_dir(&source.join("web")).unwrap();
        write_new(
            &source.join("server/start.mjs"),
            b"throw new Error('installation must not execute application code');\n",
            false,
        )
        .unwrap();
        write_new(&source.join("web/index.html"), b"first program", false).unwrap();
        atomic_json(&source.join("local-app.json"), &json!({
            "packageFormat":"local-web-app", "formatVersion":2, "runtime":"node-service",
            "app":{"id":"disposable-node-install", "name":"可丢弃安装", "version":"1.0.0"},
            "dataContract":{"id":"disposable-node-install.data","version":1},
            "web":{"root":"web", "entry":"index.html", "routing":"files"},
            "backend":{"root":"server", "entry":"start.mjs", "protocolVersion":1, "nodeMajor":24}
        })).unwrap();
        atomic_json(
            &source.join("package.json"),
            &json!({"name":"disposable-install","version":"1.0.0","dependencies":{}}),
        )
        .unwrap();
        atomic_json(&source.join("package-lock.json"), &json!({"name":"disposable-install","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"disposable-install","version":"1.0.0","dependencies":{}}}})).unwrap();
        let npm = base.join("fake-npm.sh");
        write_new(&npm, format!(
            "set -eu\n[ \"$1\" = '--prefix' ]\n[ \"$2\" = \"$PWD\" ]\n[ \"$NPM_CONFIG_USERCONFIG\" != \"$NPM_CONFIG_GLOBALCONFIG\" ]\n[ -f \"$NPM_CONFIG_USERCONFIG\" ]\n[ -f \"$NPM_CONFIG_GLOBALCONFIG\" ]\n[ -z \"${{NODE_PATH-}}\" ]\n[ -z \"${{NODE_OPTIONS-}}\" ]\n[ \"$NPM_CONFIG_LOGS_MAX\" = \"0\" ]\n{npm_body}\n"
        ).as_bytes(), false).unwrap();
        let tools = Toolchain {
            node: ExecutableInfo {
                path: "/bin/sh".into(),
                version: "test-process-fixture".into(),
            },
            npm: ExecutableInfo {
                path: npm,
                version: "test-npm-fixture".into(),
            },
            path: "/usr/bin:/bin".into(),
        };
        let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
        let tasks =
            BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1".into()).unwrap();
        let installer = NodeAppInstaller::new(paths.data_root().to_path_buf())
            .unwrap()
            .with_workspace(paths.workspace_root().to_path_buf());
        Self {
            _temp: temp,
            paths,
            store,
            tasks,
            installer,
            source,
            tools,
        }
    }
    fn fresh_target(&self) -> InstallTargetSnapshot {
        let target = InstallTargetSnapshot::fresh();
        self.installer.ownership.initialize(&target.app_id).unwrap();
        target
    }
    fn source(&self) -> VerifiedNodeSource {
        VerifiedNodeSource::new(
            format!("src_{}", "a".repeat(32)),
            self.source.clone(),
            "b".repeat(64),
            Some("c".repeat(40)),
        )
        .unwrap()
    }
    async fn prepare(&self, target: InstallTargetSnapshot) -> PreparedNodeInstall {
        let task = self
            .tasks
            .create_workspace_operation("nodeInstallPrepare")
            .unwrap();
        let (_stop, _cancel, control) = live_control();
        self.installer
            .prepare(
                &task.task_id,
                self.source(),
                target,
                self.tools.clone(),
                control,
                &self.tasks,
            )
            .await
            .unwrap()
    }
    fn apply(
        &self,
        prepared: &PreparedNodeInstall,
        target: &InstallTargetSnapshot,
    ) -> InstalledNodeGeneration {
        let task = self
            .tasks
            .create_workspace_operation("nodeInstallApply")
            .unwrap();
        let (_stop, _cancel, control) = live_control();
        self.installer
            .commit_stopped(
                &prepared.candidate_id,
                target,
                &task.task_id,
                &control,
                &self.tasks,
            )
            .unwrap()
    }
    fn update_target(
        &self,
        installed: &InstalledNodeGeneration,
        revision: u64,
    ) -> InstallTargetSnapshot {
        let slot = self.installer.slot(&installed.app_id).unwrap();
        let program = slot.join("generations").join(&installed.generation);
        InstallTargetSnapshot {
            app_id: installed.app_id.clone(),
            registration_digest: registration_digest(&slot).unwrap(),
            generation: Some(installed.generation.clone()),
            manifest: Some(
                serde_json::from_slice(&read_small(&program.join("local-app.json")).unwrap())
                    .unwrap(),
            ),
            configuration_revision: revision,
            external_registration: None,
            development_source: None,
        }
    }
    async fn shutdown(self) {
        self.tasks.shutdown().await;
        self.store.shutdown().await;
    }
}
fn live_control() -> (watch::Sender<bool>, watch::Sender<bool>, InstallControl) {
    let (stop, stopped) = watch::channel(false);
    let (cancel, cancelled) = watch::channel(false);
    (stop, cancel, InstallControl::new(stopped, cancelled))
}
const NPM_OK: &str = "mkdir -p node_modules; printf '{}' > node_modules/.package-lock.json";

#[tokio::test]
async fn deletion_waits_for_final_install_journal_and_task_writes_after_process_scope_closes() {
    use crate::{
        node_apps::NodeApps, node_development_build::process_scope::NodeReaderFault,
        owned_process::ProcessPhase,
    };
    let fixture = Arc::new(Fixture::new(NPM_OK));
    let target = fixture.fresh_target();
    let id = target.app_id.clone();
    let apps = NodeApps::new(
        fixture.paths.data_root().into(),
        "/disposable-must-not-start-node".into(),
    )
    .unwrap();
    let operation = apps
        .admit_operation(&id, "disposable installation finalization")
        .unwrap();
    let task = fixture
        .tasks
        .create_node_operation(&id, "nodeInstallPrepare")
        .unwrap();
    let source = fixture.source();
    let key = format!("source:{}", source.source_id);
    let lease = fixture.store.reserve_task(&key, &task.task_id).unwrap();
    let registry = fixture.store.processes();
    let (entered, blocked) = tokio::sync::oneshot::channel();
    let (release, wait) = tokio::sync::oneshot::channel();
    let scope = NodeProcessScope::new(registry.clone(), Arc::new((lease, operation)))
        .with_reader_fault(NodeReaderFault {
            component: "node-install/finalize".into(),
            step: "journal".into(),
            entered,
            release: Some(wait),
            panic: false,
            writer: None,
        });
    let work = {
        let fixture = fixture.clone();
        let task = task.task_id.clone();
        tokio::spawn(async move {
            let (_stop, _cancel, control) = live_control();
            fixture
                .installer
                .prepare_with_scope(
                    &task,
                    source,
                    target,
                    fixture.tools.clone(),
                    control,
                    &fixture.tasks,
                    scope,
                )
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), blocked)
        .await
        .unwrap()
        .unwrap();
    assert!(registry
        .snapshot()
        .iter()
        .all(|process| process.phase == ProcessPhase::Closed));
    let candidate = fs::read_dir(fixture.installer.operations())
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let journal: Value =
        serde_json::from_slice(&fs::read(candidate.join("install.json")).unwrap()).unwrap();
    assert_eq!(journal["state"], "preparing");
    assert!(!fixture.tasks.task(&task.task_id).unwrap().status.is_final());
    let deletion = apps.begin_delete_operation(&id).unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), deletion.wait_ready())
            .await
            .is_err()
    );
    assert!(candidate.join("program/local-app.json").exists());
    assert!(fixture.store.active_task(&key).is_some());
    release.send(()).unwrap();
    work.await.unwrap().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), deletion.wait_ready())
        .await
        .unwrap();
    assert!(fixture.tasks.task(&task.task_id).unwrap().status.is_final());
    assert!(fixture.store.active_task(&key).is_none());
    fixture.tasks.shutdown().await;
    fixture.store.shutdown().await;
}

#[tokio::test]
async fn bundled_platform_tools_are_preserved_hashed_and_not_executed() {
    let fixture = Fixture::new(&format!(
        "{NPM_OK}\nmkdir -p node_modules/platform-tools\nprintf 'MZ-disposable-windows-tool' > node_modules/platform-tools/service.exe\nprintf 'MZ-disposable-windows-library' > node_modules/platform-tools/service.DLL\nprintf 'disposable-library' > node_modules/platform-tools/library.so\nprintf 'disposable-library' > node_modules/platform-tools/library.dylib\nprintf '#!/bin/sh\\nexit 91\\n' > node_modules/platform-tools/tool.EXE\nchmod +x node_modules/platform-tools/tool.EXE"
    ));
    let target = fixture.fresh_target();
    let prepared = fixture.prepare(target.clone()).await;
    let installed = fixture.apply(&prepared, &target);
    let program = fixture
        .installer
        .slot(&installed.app_id)
        .unwrap()
        .join("generations")
        .join(&installed.generation);
    for (name, bytes) in [
        ("service.exe", b"MZ-disposable-windows-tool".as_slice()),
        ("service.DLL", b"MZ-disposable-windows-library".as_slice()),
        ("library.so", b"disposable-library".as_slice()),
        ("library.dylib", b"disposable-library".as_slice()),
        ("tool.EXE", b"#!/bin/sh\nexit 91\n".as_slice()),
    ] {
        assert_eq!(
            fs::read(program.join("node_modules/platform-tools").join(name)).unwrap(),
            bytes
        );
    }
    let (_stop, _cancel, control) = live_control();
    assert_eq!(
        program_digest(&program, &control).unwrap(),
        prepared.program_digest
    );
    fs::write(
        program.join("node_modules/platform-tools/service.exe"),
        b"changed",
    )
    .unwrap();
    assert_ne!(
        program_digest(&program, &control).unwrap(),
        prepared.program_digest
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn third_party_node_addons_still_reject_before_registration_and_keep_evidence() {
    for extension in ["node", "NODE"] {
        let fixture = Fixture::new(&format!(
            "{NPM_OK}\nprintf 'unsupported-native-addon' > node_modules/addon.{extension}"
        ));
        let task = fixture
            .tasks
            .create_workspace_operation("nodeInstallPrepare")
            .unwrap();
        let target = fixture.fresh_target();
        let (_stop, _cancel, control) = live_control();
        let error = fixture
            .installer
            .prepare(
                &task.task_id,
                fixture.source(),
                target.clone(),
                fixture.tools.clone(),
                control,
                &fixture.tasks,
            )
            .await
            .unwrap_err();
        assert!(error.message.contains(".node"), "{error:?}");
        assert!(!fixture.installer.slot(&target.app_id).unwrap().exists());
        let final_task = fixture.tasks.task(&task.task_id).unwrap();
        let candidate = final_task.result.unwrap()["candidateId"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            fs::read(
                fixture
                    .installer
                    .operation(&candidate)
                    .unwrap()
                    .join(format!("program/node_modules/addon.{extension}"))
            )
            .unwrap(),
            b"unsupported-native-addon"
        );
        fixture.shutdown().await;
    }
}

fn sentinels(slot: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    [
        (
            "configuration/configuration.json",
            b"{\"revision\":7,\"artificial\":true}".as_slice(),
        ),
        (
            "configuration/credentials/artificial-key",
            b"test-only-not-a-real-key".as_slice(),
        ),
        (
            "data/artificial.sqlite",
            b"artificial business data".as_slice(),
        ),
        ("cache/artificial", b"retain cache".as_slice()),
        (
            "runs/artificial-evidence",
            b"retain running evidence".as_slice(),
        ),
    ]
    .into_iter()
    .map(|(path, bytes)| {
        let path = slot.join(path);
        private_dir(path.parent().unwrap()).unwrap();
        write_new(&path, bytes, false).unwrap();
        (path, bytes.to_vec())
    })
    .collect()
}
fn assert_preserved(files: &[(PathBuf, Vec<u8>)]) {
    for (path, bytes) in files {
        assert_eq!(&fs::read(path).unwrap(), bytes, "{}", path.display());
    }
}

fn assert_no_task_logs(paths: &RuntimePaths, task_id: &str) {
    let directory = paths.workspace_root().join(".platform/tasks").join(task_id);
    for name in ["head.log", "tail.log", "log.json", "npm-logs"] {
        assert!(
            !directory.join(name).exists(),
            "task output must not be retained"
        );
    }
}

#[tokio::test]
async fn install_and_compatible_update_keep_uuid_configuration_data_and_old_program() {
    let fixture = Fixture::new(NPM_OK);
    let target = fixture.fresh_target();
    let prepared = fixture.prepare(target.clone()).await;
    assert_eq!(
        fixture
            .installer
            .prepared(&prepared.candidate_id)
            .unwrap()
            .candidate_id,
        prepared.candidate_id
    );
    assert!(!fixture.installer.slot(&target.app_id).unwrap().exists());
    assert_eq!(
        fixture
            .installer
            .journal(&prepared.candidate_id)
            .unwrap()
            .state,
        "prepared"
    );
    let installed = fixture.apply(&prepared, &target);
    let slot = fixture.installer.slot(&installed.app_id).unwrap();
    let old_program = slot.join("generations").join(&installed.generation);
    let files = sentinels(&slot);
    let update = fixture.update_target(&installed, 7);
    fs::write(fixture.source.join("web/index.html"), "second program").unwrap();
    let next = fixture.prepare(update.clone()).await;
    assert_eq!(
        fs::read(old_program.join("web/index.html")).unwrap(),
        b"first program"
    );
    assert_preserved(&files);
    let updated = fixture.apply(&next, &update);
    assert_eq!(updated.app_id, installed.app_id);
    assert_ne!(updated.generation, installed.generation);
    assert_preserved(&files);
    assert!(old_program.is_dir());
    // Disappearing acquisition/source material is no longer a runtime input.
    fs::rename(
        &fixture.source,
        fixture.source.with_file_name("retained-source-away"),
    )
    .unwrap();
    let program = slot.join("generations").join(&updated.generation);
    load_manifest(&program).unwrap();
    assert_eq!(
        fs::read(program.join("web/index.html")).unwrap(),
        b"second program"
    );
    assert!(program.join("node_modules/.package-lock.json").is_file());
    let repeated = fixture.apply(&next, &update);
    assert_eq!(repeated.generation, updated.generation);
    assert_eq!(fs::read_dir(slot.join("generations")).unwrap().count(), 2);
    for name in [
        "previous-program.json",
        "program-history",
        "program-rollbacks",
    ] {
        assert!(
            !slot.join(name).exists(),
            "updates do not create platform version history"
        );
    }
    fixture.shutdown().await;
}

#[tokio::test]
async fn incompatible_or_unknown_data_contract_stops_before_npm_and_retains_old_slot() {
    let fixture = Fixture::new(NPM_OK);
    let fresh = fixture.fresh_target();
    let installed = fixture.apply(&fixture.prepare(fresh.clone()).await, &fresh);
    let target = fixture.update_target(&installed, 7);
    let slot = fixture.installer.slot(&installed.app_id).unwrap();
    let files = sentinels(&slot);
    let registration = fs::read(slot.join("registration.json")).unwrap();
    let manifest_path = fixture.source.join("local-app.json");
    let manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    fs::write(
        &fixture.tools.npm.path,
        "printf executed > npm-must-not-run\nexit 91\n",
    )
    .unwrap();
    for (declaration, expected) in [
        (
            Some(json!({"id":"disposable-node-install.data","version":2})),
            "NODE_INSTALL_DATA_INCOMPATIBLE",
        ),
        (
            Some(json!({"id":"different-data","version":1})),
            "NODE_INSTALL_DATA_INCOMPATIBLE",
        ),
        (None, "NODE_INSTALL_DATA_COMPATIBILITY_UNKNOWN"),
    ] {
        let mut next = manifest.clone();
        if let Some(declaration) = declaration {
            next["dataContract"] = declaration;
        } else {
            next.as_object_mut().unwrap().remove("dataContract");
        }
        atomic_json(&manifest_path, &next).unwrap();
        let task = fixture
            .tasks
            .create_workspace_operation("nodeInstallPrepare")
            .unwrap();
        let (_stop, _cancel, control) = live_control();
        let error = fixture
            .installer
            .prepare(
                &task.task_id,
                fixture.source(),
                target.clone(),
                fixture.tools.clone(),
                control,
                &fixture.tasks,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, expected);
        let task = fixture.tasks.task(&task.task_id).unwrap();
        assert_eq!(task.error.unwrap().code, expected);
        let result = task.result.unwrap();
        let candidate = result["candidateId"].as_str().unwrap();
        let operation = fixture.installer.operation(candidate).unwrap();
        assert!(operation.join("program/local-app.json").is_file());
        assert!(!operation.join("program/npm-must-not-run").exists());
        assert!(!operation.join("program/node_modules").exists());
        assert_eq!(
            fixture.installer.journal(candidate).unwrap().state,
            "failed"
        );
        assert_eq!(
            fs::read(slot.join("registration.json")).unwrap(),
            registration
        );
        assert_preserved(&files);
        assert_eq!(fs::read_dir(slot.join("generations")).unwrap().count(), 1);
    }
    fixture.shutdown().await;
}

#[tokio::test]
async fn retained_prepared_updates_are_rechecked_but_legacy_committed_history_stays_readable() {
    for legacy in [true, false] {
        let fixture = Fixture::new(NPM_OK);
        let source_manifest = fixture.source.join("local-app.json");
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&source_manifest).unwrap()).unwrap();
        if legacy {
            manifest.as_object_mut().unwrap().remove("dataContract");
            atomic_json(&source_manifest, &manifest).unwrap();
        }
        let fresh = fixture.fresh_target();
        let first = fixture.prepare(fresh.clone()).await;
        let installed = fixture.apply(&first, &fresh);
        let target = fixture.update_target(&installed, 7);
        let slot = fixture.installer.slot(&installed.app_id).unwrap();
        let files = sentinels(&slot);
        let registration = fs::read(slot.join("registration.json")).unwrap();
        let old_manifest = fs::read(
            slot.join("generations")
                .join(&installed.generation)
                .join("local-app.json"),
        )
        .unwrap();
        if !legacy {
            manifest["dataContract"]["version"] = json!(2);
            atomic_json(&source_manifest, &manifest).unwrap();
        }
        // Model a preparation persisted by the earlier, app/config-only policy.
        // Its actual copied bytes are valid, but today's update policy rejects it.
        let candidate = fixture.prepare(fixture.fresh_target()).await;
        let mut journal = fixture.installer.journal(&candidate.candidate_id).unwrap();
        journal.target = target.clone();
        journal.prepared.as_mut().unwrap().app_id = installed.app_id.clone();
        fixture.installer.save(&journal).unwrap();
        let expected = if legacy {
            "NODE_INSTALL_DATA_COMPATIBILITY_UNKNOWN"
        } else {
            "NODE_INSTALL_DATA_INCOMPATIBLE"
        };
        assert_eq!(
            fixture
                .installer
                .prepared(&candidate.candidate_id)
                .unwrap_err()
                .code,
            expected
        );
        let task = fixture
            .tasks
            .create_workspace_operation("nodeInstallApply")
            .unwrap();
        let (_stop, _cancel, control) = live_control();
        assert_eq!(
            fixture
                .installer
                .commit_stopped(
                    &candidate.candidate_id,
                    &target,
                    &task.task_id,
                    &control,
                    &fixture.tasks
                )
                .unwrap_err()
                .code,
            expected
        );
        assert_eq!(
            fixture
                .installer
                .journal(&candidate.candidate_id)
                .unwrap()
                .state,
            "prepared"
        );
        assert!(fixture
            .installer
            .operation(&candidate.candidate_id)
            .unwrap()
            .join("program")
            .is_dir());
        assert!(fixture
            .installer
            .committed(&first.candidate_id)
            .unwrap()
            .is_some());
        assert!(fixture
            .installer
            .list()
            .unwrap()
            .iter()
            .any(|item| item["candidateId"] == candidate.candidate_id));
        assert_eq!(
            fixture.apply(&first, &fresh).generation,
            installed.generation
        );
        assert_eq!(
            fs::read(slot.join("registration.json")).unwrap(),
            registration
        );
        assert_eq!(
            fs::read(
                slot.join("generations")
                    .join(&installed.generation)
                    .join("local-app.json")
            )
            .unwrap(),
            old_manifest
        );
        assert_preserved(&files);
        assert_eq!(fs::read_dir(slot.join("generations")).unwrap().count(), 1);
        fixture.shutdown().await;
    }
}

#[tokio::test]
async fn npm_failure_retains_candidate_and_never_registers_or_rewrites_existing_slot() {
    let mut fixture = Fixture::new(NPM_OK);
    let first = fixture.fresh_target();
    let installed = fixture.apply(&fixture.prepare(first.clone()).await, &first);
    let slot = fixture.installer.slot(&installed.app_id).unwrap();
    let files = sentinels(&slot);
    let before = fs::read(slot.join("registration.json")).unwrap();
    fs::write(&fixture.tools.npm.path, r#"
mkdir -p node_modules
printf candidate-only > node_modules/evidence
awk 'BEGIN { for (i = 0; i < 42000; i++) printf "~"; print "\nstdout-tail" }'
awk 'BEGIN { for (i = 0; i < 42000; i++) printf "^"; print "\nstderr-tail: simulated E404 download failure" }' >&2
printf 'Authorization: Bearer artificial-diagnostic-credential\n' >&2
exit 42
"#).unwrap();
    fixture.tools.npm.version = "fault-injection".into();
    let task = fixture
        .tasks
        .create_workspace_operation("nodeInstallPrepare")
        .unwrap();
    let (_stop, _cancel, control) = live_control();
    let error = fixture
        .installer
        .prepare(
            &task.task_id,
            fixture.source(),
            fixture.update_target(&installed, 7),
            fixture.tools.clone(),
            control,
            &fixture.tasks,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_INSTALL_NPM_FAILED");
    let diagnostics = &error.details["processDiagnostics"];
    assert_eq!(diagnostics["exitCode"], 42);
    assert_eq!(diagnostics["command"], "npm ci --omit=dev --ignore-scripts");
    assert!(diagnostics["stdout"]
        .as_str()
        .unwrap()
        .contains("stdout-tail"));
    assert!(diagnostics["stderr"]
        .as_str()
        .unwrap()
        .contains("simulated E404 download failure"));
    assert!(!diagnostics
        .to_string()
        .contains("artificial-diagnostic-credential"));
    assert_eq!(diagnostics["truncated"], true);
    let final_task = fixture.tasks.task(&task.task_id).unwrap();
    assert!(final_task.status.is_final());
    assert_eq!(
        final_task.error.as_ref().unwrap().details["processDiagnostics"],
        *diagnostics
    );
    assert_no_task_logs(&fixture.paths, &task.task_id);
    let id = final_task.result.unwrap()["candidateId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(fixture
        .installer
        .operation(&id)
        .unwrap()
        .join("program/node_modules/evidence")
        .is_file());
    assert_eq!(fs::read(slot.join("registration.json")).unwrap(), before);
    assert_preserved(&files);
    fixture.shutdown().await;
}

#[tokio::test]
async fn successful_npm_with_stderr_warnings_stays_successful_without_retaining_output() {
    let fixture = Fixture::new(&format!(
        "{NPM_OK}\nprintf 'npm warn deprecated artificial-warning\n' >&2"
    ));
    let prepared = fixture.prepare(fixture.fresh_target()).await;
    let task_id = fixture
        .installer
        .journal(&prepared.candidate_id)
        .unwrap()
        .prepare_task_id;
    let task = fixture.tasks.task(&task_id).unwrap();
    assert_eq!(task.status, crate::TaskStatus::Succeeded);
    assert!(task.error.is_none());
    assert!(!serde_json::to_string(&task)
        .unwrap()
        .contains("artificial-warning"));
    assert_no_task_logs(&fixture.paths, &task_id);
    fixture.shutdown().await;
}

#[tokio::test]
async fn cancellation_terminates_npm_before_reporting_completion_and_preserves_evidence() {
    let fixture = Fixture::new("printf 'stdout-before-cancel\n'; printf 'stderr-before-cancel\n' >&2; printf '%s' \"$$\" > npm-fixture-pid; /bin/sleep 60 & wait");
    let target = fixture.fresh_target();
    let task = fixture
        .tasks
        .create_workspace_operation("nodeInstallPrepare")
        .unwrap();
    let (_stop, cancel, control) = live_control();
    let installer = fixture.installer.clone();
    let source = fixture.source();
    let tools = fixture.tools.clone();
    let tasks = fixture.tasks.clone();
    let id = task.task_id.clone();
    let expected_slot = fixture.installer.slot(&target.app_id).unwrap();
    let runner = tokio::spawn(async move {
        installer
            .prepare(&id, source, target, tools, control, &tasks)
            .await
    });
    let marker = tokio::time::timeout(Duration::from_secs(5), async {
        'wait: loop {
            if let Ok(entries) = fs::read_dir(fixture.installer.operations()) {
                for entry in entries.flatten() {
                    let marker = entry.path().join("program/npm-fixture-pid");
                    if fs::read_to_string(&marker).is_ok_and(|value| value.parse::<i32>().is_ok()) {
                        break 'wait marker;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let pid: i32 = fs::read_to_string(&marker).unwrap().parse().unwrap();
    cancel.send(true).unwrap();
    let error = runner.await.unwrap().unwrap_err();
    assert_eq!(error.code, "NODE_INSTALL_CANCELLED");
    assert_ne!(
        unsafe { libc::kill(-pid, 0) },
        0,
        "owned process group must be gone"
    );
    assert!(marker.exists(), "interrupted candidate evidence remains");
    assert!(!expected_slot.exists());
    assert_no_task_logs(&fixture.paths, &task.task_id);
    assert_eq!(
        fixture
            .tasks
            .task(&task.task_id)
            .unwrap()
            .error
            .unwrap()
            .code,
        "NODE_INSTALL_CANCELLED"
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn npm_output_limit_discards_text_and_stops_the_process_group() {
    for redirect in ["", " >&2"] {
        let fixture = Fixture::new(&format!(
            "printf '%s' \"$$\" > npm-fixture-pid; awk 'BEGIN {{ for (i = 0; i < {}; i++) printf \"x\" }}'{redirect}; /bin/sleep 60 & wait",
            MAX_OUTPUT + 1,
        ));
        let task = fixture
            .tasks
            .create_workspace_operation("nodeInstallPrepare")
            .unwrap();
        let target = fixture.fresh_target();
        let slot = fixture.installer.slot(&target.app_id).unwrap();
        let (_stop, _cancel, mut control) = live_control();
        control.deadline = Instant::now() + Duration::from_secs(5);
        let error = fixture
            .installer
            .prepare(
                &task.task_id,
                fixture.source(),
                target,
                fixture.tools.clone(),
                control,
                &fixture.tasks,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "NODE_INSTALL_OUTPUT_LIMIT");
        let final_task = fixture.tasks.task(&task.task_id).unwrap();
        assert!(final_task.status.is_final());
        assert_no_task_logs(&fixture.paths, &task.task_id);
        let candidate_id = final_task.result.as_ref().unwrap()["candidateId"]
            .as_str()
            .unwrap();
        let operation = fixture.installer.operation(candidate_id).unwrap();
        let pid: i32 = fs::read_to_string(operation.join("program/npm-fixture-pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert_ne!(
            unsafe { libc::kill(-pid, 0) },
            0,
            "output overflow must stop the owned group"
        );
        assert!(!slot.exists());
        assert!(operation.exists(), "overflow evidence remains available");
        fixture.shutdown().await;
    }
}

#[tokio::test]
async fn npm_runtime_space_limits_stop_the_group_and_preserve_old_installation() {
    let cases = [
        (
            "single-file",
            "dd if=/dev/zero of=\"$NPM_CONFIG_CACHE/oversized\" bs=1024 count=9 2>/dev/null",
            NpmSpaceBudget { max_file: 8 * 1024, ..NpmSpaceBudget::default() },
        ),
        (
            "program-and-cache-total",
            "dd if=/dev/zero of=node_modules/program-bytes bs=1024 count=8 2>/dev/null; dd if=/dev/zero of=\"$NPM_CONFIG_CACHE/cache-bytes\" bs=1024 count=8 2>/dev/null",
            NpmSpaceBudget { max_total: 12 * 1024, ..NpmSpaceBudget::default() },
        ),
        (
            "entry-count",
            "i=0; while [ \"$i\" -lt 60 ]; do printf x > \"$NPM_CONFIG_CACHE/entry-$i\"; i=$((i+1)); done",
            NpmSpaceBudget { max_entries: 32, ..NpmSpaceBudget::default() },
        ),
    ];
    for (case, body, budget) in cases {
        let fixture = Fixture::new(NPM_OK);
        let target = fixture.fresh_target();
        let previous = fixture.apply(&fixture.prepare(target.clone()).await, &target);
        let slot = fixture.installer.slot(&previous.app_id).unwrap();
        let original_pointer = fs::read(slot.join("registration.json")).unwrap();
        let retained = sentinels(&slot);
        fs::write(&fixture.tools.npm.path, format!(
            "set -eu\nmkdir -p node_modules\nprintf '%s' \"$$\" > node_modules/npm-space-pid\n{body}\n/bin/sleep 60 & wait\n"
        )).unwrap();
        let task = fixture
            .tasks
            .create_workspace_operation("nodeInstallPrepare")
            .unwrap();
        let (_stop, _cancel, mut control) = live_control();
        control.npm_space = budget;
        let error = tokio::time::timeout(
            Duration::from_secs(5),
            fixture.installer.prepare(
                &task.task_id,
                fixture.source(),
                fixture.update_target(&previous, 7),
                fixture.tools.clone(),
                control,
                &fixture.tasks,
            ),
        )
        .await
        .expect("space monitor did not stop the artificial npm")
        .unwrap_err();
        assert_eq!(error.code, "NODE_INSTALL_SPACE_LIMIT", "{case}: {error:?}");
        let final_task = fixture.tasks.task(&task.task_id).unwrap();
        assert!(final_task.status.is_final());
        assert_eq!(final_task.error.unwrap().code, "NODE_INSTALL_SPACE_LIMIT");
        let id = final_task.result.unwrap()["candidateId"]
            .as_str()
            .unwrap()
            .to_owned();
        let operation = fixture.installer.operation(&id).unwrap();
        let pid: i32 = fs::read_to_string(operation.join("program/node_modules/npm-space-pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert_ne!(
            unsafe { libc::kill(-pid, 0) },
            0,
            "{case}: process group remains"
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
        assert!(operation.join("npm-cache").is_dir());
        assert_eq!(fixture.installer.journal(&id).unwrap().state, "failed");
        assert_eq!(
            fs::read(slot.join("registration.json")).unwrap(),
            original_pointer
        );
        assert_preserved(&retained);
        fixture.shutdown().await;
    }
}

#[tokio::test]
async fn insufficient_space_before_npm_preserves_candidate_without_starting_a_process() {
    let fixture = Fixture::new("printf started > must-not-start");
    let target = fixture.fresh_target();
    let expected_slot = fixture.installer.slot(&target.app_id).unwrap();
    let task = fixture
        .tasks
        .create_workspace_operation("nodeInstallPrepare")
        .unwrap();
    let (_stop, _cancel, mut control) = live_control();
    // This asks for an impossible reserve; it never fills or modifies a disk.
    control.npm_space.minimum_free = u64::MAX;
    let error = fixture
        .installer
        .prepare(
            &task.task_id,
            fixture.source(),
            target,
            fixture.tools.clone(),
            control,
            &fixture.tasks,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_INSTALL_SPACE_INSUFFICIENT");
    let record = fixture.tasks.task(&task.task_id).unwrap();
    let id = record.result.unwrap()["candidateId"]
        .as_str()
        .unwrap()
        .to_owned();
    let operation = fixture.installer.operation(&id).unwrap();
    assert!(!operation.join("program/must-not-start").exists());
    assert!(operation.join("program/local-app.json").is_file());
    assert!(!expected_slot.exists());
    fixture.shutdown().await;
}

#[tokio::test]
async fn cancellation_before_preparation_and_stale_apply_are_final_failures() {
    let fixture = Fixture::new(NPM_OK);
    let target = fixture.fresh_target();
    let task = fixture
        .tasks
        .create_workspace_operation("nodeInstallPrepare")
        .unwrap();
    let (_stop, cancel, control) = live_control();
    cancel.send(true).unwrap();
    assert_eq!(
        fixture
            .installer
            .prepare(
                &task.task_id,
                fixture.source(),
                target.clone(),
                fixture.tools.clone(),
                control,
                &fixture.tasks
            )
            .await
            .unwrap_err()
            .code,
        "NODE_INSTALL_CANCELLED"
    );
    assert!(fixture.tasks.task(&task.task_id).unwrap().status.is_final());
    let prepared = fixture.prepare(target.clone()).await;
    let mut wrong = target;
    wrong.configuration_revision = 1;
    let apply = fixture
        .tasks
        .create_workspace_operation("nodeInstallApply")
        .unwrap();
    let (_stop, _cancel, control) = live_control();
    assert_eq!(
        fixture
            .installer
            .commit_stopped(
                &prepared.candidate_id,
                &wrong,
                &apply.task_id,
                &control,
                &fixture.tasks
            )
            .unwrap_err()
            .code,
        "NODE_INSTALL_CHANGED"
    );
    assert!(fixture
        .tasks
        .task(&apply.task_id)
        .unwrap()
        .status
        .is_final());
    fixture.shutdown().await;
}

fn journal_for_commit(
    fixture: &Fixture,
    prepared: &PreparedNodeInstall,
    apply_task: &str,
) -> Journal {
    let mut journal = fixture.installer.journal(&prepared.candidate_id).unwrap();
    let manifest = read_small(
        &fixture
            .installer
            .operation(&prepared.candidate_id)
            .unwrap()
            .join("program/local-app.json"),
    )
    .unwrap();
    journal.registration = Some(ManagedNodeRegistration {
        version: 2,
        id: prepared.app_id.clone(),
        kind: "managed".into(),
        generation: prepared.generation.clone(),
        manifest_digest: digest_bytes(&manifest),
        program_digest: prepared.program_digest.clone(),
        provenance: prepared.provenance.clone(),
        development_source: None,
    });
    journal.apply_task_id = Some(apply_task.into());
    journal
}

#[tokio::test]
async fn recovery_uses_pointer_commit_point_and_never_reverts_configuration_or_data() {
    for point in [
        CommitPoint::BeforeGeneration,
        CommitPoint::BeforePointer,
        CommitPoint::AfterPointer,
    ] {
        let fixture = Fixture::new(NPM_OK);
        let target = fixture.fresh_target();
        let installed = fixture.apply(&fixture.prepare(target.clone()).await, &target);
        let slot = fixture.installer.slot(&installed.app_id).unwrap();
        let files = sentinels(&slot);
        let original = fs::read(slot.join("registration.json")).unwrap();
        fs::write(fixture.source.join("web/index.html"), "candidate update").unwrap();
        let prepared = fixture.prepare(fixture.update_target(&installed, 7)).await;
        let task = fixture
            .tasks
            .create_workspace_operation("nodeInstallApply")
            .unwrap();
        fixture.tasks.phase(&task.task_id, "committing").unwrap();
        let mut journal = journal_for_commit(&fixture, &prepared, &task.task_id);
        let expected_commit = matches!(point, CommitPoint::AfterPointer);
        assert!(fixture
            .installer
            .commit_files(&mut journal, |current| {
                if std::mem::discriminant(&current) == std::mem::discriminant(&point) {
                    Err(failure("TEST_INTERRUPT", "simulated process interruption"))
                } else {
                    Ok(())
                }
            })
            .is_err());
        assert_preserved(&files);
        assert_eq!(
            fs::read(slot.join("registration.json")).unwrap() != original,
            expected_commit
        );
        // Startup sees the task as interrupted; the durable pointer determines
        // whether publication actually committed, including a late task failure.
        fixture.tasks.shutdown().await;
        let reloaded = BuildManager::new(
            fixture.paths.clone(),
            fixture.store.clone(),
            "http://127.0.0.1:1".into(),
        )
        .unwrap();
        let recovered = fixture.installer.recover(&reloaded).unwrap();
        let record = recovered
            .iter()
            .find(|value| value["candidateId"] == prepared.candidate_id)
            .unwrap();
        assert_eq!(
            record["operationOutcome"],
            if expected_commit {
                "committed"
            } else {
                "interrupted"
            }
        );
        assert_eq!(
            reloaded.task(&task.task_id).unwrap().result.unwrap()["operationOutcome"],
            record["operationOutcome"]
        );
        assert_preserved(&files);
        assert!(fixture
            .installer
            .operation(&prepared.candidate_id)
            .unwrap()
            .exists());
        reloaded.shutdown().await;
        fixture.store.shutdown().await;
    }
}

#[tokio::test]
async fn cancellation_after_pointer_commit_cannot_relabel_an_installed_generation() {
    let fixture = Fixture::new(NPM_OK);
    let target = fixture.fresh_target();
    let prepared = fixture.prepare(target).await;
    let task = fixture
        .tasks
        .create_workspace_operation("nodeInstallApply")
        .unwrap();
    let mut journal = journal_for_commit(&fixture, &prepared, &task.task_id);
    let (_stop, cancel, control) = live_control();
    fixture
        .installer
        .commit_files(&mut journal, |point| {
            if matches!(point, CommitPoint::AfterPointer) {
                cancel.send(true).unwrap();
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(control.check().unwrap_err().code, "NODE_INSTALL_CANCELLED");
    assert!(fixture.installer.pointer_matches(&journal).unwrap());
    fixture.installer.recover(&fixture.tasks).unwrap();
    assert_eq!(
        fixture
            .installer
            .journal(&prepared.candidate_id)
            .unwrap()
            .state,
        "committed"
    );
    fixture.shutdown().await;
}

#[tokio::test]
async fn tampered_recovery_records_are_not_reported_as_committed() {
    let fixture = Fixture::new(NPM_OK);
    let prepared = fixture.prepare(fixture.fresh_target()).await;
    let path = fixture
        .installer
        .operation(&prepared.candidate_id)
        .unwrap()
        .join("install.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["state"] = json!("committed");
    value["prepared"] = Value::Null;
    atomic_json(&path, &value).unwrap();
    let recovered = fixture.installer.recover(&fixture.tasks).unwrap();
    assert_eq!(recovered[0]["state"], "needs-attention");
    assert!(recovered[0].get("operationOutcome").is_none());
    fixture.shutdown().await;
}

#[tokio::test]
async fn recovery_records_unknown_when_registration_cannot_be_verified() {
    let fixture = Fixture::new(NPM_OK);
    let target = fixture.fresh_target();
    let prepared = fixture.prepare(target).await;
    let task = fixture
        .tasks
        .create_workspace_operation("nodeInstallApply")
        .unwrap();
    let mut journal = journal_for_commit(&fixture, &prepared, &task.task_id);
    fixture
        .installer
        .commit_files(&mut journal, |_| Ok(()))
        .unwrap();
    let slot = fixture.installer.slot(&prepared.app_id).unwrap();
    fs::rename(
        slot.join("registration.json"),
        slot.join("registration.evidence.json"),
    )
    .unwrap();
    std::os::unix::fs::symlink("registration.evidence.json", slot.join("registration.json"))
        .unwrap();
    let recovered = fixture.installer.recover(&fixture.tasks).unwrap();
    assert_eq!(recovered[0]["state"], "needs-attention");
    assert_eq!(recovered[0]["operationOutcome"], "unknown");
    assert!(slot.join("registration.evidence.json").exists());
    assert!(slot.join("generations").join(prepared.generation).exists());
    fixture.shutdown().await;
}

async fn selected_real_tools(fixture: &Fixture) -> Toolchain {
    let managed = std::env::var_os("PLATFORM_TEST_NODE_ROOT").map(PathBuf::from);
    if let Some(root) = &managed {
        let artifact_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../.test-artifacts")
            .canonicalize()
            .unwrap();
        assert!(
            root.canonicalize().unwrap().starts_with(artifact_root),
            "real-tool opt-in copies must be disposable artifacts"
        );
    }
    EnvironmentManager::with_processes(
        fixture.paths.data_root().into(),
        managed,
        fixture.store.processes(),
    )
    .resolve()
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "opt-in: uses a real selected Node/npm; no application code or network dependency"]
async fn real_npm_prepares_a_dependency_free_program_with_distinct_empty_configs() {
    let mut fixture = Fixture::new(NPM_OK);
    fixture.tools = selected_real_tools(&fixture).await;
    fixture.prepare(fixture.fresh_target()).await;
    fixture.shutdown().await;
}

#[tokio::test]
#[ignore = "opt-in: real npm retries a disposable loopback tarball twice; default backoff takes about 70 seconds"]
async fn real_npm_recovers_after_two_transient_download_failures() {
    use base64::Engine;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let mut fixture = Fixture::new(NPM_OK);
    fixture.tools = selected_real_tools(&fixture).await;
    // Gzip-compressed tar containing only package/package.json, with the same
    // name/version checked below. No package scripts or network code run.
    let tarball = base64::engine::general_purpose::STANDARD.decode(
        "H4sIAAAAAAAC/+3NPQoCMRBA4dSeQqbWNZE1greJGmT9SUKyCiLe3aiFYC0i+L7mDTPFJLfauY2fpGebbYlBfZiubNs+Wr1X65l9zfe9MXY6V0OtvuBYepfre/WfLhLcwctC1l1Jsbjl3o+z7/NZRnLyuXQx1KNpdKPlOlAAAAAAAAAAAAAAAAAAgB9xA3uEERYAKAAA",
    ).unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let counted = requests.clone();
    let response_tarball = tarball.clone();
    let router = axum::Router::new().route(
        "/disposable-retry.tgz",
        axum::routing::get(move || {
            let attempt = counted.fetch_add(1, Ordering::SeqCst);
            let tarball = response_tarball.clone();
            async move {
                if attempt < 2 {
                    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, Vec::new())
                } else {
                    (axum::http::StatusCode::OK, tarball)
                }
            }
        }),
    );
    let (stop_server, stopped) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = stopped.await;
            })
            .await
            .unwrap();
    });
    atomic_json(&fixture.source.join("package.json"), &json!({"name":"disposable-install","version":"1.0.0","dependencies":{"disposable-retry":"1.0.0"}})).unwrap();
    atomic_json(&fixture.source.join("package-lock.json"), &json!({"lockfileVersion":3,"packages":{
        "":{"name":"disposable-install","version":"1.0.0","dependencies":{"disposable-retry":"1.0.0"}},
        "node_modules/disposable-retry":{"version":"1.0.0","resolved":format!("http://{address}/disposable-retry.tgz"),
            "integrity":format!("sha512-{}", base64::engine::general_purpose::STANDARD.encode(sha2::Sha512::digest(&tarball)))}
    }})).unwrap();
    let operation = fixture
        .source
        .parent()
        .unwrap()
        .join("disposable-retry-operation");
    private_dir(&operation).unwrap();
    let (_stop, _cancel, mut control) = live_control();
    control.deadline = Instant::now() + Duration::from_secs(120);
    // Exercise the production npm runner directly: loopback HTTP is only a
    // transport fixture; source validation still requires HTTPS in production.
    let result = npm_ci(
        &operation,
        &fixture.source,
        &fixture.tools,
        &control,
        NodeProcessScope::new(fixture.store.processes(), Arc::new(())),
    )
    .await;
    let _ = stop_server.send(());
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
    result.unwrap();
    assert_eq!(requests.load(Ordering::SeqCst), 3);
    let installed: Value = serde_json::from_slice(
        &fs::read(
            fixture
                .source
                .join("node_modules/disposable-retry/package.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        installed,
        json!({"name":"disposable-retry","version":"1.0.0"})
    );
    assert!(!operation.join("npm-disabled-output").exists());
    fixture.shutdown().await;
}

#[tokio::test]
#[ignore = "opt-in: real npm attempts only an intentionally refused loopback dependency download"]
async fn real_npm_download_failure_leaves_an_unregistered_candidate() {
    let mut fixture = Fixture::new(NPM_OK);
    fixture.tools = selected_real_tools(&fixture).await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    atomic_json(&fixture.source.join("package.json"), &json!({"name":"disposable-install","version":"1.0.0","dependencies":{"disposable-refused":"1.0.0"}})).unwrap();
    atomic_json(&fixture.source.join("package-lock.json"), &json!({"lockfileVersion":3,"packages":{
        "":{"dependencies":{"disposable-refused":"1.0.0"}},
        "node_modules/disposable-refused":{"version":"1.0.0","resolved":format!("https://{address}/disposable-refused.tgz")}
    }})).unwrap();
    let task = fixture
        .tasks
        .create_workspace_operation("nodeInstallPrepare")
        .unwrap();
    let (_stop, _cancel, mut control) = live_control();
    control.deadline = Instant::now() + Duration::from_secs(120);
    let target = fixture.fresh_target();
    let slot = fixture.installer.slot(&target.app_id).unwrap();
    let error = fixture
        .installer
        .prepare(
            &task.task_id,
            fixture.source(),
            target,
            fixture.tools.clone(),
            control,
            &fixture.tasks,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_INSTALL_NPM_FAILED");
    assert_no_task_logs(&fixture.paths, &task.task_id);
    let diagnostics = &error.details["processDiagnostics"];
    assert!(diagnostics["exitCode"]
        .as_i64()
        .is_some_and(|code| code != 0));
    assert!(
        diagnostics["stderr"]
            .as_str()
            .unwrap()
            .contains("ECONNREFUSED"),
        "{error:?}"
    );
    assert!(!slot.exists());
    fixture.shutdown().await;
}

#[path = "node_app_install_policy_tests.rs"]
mod policy;

#[tokio::test]
async fn late_start_cleanup_gate_blocks_pointer_publication_and_preserves_existing_data() {
    let fixture = Fixture::new(NPM_OK);
    let fresh = fixture.fresh_target();
    let installed = fixture.apply(&fixture.prepare(fresh.clone()).await, &fresh);
    let slot = fixture.installer.slot(&installed.app_id).unwrap();
    let files = sentinels(&slot);
    let registration = fs::read(slot.join("registration.json")).unwrap();
    let target = fixture.update_target(&installed, 7);
    let prepared = fixture.prepare(target.clone()).await;
    let task = fixture
        .tasks
        .create_workspace_operation("nodeInstallApply")
        .unwrap();
    let mut journal = journal_for_commit(&fixture, &prepared, &task.task_id);
    let gate = slot.join(".start-intent-cleanup-disposable-late-publication");
    let error = fixture
        .installer
        .commit_files(&mut journal, |point| {
            if matches!(point, CommitPoint::BeforePointer) {
                fs::write(&gate, b"preserve-unconfirmed-intent-cleanup")?;
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, "NODE_INSTALL_BUSY");
    assert_eq!(
        fs::read(slot.join("registration.json")).unwrap(),
        registration
    );
    assert_eq!(
        fs::read(gate).unwrap(),
        b"preserve-unconfirmed-intent-cleanup"
    );
    assert_preserved(&files);
    assert_eq!(
        fixture.installer.check_target(&target).unwrap_err().code,
        "NODE_INSTALL_BUSY"
    );
    fixture.shutdown().await;
}
