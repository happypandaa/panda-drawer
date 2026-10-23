use super::*;
use crate::{
    builds::{atomic_json, BuildManager},
    environment::ExecutableInfo,
    git_environment::GitEnvironmentManager,
    source_acquisition::{CaptureLocalSourceInput, SourceAcquisitionManager},
    ProjectStore, RuntimePaths,
};
use serde_json::json;
use std::{os::unix::fs::symlink, sync::Arc};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    operation: PathBuf,
    tools: Toolchain,
    manifest: Value,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().canonicalize().unwrap();
        let root = directory.join("disposable-node-authoritative-source");
        fs::create_dir_all(root.join("public")).unwrap();
        fs::create_dir_all(root.join("server")).unwrap();
        fs::write(root.join("public/index.html"), "disposable program").unwrap();
        fs::write(
            root.join("server/main.mjs"),
            "throw Error('never start the application during builds')",
        )
        .unwrap();
        let manifest = json!({
            "packageFormat":"local-web-app", "formatVersion":2, "runtime":"node-service",
            "app":{"id":"disposable.node.build", "name":"可丢弃开发构建", "version":"1.0"},
            "web":{"root":"public", "entry":"index.html", "routing":"files"},
            "backend":{"protocolVersion":1,"root":"server","entry":"main.mjs","nodeMajor":24}
        });
        atomic_json(&root.join("local-app.json"), &manifest).unwrap();
        atomic_json(&root.join("package.json"), &json!({"name":"disposable-build","version":"1.0.0","scripts":{"build":"node build.mjs"}})).unwrap();
        atomic_json(&root.join("package-lock.json"), &json!({"name":"disposable-build","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"disposable-build","version":"1.0.0"}}})).unwrap();
        let fake_npm = directory.join("disposable-npm.sh");
        fs::write(&fake_npm, "set -eu\nif [ \"$1\" = run ]; then\n mkdir release\n cp local-app.json package.json package-lock.json release/\n cp -R public server release/\nfi\n").unwrap();
        let tools = Toolchain {
            node: ExecutableInfo {
                path: "/bin/sh".into(),
                version: "artificial build interpreter".into(),
            },
            npm: ExecutableInfo {
                path: fake_npm,
                version: "artificial npm".into(),
            },
            path: "/usr/bin:/bin".into(),
        };
        Self {
            operation: directory.join("disposable-build-operation"),
            _temp: temp,
            root,
            tools,
            manifest,
        }
    }
    fn npm(&mut self) {
        self.manifest["development"] =
            json!({"sourceDir":".","build":{"kind":"npm","script":"build"},"outputDir":"release"});
        self.write_manifest();
    }
    fn write_manifest(&self) {
        atomic_json(&self.root.join("local-app.json"), &self.manifest).unwrap();
    }
    async fn build(&self) -> Result<NodeDevelopmentBuildOutput> {
        let (_stop, stop) = watch::channel(false);
        let (_cancel, cancel) = watch::channel(false);
        build_repository(&self.root, &self.operation, &self.tools, stop, cancel).await
    }
}

#[test]
fn development_declaration_preserves_legacy_and_validates_one_root() {
    let fixture = Fixture::new();
    assert!(
        TrialManifest::parse(&serde_json::to_vec(&fixture.manifest).unwrap())
            .unwrap()
            .development
            .is_none()
    );
    for declaration in [
        json!({"sourceDir":".","build":{"kind":"none"},"outputDir":"."}),
        json!({"sourceDir":".","build":{"kind":"npm","script":"build:app"},"outputDir":"release"}),
    ] {
        let mut value = fixture.manifest.clone();
        value["development"] = declaration;
        TrialManifest::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
    }
    for declaration in [
        json!({"sourceDir":"server","build":{"kind":"none"},"outputDir":"."}),
        json!({"sourceDir":".","build":{"kind":"npm","script":"build"},"outputDir":"."}),
        json!({"sourceDir":".","build":{"kind":"npm","script":"build; echo no"},"outputDir":"release"}),
        json!({"sourceDir":".","build":{"kind":"npm","script":"build"},"outputDir":"../release"}),
        json!({"sourceDir":".","build":{"kind":"npm","script":"build"},"outputDir":"data/release"}),
        json!({"sourceDir":".","build":{"kind":"none","script":"build"},"outputDir":"."}),
        json!({"sourceDir":".","build":{"kind":"none"},"outputDir":"release"}),
        json!({"sourceDir":".","build":{"kind":"none"},"outputDir":".","unknown":true}),
    ] {
        let mut value = fixture.manifest.clone();
        value["development"] = declaration;
        assert!(
            TrialManifest::parse(&serde_json::to_vec(&value).unwrap()).is_err(),
            "{value}"
        );
    }
}

#[tokio::test]
async fn direct_javascript_copies_program_without_second_git_or_private_inputs() {
    let fixture = Fixture::new();
    for (path, content) in [
        (".git/HEAD", "preserve-author-history"),
        ("data/user.db", "preserve-data"),
        (".data/private", "preserve-local-data"),
        ("user/taste.md", "preserve-private-preferences"),
        ("work/history.png", "preserve-private-history"),
        ("unknown-private/local.json", "unknown-not-source"),
        (
            "configuration/credentials/key",
            "artificial-not-a-real-secret",
        ),
        ("node_modules/private/module.js", "preserve-dependencies"),
        (".env", "artificial-env-value"),
    ] {
        let file = fixture.root.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, content).unwrap();
    }
    fs::create_dir_all(fixture.root.join("server/user")).unwrap();
    fs::write(
        fixture.root.join("server/user/routes.mjs"),
        "export const publicRoute = true;",
    )
    .unwrap();
    fs::write(
        fixture.root.join(".env.example"),
        "PLACEHOLDER=public-template",
    )
    .unwrap();
    let output = fixture.build().await.unwrap();
    assert!(!output.built);
    assert!(output.program_root.join("server/user/routes.mjs").is_file());
    assert!(output.program_root.join(".env.example").is_file());
    output.verify().unwrap();
    assert!(output.program_root.join("server/main.mjs").is_file());
    for path in [
        ".git",
        "data",
        ".data",
        "user",
        "work",
        "unknown-private",
        "configuration",
        "node_modules",
        ".env",
    ] {
        assert!(!output.program_root.join(path).exists());
        assert!(fixture.root.join(path).exists());
        assert!(output.excluded_paths.iter().any(|value| value == path));
    }
    assert_eq!(
        fs::read_to_string(fixture.root.join(".git/HEAD")).unwrap(),
        "preserve-author-history"
    );
    assert_ne!(
        fs::metadata(fixture.root.join("server/main.mjs"))
            .unwrap()
            .ino(),
        fs::metadata(output.program_root.join("server/main.mjs"))
            .unwrap()
            .ino()
    );
}

#[tokio::test]
async fn explicit_npm_build_uses_copy_and_distinct_output() {
    let mut fixture = Fixture::new();
    fixture.npm();
    fs::create_dir(fixture.root.join("release")).unwrap();
    fs::write(fixture.root.join("release/old"), "old output retained").unwrap();
    let output = fixture.build().await.unwrap();
    assert!(output.built);
    output.verify().unwrap();
    assert!(output.program_root.join("public/index.html").is_file());
    assert!(!output.program_root.join("release").exists());
    assert_eq!(
        fs::read_to_string(fixture.root.join("release/old")).unwrap(),
        "old output retained"
    );
    assert!(!fixture.root.join("node_modules").exists());
}

#[tokio::test]
async fn unknown_links_hard_links_and_special_files_are_rejected() {
    let fixture = Fixture::new();
    symlink("server/main.mjs", fixture.root.join("linked.mjs")).unwrap();
    assert!(fixture.build().await.is_err());
    fs::remove_file(fixture.root.join("linked.mjs")).unwrap();
    fs::hard_link(
        fixture.root.join("server/main.mjs"),
        fixture.root.join("hard.mjs"),
    )
    .unwrap();
    assert!(fixture.build().await.is_err());
    fs::remove_file(fixture.root.join("hard.mjs")).unwrap();
    let fifo =
        std::ffi::CString::new(fixture.root.join("pipe").as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(fixture.build().await.is_err());
}

#[tokio::test]
async fn existing_or_overlapping_operation_never_overwrites_files() {
    let fixture = Fixture::new();
    fs::create_dir(&fixture.operation).unwrap();
    fs::write(fixture.operation.join("keep"), "preserved").unwrap();
    assert!(fixture.build().await.is_err());
    let (_stop, stop) = watch::channel(false);
    let (_cancel, cancel) = watch::channel(false);
    assert!(build_repository(
        &fixture.root,
        &fixture.root.join("operation"),
        &fixture.tools,
        stop,
        cancel
    )
    .await
    .is_err());
    assert_eq!(
        fs::read_to_string(fixture.operation.join("keep")).unwrap(),
        "preserved"
    );
}

#[tokio::test]
async fn failed_or_mismatched_build_never_returns_installable_program() {
    let mut fixture = Fixture::new();
    fixture.npm();
    fs::write(&fixture.tools.npm.path, "exit 3\n").unwrap();
    assert_eq!(
        fixture.build().await.unwrap_err().code,
        "NODE_DEVELOPMENT_BUILD_FAILED"
    );
    assert!(fixture.operation.is_dir());
    assert!(!fixture.operation.join("program").exists());
    let mut fixture = Fixture::new();
    fixture.npm();
    fs::write(&fixture.tools.npm.path, "set -eu\nif [ \"$1\" = run ]; then mkdir release; cp local-app.json package.json package-lock.json release/; cp -R public server release/; sed 's/disposable.node.build/other.node/' release/local-app.json > release/changed; mv release/changed release/local-app.json; fi\n").unwrap();
    assert!(fixture.build().await.is_err());
}

#[tokio::test]
async fn cancellation_stops_owned_build_process_group() {
    let mut fixture = Fixture::new();
    fixture.npm();
    fs::write(
        &fixture.tools.npm.path,
        "sleep 30 &\necho $! > child.pid\nwait\n",
    )
    .unwrap();
    let (_stop, stop) = watch::channel(false);
    let (cancel_sender, cancel) = watch::channel(false);
    let root = fixture.root.clone();
    let operation = fixture.operation.clone();
    let tools = fixture.tools.clone();
    let task =
        tokio::spawn(
            async move { build_repository(&root, &operation, &tools, stop, cancel).await },
        );
    let pid_file = fixture.operation.join("repository/child.pid");
    tokio::time::timeout(Duration::from_secs(3), async {
        while !pid_file.exists() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let pid: i32 = fs::read_to_string(pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    cancel_sender.send(true).unwrap();
    assert_eq!(
        task.await.unwrap().unwrap_err().code,
        "NODE_DEVELOPMENT_CANCELLED"
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while unsafe { libc::kill(pid, 0) } == 0 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!fixture.operation.join("program").exists());
}

#[tokio::test]
async fn artifact_token_rejects_changed_or_replaced_program() {
    let fixture = Fixture::new();
    let output = fixture.build().await.unwrap();
    fs::write(output.program_root.join("public/index.html"), "changed").unwrap();
    assert_eq!(
        output.verify().unwrap_err().code,
        "NODE_DEVELOPMENT_CHANGED"
    );
    let fixture = Fixture::new();
    let output = fixture.build().await.unwrap();
    fs::rename(
        &output.program_root,
        fixture.operation.join("original-program"),
    )
    .unwrap();
    fs::create_dir(&output.program_root).unwrap();
    assert_eq!(
        output.verify().unwrap_err().code,
        "NODE_DEVELOPMENT_CHANGED"
    );
}

#[tokio::test]
async fn trusted_build_capture_accepts_platform_output_without_weakening_public_capture() {
    let fixture = Fixture::new();
    let package_path = fixture.root.join("package.json");
    let mut package: Value = serde_json::from_slice(&fs::read(&package_path).unwrap()).unwrap();
    package["dependencies"] = json!({"regular":"~2.1.34"});
    atomic_json(&package_path, &package).unwrap();
    let lock_path = fixture.root.join("package-lock.json");
    let mut lock: Value = serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
    lock["packages"][""]["dependencies"] = json!({"regular":"~2.1.34"});
    lock["packages"]["node_modules/regular"] = json!({"version":"2.1.34","resolved":"https://registry.invalid/disposable-regular-2.1.34.tgz","dependencies":{"nested":"~1.0.0"}});
    atomic_json(&lock_path, &lock).unwrap();
    let (_temp, paths) = RuntimePaths::for_test();
    fs::create_dir_all(paths.data_root().join("disposable-builds")).unwrap();
    let operation = paths.data_root().join("disposable-builds/build");
    let (_stop_sender, stop) = watch::channel(false);
    let (_cancel_sender, cancel) = watch::channel(false);
    let output = build_repository(
        &fixture.root,
        &operation,
        &fixture.tools,
        stop.clone(),
        cancel,
    )
    .await
    .unwrap();
    let root = output.program_root.clone();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().to_path_buf()).unwrap());
    let tasks =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:1".into()).unwrap();
    let sources = SourceAcquisitionManager::new(
        paths.clone(),
        store,
        tasks.clone(),
        GitEnvironmentManager::new(paths.data_root().to_path_buf(), None),
    )
    .unwrap();
    assert!(sources
        .capture(
            CaptureLocalSourceInput {
                source_path: root,
                name: None
            },
            stop.clone()
        )
        .await
        .is_err());
    let task = sources
        .capture_built_output(
            output,
            Some("可丢弃构建来源".into()),
            Some("a".repeat(40)),
            stop,
        )
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let record = tasks.task(&task.task_id).unwrap();
            if record.status.is_final() {
                break record;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(result.error.is_none(), "{:?}", result.error);
    let result = result.result.unwrap();
    assert_eq!(result["status"], "ready");
    assert_eq!(result["commit"], "a".repeat(40));
    assert_eq!(result["capturePolicy"], "node-development-build-v1");
    sources
        .verify_ready(
            result["sourceId"].as_str().unwrap(),
            result["contentDigest"].as_str().unwrap(),
        )
        .unwrap();
    sources.shutdown().await;
}

#[tokio::test]
#[ignore = "opt-in: real selected Node/npm, disposable source only, no network dependencies"]
async fn real_npm_build_runs_explicit_script_without_pre_post_hooks_or_original_writes() {
    let mut fixture = Fixture::new();
    fixture.npm();
    fs::create_dir_all(fixture.root.join("server/user")).unwrap();
    fs::write(
        fixture.root.join("server/user/routes.mjs"),
        "export const route = true;",
    )
    .unwrap();
    let node = PathBuf::from(std::env::var_os("PLATFORM_TEST_NODE").expect("PLATFORM_TEST_NODE"));
    let npm = PathBuf::from(std::env::var_os("PLATFORM_TEST_NPM").expect("PLATFORM_TEST_NPM"));
    fixture.tools = Toolchain {
        path: format!("{}:/usr/bin:/bin", node.parent().unwrap().display()),
        node: ExecutableInfo {
            path: node,
            version: "selected real Node".into(),
        },
        npm: ExecutableInfo {
            path: npm,
            version: "selected real npm".into(),
        },
    };
    atomic_json(&fixture.root.join("package.json"), &json!({"name":"disposable-build","version":"1.0.0","scripts":{"build":"node build.mjs", "prebuild":"node -e \"throw Error('prebuild must not run')\"","postbuild":"node -e \"throw Error('postbuild must not run')\"","prepare":"node -e \"throw Error('prepare must not run')\""}})).unwrap();
    fs::write(fixture.root.join("build.mjs"), "import fs from 'node:fs'; if (process.env.APP_DATA_DIR || process.env.APP_API_KEY || process.env.NODE_OPTIONS) throw Error('unexpected inherited app configuration'); fs.mkdirSync('release'); for (const file of ['local-app.json','package.json','package-lock.json','public','server']) fs.cpSync(file, 'release/'+file, {recursive:true});").unwrap();
    let output = fixture.build().await.unwrap();
    assert!(output.built);
    output.verify().unwrap();
    assert!(!fixture.root.join("release").exists());
    assert!(!fixture.root.join("node_modules").exists());
    assert!(!output.program_root.join("node_modules").exists());
    assert!(output.program_root.join("server/user/routes.mjs").is_file());
}

#[tokio::test]
async fn reviewed_git_scope_excludes_ignored_private_files_but_keeps_tracked_ignored_source() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root.join(".gitignore"),
        "public/untracked-private.json\npublic/tracked-but-ignored.txt\n",
    )
    .unwrap();
    symlink(
        "/disposable-private-source-that-must-not-be-opened",
        fixture.root.join("public/untracked-private.json"),
    )
    .unwrap();
    fs::write(
        fixture.root.join("public/tracked-but-ignored.txt"),
        "explicitly tracked source input",
    )
    .unwrap();
    let paths = [
        "local-app.json",
        "package.json",
        "package-lock.json",
        ".gitignore",
        "public/index.html",
        "public/tracked-but-ignored.txt",
        "server/main.mjs",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let (_stop, stop) = watch::channel(false);
    let (_cancel, cancel) = watch::channel(false);
    let output = build_repository_scoped(
        &fixture.root,
        &fixture.operation,
        &fixture.tools,
        stop,
        cancel,
        &paths,
    )
    .await
    .unwrap();
    assert!(!output
        .program_root
        .join("public/untracked-private.json")
        .exists());
    assert_eq!(
        fs::read_to_string(output.program_root.join("public/tracked-but-ignored.txt")).unwrap(),
        "explicitly tracked source input"
    );
    assert!(output
        .excluded_paths
        .contains(&"public/untracked-private.json".into()));
    assert!(
        fs::symlink_metadata(fixture.root.join("public/untracked-private.json"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
