//! Actual official npm in newly created disposable source and operation trees.
use super::*;
use crate::environment::EnvironmentManager;
use sha2::{Digest, Sha256};

struct Fixture {
    _temp: tempfile::TempDir,
    project: PathBuf,
    operation: PathBuf,
    manifest: PandaManifest,
    _stop: watch::Sender<bool>,
    cancel: watch::Sender<bool>,
    control: DependencyControl,
}
impl Fixture {
    fn new(long: bool) -> Self {
        let temp = tempfile::Builder::new()
            .prefix("panda-disposable-w4-npm-中文 space-")
            .tempdir()
            .unwrap();
        let base = temp.path().canonicalize().unwrap();
        let mut project = base.join("项目 中文 space");
        if long {
            for i in 0..9 {
                project = project.join(format!("nested-{i}-long-path-for-npm"));
            }
        }
        fs::create_dir_all(project.join("web")).unwrap();
        fs::create_dir(project.join("backend")).unwrap();
        fs::write(project.join("web/index.html"), "disposable").unwrap();
        fs::write(
            project.join("backend/main.mjs"),
            "throw Error('application must not run in npm');",
        )
        .unwrap();
        let operation = base.join("operation 中文 space");
        private_dir(&operation).unwrap();
        let manifest = PandaManifest::parse(
            &serde_json::to_vec(&json!({
                "manifestVersion":1,"runtime":"node",
                "app":{"name":"临时 npm","version":"1"},
                "web":{"root":"web","entry":"index.html"},
                "backend":{"root":"backend","entry":"main.mjs","protocolVersion":1,"nodeMajor":24}
            }))
            .unwrap(),
        )
        .unwrap();
        let (stop, rx) = watch::channel(false);
        let (cancel, cx) = watch::channel(false);
        Self {
            _temp: temp,
            project,
            operation,
            manifest,
            _stop: stop,
            cancel,
            control: DependencyControl::new(rx, cx),
        }
    }
    fn package(&self, root: &Path) {
        fs::write(root.join("package.json"),serde_json::to_vec(&json!({"name":"disposable-npm","version":"1.0.0",
            "scripts":{"preinstall":"node lifecycle.mjs","install":"node lifecycle.mjs","postinstall":"node lifecycle.mjs","prepare":"node lifecycle.mjs"}})).unwrap()).unwrap();
        fs::write(
            root.join("package-lock.json"),
            serde_json::to_vec(
                &json!({"name":"disposable-npm","version":"1.0.0","lockfileVersion":3,
            "packages":{"":{"name":"disposable-npm","version":"1.0.0","hasInstallScript":true}}}),
            )
            .unwrap(),
        )
        .unwrap();
        fs::write(
            root.join("lifecycle.mjs"),
            "import{writeFileSync}from'node:fs';writeFileSync('must-not-run','unsafe')",
        )
        .unwrap();
    }
}
fn unusable_tools() -> Toolchain {
    use crate::environment::ExecutableInfo;
    Toolchain {
        node: ExecutableInfo {
            path: r"C:\missing-disposable\node.exe".into(),
            version: "not executed".into(),
        },
        npm: ExecutableInfo {
            path: r"C:\missing-disposable\npm-cli.js".into(),
            version: "not executed".into(),
        },
        path: String::new(),
    }
}

#[tokio::test]
async fn zero_dependencies_skip_tools_and_operation_material_and_precancel_does_not_spawn() {
    let f = Fixture::new(false);
    let scope = NodeProcessScope::standalone();
    assert_eq!(
        prepare_dependencies(
            &f.project,
            &f.manifest,
            &f.operation,
            &unusable_tools(),
            &f.control,
            scope.clone()
        )
        .await
        .unwrap(),
        DependencyPreparation::Skipped
    );
    assert_eq!(fs::read_dir(&f.operation).unwrap().count(), 0);
    f.package(&f.project);
    f.cancel.send(true).unwrap();
    assert_eq!(
        prepare_dependencies(
            &f.project,
            &f.manifest,
            &f.operation,
            &unusable_tools(),
            &f.control,
            scope.clone()
        )
        .await
        .unwrap_err()
        .code,
        "NODE_INSTALL_CANCELLED"
    );
    assert!(scope.processes.snapshot().is_empty());
    assert_eq!(fs::read_dir(&f.operation).unwrap().count(), 0);
}

#[tokio::test]
async fn missing_lock_dual_roots_and_repository_npmrc_fail_before_tool_execution() {
    for problem in ["missing-lock", "dual-root", "npmrc"] {
        let f = Fixture::new(false);
        f.package(&f.project);
        match problem {
            "missing-lock" => fs::remove_file(f.project.join("package-lock.json")).unwrap(),
            "dual-root" => f.package(&f.project.join("backend")),
            "npmrc" => fs::write(f.project.join(".npmrc"), "ignore-scripts=false\n").unwrap(),
            _ => unreachable!(),
        }
        let scope = NodeProcessScope::standalone();
        assert_eq!(
            prepare_dependencies(
                &f.project,
                &f.manifest,
                &f.operation,
                &unusable_tools(),
                &f.control,
                scope.clone()
            )
            .await
            .unwrap_err()
            .code,
            "NODE_DEPENDENCY_NEEDS_ADAPTATION"
        );
        assert!(scope.processes.snapshot().is_empty());
        assert_eq!(fs::read_dir(&f.operation).unwrap().count(), 0);
        assert!(!f.project.join("must-not-run").exists());
    }
}

#[tokio::test]
#[ignore = "Explicit fixed PANDA_W3_OFFICIAL_NODE with its original official npm tree required"]
async fn official_npm_runs_at_root_or_backend_in_long_chinese_paths_without_lifecycle_scripts() {
    let node =
        PathBuf::from(std::env::var_os("PANDA_W3_OFFICIAL_NODE").expect("explicit official Node"));
    let release: Value = serde_json::from_str(crate::node_tool_layout::RUNTIME_RELEASE).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(fs::read(&node).unwrap())),
        release["binary"]["sha256"].as_str().unwrap()
    );
    for backend in [false, true] {
        let f = Fixture::new(true);
        let root = if backend {
            f.project.join("backend")
        } else {
            f.project.clone()
        };
        f.package(&root);
        let mut package: Value =
            serde_json::from_slice(&fs::read(root.join("package.json")).unwrap()).unwrap();
        package["dependencies"] = json!({"is-number":"7.0.0"});
        fs::write(
            root.join("package.json"),
            serde_json::to_vec(&package).unwrap(),
        )
        .unwrap();
        fs::write(root.join("package-lock.json"), serde_json::to_vec(&json!({"name":"disposable-npm","version":"1.0.0","lockfileVersion":3,
            "packages":{"":{"name":"disposable-npm","version":"1.0.0","hasInstallScript":true,"dependencies":{"is-number":"7.0.0"}},
            "node_modules/is-number":{"version":"7.0.0","resolved":"https://registry.npmjs.org/is-number/-/is-number-7.0.0.tgz",
            "integrity":"sha512-41Cifkg6e8TylSpdtTpeLVMqvSBEVzTttHvERD741+pnZ8ANv0004MRL43QKPDlK9cGvNp6NZWZUBlbGXYxxng=="}}})).unwrap()).unwrap();
        assert!(root.as_os_str().len() > 260);
        let environment = EnvironmentManager::new(f._temp.path().join("settings"));
        let info = environment
            .configure_node(Some(node.clone()))
            .await
            .unwrap();
        assert!(info.ready, "{:?}", info.issues);
        let tools = environment.resolve().await.unwrap();
        let scope = NodeProcessScope::standalone()
            .with_operation(&f.operation)
            .unwrap();
        let before = fs::read(root.join("package.json")).unwrap();
        let result = prepare_dependencies(
            &f.project,
            &f.manifest,
            &f.operation,
            &tools,
            &f.control,
            scope.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            result,
            DependencyPreparation::Installed {
                npm_root: root.clone()
            }
        );
        assert_eq!(fs::read(root.join("package.json")).unwrap(), before);
        assert!(!root.join("must-not-run").exists());
        let installed = root.join("node_modules/is-number/index.js");
        assert!(installed.as_os_str().len() > 260);
        assert!(fs::read_to_string(installed)
            .unwrap()
            .contains("module.exports"));
        assert!(f.operation.join("npm-home/user.npmrc").is_file());
        assert!(scope.processes.shutdown().await.is_empty());
        assert!(scope
            .processes
            .snapshot()
            .iter()
            .all(|s| s.phase == ProcessPhase::Closed));
    }
}
