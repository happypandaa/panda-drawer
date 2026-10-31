//! All paths belong to fresh disposable roots. Inspection and registration
//! never execute a Node/npm/Git command; the last opt-in test runs the starter.
use super::*;

struct Fixture {
    _temporary: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    manager: NodeApps,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-w4-directory-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("selected-directory 中文 space");
        let data = root.join("disposable-platform-data");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&data).unwrap();
        let manager = NodeApps::new(data, root.join("never-executed-node.exe")).unwrap();
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
    fn input(&self) -> DirectoryInitializeInput {
        DirectoryInitializeInput {
            expected_revision: self.preview()["expectedRevision"].as_str().unwrap().into(),
            name: "Windows 起始应用 <private>".into(),
        }
    }
    fn existing(&self) {
        fs::create_dir(self.source.join("web")).unwrap();
        fs::create_dir(self.source.join("backend")).unwrap();
        fs::write(
            self.source.join("web/index.html"),
            b"<p>Preserve external original</p>",
        )
        .unwrap();
        fs::write(
            self.source.join("backend/server.mjs"),
            b"throw new Error('must never execute during directory onboarding');",
        )
        .unwrap();
        fs::write(self.source.join("local-app.json"),serde_json::to_vec(&json!({
            "packageFormat":"local-web-app","formatVersion":2,"runtime":"node-service",
            "app":{"id":"dev.disposable.external.windows","name":"External original","version":"1"},
            "web":{"root":"web","entry":"index.html","routing":"files"},
            "backend":{"protocolVersion":1,"root":"backend","entry":"server.mjs","nodeMajor":24}
        })).unwrap()).unwrap();
        // Local registration must not validate/install dependency declarations.
        fs::write(
            self.source.join("package.json"),
            b"{\"scripts\":{\"install\":\"must-never-run\"}}",
        )
        .unwrap();
        fs::write(
            self.source.join("package-lock.json"),
            b"not an npm lock; external original must remain untouched",
        )
        .unwrap();
    }
    fn no_owned_material(&self) {
        assert!(!self.manager.root.exists());
        for name in ["node-project-ownership", "node-source-owners"] {
            let path = self.manager.data_root.join(name);
            if path.exists() {
                assert!(fs::read_dir(path).unwrap().next().is_none());
            }
        }
    }
    fn pending_id(&self) -> String {
        self.manager
            .pending_generated_source_id(&self.source)
            .unwrap()
            .unwrap()
    }
}
fn source_argument(fixture: &Fixture, style: &str) -> PathBuf {
    if style == "verbatim" {
        return fixture.source.clone();
    }
    let conventional =
        platform_native::windows::api_paths::conventional_path(&fixture.source).unwrap();
    assert!(!conventional.to_str().unwrap().starts_with(r"\\?\"));
    if style == "forward-slashes" {
        PathBuf::from(conventional.to_str().unwrap().replace('\\', "/"))
    } else {
        conventional
    }
}

#[tokio::test]
async fn dos_folder_picker_paths_roundtrip_inspect_initialize_and_confirm_with_canonical_identity()
{
    for style in ["forward-slashes", "backslashes"] {
        let fixture = Fixture::new();
        let source = source_argument(&fixture, style);
        assert_ne!(source.as_os_str(), fixture.source.as_os_str());
        let identity = PersistedIdentity::capture(&fixture.source).unwrap();
        let preview = fixture.manager.inspect_directory(&source).unwrap();
        assert_eq!(preview["sourcePath"], json!(source));
        assert_eq!(preview["state"], "empty");
        assert_eq!(
            preview["expectedRevision"],
            fixture.preview()["expectedRevision"]
        );
        assert!(preview["reviewPrompt"]
            .as_str()
            .unwrap()
            .contains(&serde_json::to_string(&source).unwrap()));
        let initialized = fixture
            .manager
            .initialize_directory(
                &source,
                DirectoryInitializeInput {
                    expected_revision: preview["expectedRevision"].as_str().unwrap().into(),
                    name: "DOS 路径往返起始应用".into(),
                },
            )
            .unwrap();
        assert_eq!(initialized["sourcePath"], json!(source));
        assert_eq!(initialized["state"], "ready");
        let pending = initialized["pendingAppId"].as_str().unwrap();
        let registered = fixture
            .manager
            .register_directory(
                source.clone(),
                initialized["expectedRevision"].as_str().unwrap().into(),
            )
            .await
            .unwrap();
        assert_eq!(registered["sourcePath"], json!(source));
        assert_eq!(registered["id"], pending);
        assert_eq!(registered["ownership"], "managed");
        let stored = fixture.manager.registrations().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].source_path, fixture.source);
        assert!(identity.matches_path(&stored[0].source_path).unwrap());
        assert_eq!(
            fixture
                .manager
                .project_ownership()
                .generated_source(pending)
                .unwrap(),
            Some(fixture.source.clone())
        );
        let repeated = fixture.manager.inspect_directory(&source).unwrap();
        assert_eq!(repeated["sourcePath"], json!(source));
        assert_eq!(repeated["existingApp"]["id"], pending);
        let exact = fixture.manager.inspect_directory(&fixture.source).unwrap();
        assert_eq!(exact["sourcePath"], json!(fixture.source));
        assert_eq!(exact["existingApp"]["id"], pending);
        assert!(fixture.manager.owned_process_ids().await.is_empty());
    }
}

#[tokio::test]
async fn dos_and_explicit_verbatim_paths_roundtrip_both_combined_and_external_registration() {
    for style in ["forward-slashes", "backslashes", "verbatim"] {
        for external in [false, true] {
            let fixture = Fixture::new();
            if external {
                fixture.existing();
            }
            let source = source_argument(&fixture, style);
            let identity = PersistedIdentity::capture(&fixture.source).unwrap();
            let before = snapshot(&fixture.source);
            let preview = fixture.manager.inspect_directory(&source).unwrap();
            assert_eq!(preview["sourcePath"], json!(source));
            let revision = preview["expectedRevision"].as_str().unwrap().to_string();
            let registered = if external {
                fixture
                    .manager
                    .register_directory(source.clone(), revision)
                    .await
                    .unwrap()
            } else {
                fixture
                    .manager
                    .initialize_and_register_directory(
                        source.clone(),
                        DirectoryInitializeInput {
                            expected_revision: revision,
                            name: "原路径表示保持不变".into(),
                        },
                    )
                    .await
                    .unwrap()
            };
            assert_eq!(registered["sourcePath"], json!(source));
            assert_eq!(
                registered["ownership"],
                if external { "external" } else { "managed" }
            );
            let id = registered["id"].as_str().unwrap();
            let stored = fixture.manager.registrations().unwrap();
            assert_eq!(stored.len(), 1);
            assert_eq!(stored[0].source_path, fixture.source);
            assert!(identity.matches_path(&stored[0].source_path).unwrap());
            let rechecked = fixture.manager.inspect_directory(&source).unwrap();
            assert_eq!(rechecked["sourcePath"], json!(source));
            assert_eq!(rechecked["existingApp"]["id"], id);
            if external {
                assert_eq!(snapshot(&fixture.source), before);
            }
            assert!(!fixture.source.join("node_modules").exists());
            assert!(fixture.manager.owned_process_ids().await.is_empty());
        }
    }
}

#[tokio::test]
async fn explicit_verbatim_trailing_dot_keeps_its_namespace_and_never_uses_the_dotless_sibling() {
    let fixture = Fixture::new();
    let source = fixture.root.join("CLI exact namespace.");
    let sibling = fixture.root.join("CLI exact namespace");
    native::create_private_directory(&source).unwrap();
    fs::create_dir(&sibling).unwrap();
    fs::write(sibling.join("foreign.txt"), b"different physical directory").unwrap();
    let before = snapshot(&sibling);
    let identity = PersistedIdentity::capture(&source).unwrap();
    assert!(!identity.matches_path(&sibling).unwrap());
    assert_eq!(
        platform_native::windows::api_paths::conventional_path(&source).unwrap(),
        source
    );
    let preview = fixture.manager.inspect_directory(&source).unwrap();
    assert_eq!(preview["sourcePath"], json!(source));
    assert!(preview["reviewPrompt"]
        .as_str()
        .unwrap()
        .contains(&serde_json::to_string(&source).unwrap()));
    let registered = fixture
        .manager
        .initialize_and_register_directory(
            source.clone(),
            DirectoryInitializeInput {
                expected_revision: preview["expectedRevision"].as_str().unwrap().into(),
                name: "Exact verbatim namespace".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(registered["sourcePath"], json!(source));
    let stored = fixture.manager.registrations().unwrap();
    assert_eq!(stored.len(), 1);
    assert!(identity.matches_path(&stored[0].source_path).unwrap());
    assert_eq!(snapshot(&sibling), before);
    assert!(source.join("local-app.json").is_file());
    assert!(!sibling.join("local-app.json").exists());
    assert!(fixture.manager.owned_process_ids().await.is_empty());
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, (native::FileIdentity, Vec<u8>)> {
    fn collect(
        root: &Path,
        path: &Path,
        result: &mut BTreeMap<PathBuf, (native::FileIdentity, Vec<u8>)>,
    ) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().into(),
                    (
                        native::open_no_reparse(&path).unwrap().identity,
                        fs::read(&path).unwrap(),
                    ),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    collect(root, root, &mut result);
    result
}
fn readonly(path: &Path, value: bool) {
    native::set_read_only(path, value).unwrap();
}
fn junction(link: &Path, target: &Path) {
    let output = std::process::Command::new(
        platform_native::windows::system_paths::SystemPaths::current()
            .unwrap()
            .system
            .join("cmd.exe"),
    )
    .args(["/d", "/c", "mklink", "/J"])
    .arg(link)
    .arg(target)
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn inspection_is_read_only_and_does_not_read_unrelated_readonly_secret_contents() {
    let fixture = Fixture::new();
    let source = native::open_no_reparse(&fixture.source).unwrap();
    assert!(native::verify_private(&source.file).is_err());
    drop(source);
    assert_eq!(fixture.preview()["state"], "empty");
    let secret = fixture.source.join(".env");
    fs::write(&secret, b"DISPOSABLE_SECRET_DO_NOT_READ").unwrap();
    readonly(&secret, true);
    let before = native::open_no_reparse(&secret).unwrap().identity;
    let preview = fixture.preview();
    assert_eq!(preview["state"], "needs-review");
    assert!(!preview
        .to_string()
        .contains("DISPOSABLE_SECRET_DO_NOT_READ"));
    assert!(preview["reviewPrompt"]
        .as_str()
        .unwrap()
        .contains(REVIEW_SKILL));
    assert_eq!(native::open_no_reparse(&secret).unwrap().identity, before);
    assert!(fs::metadata(&secret).unwrap().permissions().readonly());
    assert_eq!(fs::read(&secret).unwrap(), b"DISPOSABLE_SECRET_DO_NOT_READ");
    assert!(fs::read_dir(&fixture.manager.data_root)
        .unwrap()
        .next()
        .is_none());
    readonly(&secret, false);
}

#[tokio::test]
async fn external_directory_confirmation_registers_the_original_without_copy_npm_or_runtime() {
    let fixture = Fixture::new();
    fixture.existing();
    readonly(&fixture.source.join("local-app.json"), true);
    let before = snapshot(&fixture.source);
    let preview = fixture.preview();
    assert_eq!(preview["state"], "ready");
    let registered = fixture
        .manager
        .register_directory(
            fixture.source.clone(),
            preview["expectedRevision"].as_str().unwrap().into(),
        )
        .await
        .unwrap();
    let id = registered["id"].as_str().unwrap();
    assert_eq!(registered["ownership"], "external");
    assert_eq!(registered["sourcePath"], json!(fixture.source));
    assert!(!fixture
        .manager
        .project_ownership()
        .has_generated_source(id, &fixture.source)
        .unwrap());
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    assert!(!fixture.source.join("node_modules").exists());
    assert_eq!(snapshot(&fixture.source), before);
    assert!(fs::metadata(fixture.source.join("local-app.json"))
        .unwrap()
        .permissions()
        .readonly());
    let existing = fixture.preview();
    assert_eq!(existing["existingApp"]["id"], id);
    assert!(fixture
        .manager
        .register_directory(
            fixture.source.clone(),
            existing["expectedRevision"].as_str().unwrap().into()
        )
        .await
        .is_err());
    assert_eq!(fixture.manager.list().await.unwrap().len(), 1);
    readonly(&fixture.source.join("local-app.json"), false);
}

#[tokio::test]
async fn generated_directory_binds_one_uuid_before_writing_and_reuses_it_when_confirmed() {
    let fixture = Fixture::new();
    let original = native::open_no_reparse(&fixture.source).unwrap().identity;
    let mut witnessed = None;
    let initialized = fixture
        .manager
        .initialize_directory_admitted(&fixture.source, fixture.input(), |_| {
            let id = fixture
                .manager
                .pending_generated_source_id(&fixture.source)?
                .unwrap();
            assert!(fixture
                .manager
                .project_ownership()
                .has_generated_source(&id, &fixture.source)?);
            assert_eq!(native::open_no_reparse(&fixture.source)?.identity, original);
            if let Some(previous) = &witnessed {
                assert_eq!(previous, &id);
            } else {
                witnessed = Some(id);
            }
            Ok(())
        })
        .unwrap();
    let id = initialized["pendingAppId"].as_str().unwrap();
    assert_eq!(witnessed.as_deref(), Some(id));
    assert_eq!(fixture.pending_id(), id);
    assert_eq!(initialized["state"], "ready");
    assert!(fixture.manager.list().await.unwrap().is_empty());
    for path in snapshot(&fixture.source).keys() {
        native::verify_private(
            &native::open_no_reparse(&fixture.source.join(path))
                .unwrap()
                .file,
        )
        .unwrap();
    }
    assert!(
        native::verify_private(&native::open_no_reparse(&fixture.source).unwrap().file).is_err()
    );
    assert_eq!(
        native::open_no_reparse(&fixture.source).unwrap().identity,
        original
    );
    let registered = fixture
        .manager
        .register_directory(
            fixture.source.clone(),
            initialized["expectedRevision"].as_str().unwrap().into(),
        )
        .await
        .unwrap();
    assert_eq!(registered["id"], id);
    assert_eq!(registered["ownership"], "managed");
    assert_eq!(
        fixture
            .manager
            .project_ownership()
            .generated_source(id)
            .unwrap(),
        Some(fixture.source.clone())
    );
    assert!(!fixture.source.join("node_modules").exists());
    assert!(fixture.manager.owned_process_ids().await.is_empty());
}

#[tokio::test]
async fn combined_generation_and_registration_produces_the_same_ready_starter() {
    let fixture = Fixture::new();
    let registered = fixture
        .manager
        .initialize_and_register_directory(fixture.source.clone(), fixture.input())
        .await
        .unwrap();
    assert_eq!(registered["ownership"], "managed");
    assert_eq!(registered["state"], "stopped");
    assert_eq!(
        fs::read_to_string(fixture.source.join("backend/server.mjs")).unwrap(),
        SERVER
    );
    assert_eq!(
        fs::read_to_string(fixture.source.join("web/app.js")).unwrap(),
        FRONTEND
    );
    assert!(fs::read_to_string(fixture.source.join("web/index.html"))
        .unwrap()
        .contains("&lt;private&gt;"));
    assert!(fixture.manager.owned_process_ids().await.is_empty());
}

#[tokio::test]
async fn stale_review_after_top_level_or_declared_file_change_never_registers() {
    for declared in [false, true] {
        let fixture = Fixture::new();
        fixture.existing();
        let preview = fixture.preview();
        if declared {
            let path = fixture.source.join("backend/server.mjs");
            fs::write(&path, b"changed externally").unwrap();
            fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
                .unwrap();
        } else {
            fs::write(
                fixture.source.join(".env"),
                b"preserve unrelated concurrent file",
            )
            .unwrap();
        }
        let before = snapshot(&fixture.source);
        let error = fixture
            .manager
            .register_directory(
                fixture.source.clone(),
                preview["expectedRevision"].as_str().unwrap().into(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "NODE_DIRECTORY_CHANGED");
        assert_eq!(snapshot(&fixture.source), before);
        fixture.no_owned_material();
    }
}

#[test]
fn concurrent_content_before_first_creation_is_preserved_and_no_project_is_generated() {
    let fixture = Fixture::new();
    let error = fixture
        .manager
        .initialize_directory_admitted(&fixture.source, fixture.input(), |_| {
            fs::write(
                fixture.source.join("user-content"),
                b"preserve concurrent original",
            )?;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.code, "NODE_DIRECTORY_CHANGED");
    assert_eq!(error.details["partialFilesRetained"], false);
    assert_eq!(
        fs::read(fixture.source.join("user-content")).unwrap(),
        b"preserve concurrent original"
    );
    assert_eq!(fs::read_dir(&fixture.source).unwrap().count(), 1);
    fixture.no_owned_material();
}

#[test]
fn partial_generation_actually_rolls_back_its_readonly_file_and_all_of_its_directories() {
    let fixture = Fixture::new();
    let mut witnessed = false;
    let error = fixture
        .manager
        .initialize_directory_admitted(&fixture.source, fixture.input(), |path| {
            if path == Path::new("web/app.js") {
                let index = fixture.source.join("web/index.html");
                assert!(!fs::read(&index)?.is_empty());
                native::verify_private(&native::open_no_reparse(&index)?.file)?;
                readonly(&index, true);
                witnessed = true;
                return Err(directory_error(
                    "DISPOSABLE_WRITE_FAILURE",
                    "Controlled failure after actual private file creation",
                ));
            }
            Ok(())
        })
        .unwrap_err();
    assert!(witnessed);
    assert_eq!(error.code, "DISPOSABLE_WRITE_FAILURE");
    assert_eq!(error.details["partialFilesRetained"], false);
    assert_eq!(error.details["operationOutcome"], "not_started");
    assert!(fs::read_dir(&fixture.source).unwrap().next().is_none());
    fixture.no_owned_material();
}

#[test]
fn rollback_preserves_a_concurrent_unrelated_file_without_adopting_it() {
    let fixture = Fixture::new();
    let error = fixture
        .manager
        .initialize_directory_admitted(&fixture.source, fixture.input(), |path| {
            if path == Path::new("web/app.js") {
                fs::write(fixture.source.join("user-file"), b"foreign bytes")?;
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.details["partialFilesRetained"], false);
    assert_eq!(fs::read_dir(&fixture.source).unwrap().count(), 1);
    assert_eq!(
        fs::read(fixture.source.join("user-file")).unwrap(),
        b"foreign bytes"
    );
    fixture.no_owned_material();
}

#[test]
fn changed_generated_bytes_are_preserved_with_the_pending_original_ownership() {
    let fixture = Fixture::new();
    let error = fixture
        .manager
        .initialize_directory_admitted(&fixture.source, fixture.input(), |path| {
            if path == Path::new("web/app.js") {
                fs::write(
                    fixture.source.join("web/index.html"),
                    b"concurrent authored edit",
                )?;
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.details["partialFilesRetained"], true);
    assert_eq!(
        fs::read(fixture.source.join("web/index.html")).unwrap(),
        b"concurrent authored edit"
    );
    assert_eq!(
        fixture.pending_id(),
        error.details["appId"].as_str().unwrap()
    );
    assert!(!fixture.source.join("local-app.json").exists());
}

#[test]
fn replaced_created_directory_and_displaced_original_are_never_written_or_deleted() {
    let fixture = Fixture::new();
    let preserved = fixture.root.join("preserved-created-web");
    let error = fixture
        .manager
        .initialize_directory_admitted(&fixture.source, fixture.input(), |path| {
            if path == Path::new("web/index.html") {
                fs::rename(fixture.source.join("web"), &preserved)?;
                fs::create_dir(fixture.source.join("web"))?;
                fs::write(fixture.source.join("web/foreign"), b"preserve replacement")?;
            }
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.details["partialFilesRetained"], true);
    assert!(fs::read_dir(&preserved).unwrap().next().is_none());
    assert_eq!(
        fs::read(fixture.source.join("web/foreign")).unwrap(),
        b"preserve replacement"
    );
    assert!(!fixture.source.join("web/index.html").exists());
    assert_eq!(
        fixture.pending_id(),
        error.details["appId"].as_str().unwrap()
    );
}

#[test]
fn a_bound_initialization_root_cannot_be_renamed_out_from_under_the_writer() {
    let fixture = Fixture::new();
    let moved = fixture.root.join("must-not-move-selected-root");
    let identity = native::open_no_reparse(&fixture.source).unwrap().identity;
    let error = fixture
        .manager
        .initialize_directory_admitted(&fixture.source, fixture.input(), |_| {
            fs::rename(&fixture.source, &moved)?;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.details["partialFilesRetained"], false);
    assert!(!moved.exists());
    assert_eq!(
        native::open_no_reparse(&fixture.source).unwrap().identity,
        identity
    );
    assert!(fs::read_dir(&fixture.source).unwrap().next().is_none());
    fixture.no_owned_material();
}

#[test]
fn hardlinked_or_oversized_public_manifest_and_hardlinked_backend_remain_review_material() {
    for mode in ["manifest", "backend", "oversized"] {
        let fixture = Fixture::new();
        fixture.existing();
        if mode == "oversized" {
            fs::write(fixture.source.join("local-app.json"), vec![b'x'; 65537]).unwrap();
        } else {
            let path = fixture.source.join(if mode == "manifest" {
                "local-app.json"
            } else {
                "backend/server.mjs"
            });
            fs::hard_link(&path, fixture.root.join("preserved-external-hardlink")).unwrap();
        }
        let before = snapshot(&fixture.source);
        assert_eq!(fixture.preview()["state"], "needs-review");
        assert_eq!(snapshot(&fixture.source), before);
        fixture.no_owned_material();
    }
}

#[test]
fn real_root_and_declared_junctions_are_rejected_without_reading_or_changing_targets() {
    let fixture = Fixture::new();
    fixture.existing();
    let target = fixture.root.join("external-junction-target");
    fs::create_dir(&target).unwrap();
    fs::write(
        target.join("server.mjs"),
        b"outside original not executable",
    )
    .unwrap();
    let before = snapshot(&target);
    let link = fixture.root.join("selected-root-junction");
    junction(&link, &fixture.source);
    assert!(fixture.manager.inspect_directory(&link).is_err());
    fs::remove_dir(&link).unwrap();
    fs::rename(
        fixture.source.join("backend"),
        fixture.root.join("preserved-backend"),
    )
    .unwrap();
    junction(&fixture.source.join("backend"), &target);
    assert_eq!(fixture.preview()["state"], "needs-review");
    assert_eq!(snapshot(&target), before);
    fs::remove_dir(fixture.source.join("backend")).unwrap();
    fixture.no_owned_material();
}

#[test]
fn an_unrelated_top_level_junction_is_observed_without_following_its_target() {
    let fixture = Fixture::new();
    let target = fixture.root.join("external-private-target");
    fs::create_dir(&target).unwrap();
    fs::write(
        target.join("secret"),
        b"DISPOSABLE_TARGET_SECRET_DO_NOT_READ",
    )
    .unwrap();
    let link = fixture.source.join("unrelated-link");
    junction(&link, &target);
    let preview = fixture.preview();
    assert_eq!(preview["state"], "needs-review");
    assert!(!preview
        .to_string()
        .contains("DISPOSABLE_TARGET_SECRET_DO_NOT_READ"));
    assert_eq!(
        fs::read(target.join("secret")).unwrap(),
        b"DISPOSABLE_TARGET_SECRET_DO_NOT_READ"
    );
    fs::remove_dir(link).unwrap();
    fixture.no_owned_material();
}

#[tokio::test]
async fn generated_starter_survives_registration_failure_and_reinspection_reuses_its_uuid() {
    let fixture = Fixture::new();
    let input = fixture.input();
    fs::write(&fixture.manager.root, b"disposable registration obstacle").unwrap();
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
    let id = fixture.pending_id();
    assert_eq!(error.details["appId"], id);
    let before = snapshot(&fixture.source);
    fs::remove_file(&fixture.manager.root).unwrap();
    let preview = fixture.preview();
    let registered = fixture
        .manager
        .register_directory(
            fixture.source.clone(),
            preview["expectedRevision"].as_str().unwrap().into(),
        )
        .await
        .unwrap();
    assert_eq!(registered["id"], id);
    assert_eq!(snapshot(&fixture.source), before);
}

#[tokio::test]
async fn valid_concurrent_edit_after_generation_requires_reinspection_before_registration() {
    let fixture = Fixture::new();
    let mut edited = false;
    let error = fixture
        .manager
        .initialize_and_register_directory_admitted(
            fixture.source.clone(),
            fixture.input(),
            |_| Ok(()),
            |source| {
                if !edited {
                    let path = source.join("local-app.json");
                    let mut value: Value = serde_json::from_slice(&fs::read(&path)?)?;
                    value["app"]["name"] = json!("Unconfirmed concurrent author name");
                    fs::write(path, serde_json::to_vec(&value)?)?;
                    edited = true;
                }
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
    let id = fixture.pending_id();
    let preview = fixture.preview();
    assert_eq!(preview["state"], "ready");
    let added = fixture
        .manager
        .register_directory(
            fixture.source.clone(),
            preview["expectedRevision"].as_str().unwrap().into(),
        )
        .await
        .unwrap();
    assert_eq!(added["id"], id);
    assert_eq!(added["name"], "Unconfirmed concurrent author name");
}

#[tokio::test]
#[ignore = "Explicit pinned PANDA_W3_OFFICIAL_NODE required; generates and runs only a new disposable starter"]
async fn generated_starter_runs_through_the_confirmed_windows_directory_entry() {
    let mut fixture = Fixture::new();
    let node = PathBuf::from(
        std::env::var_os("PANDA_W3_OFFICIAL_NODE").expect("explicit pinned official Node"),
    );
    fixture.manager = NodeApps::new(fixture.manager.data_root.clone(), node).unwrap();
    let registered = fixture
        .manager
        .initialize_and_register_directory(fixture.source.clone(), fixture.input())
        .await
        .unwrap();
    let id = registered["id"].as_str().unwrap();
    fixture.manager.start(id).await.unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let open = fixture.manager.open_url(id).await.unwrap();
    let origin = url::Url::parse(&open)
        .unwrap()
        .origin()
        .ascii_serialization();
    let response = client.get(open).send().await.unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SEE_OTHER);
    let cookie = response.headers()[reqwest::header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let body = client
        .get(format!("{origin}/api/hello"))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["message"],
        "你的本地应用已经连通。"
    );
    fixture.manager.stop(id).await.unwrap();
    assert!(fixture.manager.owned_process_ids().await.is_empty());
    assert!(!fixture.source.join("node_modules").exists());
}
