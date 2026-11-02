//! Only disposable directories and declarations are used. No runtime, npm,
//! credential store or existing application is opened by these regressions.
use super::*;
use std::os::unix::fs::{symlink, FileTypeExt};

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
    fn assert_pending_generation(&self) {
        assert!(!self.manager.root.exists());
        assert_eq!(
            fs::read_dir(self.manager.data_root.join("node-project-ownership"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            fs::read_dir(self.manager.data_root.join("node-source-owners"))
                .unwrap()
                .count(),
            1
        );
    }
    fn assert_no_owned_materials(&self) {
        assert!(!self.manager.root.exists());
        for directory in ["node-project-ownership", "node-source-owners"] {
            let path = self.manager.data_root.join(directory);
            if path.exists() {
                assert!(fs::read_dir(path).unwrap().next().is_none());
            }
        }
    }
}

fn source_files(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn collect(
        root: &Path,
        current: &Path,
        files: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>,
    ) {
        for entry in fs::read_dir(current).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                collect(root, &entry.path(), files);
            } else {
                assert!(entry.file_type().unwrap().is_file());
                files.insert(
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    collect(root, root, &mut files);
    files
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

#[tokio::test]
async fn local_directory_registration_accepts_edited_display_metadata() {
    let fixture = Fixture::new();
    fixture.initialize("Disposable editable metadata");
    let path = fixture.source.join("panda.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["app"]["name"] = json!("Edited display name");
    manifest["app"]["version"] = json!("2.0.0");
    fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let preview = fixture.preview();
    assert_eq!(preview["state"], "ready", "{preview}");
    let added = fixture
        .manager
        .register_directory(
            fixture.source.clone(),
            preview["expectedRevision"].as_str().unwrap().into(),
        )
        .await
        .unwrap();
    assert_eq!(added["sourcePath"], json!(fixture.source));
    assert_eq!(added["name"], "Edited display name");
    assert!(fixture.manager.running_ids().await.is_empty());
}

#[test]
fn generated_development_instructions_describe_direct_run_and_editable_manifest() {
    let fixture = Fixture::new();
    fixture.initialize("Disposable current instructions");
    let text = fs::read_to_string(fixture.source.join("DEVELOPMENT.md")).unwrap();
    for current in [
        "直接修改这个文件夹",
        "panda.json",
        "重开后生效",
        "新增必填配置",
        "data/",
    ] {
        assert!(text.contains(current), "{current}");
    }
    for retired in [
        "独立安装副本",
        "应用修改",
        "不要直接改变这些声明",
        "应用标识、配置和数据契约不兼容",
    ] {
        assert!(!text.contains(retired), "{retired}");
    }
}

#[tokio::test]
async fn registered_directory_is_identified_before_a_damaged_manifest_or_entry_budget() {
    let fixture = Fixture::new();
    let input = fixture.input("disposable existing application");
    let registered = fixture
        .manager
        .initialize_and_register_directory(fixture.source.clone(), input)
        .await
        .unwrap();
    let id = registered["id"].as_str().unwrap();
    let registration = fixture.manager.root.join(id).join("registration.json");
    let original_registration = fs::read(&registration).unwrap();

    fs::write(
        fixture.source.join("panda.json"),
        b"{broken disposable declaration",
    )
    .unwrap();
    let damaged = fixture.preview();
    assert_eq!(damaged["existingApp"]["id"], id);
    assert_eq!(damaged["category"], "insufficient");
    assert!(damaged.get("inspection").is_none());

    for index in 0..=MAX_ENTRIES {
        fs::write(
            fixture.source.join(format!("disposable-entry-{index}")),
            b"preserve",
        )
        .unwrap();
    }
    assert_eq!(
        fixture
            .manager
            .directory_preview(&fixture.source)
            .err()
            .unwrap()
            .code,
        "NODE_DIRECTORY_LIMIT"
    );
    let large = fixture.preview();
    assert_eq!(large["existingApp"]["id"], id);
    assert_eq!(fixture.manager.registrations().unwrap().len(), 1);
    assert_eq!(fs::read(&registration).unwrap(), original_registration);
    assert_eq!(
        fs::read(fixture.source.join("panda.json")).unwrap(),
        b"{broken disposable declaration"
    );
    assert!(fixture.manager.running_ids().await.is_empty());
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    assert!(!fixture.manager.node_path.exists());
    fixture.manager.stop_all().await.unwrap();
}

#[test]
fn directory_categories_distinguish_platform_python_and_frontend_materials_without_reading_secrets()
{
    for (files, category, format, code) in [
        (
            vec![
                ("requirements.txt", "disposable dependency"),
                ("main.py", "raise RuntimeError('never execute')"),
            ],
            "unsupported",
            None,
            "EXTERNAL_RUNTIME",
        ),
        (
            vec![
                ("package.json", r#"{"scripts":{"dev":"vite"}}"#),
                ("requirements.txt", "optional tooling"),
                ("main.py", "not necessarily the runtime"),
            ],
            "needs-changes",
            None,
            "MISSING_DECLARATION",
        ),
        (
            vec![(
                "requirements.txt",
                "insufficient to identify the actual runtime",
            )],
            "needs-changes",
            None,
            "MISSING_DECLARATION",
        ),
    ] {
        let fixture = Fixture::new();
        for (path, content) in &files {
            fs::write(fixture.source.join(path), content).unwrap();
        }
        let secret = fixture.source.join(".env");
        let name = c_name(secret.as_os_str()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let private_database = fixture.source.join("records.sqlite");
        fs::write(
            &private_database,
            b"DO_NOT_INSPECT_DISPOSABLE_BUSINESS_RECORDS",
        )
        .unwrap();
        let result = fixture.preview();
        assert_eq!(result["state"], "needs-review");
        assert_eq!(result["category"], category);
        assert_eq!(result["projectFormat"].as_str(), format);
        assert_eq!(result["reasonCodes"], json!([code]));
        assert!(!result
            .to_string()
            .contains("DO_NOT_INSPECT_DISPOSABLE_BUSINESS_RECORDS"));
        for (path, content) in files {
            assert_eq!(
                fs::read_to_string(fixture.source.join(path)).unwrap(),
                content
            );
        }
        assert_eq!(
            fs::read(private_database).unwrap(),
            b"DO_NOT_INSPECT_DISPOSABLE_BUSINESS_RECORDS"
        );
        assert!(fs::symlink_metadata(secret).unwrap().file_type().is_fifo());
        fixture.assert_no_platform_writes();
    }
}

#[test]
fn initialization_generates_complete_zero_dependency_external_source_without_registration() {
    let fixture = Fixture::new();
    let result = fixture.initialize("一次性 <script> & 名称");
    assert_eq!(result["state"], "ready");
    let manifest = PandaManifest::load(&fixture.source).unwrap();
    assert_eq!(manifest.app.name, "一次性 <script> & 名称");
    assert_eq!(manifest.manifest_version, 1);
    let declaration: Value =
        serde_json::from_slice(&fs::read(fixture.source.join("panda.json")).unwrap()).unwrap();
    assert!(declaration["app"].get("id").is_none());
    assert_eq!(
        fs::read_to_string(fixture.source.join(".gitignore"))
            .unwrap()
            .lines()
            .take(2)
            .collect::<Vec<_>>(),
        ["node_modules/", "data/"]
    );
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
    fixture.assert_pending_generation();
    let second = Fixture::new();
    second.initialize("same display name");
    assert_ne!(
        result["pendingAppId"],
        json!(second
            .manager
            .pending_generated_source_id(&second.source)
            .unwrap()
            .unwrap())
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
    fixture.assert_no_owned_materials();
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
    assert!(!fixture.source.join("panda.json").exists());
    fixture.assert_pending_generation();
}

#[test]
fn partial_initialization_rolls_back_generated_files_and_preserves_only_concurrent_user_file() {
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
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert_eq!(error.details["partialFilesRetained"], false);
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
    assert!(!fixture.source.join("backend").exists());
    assert!(!fixture.source.join("web").exists());
    assert!(!fixture.source.join(".agents").exists());
    fixture.assert_no_owned_materials();
    assert!(!fixture.source.join("panda.json").exists());
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
fn declared_internal_links_are_accepted_but_shared_entry_files_are_rejected() {
    let fixture = Fixture::new();
    fixture.initialize("Disposable linked entries");
    let backend = fixture.source.join("backend/server.mjs");
    let actual = fixture.source.join("backend/actual-server.mjs");
    fs::rename(&backend, &actual).unwrap();
    symlink("actual-server.mjs", &backend).unwrap();
    let entry = fixture.source.join("web/index.html");
    fs::rename(&entry, fixture.source.join("web/actual-index.html")).unwrap();
    symlink("actual-index.html", &entry).unwrap();
    assert_eq!(fixture.preview()["state"], "ready");
    fs::hard_link(&actual, fixture.root.join("shared-entry")).unwrap();
    assert_eq!(fixture.preview()["state"], "needs-review");
    assert!(!fixture.manager.root.exists());
}

#[test]
fn unsafe_or_invalid_declarations_remain_reviewable_without_reading_targets() {
    let fixture = Fixture::new();
    let secret = fixture.root.join("disposable-secret-never-preview");
    fs::write(&secret, "DO_NOT_INCLUDE_THIS_SECRET").unwrap();
    symlink(&secret, fixture.source.join("panda.json")).unwrap();
    let preview = fixture.preview();
    assert_eq!(preview["state"], "needs-review");
    assert!(preview["reviewPrompt"]
        .as_str()
        .unwrap()
        .contains(REVIEW_SKILL));
    assert!(!preview.to_string().contains("DO_NOT_INCLUDE_THIS_SECRET"));
    assert!(!preview["issues"].as_array().unwrap().is_empty());
    assert!(fs::symlink_metadata(fixture.source.join("panda.json"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::read_to_string(&secret).unwrap(),
        "DO_NOT_INCLUDE_THIS_SECRET"
    );
    fs::remove_file(fixture.source.join("panda.json")).unwrap();
    fs::write(fixture.source.join("panda.json"), vec![b'x'; 65537]).unwrap();
    assert_eq!(fixture.preview()["state"], "needs-review");
    fs::write(fixture.source.join("panda.json"), "{invalid").unwrap();
    assert_eq!(fixture.preview()["state"], "needs-review");
    fs::remove_file(fixture.source.join("panda.json")).unwrap();
    let fifo = c_name(fixture.source.join("panda.json").as_os_str()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert_eq!(fixture.preview()["state"], "needs-review");
    assert!(read_small(&fixture.source.join("panda.json")).is_err());
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
    assert!(
        fixture.manager.list().await.unwrap().is_empty(),
        "Generating source alone must not register an application"
    );
    fixture.assert_pending_generation();
    let revision = ready["expectedRevision"].as_str().unwrap().to_owned();
    let before = source_files(&fixture.source);
    let first = fixture
        .manager
        .register_directory(fixture.source.clone(), revision.clone())
        .await
        .unwrap();
    assert_eq!(first["state"], "stopped");
    assert_eq!(first["id"], ready["pendingAppId"]);
    let listed = fixture.manager.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], first["id"]);
    assert_eq!(listed[0]["sourcePath"], json!(fixture.source));
    assert_eq!(listed[0]["name"], "external starter");
    let second = fixture
        .manager
        .register_directory(fixture.source.clone(), revision)
        .await
        .unwrap();
    assert_eq!(first["id"], second["id"]);
    assert_eq!(fixture.manager.registrations().unwrap().len(), 1);
    assert_eq!(source_files(&fixture.source), before);
    assert!(fixture.manager.running_ids().await.is_empty());
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    assert!(!fixture.manager.node_path.exists());
    fixture.manager.stop_all().await.unwrap();

    // Read through a fresh manager after the original has shut down: the list
    // must come from the durable registration, independent of UI callbacks.
    let reopened = NodeApps::new(
        fixture.manager.data_root.clone(),
        fixture.manager.node_path.clone(),
    )
    .unwrap();
    let persisted = reopened.list().await.unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0]["id"], first["id"]);
    assert_eq!(persisted[0]["sourcePath"], json!(fixture.source));
    assert_eq!(persisted[0]["state"], "stopped");
    assert!(reopened.running_ids().await.is_empty());
    assert!(reopened.owned_process_ids().await.is_empty());
    assert_eq!(source_files(&fixture.source), before);
    assert!(!fixture
        .manager
        .root
        .join(first["id"].as_str().unwrap())
        .join("active.json")
        .exists());
    reopened.stop_all().await.unwrap();
}

#[tokio::test]
async fn one_confirmed_generation_registers_durably_without_starting_or_later_source_changes() {
    let fixture = Fixture::new();
    let input = fixture.input("One confirmed project");
    let result = fixture
        .manager
        .initialize_and_register_directory(fixture.source.clone(), input)
        .await
        .unwrap();
    assert_eq!(result["name"], "One confirmed project");
    assert_eq!(result["sourcePath"], json!(fixture.source));
    assert_eq!(result["state"], "stopped");
    assert_eq!(
        crate::panda_manifest::PandaManifest::load(&fixture.source)
            .unwrap()
            .backend
            .unwrap()
            .effective_api_prefixes(),
        vec!["/api"]
    );
    assert!(Uuid::parse_str(result["id"].as_str().unwrap()).is_ok());
    let generated = source_files(&fixture.source);
    let listed = fixture.manager.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], result["id"]);
    let reopened = NodeApps::new(
        fixture.manager.data_root.clone(),
        fixture.manager.node_path.clone(),
    )
    .unwrap();
    let persisted = reopened.list().await.unwrap();
    assert_eq!(persisted.len(), 1);
    assert_eq!(persisted[0]["id"], result["id"]);
    assert_eq!(persisted[0]["sourcePath"], result["sourcePath"]);
    assert!(reopened.running_ids().await.is_empty());
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    assert_eq!(source_files(&fixture.source), generated);
    assert!(!fixture.manager.node_path.exists());
    assert!(!fixture
        .manager
        .root
        .join(result["id"].as_str().unwrap())
        .join("active.json")
        .exists());
    let stale = fixture
        .manager
        .initialize_and_register_directory(
            fixture.source.clone(),
            fixture.input("Do not overwrite"),
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code, "NODE_DIRECTORY_CHANGED");
    assert_eq!(fixture.preview()["existingApp"]["id"], result["id"]);
    assert_eq!(fixture.manager.list().await.unwrap().len(), 1);
    assert_eq!(source_files(&fixture.source), generated);
    reopened.stop_all().await.unwrap();
    fixture.manager.stop_all().await.unwrap();
}

#[tokio::test]
async fn combined_generation_rejects_stale_confirmation_and_shutdown_before_writing() {
    let fixture = Fixture::new();
    let input = fixture.input("Stale confirmation");
    fs::write(fixture.source.join("user.txt"), b"keep this user file").unwrap();
    let error = fixture
        .manager
        .initialize_and_register_directory(fixture.source.clone(), input)
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_DIRECTORY_CHANGED");
    assert_eq!(
        fs::read(fixture.source.join("user.txt")).unwrap(),
        b"keep this user file"
    );
    assert_eq!(source_files(&fixture.source).len(), 1);
    fixture.assert_no_platform_writes();
    let stopped = Fixture::new();
    let input = stopped.input("Stopped confirmation");
    stopped.manager.stop_all().await.unwrap();
    let error = stopped
        .manager
        .initialize_and_register_directory(stopped.source.clone(), input)
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_DIRECTORY_STOPPING");
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert!(source_files(&stopped.source).is_empty());
    stopped.assert_no_platform_writes();
}

#[tokio::test]
async fn combined_generation_failure_rolls_back_its_files_claim_and_ownership() {
    let fixture = Fixture::new();
    let error = fixture
        .manager
        .initialize_and_register_directory_admitted(
            fixture.source.clone(),
            fixture.input("Partial project"),
            |path| {
                if path == Path::new("web/app.js") {
                    return Err(directory_error(
                        "DISPOSABLE_WRITE_FAILURE",
                        "Artificial write failure",
                    ));
                }
                Ok(())
            },
            |_| Ok(()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "DISPOSABLE_WRITE_FAILURE");
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert_eq!(error.details["partialFilesRetained"], false);
    assert!(fs::read_dir(&fixture.source).unwrap().next().is_none());
    assert!(!fixture.source.join("web/app.js").exists());
    assert!(!fixture.source.join("panda.json").exists());
    fixture.assert_no_owned_materials();
    assert_eq!(fixture.preview()["state"], "empty");
}

#[tokio::test]
async fn generated_files_survive_registration_failure_and_explicit_reinspection_can_finish_adding()
{
    let fixture = Fixture::new();
    // A real filesystem obstacle affects only registration, after generation.
    let input = fixture.input("Retained generated project");
    fs::write(&fixture.manager.root, b"registration directory is blocked").unwrap();
    let error = fixture
        .manager
        .initialize_and_register_directory(fixture.source.clone(), input)
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_DIRECTORY_ADD_FAILED");
    assert_eq!(
        error.details["operationOutcome"],
        "initialized_not_registered"
    );
    assert_eq!(error.details["initializationCompleted"], true);
    assert_eq!(error.details["partialFilesRetained"], true);
    assert_eq!(error.details["requiresRecheck"], true);
    assert!(fixture.source.join("panda.json").is_file());
    assert_eq!(
        fs::read(&fixture.manager.root).unwrap(),
        b"registration directory is blocked"
    );
    let generated = source_files(&fixture.source);
    fs::remove_file(&fixture.manager.root).unwrap();
    assert!(fixture.manager.list().await.unwrap().is_empty());
    let ready = fixture.preview();
    assert_eq!(ready["state"], "ready");
    let added = fixture
        .manager
        .register_directory(
            fixture.source.clone(),
            ready["expectedRevision"].as_str().unwrap().into(),
        )
        .await
        .unwrap();
    assert_eq!(fixture.manager.list().await.unwrap()[0]["id"], added["id"]);
    assert_eq!(added["id"], error.details["appId"]);
    assert_eq!(source_files(&fixture.source), generated);
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    fixture.manager.stop_all().await.unwrap();
}

#[tokio::test]
async fn combined_admission_cannot_adopt_valid_external_edits_after_generation() {
    let fixture = Fixture::new();
    let error = fixture
        .manager
        .initialize_and_register_directory_admitted(
            fixture.source.clone(),
            fixture.input("Generated name"),
            |_| Ok(()),
            |source| {
                let declaration = source.join("panda.json");
                let mut manifest: Value = serde_json::from_slice(&fs::read(&declaration)?)?;
                manifest["app"]["name"] = json!("Unconfirmed external name");
                fs::write(declaration, serde_json::to_vec_pretty(&manifest)?)?;
                fs::write(
                    source.join("web/index.html"),
                    b"<h1>Unconfirmed but valid edited page</h1>",
                )?;
                Ok(())
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "NODE_DIRECTORY_ADD_FAILED");
    assert_eq!(
        error.details["operationOutcome"],
        "initialized_not_registered"
    );
    assert_eq!(error.details["cause"]["code"], "NODE_DIRECTORY_CHANGED");
    fixture.assert_pending_generation();
    assert_eq!(
        fs::read(fixture.source.join("web/index.html")).unwrap(),
        b"<h1>Unconfirmed but valid edited page</h1>"
    );
    let ready = fixture.preview();
    assert_eq!(ready["state"], "ready");
    assert_eq!(
        ready["inspection"]["app"]["name"],
        "Unconfirmed external name"
    );
    let added = fixture
        .manager
        .register_directory(
            fixture.source.clone(),
            ready["expectedRevision"].as_str().unwrap().into(),
        )
        .await
        .unwrap();
    assert_eq!(added["name"], "Unconfirmed external name");
    assert_eq!(fixture.manager.list().await.unwrap().len(), 1);
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    fixture.manager.stop_all().await.unwrap();
}

#[tokio::test]
async fn registration_publication_and_postcommit_detail_failures_report_actual_uncertainty() {
    for fail_detail in [false, true] {
        let fixture = Fixture::new();
        let mut retained = None;
        let error = fixture
            .manager
            .initialize_and_register_directory_admitted(
                fixture.source.clone(),
                fixture.input("Retained registration outcome"),
                |_| Ok(()),
                |_| {
                    // The last precommit check sees the newly allocated private slot.
                    // Fail on actual filesystem content, without timing a race.
                    if retained.is_none() && fixture.manager.root.is_dir() {
                        for entry in fs::read_dir(&fixture.manager.root)? {
                            let slot = entry?.path();
                            if slot.join("configuration").is_dir()
                                && !slot.join("registration.json").exists()
                            {
                                let path = if fail_detail {
                                    slot.join("configuration/configuration.json")
                                } else {
                                    slot.join("registration.json")
                                };
                                fs::write(&path, b"retained invalid fixture, do not overwrite")?;
                                retained = Some(path);
                                break;
                            }
                        }
                    }
                    Ok(())
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "NODE_DIRECTORY_ADD_FAILED");
        assert_eq!(error.details["initializationCompleted"], true);
        assert_eq!(error.details["partialFilesRetained"], true);
        let retained = retained.unwrap();
        assert_eq!(
            fs::read(&retained).unwrap(),
            b"retained invalid fixture, do not overwrite"
        );
        let generated = source_files(&fixture.source);
        if fail_detail {
            assert_eq!(
                error.details["operationOutcome"],
                "registered_needs_attention"
            );
            assert_eq!(
                error.details["cause"]["details"]["registrationCommitted"],
                true
            );
            let id = error.details["cause"]["details"]["appId"].as_str().unwrap();
            let registration: Value = serde_json::from_slice(
                &fs::read(fixture.manager.root.join(id).join("registration.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(registration["id"], id);
            let listed = fixture.manager.list().await.unwrap();
            assert_eq!(listed.len(), 1);
            assert_eq!(listed[0]["id"], id);
            assert_eq!(listed[0]["state"], "failed");
            let registration_bytes =
                fs::read(fixture.manager.root.join(id).join("registration.json")).unwrap();
            fs::remove_file(retained).unwrap();
            let existing = fixture.preview();
            assert_eq!(existing["existingApp"]["id"], id);
            let added = fixture.manager.configuration(id).await.unwrap();
            assert_eq!(added["id"], id);
            let reopened = NodeApps::new(
                fixture.manager.data_root.clone(),
                fixture.manager.node_path.clone(),
            )
            .unwrap();
            let reused = reopened.configuration(id).await.unwrap();
            assert_eq!(reused["id"], id);
            assert_eq!(reused["state"], "stopped");
            assert_eq!(reopened.list().await.unwrap().len(), 1);
            assert_eq!(
                fs::read(fixture.manager.root.join(id).join("registration.json")).unwrap(),
                registration_bytes
            );
            assert!(reopened.owned_process_ids().await.is_empty());
            reopened.stop_all().await.unwrap();
        } else {
            assert_eq!(
                error.details["operationOutcome"],
                "initialized_registration_unconfirmed"
            );
            assert_eq!(
                error.details["cause"]["details"]["registrationAttempted"],
                true
            );
            assert!(fixture.manager.list().await.is_err());
        }
        assert_eq!(source_files(&fixture.source), generated);
        assert!(fixture.manager.owned_process_ids().await.is_empty());
        fixture.manager.stop_all().await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_waits_for_accepted_combined_creation_and_rejects_queued_duplicate() {
    let fixture = Fixture::new();
    let input = fixture.input("Accepted combined project");
    let queued_input = input.clone();
    let manager = fixture.manager.clone();
    let source = fixture.source.clone();
    let runtime = tokio::runtime::Handle::current();
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let creating = std::thread::spawn(move || {
        let mut first = true;
        runtime.block_on(manager.initialize_and_register_directory_admitted(
            source,
            input,
            |_| {
                if first {
                    first = false;
                    entered_tx.send(()).unwrap();
                    release_rx
                        .recv_timeout(std::time::Duration::from_secs(10))
                        .unwrap();
                }
                Ok(())
            },
            |_| Ok(()),
        ))
    });
    entered_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let manager = fixture.manager.clone();
    let source = fixture.source.clone();
    let queued = tokio::spawn(async move {
        manager
            .initialize_and_register_directory(source, queued_input)
            .await
    });
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
    assert!(!queued.is_finished());
    release_tx.send(()).unwrap();
    let accepted = creating.join().unwrap().unwrap();
    assert_eq!(accepted["state"], "stopped");
    let rejected = queued.await.unwrap().unwrap_err();
    assert_eq!(rejected.code, "NODE_DIRECTORY_STOPPING");
    assert_eq!(rejected.details["operationOutcome"], "not_started");
    stopping.await.unwrap().unwrap();
    let listed = fixture.manager.list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], accepted["id"]);
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    assert!(fixture.source.join("panda.json").is_file());
}

#[tokio::test]
async fn changed_declaration_or_root_cannot_reuse_registration_confirmation() {
    let fixture = Fixture::new();
    fixture.initialize("name");
    let revision = fixture.revision();
    let path = fixture.source.join("panda.json");
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
    fixture.assert_pending_generation();
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
    let path = fixture.source.join("panda.json");
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
    fixture.assert_pending_generation();
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
    fixture.assert_pending_generation();
}

#[tokio::test]
async fn selected_empty_folder_keeps_its_identity_and_all_files_after_project_deletion() {
    let fixture = Fixture::new();
    let pending = fixture.initialize("Disposable generated ownership");
    let id = pending["pendingAppId"].as_str().unwrap();
    let registered = fixture
        .manager
        .register_directory(fixture.source.clone(), fixture.revision())
        .await
        .unwrap();
    assert_eq!(registered["id"], id);
    assert!(!fixture
        .manager
        .project_ownership()
        .has_generated_source(id, &fixture.source)
        .unwrap());
    assert_eq!(registered["ownership"], "external");
    fs::create_dir(fixture.source.join("data")).unwrap();
    fs::write(fixture.source.join("data/retained.txt"), b"user data").unwrap();
    fs::create_dir(fixture.source.join(".git")).unwrap();
    fs::write(
        fixture.source.join(".git/disposable-uncommitted-edit"),
        b"all generated project contents belong to this instance",
    )
    .unwrap();
    fs::write(
        fixture.source.join("web/app.js"),
        b"// AI uncommitted changes",
    )
    .unwrap();
    let preview = fixture.manager.delete_preview(id).await.unwrap();
    assert_eq!(preview["preservedPaths"], json!([fixture.source]));
    fixture
        .manager
        .delete(
            id,
            DeleteNodeAppInput {
                expected_revision: preview["expectedRevision"].as_str().unwrap().into(),
            },
        )
        .await
        .unwrap();
    assert!(fixture.source.is_dir());
    assert_eq!(
        fs::read(fixture.source.join("data/retained.txt")).unwrap(),
        b"user data"
    );
    assert!(fixture
        .source
        .join(".git/disposable-uncommitted-edit")
        .is_file());
    assert!(!fixture.manager.root.join(id).exists());
    assert!(!fixture
        .manager
        .project_ownership()
        .directory(id)
        .unwrap()
        .exists());
    for entry in fs::read_dir(fixture.manager.data_root.join("node-source-owners")).unwrap() {
        let entry = entry.unwrap();
        assert_eq!(entry.file_name(), ".pending-deletions");
        assert!(fs::read_dir(entry.path()).unwrap().next().is_none());
    }
}

#[tokio::test]
async fn initialized_user_folder_is_not_rebound_or_adopted_by_migration_helpers() {
    let fixture = Fixture::new();
    let old = fixture.root.join("disposable-old-workspace");
    let new = fixture.root.join("disposable-new-workspace");
    let source = old.join("custom/disposable-user-starter");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir(&new).unwrap();
    fixture.manager.set_source_workspace(old.clone()).unwrap();
    let preview = fixture.manager.inspect_directory(&source).unwrap();
    let added = fixture
        .manager
        .initialize_and_register_directory(
            source.clone(),
            DirectoryInitializeInput {
                expected_revision: preview["expectedRevision"].as_str().unwrap().into(),
                name: "Disposable user starter".into(),
            },
        )
        .await
        .unwrap();
    let id = added["id"].as_str().unwrap();
    let before = source_files(&source);
    crate::node_apps::source_ownership::rebind_published_after_migration(
        &fixture.manager.data_root,
        &old,
        &new,
    )
    .unwrap();
    crate::node_project_ownership::NodeProjectOwnership::rebind_after_migration(
        &fixture.manager.data_root,
        &old,
        &new,
    )
    .unwrap();
    let detail = fixture.manager.configuration(id).await.unwrap();
    assert_eq!(detail["sourcePath"], json!(source));
    assert_eq!(detail["ownership"], "external");
    assert_eq!(source_files(&source), before);
    assert!(!new.join("custom/disposable-user-starter").exists());
    assert!(fixture
        .manager
        .project_ownership()
        .user_source_roots()
        .unwrap()
        .contains(&source));
    assert!(!fixture
        .manager
        .project_ownership()
        .owned_paths(id)
        .unwrap()
        .contains(&source));
}

#[test]
fn ordinary_web_previews_allow_unfinished_output_without_modifying_source() {
    for (entry_exists, build) in [(true, false), (false, true), (false, false)] {
        let f = Fixture::new();
        let mut declaration = json!({"manifestVersion":1,"runtime":"web","app":{"name":"普通网页"},"web":{"root":"dist","entry":"index.html"}});
        if build {
            declaration["development"] = json!({"build":{"kind":"npm","script":"bundle"}});
        }
        if entry_exists {
            fs::create_dir(f.source.join("dist")).unwrap();
            fs::write(f.source.join("dist/index.html"), "plain web").unwrap();
        }
        let bytes = serde_json::to_vec(&declaration).unwrap();
        fs::write(f.source.join("panda.json"), &bytes).unwrap();
        let preview = f.preview();
        assert_eq!(preview["state"], "ready", "{preview}");
        assert_eq!(preview["inspection"]["runtime"], "web");
        assert_eq!(preview["inspection"]["entryMissing"], !entry_exists);
        assert_eq!(preview["inspection"]["needsBuild"], !entry_exists && build);
        assert_eq!(fs::read(f.source.join("panda.json")).unwrap(), bytes);
        assert!(!f.source.join("data").exists());
        f.assert_no_platform_writes();
    }
}

#[test]
fn invalid_web_metadata_still_needs_review_with_specific_field_reason() {
    let f = Fixture::new();
    let mut declaration = json!({"manifestVersion":1,"runtime":"web","app":{"name":"网页"},"web":{"root":".","entry":"index.html"},"documentation":{"readme":"missing.md"}});
    fs::write(f.source.join("index.html"), "web").unwrap();
    fs::write(
        f.source.join("panda.json"),
        serde_json::to_vec(&declaration).unwrap(),
    )
    .unwrap();
    let preview = f.preview();
    assert_eq!(preview["state"], "needs-review");
    assert!(preview["issues"]
        .to_string()
        .contains("documentation.readme"));
    declaration.as_object_mut().unwrap().remove("documentation");
    declaration["unknownField"] = json!(true);
    fs::write(
        f.source.join("panda.json"),
        serde_json::to_vec(&declaration).unwrap(),
    )
    .unwrap();
    assert!(f.preview()["issues"].to_string().contains("unknownField"));
    f.assert_no_platform_writes();
}
