//! Only disposable directories and declarations are used. No runtime, npm,
//! credential store or existing application is opened by these regressions.
use super::*;
use std::os::unix::fs::symlink;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    manager: Arc<NodeApps>,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("disposable-local-directory");
        let data = root.join("disposable-platform-data");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&data).unwrap();
        let manager = Arc::new(NodeApps::new(data, root.join("nonexistent-node")).unwrap());
        Self {
            _temporary: temporary,
            root,
            source,
            manager,
        }
    }
    fn preview(&self) -> Value {
        self.manager.inspect_directory(&self.source).unwrap()
    }
    fn input(&self, name: &str) -> DirectoryInitializeInput {
        DirectoryInitializeInput {
            expected_revision: self.preview()["expectedRevision"].as_str().unwrap().into(),
            name: name.into(),
        }
    }
    fn initialize(&self, name: &str) -> Value {
        self.manager
            .initialize_directory(&self.source, self.input(name))
            .unwrap()
    }
    fn revision(&self) -> String {
        self.preview()["expectedRevision"].as_str().unwrap().into()
    }
    fn assert_unregistered(&self) {
        assert!(self.manager.registrations().unwrap().is_empty());
    }
    fn assert_no_platform_writes(&self) {
        assert!(!self.manager.root.exists());
        assert!(fs::read_dir(&self.manager.data_root)
            .unwrap()
            .next()
            .is_none());
    }
}

#[test]
fn inspection_is_read_only_and_hidden_or_unrelated_files_are_never_empty() {
    let fixture = Fixture::new();
    let preview = fixture.preview();
    assert_eq!(preview["state"], "empty");
    assert!(preview.get("inspection").is_none());
    let prompt = preview["reviewPrompt"].as_str().unwrap();
    assert!(prompt.contains(REVIEW_SKILL));
    assert!(prompt.contains(&serde_json::to_string(&fixture.source).unwrap()));
    fixture.assert_no_platform_writes();
    fs::write(fixture.source.join(".env"), "DISPOSABLE_SECRET_DO_NOT_READ").unwrap();
    fs::write(fixture.source.join("pom.xml"), "<project/>").unwrap();
    fs::write(
        fixture.source.join("package.json"),
        "{\"scripts\":{\"start\":\"never execute\"}}",
    )
    .unwrap();
    let preview = fixture.preview();
    assert_eq!(preview["state"], "needs-review");
    assert!(!preview["reviewPrompt"]
        .as_str()
        .unwrap()
        .contains("DISPOSABLE_SECRET_DO_NOT_READ"));
    assert!(preview["issues"].as_array().unwrap().len() == 1);
    fixture.assert_no_platform_writes();
    assert_eq!(
        fs::read_to_string(fixture.source.join(".env")).unwrap(),
        "DISPOSABLE_SECRET_DO_NOT_READ"
    );
}

#[test]
fn initialization_generates_complete_zero_dependency_external_source_without_registration() {
    let fixture = Fixture::new();
    let result = fixture.initialize("一次性 <script> & 名称");
    assert_eq!(result["state"], "ready");
    let manifest = TrialManifest::load(&fixture.source).unwrap();
    assert_eq!(manifest.app.name, "一次性 <script> & 名称");
    assert!(manifest.app.id.starts_with("local.generated."));
    assert_eq!(
        manifest.data_contract.as_ref().unwrap().id,
        format!("{}.data", manifest.app.id)
    );
    assert_eq!(manifest.data_contract.as_ref().unwrap().version, 1);
    assert_eq!(
        fs::read_to_string(fixture.source.join("backend/server.mjs")).unwrap(),
        SERVER
    );
    assert_eq!(
        fs::read_to_string(fixture.source.join("web/app.js")).unwrap(),
        FRONTEND
    );
    let html = fs::read_to_string(fixture.source.join("web/index.html")).unwrap();
    assert!(html.contains("一次性 &lt;script&gt; &amp; 名称"));
    assert!(!html.contains("<script> & 名称"));
    assert_eq!(
        fs::read_to_string(
            fixture
                .source
                .join(".agents/skills/local-folder-review/SKILL.md")
        )
        .unwrap(),
        REVIEW_SKILL
    );
    assert!(fixture.source.join("README.md").is_file());
    assert!(fixture.source.join("DEVELOPMENT.md").is_file());
    let package: Value =
        serde_json::from_slice(&fs::read(fixture.source.join("package.json")).unwrap()).unwrap();
    let lock: Value =
        serde_json::from_slice(&fs::read(fixture.source.join("package-lock.json")).unwrap())
            .unwrap();
    assert_eq!(package["private"], true);
    assert_eq!(package["type"], "module");
    assert!(package.get("dependencies").is_none());
    assert!(package.get("scripts").is_none());
    assert_eq!(lock["lockfileVersion"], 3);
    assert_eq!(lock["name"], package["name"]);
    assert_eq!(lock["packages"].as_object().unwrap().len(), 1);
    assert_eq!(lock["packages"][""]["name"], package["name"]);
    fixture.assert_no_platform_writes();
    let second = Fixture::new();
    second.initialize("same display name");
    assert_ne!(
        manifest.app.id,
        TrialManifest::load(&second.source).unwrap().app.id
    );
}

#[test]
fn stale_empty_revision_and_nonempty_confirmation_preserve_existing_files() {
    let fixture = Fixture::new();
    let input = fixture.input("never initialize over data");
    fs::write(fixture.source.join(".new-user-file"), "preserve").unwrap();
    let error = fixture
        .manager
        .initialize_directory(&fixture.source, input)
        .unwrap_err();
    assert_eq!(error.code, "NODE_DIRECTORY_CHANGED");
    assert_eq!(error.details["operationOutcome"], "not_started");
    let error = fixture
        .manager
        .initialize_directory(&fixture.source, fixture.input("name"))
        .unwrap_err();
    assert_eq!(error.code, "NODE_DIRECTORY_NOT_EMPTY");
    assert_eq!(
        fs::read_to_string(fixture.source.join(".new-user-file")).unwrap(),
        "preserve"
    );
    assert_eq!(fs::read_dir(&fixture.source).unwrap().count(), 1);
    fixture.assert_no_platform_writes();
}

#[test]
fn root_replacement_invalidates_confirmed_empty_revision() {
    let fixture = Fixture::new();
    let input = fixture.input("name");
    let retained = fixture.root.join("retained-original-root");
    fs::rename(&fixture.source, &retained).unwrap();
    fs::create_dir(&fixture.source).unwrap();
    let error = fixture
        .manager
        .initialize_directory(&fixture.source, input)
        .unwrap_err();
    assert_eq!(error.code, "NODE_DIRECTORY_CHANGED");
    assert!(fs::read_dir(retained).unwrap().next().is_none());
    assert!(fs::read_dir(&fixture.source).unwrap().next().is_none());
}

#[test]
fn concurrent_addition_before_first_write_is_not_removed_or_overwritten() {
    let fixture = Fixture::new();
    let error = fixture
        .manager
        .initialize_directory_admitted(&fixture.source, fixture.input("name"), |_| {
            fs::write(fixture.source.join("user-content"), "preserve")?;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert_eq!(error.details["createdPaths"], json!([]));
    assert_eq!(
        fs::read_to_string(fixture.source.join("user-content")).unwrap(),
        "preserve"
    );
    assert!(!fixture.source.join("web").exists());
}

#[test]
fn replacing_created_directory_never_writes_to_moved_or_new_directory() {
    let fixture = Fixture::new();
    let retained = fixture.root.join("retained-created-web");
    let error = fixture
        .manager
        .initialize_directory_admitted(&fixture.source, fixture.input("name"), |path| {
            if path == Path::new("web/index.html") {
                fs::rename(fixture.source.join("web"), &retained)?;
                fs::create_dir(fixture.source.join("web"))?;
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, "NODE_DIRECTORY_CHANGED");
    assert_eq!(
        error.details["operationOutcome"],
        "initialization_incomplete"
    );
    assert_eq!(error.details["partialFilesRetained"], true);
    assert!(error.details["createdPaths"]
        .as_array()
        .unwrap()
        .contains(&json!("web")));
    assert!(fs::read_dir(retained).unwrap().next().is_none());
    assert!(fs::read_dir(fixture.source.join("web"))
        .unwrap()
        .next()
        .is_none());
    assert!(!fixture.source.join("local-app.json").exists());
    fixture.assert_no_platform_writes();
}

#[test]
fn partial_initialization_keeps_generated_files_and_concurrent_user_file() {
    let fixture = Fixture::new();
    let error = fixture
        .manager
        .initialize_directory_admitted(&fixture.source, fixture.input("name"), |path| {
            if path == Path::new("README.md") {
                fs::write(fixture.source.join("README.md"), "concurrent user data")?;
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(
        error.details["operationOutcome"],
        "initialization_incomplete"
    );
    assert_eq!(error.details["partialFilesRetained"], true);
    assert!(error.details["createdPaths"]
        .as_array()
        .unwrap()
        .contains(&json!("backend/server.mjs")));
    assert!(!error.details["createdPaths"]
        .as_array()
        .unwrap()
        .contains(&json!("README.md")));
    assert_eq!(
        fs::read_to_string(fixture.source.join("README.md")).unwrap(),
        "concurrent user data"
    );
    assert_eq!(
        fs::read_to_string(fixture.source.join("backend/server.mjs")).unwrap(),
        SERVER
    );
    assert!(!fixture.source.join("local-app.json").exists());
    assert_eq!(fixture.preview()["state"], "needs-review");
}

#[test]
fn replacing_generated_file_with_fifo_fails_without_waiting_for_writer() {
    let fixture = Fixture::new();
    let started = std::time::Instant::now();
    let error = fixture
        .manager
        .initialize_directory_admitted(&fixture.source, fixture.input("name"), |path| {
            if path == Path::new("web/app.js") {
                fs::remove_file(fixture.source.join("web/index.html"))?;
                let fifo = c_name(fixture.source.join("web/index.html").as_os_str())?;
                assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
            }
            Ok(())
        })
        .unwrap_err();
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(error.code, "NODE_DIRECTORY_CHANGED");
    assert_eq!(
        error.details["operationOutcome"],
        "initialization_incomplete"
    );
    assert!(!fixture.source.join("web/app.js").exists());
}

#[test]
fn unsafe_or_invalid_declarations_remain_reviewable_without_reading_targets() {
    let fixture = Fixture::new();
    let secret = fixture.root.join("disposable-secret-never-preview");
    fs::write(&secret, "DO_NOT_INCLUDE_THIS_SECRET").unwrap();
    symlink(&secret, fixture.source.join("local-app.json")).unwrap();
    let preview = fixture.preview();
    assert_eq!(preview["state"], "needs-review");
    assert!(preview["reviewPrompt"]
        .as_str()
        .unwrap()
        .contains(REVIEW_SKILL));
    assert!(!preview.to_string().contains("DO_NOT_INCLUDE_THIS_SECRET"));
    assert!(!preview["issues"].as_array().unwrap().is_empty());
    assert!(fs::symlink_metadata(fixture.source.join("local-app.json"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::read_to_string(&secret).unwrap(),
        "DO_NOT_INCLUDE_THIS_SECRET"
    );
    fs::remove_file(fixture.source.join("local-app.json")).unwrap();
    fs::write(fixture.source.join("local-app.json"), vec![b'x'; 65537]).unwrap();
    assert_eq!(fixture.preview()["state"], "needs-review");
    fs::write(fixture.source.join("local-app.json"), "{invalid").unwrap();
    assert_eq!(fixture.preview()["state"], "needs-review");
    fs::remove_file(fixture.source.join("local-app.json")).unwrap();
    let fifo = c_name(fixture.source.join("local-app.json").as_os_str()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert_eq!(fixture.preview()["state"], "needs-review");
    assert!(read_small(&fixture.source.join("local-app.json")).is_err());
    fixture.assert_no_platform_writes();
}

#[test]
fn declared_entry_symlinks_are_review_issues_but_root_scope_violations_are_errors() {
    let fixture = Fixture::new();
    fixture.initialize("safe metadata");
    fs::remove_file(fixture.source.join("web/index.html")).unwrap();
    let secret = fixture.root.join("disposable-secret");
    fs::write(&secret, "ENTRY_SECRET_NOT_READ").unwrap();
    symlink(&secret, fixture.source.join("web/index.html")).unwrap();
    let result = fixture.preview();
    assert_eq!(result["state"], "needs-review");
    assert!(!result.to_string().contains("ENTRY_SECRET_NOT_READ"));
    assert!(!result["issues"].as_array().unwrap().is_empty());
    assert!(fixture
        .manager
        .inspect_directory(&fixture.manager.data_root)
        .is_err());
    assert!(fixture.manager.inspect_directory(&fixture.root).is_err());
    let managed = fixture.manager.data_root.join("managed-source");
    fs::create_dir(&managed).unwrap();
    assert!(fixture.manager.inspect_directory(&managed).is_err());
    let linked = fixture.root.join("linked-source");
    symlink(&fixture.source, &linked).unwrap();
    assert!(fixture.manager.inspect_directory(&linked).is_err());
    assert!(fixture
        .manager
        .inspect_directory(&fixture.root.join("missing"))
        .is_err());
    assert!(!fixture.manager.root.exists());
}

#[tokio::test]
async fn bound_registration_requires_ready_current_revision_and_preserves_same_path_identity() {
    let fixture = Fixture::new();
    let empty_revision = fixture.revision();
    assert_eq!(
        fixture
            .manager
            .register_directory(fixture.source.clone(), empty_revision)
            .await
            .unwrap_err()
            .code,
        "NODE_DIRECTORY_REVIEW_REQUIRED"
    );
    fixture.assert_no_platform_writes();
    let ready = fixture.initialize("external starter");
    let revision = ready["expectedRevision"].as_str().unwrap().to_owned();
    let before = fs::read(fixture.source.join("local-app.json")).unwrap();
    let first = fixture
        .manager
        .register_directory(fixture.source.clone(), revision.clone())
        .await
        .unwrap();
    assert_eq!(first["state"], "stopped");
    let second = fixture
        .manager
        .register_directory(fixture.source.clone(), revision)
        .await
        .unwrap();
    assert_eq!(first["id"], second["id"]);
    assert_eq!(fixture.manager.registrations().unwrap().len(), 1);
    assert_eq!(
        fs::read(fixture.source.join("local-app.json")).unwrap(),
        before
    );
    assert!(!fixture.manager.node_path.exists());
    fixture.manager.stop_all().await.unwrap();
}

#[tokio::test]
async fn changed_declaration_or_root_cannot_reuse_registration_confirmation() {
    let fixture = Fixture::new();
    fixture.initialize("name");
    let revision = fixture.revision();
    let path = fixture.source.join("local-app.json");
    let mut declaration: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    declaration["app"]["name"] = json!("unpreviewed change");
    fs::write(&path, serde_json::to_vec(&declaration).unwrap()).unwrap();
    assert_eq!(
        fixture
            .manager
            .register_directory(fixture.source.clone(), revision)
            .await
            .unwrap_err()
            .code,
        "NODE_DIRECTORY_CHANGED"
    );
    fixture.assert_unregistered();
    fixture.assert_no_platform_writes();
    let revision = fixture.revision();
    fs::rename(&fixture.source, fixture.root.join("retained-source")).unwrap();
    fs::create_dir(&fixture.source).unwrap();
    assert_eq!(
        fixture
            .manager
            .register_directory(fixture.source.clone(), revision)
            .await
            .unwrap_err()
            .code,
        "NODE_DIRECTORY_CHANGED"
    );
    fixture.assert_unregistered();
}

#[tokio::test]
async fn registration_pins_preview_bytes_even_if_declaration_changes_after_last_preview() {
    let fixture = Fixture::new();
    fixture.initialize("confirmed name");
    let preview = fixture.manager.directory_preview(&fixture.source).unwrap();
    let path = fixture.source.join("local-app.json");
    let mut declaration: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    declaration["app"]["name"] = json!("unconfirmed replacement");
    let inner = fixture.manager.inner.lock().await;
    let mut once = false;
    let error = fixture
        .manager
        .register_with_check(
            fixture.source.clone(),
            &inner,
            preview.manifest_digest.as_deref(),
            || {
                if !once {
                    once = true;
                    fs::write(&path, serde_json::to_vec(&declaration).unwrap())?;
                }
                Ok(())
            },
        )
        .unwrap_err();
    assert_eq!(error.code, "NODE_DIRECTORY_CHANGED");
    fixture.assert_unregistered();
    fixture.assert_no_platform_writes();
}

#[tokio::test]
async fn shutdown_rejects_new_initialization_without_creating_files() {
    let fixture = Fixture::new();
    let input = fixture.input("name");
    fixture.manager.stop_all().await.unwrap();
    let error = fixture
        .manager
        .initialize_directory(&fixture.source, input)
        .unwrap_err();
    assert_eq!(error.code, "NODE_DIRECTORY_STOPPING");
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert!(fs::read_dir(&fixture.source).unwrap().next().is_none());
    fixture.assert_no_platform_writes();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_waits_for_accepted_initialization_and_rejects_queued_one() {
    let fixture = Fixture::new();
    let second = fixture.root.join("disposable-queued-directory");
    fs::create_dir(&second).unwrap();
    let second_revision = fixture.manager.inspect_directory(&second).unwrap()["expectedRevision"]
        .as_str()
        .unwrap()
        .to_owned();
    let manager = fixture.manager.clone();
    let source = fixture.source.clone();
    let input = fixture.input("accepted name");
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let initialization = std::thread::spawn(move || {
        let mut first = true;
        manager.initialize_directory_admitted(&source, input, |_| {
            if first {
                first = false;
                entered_tx.send(()).unwrap();
                release_rx
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
            }
            Ok(())
        })
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let manager = fixture.manager.clone();
    let stopping = tokio::spawn(async move { manager.stop_all().await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !fixture
            .manager
            .directory_stopping
            .load(std::sync::atomic::Ordering::Acquire)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(!stopping.is_finished());
    let manager = fixture.manager.clone();
    let second_source = second.clone();
    let queued = std::thread::spawn(move || {
        manager.initialize_directory(
            &second_source,
            DirectoryInitializeInput {
                expected_revision: second_revision,
                name: "queued name".into(),
            },
        )
    });
    release_tx.send(()).unwrap();
    assert_eq!(initialization.join().unwrap().unwrap()["state"], "ready");
    stopping.await.unwrap().unwrap();
    assert_eq!(
        queued.join().unwrap().unwrap_err().code,
        "NODE_DIRECTORY_STOPPING"
    );
    assert!(fs::read_dir(second).unwrap().next().is_none());
    fixture.assert_no_platform_writes();
}
