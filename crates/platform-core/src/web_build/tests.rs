use super::*;

#[test]
fn npm_lock_requires_current_format_and_matching_dependencies() {
    let package = json!({"dependencies":{"example":"1.2.3"}});
    assert_eq!(
        validate_lock(&package, &json!({"lockfileVersion":1}))
            .unwrap_err()
            .code,
        "LOCKFILE_INVALID"
    );
    assert_eq!(
        validate_lock(&package, &json!({"lockfileVersion":3,"packages":{"":{}}}))
            .unwrap_err()
            .code,
        "LOCKFILE_MISMATCH"
    );
    validate_lock(
        &package,
        &json!({"lockfileVersion":2,"packages":{"":{"dependencies":{"example":"1.2.3"}}}}),
    )
    .unwrap();
}

#[cfg(unix)]
mod invocation {
    use super::*;
    use std::{fs, sync::Mutex};
    struct Fixture {
        _temporary: tempfile::TempDir,
        root: PathBuf,
        source: PathBuf,
        context: WebBuildContext,
        _stop: watch::Sender<bool>,
        cancel: watch::Sender<bool>,
        phases: Arc<Mutex<Vec<String>>>,
    }
    impl Fixture {
        async fn new(script: &str) -> Self {
            let temporary = tempfile::tempdir().unwrap();
            let base = temporary.path().canonicalize().unwrap();
            let tools = EnvironmentManager::new(base.join("discovery"))
                .resolve()
                .await
                .expect("web build tests require a real supported Node 22.12+/24 on PATH");
            let selected = base.join("tools/bin/node");
            fs::create_dir_all(selected.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(&tools.node.path, &selected).unwrap();
            let npm = base.join("tools/lib/node_modules/npm/bin/npm-cli.js");
            fs::create_dir_all(npm.parent().unwrap()).unwrap();
            // Exercise real process/tool ownership and script execution without installing
            // dependencies on the developer machine. npm's install action is a fixture.
            fs::write(&npm,r#"const fs=require('fs'),cp=require('child_process');let args=process.argv.slice(2);if(args.includes('--version')){console.log('11.19.1');process.exit(0)}fs.appendFileSync('invocations.jsonl',JSON.stringify({args,cwd:process.cwd(),home:process.env.HOME})+'\n');if(args.includes('ci')){fs.mkdirSync('node_modules',{recursive:true});fs.writeFileSync('node_modules/installation-marker','in-place');process.exit(0)}let i=args.indexOf('run'),name=args[i+1],pkg=JSON.parse(fs.readFileSync('package.json'));if(!pkg.scripts[name])process.exit(87);let result=cp.spawnSync(process.execPath,[pkg.scripts[name].slice(5)],{stdio:'inherit'});process.exit(result.status??88)"#).unwrap();
            let settings = base.join("settings");
            fs::create_dir(&settings).unwrap();
            fs::write(
                settings.join("toolchain.json"),
                serde_json::to_vec(&json!({"schemaVersion":1,"nodePath":selected})).unwrap(),
            )
            .unwrap();
            let root = base.join("project");
            let source = root.join("frontend");
            fs::create_dir_all(&source).unwrap();
            fs::write(source.join("package.json"),r#"{"name":"disposable-web-build","scripts":{"bundle:web":"node build.cjs","build":"node wrong.cjs"}}"#).unwrap();
            fs::write(
                source.join("package-lock.json"),
                r#"{"lockfileVersion":3,"packages":{"":{}}}"#,
            )
            .unwrap();
            fs::write(source.join("build.cjs"), script).unwrap();
            fs::write(source.join("wrong.cjs"), "process.exit(86)").unwrap();
            fs::write(root.join("panda.json"),serde_json::to_vec(&json!({"manifestVersion":1,"runtime":"web","app":{"name":"Disposable build"},"web":{"root":"site","entry":"home.html"},"development":{"sourceDir":"frontend","build":{"kind":"npm","script":"bundle:web"}}})).unwrap()).unwrap();
            let (stop_sender, stop) = watch::channel(false);
            let (cancel_sender, cancel) = watch::channel(false);
            let phases = Arc::new(Mutex::new(vec![]));
            let observed = phases.clone();
            let context = WebBuildContext {
                environment: EnvironmentManager::new(settings),
                operation_root: base.join("private/build"),
                cache_root: base.join("private/cache"),
                processes: OwnedProcessRegistry::default(),
                owner: Arc::new(()),
                stop,
                cancel,
                progress: Arc::new(move |phase| {
                    observed.lock().unwrap().push(phase.into());
                    Ok(())
                }),
            };
            Self {
                _temporary: temporary,
                root,
                source,
                context,
                _stop: stop_sender,
                cancel: cancel_sender,
                phases,
            }
        }
        fn target(&self) -> WebBuildTarget {
            WebBuildTarget {
                project_root: self.root.clone(),
                manifest: PandaManifest::load(&self.root).unwrap(),
            }
        }
    }
    const BUILD:&str="const fs=require('fs');fs.mkdirSync('../site',{recursive:true});fs.writeFileSync('../site/home.html','fresh files');";
    #[tokio::test]
    async fn uses_declared_script_and_original_source_without_output_arguments_or_control_files() {
        let fixture = Fixture::new(BUILD).await;
        let target = fixture.target();
        let source = fixture.source.clone();
        let root = fixture.root.clone();
        let phases = fixture.phases.clone();
        let result = WebBuildRunner::run(target, fixture.context).await.unwrap();
        assert_eq!(result.entry, root.join("site/home.html"));
        assert_eq!(fs::read_to_string(&result.entry).unwrap(), "fresh files");
        assert!(!root.join(".platform").exists());
        assert!(source.join("node_modules/installation-marker").is_file());
        let calls: Vec<Value> = fs::read_to_string(source.join("invocations.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["cwd"], json!(source));
        let args = calls[1]["args"].as_array().unwrap();
        assert_eq!(args.last(), Some(&json!("bundle:web")));
        assert!(!args.iter().any(|arg| arg == "--outDir"));
        assert_eq!(
            *phases.lock().unwrap(),
            vec!["environment", "dependencies", "build", "checking-entry"]
        );
    }
    #[tokio::test]
    async fn failed_build_preserves_all_changes_in_the_original_folder_and_reports_details() {
        let fixture=Fixture::new("const fs=require('fs');fs.mkdirSync('../site',{recursive:true});fs.writeFileSync('../site/home.html','changed before failure');console.error('disposable script failure');process.exit(42)").await;
        let target = fixture.target();
        let root = fixture.root.clone();
        fs::create_dir(root.join("site")).unwrap();
        fs::write(root.join("site/home.html"), "previous").unwrap();
        let error = WebBuildRunner::run(target, fixture.context)
            .await
            .unwrap_err();
        assert_eq!(error.code, "BUILD_FAILED");
        assert_eq!(
            fs::read_to_string(root.join("site/home.html")).unwrap(),
            "changed before failure"
        );
        assert_eq!(
            error.details["processDiagnostics"]["command"],
            "npm run bundle:web"
        );
        assert_eq!(error.details["processDiagnostics"]["exitCode"], 42);
        assert!(error.details["processDiagnostics"]["stderr"]
            .as_str()
            .unwrap()
            .contains("disposable script failure"));
    }
    #[tokio::test]
    async fn real_npm_runs_install_lifecycle_and_the_declared_script_in_the_same_directory() {
        let mut fixture = Fixture::new(BUILD).await;
        let tools = EnvironmentManager::new(fixture.root.join("unused-discovery"))
            .resolve()
            .await
            .unwrap();
        let discovery = fixture.root.parent().unwrap().join("actual-npm-discovery");
        fs::create_dir(&discovery).unwrap();
        fs::write(
            discovery.join("toolchain.json"),
            serde_json::to_vec(&json!({"schemaVersion":1,"nodePath":tools.node.path})).unwrap(),
        )
        .unwrap();
        fixture.context.environment = EnvironmentManager::new(discovery);
        fs::write(fixture.source.join("package.json"),r#"{"name":"disposable-real-npm-web-build","version":"1.0.0","scripts":{"postinstall":"node install.cjs","bundle:web":"node build.cjs","build":"node wrong.cjs"}}"#).unwrap();
        fs::write(fixture.source.join("package-lock.json"),r#"{"name":"disposable-real-npm-web-build","version":"1.0.0","lockfileVersion":3,"packages":{"":{"name":"disposable-real-npm-web-build","version":"1.0.0","hasInstallScript":true}}}"#).unwrap();
        fs::write(
            fixture.source.join("install.cjs"),
            "require('fs').writeFileSync('actual-install-marker',process.cwd())",
        )
        .unwrap();
        let target = fixture.target();
        let source = fixture.source.clone();
        let root = fixture.root.clone();
        let result = WebBuildRunner::run(target, fixture.context).await.unwrap();
        assert_eq!(fs::read_to_string(&result.entry).unwrap(), "fresh files");
        assert_eq!(
            fs::read_to_string(source.join("actual-install-marker")).unwrap(),
            source.to_string_lossy()
        );
        assert!(
            !source.join("invocations.jsonl").exists(),
            "The artificial npm adapter must not run"
        );
        assert!(!root.join(".platform").exists());
    }
    #[tokio::test]
    async fn missing_lock_fails_before_any_script_and_missing_result_is_explicit() {
        let fixture = Fixture::new("console.log('does not create entry')").await;
        let target = fixture.target();
        let lock = fixture.source.join("package-lock.json");
        let original = fs::read(&lock).unwrap();
        fs::remove_file(&lock).unwrap();
        let error = WebBuildRunner::run(target, fixture.context)
            .await
            .unwrap_err();
        assert_eq!(error.code, "LOCKFILE_MISSING");
        assert!(!fixture.source.join("invocations.jsonl").exists());
        fs::write(lock, original).unwrap();
        let other = Fixture::new("console.log('does not create entry')").await;
        let target = other.target();
        assert_eq!(
            WebBuildRunner::run(target, other.context)
                .await
                .unwrap_err()
                .code,
            "BUILD_ENTRY_MISSING"
        );
    }
    #[tokio::test]
    async fn cancellation_waits_for_the_owned_script_to_stop() {
        let fixture = Fixture::new(
            "const fs=require('fs');setInterval(()=>fs.appendFileSync('ticks','x'),10)",
        )
        .await;
        let target = fixture.target();
        let ticks = fixture.source.join("ticks");
        let cancel = fixture.cancel.clone();
        let processes = fixture.context.processes.clone();
        let context = fixture.context;
        let task = tokio::spawn(WebBuildRunner::run(target, context));
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ticks.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        cancel.send_replace(true);
        let error = task.await.unwrap().unwrap_err();
        assert_eq!(error.code, "BUILD_INTERRUPTED");
        let before = fs::read(&ticks).unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(fs::read(&ticks).unwrap(), before);
        assert!(processes.shutdown().await.is_empty());
    }
}
