use super::*;
use crate::environment::{EnvironmentManager, ExecutableInfo};
use std::sync::Arc;

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    project: PathBuf,
    operation: PathBuf,
    manifest: PandaManifest,
    tools: Toolchain,
    _stop: watch::Sender<bool>,
    cancel: watch::Sender<bool>,
    control: DependencyControl,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::Builder::new()
            .prefix("disposable-node-direct-dependencies-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let project = root.join("project");
        let operation = root.join("operation");
        fs::create_dir_all(project.join("backend")).unwrap();
        fs::create_dir_all(project.join("web")).unwrap();
        fs::create_dir(&operation).unwrap();
        fs::write(project.join("web/index.html"), "disposable").unwrap();
        fs::write(
            project.join("backend/server.mjs"),
            "throw Error('must not start app');",
        )
        .unwrap();
        let manifest = PandaManifest::parse(
            &serde_json::to_vec(&json!({
                "manifestVersion":2,"app":{"name":"Disposable dependencies","version":"1"},
                "web":{"root":"web","entry":"index.html"},
                "backend":{"root":"backend","entry":"server.mjs","protocolVersion":1,"nodeMajor":24}
            }))
            .unwrap(),
        )
        .unwrap();
        let fake = root.join("fake-npm.sh");
        fs::write(&fake, "set -eu\n[ \"$1\" = '--prefix' ]\n[ \"$2\" = \"$PWD\" ]\n[ -f \"$NPM_CONFIG_USERCONFIG\" ]\n[ -f \"$NPM_CONFIG_GLOBALCONFIG\" ]\n[ \"$NPM_CONFIG_USERCONFIG\" != \"$NPM_CONFIG_GLOBALCONFIG\" ]\n[ -z \"${NODE_OPTIONS-}\" ]\n[ -z \"${NODE_PATH-}\" ]\nprintf '%s\\n' \"$@\" > dependency-arguments.txt\n").unwrap();
        let tools = Toolchain {
            node: ExecutableInfo {
                path: "/bin/sh".into(),
                version: "test".into(),
            },
            npm: ExecutableInfo {
                path: fake,
                version: "test".into(),
            },
            path: "/usr/bin:/bin".into(),
        };
        let (stop, stop_rx) = watch::channel(false);
        let (cancel, cancel_rx) = watch::channel(false);
        Self {
            _temp: temp,
            root,
            project,
            operation,
            manifest,
            tools,
            _stop: stop,
            cancel,
            control: DependencyControl::new(stop_rx, cancel_rx),
        }
    }
    fn package(&self, directory: &Path) {
        fs::create_dir_all(directory).unwrap();
        fs::write(directory.join("package.json"), serde_json::to_vec(&json!({"name":"disposable-direct-dependencies","version":"1.0.0","scripts":{"preinstall":"node marker.mjs", "install":"node marker.mjs", "postinstall":"node marker.mjs"}})).unwrap()).unwrap();
        fs::write(directory.join("package-lock.json"), serde_json::to_vec(&json!({"name":"disposable-direct-dependencies","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"disposable-direct-dependencies","version":"1.0.0"}}})).unwrap()).unwrap();
        fs::write(directory.join("marker.mjs"), "import{writeFileSync}from'node:fs';writeFileSync('repository-script-executed','unsafe');").unwrap();
    }
    async fn prepare(&self) -> Result<DependencyPreparation> {
        prepare_dependencies(
            &self.project,
            &self.manifest,
            &self.operation,
            &self.tools,
            &self.control,
            NodeProcessScope::standalone(),
        )
        .await
    }
}

#[tokio::test]
async fn zero_dependencies_skip_tools_and_material_creation() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.prepare().await.unwrap(),
        DependencyPreparation::Skipped
    );
    assert_eq!(fs::read_dir(&fixture.operation).unwrap().count(), 0);
    assert!(!fixture.project.join("dependency-arguments.txt").exists());
}

#[tokio::test]
async fn production_dependencies_use_selected_directory_and_isolated_npm() {
    for backend in [false, true] {
        let fixture = Fixture::new();
        let package = if backend {
            fixture.project.join("backend")
        } else {
            fixture.project.clone()
        };
        fixture.package(&package);
        assert_eq!(
            fixture.prepare().await.unwrap(),
            DependencyPreparation::Installed {
                npm_root: package.clone()
            }
        );
        let args = fs::read_to_string(package.join("dependency-arguments.txt")).unwrap();
        for flag in [
            "ci",
            "--omit=dev",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--fetch-retries=2",
            "--fetch-timeout=60000",
        ] {
            assert!(args.lines().any(|line| line == flag), "missing {flag}");
        }
        assert!(!package.join("repository-script-executed").exists());
        assert!(fixture.operation.join("npm-cache").is_dir());
        assert!(fixture.operation.join("npm-home/user.npmrc").is_file());
    }
}

#[test]
fn same_backend_and_project_root_are_counted_once() {
    let mut fixture = Fixture::new();
    fixture.manifest.backend.as_mut().unwrap().root = ".".into();
    fixture.package(&fixture.project);
    assert_eq!(
        dependency_root(&fixture.project, &fixture.manifest).unwrap(),
        Some(fixture.project.clone())
    );
}

#[tokio::test]
async fn missing_locks_ambiguous_inputs_and_bad_formats_fail_before_npm() {
    for problem in [
        "missing-lock",
        "both-roots",
        "lock-only",
        "bad-json",
        "old-lock",
        "shrinkwrap",
    ] {
        let fixture = Fixture::new();
        fixture.package(&fixture.project);
        match problem {
            "missing-lock" => fs::remove_file(fixture.project.join("package-lock.json")).unwrap(),
            "both-roots" => fixture.package(&fixture.project.join("backend")),
            "lock-only" => fs::remove_file(fixture.project.join("package.json")).unwrap(),
            "bad-json" => fs::write(fixture.project.join("package.json"), "[]").unwrap(),
            "old-lock" => fs::write(
                fixture.project.join("package-lock.json"),
                "{\"lockfileVersion\":1}",
            )
            .unwrap(),
            "shrinkwrap" => fs::write(fixture.project.join("npm-shrinkwrap.json"), "{}").unwrap(),
            _ => unreachable!(),
        }
        assert_eq!(
            fixture.prepare().await.unwrap_err().code,
            "NODE_DEPENDENCY_NEEDS_ADAPTATION",
            "{problem}"
        );
        assert!(!fixture.project.join("dependency-arguments.txt").exists());
        assert_eq!(fs::read_dir(&fixture.operation).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn repository_npmrc_cannot_reenable_lifecycle_scripts() {
    for backend in [false, true] {
        let fixture = Fixture::new();
        fixture.package(&fixture.project);
        let directory = if backend {
            fixture.project.join("backend")
        } else {
            fixture.project.clone()
        };
        fs::write(
            directory.join(".npmrc"),
            "ignore-scripts=false\nsecret-token=do-not-echo\n",
        )
        .unwrap();
        let error = fixture.prepare().await.unwrap_err();
        assert_eq!(error.code, "NODE_DEPENDENCY_NEEDS_ADAPTATION");
        assert!(!serde_json::to_string(&error)
            .unwrap()
            .contains("do-not-echo"));
        assert!(!fixture.project.join("dependency-arguments.txt").exists());
        assert!(!fixture.project.join("repository-script-executed").exists());
    }
}

#[test]
fn space_budget_aggregates_project_and_cache_without_double_counting() {
    let mut fixture = Fixture::new();
    fs::write(
        fixture.project.join("disposable-project-payload"),
        vec![0u8; 16],
    )
    .unwrap();
    fs::write(
        fixture.operation.join("disposable-cache-payload"),
        vec![0u8; 16],
    )
    .unwrap();
    let project_size: u64 = [
        "disposable-project-payload",
        "web/index.html",
        "backend/server.mjs",
    ]
    .iter()
    .map(|path| fs::metadata(fixture.project.join(path)).unwrap().len())
    .sum();
    fixture.control.npm_space.max_total = project_size + 15;
    assert_eq!(
        scan_dependency_space(
            &[&fixture.project, &fixture.operation],
            &fixture.control,
            |_| Ok(u64::MAX)
        )
        .unwrap_err()
        .code,
        "NODE_INSTALL_SPACE_LIMIT"
    );
    fixture.control.npm_space.max_total = project_size;
    scan_dependency_space(
        &[&fixture.project, &fixture.project.join("backend")],
        &fixture.control,
        |_| Ok(u64::MAX),
    )
    .unwrap();
}

#[tokio::test]
async fn cancelled_preparation_never_starts_npm() {
    let fixture = Fixture::new();
    fixture.package(&fixture.project);
    fixture.cancel.send(true).unwrap();
    assert_eq!(
        fixture.prepare().await.unwrap_err().code,
        "NODE_INSTALL_CANCELLED"
    );
    assert!(!fixture.project.join("dependency-arguments.txt").exists());
}

#[test]
fn installed_native_extensions_and_external_links_need_adaptation() {
    let fixture = Fixture::new();
    let modules = fixture.project.join("node_modules");
    fs::create_dir(&modules).unwrap();
    fs::write(modules.join("disposable.node"), "native").unwrap();
    assert!(validate_dependency_tree(&modules, &fixture.project).is_err());
    fs::remove_file(modules.join("disposable.node")).unwrap();
    std::os::unix::fs::symlink(&fixture.root, modules.join("external")).unwrap();
    assert!(validate_dependency_tree(&modules, &fixture.project).is_err());
}

#[tokio::test]
#[ignore = "opt-in: real selected Node/npm, dependency-free lock, no network packages"]
async fn real_npm_skips_all_repository_lifecycle_scripts_in_the_published_directory() {
    let mut fixture = Fixture::new();
    fixture.package(&fixture.project);
    fixture.tools = EnvironmentManager::new(fixture.root.join("disposable-toolchain-settings"))
        .resolve()
        .await
        .unwrap();
    let registry = crate::owned_process::OwnedProcessRegistry::default();
    prepare_dependencies(
        &fixture.project,
        &fixture.manifest,
        &fixture.operation,
        &fixture.tools,
        &fixture.control,
        NodeProcessScope::new(registry.clone(), Arc::new(())),
    )
    .await
    .unwrap();
    assert!(!fixture.project.join("repository-script-executed").exists());
    assert!(!fixture.project.join("dependency-arguments.txt").exists());
    assert!(registry.shutdown().await.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_waits_for_actual_npm_output_reader_before_releasing_operation() {
    let fixture = Arc::new(Fixture::new());
    fixture.package(&fixture.project);
    let registry = crate::owned_process::OwnedProcessRegistry::default();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = tokio::sync::oneshot::channel();
    let scope = NodeProcessScope::new(registry.clone(), fixture.clone()).with_reader_fault(
        crate::node_process_scope::NodeReaderFault {
            component: "node-install/npm".into(),
            step: "ci".into(),
            entered,
            release: Some(blocked),
            panic: false,
            writer: None,
        },
    );
    let work = {
        let fixture = fixture.clone();
        tokio::spawn(async move {
            prepare_dependencies(
                &fixture.project,
                &fixture.manifest,
                &fixture.operation,
                &fixture.tools,
                &fixture.control,
                scope,
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();
    fixture.cancel.send(true).unwrap();
    tokio::time::sleep(Duration::from_millis(75)).await;
    assert!(!work.is_finished());
    assert!(fixture.operation.join("npm-cache").is_dir());
    release.send(()).unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), work)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.details["processResourceClosed"], true);
    assert!(registry.shutdown().await.is_empty());
}
