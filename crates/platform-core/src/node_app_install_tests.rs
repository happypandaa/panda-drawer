use super::*;
use crate::{environment::EnvironmentManager, ProjectStore, RuntimePaths};
use std::sync::Arc;

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
            "set -eu\n[ \"$1\" = '--prefix' ]\n[ \"$2\" = \"$PWD\" ]\n[ \"$NPM_CONFIG_USERCONFIG\" != \"$NPM_CONFIG_GLOBALCONFIG\" ]\n[ -f \"$NPM_CONFIG_USERCONFIG\" ]\n[ -f \"$NPM_CONFIG_GLOBALCONFIG\" ]\n[ -z \"${{NODE_PATH-}}\" ]\n[ -z \"${{NODE_OPTIONS-}}\" ]\n{npm_body}\n"
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
        let installer = NodeAppInstaller::new(paths.data_root().to_path_buf()).unwrap();
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
async fn bundled_platform_tools_are_preserved_hashed_and_not_executed() {
    let fixture = Fixture::new(&format!(
        "{NPM_OK}\nmkdir -p node_modules/platform-tools\nprintf 'MZ-disposable-windows-tool' > node_modules/platform-tools/service.exe\nprintf 'MZ-disposable-windows-library' > node_modules/platform-tools/service.DLL\nprintf 'disposable-library' > node_modules/platform-tools/library.so\nprintf 'disposable-library' > node_modules/platform-tools/library.dylib\nprintf '#!/bin/sh\\nexit 91\\n' > node_modules/platform-tools/tool.EXE\nchmod +x node_modules/platform-tools/tool.EXE"
    ));
    let target = InstallTargetSnapshot::fresh();
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
        let target = InstallTargetSnapshot::fresh();
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

fn public_logs(tasks: &BuildManager, task_id: &str) -> String {
    let mut cursor = 0;
    let mut text = String::new();
    loop {
        let page = tasks.logs(task_id, cursor).unwrap();
        assert!(!page.truncated);
        assert!(page.gaps.is_empty());
        text.push_str(&page.text);
        if page.eof {
            return text;
        }
        assert!(
            page.next_cursor > cursor,
            "final task log must make progress"
        );
        cursor = page.next_cursor;
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

#[tokio::test]
async fn install_and_compatible_update_keep_uuid_configuration_data_and_old_program() {
    let fixture = Fixture::new(NPM_OK);
    let target = InstallTargetSnapshot::fresh();
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
    fixture.shutdown().await;
}

#[tokio::test]
async fn npm_failure_retains_candidate_and_never_registers_or_rewrites_existing_slot() {
    let mut fixture = Fixture::new(NPM_OK);
    let first = InstallTargetSnapshot::fresh();
    let installed = fixture.apply(&fixture.prepare(first.clone()).await, &first);
    let slot = fixture.installer.slot(&installed.app_id).unwrap();
    let files = sentinels(&slot);
    let before = fs::read(slot.join("registration.json")).unwrap();
    fs::write(&fixture.tools.npm.path, r#"
mkdir -p node_modules
printf candidate-only > node_modules/evidence
awk 'BEGIN { for (i = 0; i < 42000; i++) printf "~"; print "\nstdout-tail" }'
awk 'BEGIN { for (i = 0; i < 42000; i++) printf "^"; print "\nstderr-tail: simulated E404 download failure" }' >&2
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
    let final_task = fixture.tasks.task(&task.task_id).unwrap();
    assert!(final_task.status.is_final());
    let logs = public_logs(&fixture.tasks, &task.task_id);
    assert!(logs.len() > 64_000, "exercise the public log's next cursor");
    assert_eq!(logs.bytes().filter(|byte| *byte == b'~').count(), 42000);
    assert_eq!(logs.bytes().filter(|byte| *byte == b'^').count(), 42000);
    assert!(logs.contains("stdout-tail"));
    assert!(logs.contains("stderr-tail: simulated E404 download failure"));
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
async fn cancellation_terminates_npm_before_reporting_completion_and_preserves_evidence() {
    let fixture = Fixture::new("printf 'stdout-before-cancel\n'; printf 'stderr-before-cancel\n' >&2; printf '%s' \"$$\" > npm-fixture-pid; /bin/sleep 60 & wait");
    let target = InstallTargetSnapshot::fresh();
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
    let logs = public_logs(&fixture.tasks, &task.task_id);
    assert!(logs.contains("stdout-before-cancel"));
    assert!(logs.contains("stderr-before-cancel"));
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
async fn npm_output_limit_preserves_bounded_public_output_and_stops_the_process_group() {
    for redirect in ["", " >&2"] {
        let fixture = Fixture::new(&format!(
            "printf '%s' \"$$\" > npm-fixture-pid; awk 'BEGIN {{ for (i = 0; i < {}; i++) printf \"x\" }}'{redirect}; /bin/sleep 60 & wait",
            MAX_OUTPUT + 1,
        ));
        let task = fixture
            .tasks
            .create_workspace_operation("nodeInstallPrepare")
            .unwrap();
        let target = InstallTargetSnapshot::fresh();
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
        let logs = public_logs(&fixture.tasks, &task.task_id);
        let output = logs.rsplit_once('\n').unwrap().1;
        assert_eq!(output, "x".repeat(MAX_OUTPUT));
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
        let target = InstallTargetSnapshot::fresh();
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
    let target = InstallTargetSnapshot::fresh();
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
    let target = InstallTargetSnapshot::fresh();
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
        let target = InstallTargetSnapshot::fresh();
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
    let target = InstallTargetSnapshot::fresh();
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
    let prepared = fixture.prepare(InstallTargetSnapshot::fresh()).await;
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
    let target = InstallTargetSnapshot::fresh();
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

#[tokio::test]
#[ignore = "opt-in: uses a real selected Node/npm; no application code or network dependency"]
async fn real_npm_prepares_a_dependency_free_program_with_distinct_empty_configs() {
    let mut fixture = Fixture::new(NPM_OK);
    fixture.tools = EnvironmentManager::new(fixture.paths.data_root().to_path_buf())
        .resolve()
        .await
        .unwrap();
    fixture.prepare(InstallTargetSnapshot::fresh()).await;
    fixture.shutdown().await;
}

#[tokio::test]
#[ignore = "opt-in: real npm attempts only an intentionally refused loopback dependency download"]
async fn real_npm_download_failure_leaves_an_unregistered_candidate() {
    let mut fixture = Fixture::new(NPM_OK);
    fixture.tools = EnvironmentManager::new(fixture.paths.data_root().to_path_buf())
        .resolve()
        .await
        .unwrap();
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
    control.deadline = Instant::now() + Duration::from_secs(20);
    let target = InstallTargetSnapshot::fresh();
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
    assert!(public_logs(&fixture.tasks, &task.task_id).contains("ECONNREFUSED"));
    assert!(!slot.exists());
    fixture.shutdown().await;
}

#[path = "node_app_install_policy_tests.rs"]
mod policy;
