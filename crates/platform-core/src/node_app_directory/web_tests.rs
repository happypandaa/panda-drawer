//! The same public local admission path runs on macOS and Windows.
use super::*;

#[tokio::test]
async fn local_web_registers_user_source_with_ready_or_unfinished_entry_state() {
    let (temporary, paths) = crate::RuntimePaths::for_test();
    let service = crate::RuntimeService::start(paths.clone(), paths.acquire_lock().unwrap())
        .await
        .unwrap();
    let manager = NodeApps::with_runtime(
        paths.data_root().into(),
        temporary.path().join("must-not-run-node"),
        &service.handle(),
    )
    .unwrap();
    for (name, entry_exists, build, state) in [
        ("ready", true, false, "ready"),
        ("needs-build", false, true, "needsBuild"),
        ("missing-entry", false, false, "entryMissing"),
    ] {
        let source = temporary
            .path()
            .canonicalize()
            .unwrap()
            .join(format!("disposable-local-web-{name}"));
        fs::create_dir(&source).unwrap();
        let mut manifest = json!({"manifestVersion":1,"runtime":"web","app":{"name":name},"web":{"root":"dist","entry":"index.html"}});
        if build {
            manifest["development"] = json!({"build":{"kind":"npm","script":"build"}});
        }
        if entry_exists {
            fs::create_dir(source.join("dist")).unwrap();
            fs::write(source.join("dist/index.html"), "user-owned web entry").unwrap();
        }
        fs::write(
            source.join("panda.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let before = fs::read(source.join("panda.json")).unwrap();
        let preview = manager.inspect_directory(&source).unwrap();
        assert_eq!(preview["state"], "ready", "{preview}");
        let added = manager
            .register_directory(
                source.clone(),
                preview["expectedRevision"].as_str().unwrap().into(),
            )
            .await
            .unwrap();
        assert_eq!(added["runtime"], "web");
        assert_eq!(added["ownership"], "user");
        assert_eq!(added["sourcePreservedOnDelete"], true);
        assert_eq!(added["projectRoot"], json!(source));
        assert_eq!(added["state"], state);
        assert_eq!(fs::read(source.join("panda.json")).unwrap(), before);
        assert!(!source.join("data").exists());
        assert!(!source.join("node_modules").exists());
        let duplicate = manager.inspect_directory(&source).unwrap();
        assert_eq!(duplicate["state"], "needs-review");
        assert_eq!(duplicate["existingApp"]["id"], added["id"]);
    }
    manager.stop_all().await.unwrap();
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn headless_web_registration_reports_unavailable_without_claiming_source() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let source = root.join("disposable-web-source");
    let data = root.join("disposable-platform-data");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&data).unwrap();
    fs::write(source.join("index.html"), "web").unwrap();
    fs::write(source.join("panda.json"), serde_json::to_vec(&json!({"manifestVersion":1,"runtime":"web","app":{"name":"Unconfigured"},"web":{"root":".","entry":"index.html"}})).unwrap()).unwrap();
    let manager = NodeApps::new(data.clone(), root.join("must-not-run-node")).unwrap();
    let preview = manager.inspect_directory(&source).unwrap();
    let issue = manager
        .register_directory(
            source.clone(),
            preview["expectedRevision"].as_str().unwrap().into(),
        )
        .await
        .unwrap_err();
    assert_eq!(issue.code, "WEB_DIRECTORY_UNAVAILABLE");
    assert!(fs::read_dir(data).unwrap().next().is_none());
    assert_eq!(fs::read_dir(source).unwrap().count(), 2);
}
