//! The opt-in real npm regression reads only supplied local tool artifacts and
//! TempDir artificial projects; it never uses a global user npm configuration.
use super::*;

async fn fixture() -> (
    tempfile::TempDir,
    RuntimePaths,
    Arc<ProjectStore>,
    Arc<BuildManager>,
    String,
    PathBuf,
) {
    let (temporary, paths) = RuntimePaths::for_test();
    let store = Arc::new(ProjectStore::new(paths.workspace_root().into()).unwrap());
    let manager =
        BuildManager::new(paths.clone(), store.clone(), "http://127.0.0.1:9".into()).unwrap();
    let id = "task_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned();
    ensure_dir(&manager.task_dir(&id)).unwrap();
    let source = paths.workspace_root().join("artificial-npm-source");
    ensure_dir(&source).unwrap();
    (temporary, paths, store, manager, id, source)
}

#[tokio::test]
#[ignore = "opt-in: supplied local Node/npm artifacts, dependency-free offline TempDir build"]
async fn real_npm_preserves_twenty_shared_logs_and_writes_only_task_logs() {
    let node = PathBuf::from(
        std::env::var_os("PANDA_DRAWER_TEST_NODE").expect("explicit local Node artifact"),
    );
    let npm = PathBuf::from(
        std::env::var_os("PANDA_DRAWER_TEST_NPM_CLI").expect("explicit local npm CLI artifact"),
    );
    assert!(node.is_absolute() && npm.is_absolute());
    let (_temporary, paths, store, manager, id, source) = fixture().await;
    let cache = paths.workspace_root().join(".platform/cache/npm");
    let previous_logs = cache.join("_logs");
    ensure_dir(&previous_logs).unwrap();
    let mut before = Vec::new();
    for index in 0..20 {
        let path = previous_logs.join(format!("2026-10-22T00_00_{index:02}_000Z-debug-0.log"));
        fs::write(&path, format!("artificial retained log {index}\n")).unwrap();
        let info = fs::metadata(&path).unwrap();
        before.push((
            path.clone(),
            fs::read(&path).unwrap(),
            info.ino(),
            info.mtime(),
            info.mtime_nsec(),
        ));
    }
    fs::write(source.join("package.json"), r#"{"name":"artificial-local-log-test","version":"1.0.0","scripts":{"build":"node -e \"require('fs').writeFileSync('artificial-built.txt','built')\""}}"#).unwrap();
    fs::write(source.join("package-lock.json"), r#"{"name":"artificial-local-log-test","version":"1.0.0","lockfileVersion":3,"requires":true,"packages":{"":{"name":"artificial-local-log-test","version":"1.0.0"}}}"#).unwrap();
    // Inherited project configuration deliberately points to retained shared
    // logs; the production command's explicit CLI option must override it.
    fs::write(
        source.join(".npmrc"),
        format!("logs-dir={}\n", previous_logs.display()),
    )
    .unwrap();
    let user_config = source.join("artificial-user.npmrc");
    let global_config = source.join("artificial-global.npmrc");
    fs::write(
        &user_config,
        format!("logs-dir={}\n", previous_logs.display()),
    )
    .unwrap();
    fs::write(&global_config, "").unwrap();
    let tools = Toolchain {
        path: format!("{}:/usr/bin:/bin", node.parent().unwrap().display()),
        node: crate::environment::ExecutableInfo {
            path: node,
            version: "explicit local test artifact".into(),
        },
        npm: crate::environment::ExecutableInfo {
            path: npm,
            version: "explicit local test artifact".into(),
        },
    };
    for operation in [&["ci", "--ignore-scripts"][..], &["run", "build"][..]] {
        let mut args = vec![
            "--userconfig",
            user_config.to_str().unwrap(),
            "--globalconfig",
            global_config.to_str().unwrap(),
            "--offline",
            "--no-audit",
            "--no-fund",
            "--registry=http://127.0.0.1:9",
        ];
        args.extend(operation);
        manager
            .command(&id, &tools, &source, &args, Instant::now())
            .await
            .unwrap();
    }
    assert_eq!(
        fs::read(source.join("artificial-built.txt")).unwrap(),
        b"built"
    );
    for (path, bytes, inode, seconds, nanos) in before {
        let info = fs::metadata(&path).unwrap();
        assert_eq!(fs::read(path).unwrap(), bytes);
        assert_eq!(
            (info.ino(), info.mtime(), info.mtime_nsec()),
            (inode, seconds, nanos)
        );
    }
    assert_eq!(fs::read_dir(previous_logs).unwrap().count(), 20);
    assert!(
        fs::read_dir(manager.task_dir(&id).join("npm-logs"))
            .unwrap()
            .count()
            >= 2
    );
    manager.shutdown().await;
    store.shutdown().await;
}
