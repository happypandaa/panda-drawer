use super::super::*;
use std::os::unix::fs::symlink;
use tempfile::TempDir;

struct LiveControl {
    _stop: watch::Sender<bool>,
    _cancel: watch::Sender<bool>,
    control: InstallControl,
}

impl LiveControl {
    fn new() -> Self {
        let (stop, stop_rx) = watch::channel(false);
        let (cancel, cancel_rx) = watch::channel(false);
        Self {
            _stop: stop,
            _cancel: cancel,
            control: InstallControl::new(stop_rx, cancel_rx),
        }
    }
}

fn disposable_root() -> (TempDir, PathBuf) {
    let temp = tempfile::Builder::new()
        .prefix("disposable-node-install-policy-")
        .tempdir()
        .unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    (temp, root)
}

fn put(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn npm_fixture() -> (Value, Value) {
    let package = json!({
        "name": "disposable-install-policy",
        "version": "1.0.0",
        "dependencies": {"tiny": "^1.2.0"},
        "scripts": {"postinstall": "node ./not-executed.cjs"}
    });
    let lock = json!({
        "name": "disposable-install-policy",
        "version": "1.0.0",
        "lockfileVersion": 3,
        "packages": {
            "": {
                "name": "disposable-install-policy",
                "version": "1.0.0",
                "dependencies": {"tiny": "^1.2.0"}
            },
            "node_modules/tiny": {
                "version": "1.2.3",
                "resolved": "https://registry.npmjs.org/tiny/-/tiny-1.2.3.tgz",
                "bin": {"tiny": "./cli.js"}
            }
        }
    });
    (package, lock)
}

fn write_npm(root: &Path, package: &Value, lock: &Value) {
    put(root, "package.json", &serde_json::to_vec(package).unwrap());
    put(
        root,
        "package-lock.json",
        &serde_json::to_vec(lock).unwrap(),
    );
}

#[test]
fn npm_registry_locks_allow_ranges_tags_and_package_metadata() {
    let (_temp, root) = disposable_root();
    for lock_version in [2, 3] {
        for requirement in ["^1.2.0", ">=1.0.0 <2.0.0", "latest", "1.2.3-beta.1"] {
            let (mut package, mut lock) = npm_fixture();
            package["dependencies"]["tiny"] = json!(requirement);
            lock["packages"][""]["dependencies"]["tiny"] = json!(requirement);
            lock["lockfileVersion"] = json!(lock_version);
            if lock_version == 2 {
                lock["dependencies"] = json!({"tiny": {
                    "version": "1.2.3",
                    "resolved": "https://registry.npmjs.org/tiny/-/tiny-1.2.3.tgz"
                }});
            }
            write_npm(&root, &package, &lock);
            validate_npm_input(&root).unwrap();
        }
    }
    assert!(!root.join("not-executed.cjs").exists());
}

#[test]
fn npm_root_specs_reject_files_git_shorthand_urls_workspaces_and_aliases() {
    let (_temp, root) = disposable_root();
    for requirement in [
        ".",
        "..",
        "FILE:../outside",
        "file:../outside",
        "owner/repo",
        "https://github.com/owner/repo.git",
        "git+https://github.com/owner/repo.git",
        "workspace:*",
        "link:../outside",
        "npm:other@1.0.0",
        "C:\\outside\\package",
    ] {
        let (mut package, mut lock) = npm_fixture();
        package["dependencies"]["tiny"] = json!(requirement);
        lock["packages"][""]["dependencies"]["tiny"] = json!(requirement);
        write_npm(&root, &package, &lock);
        let error = validate_npm_input(&root).unwrap_err();
        assert_eq!(error.code, "NODE_INSTALL_INVALID", "{requirement}");
    }
}

#[test]
fn npm_transitive_and_workspace_declarations_cannot_bypass_spec_policy() {
    let (_temp, root) = disposable_root();
    for section in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        let (package, mut lock) = npm_fixture();
        lock["packages"]["node_modules/tiny"][section] = json!({"nested": "owner/repo"});
        write_npm(&root, &package, &lock);
        assert_eq!(
            validate_npm_input(&root).unwrap_err().code,
            "NODE_INSTALL_INVALID"
        );
    }
    let (package, mut lock) = npm_fixture();
    lock["lockfileVersion"] = json!(2);
    lock["dependencies"] = json!({"tiny": {
        "version": "1.2.3",
        "requires": {"nested": "FILE:../outside"}
    }});
    write_npm(&root, &package, &lock);
    assert_eq!(
        validate_npm_input(&root).unwrap_err().code,
        "NODE_INSTALL_INVALID"
    );

    let (mut package, lock) = npm_fixture();
    package["workspaces"] = json!(["packages/*"]);
    write_npm(&root, &package, &lock);
    assert_eq!(
        validate_npm_input(&root).unwrap_err().code,
        "NODE_INSTALL_INVALID"
    );
}

#[test]
fn npm_lock_downloads_require_credentialless_https_archives() {
    let (_temp, root) = disposable_root();
    for resolved in [
        "FILE:../outside",
        "https://github.com/owner/repo.git",
        "git+https://github.com/owner/repo.git",
        "http://registry.example.test/tiny.tgz",
        "https://user:fake-password@registry.example.test/tiny.tgz",
        "https://registry.example.test/tiny.tgz?token=fake",
        "https://registry.example.test/tiny.tgz#fragment",
    ] {
        let (package, mut lock) = npm_fixture();
        lock["packages"]["node_modules/tiny"]["resolved"] = json!(resolved);
        write_npm(&root, &package, &lock);
        assert_eq!(
            validate_npm_input(&root).unwrap_err().code,
            "NODE_INSTALL_INVALID"
        );
    }
}

#[test]
fn source_copy_excludes_local_state_and_never_shares_program_files() {
    let (_temp, root) = disposable_root();
    let source = root.join("source");
    let copied = root.join("candidate");
    let control = LiveControl::new();
    put(&source, "backend/server.cjs", b"source program");
    put(
        &source,
        "backend/config.js",
        b"program configuration helper",
    );
    put(&source, "web/index.html", b"static page");
    fs::set_permissions(
        source.join("backend/server.cjs"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let excluded_files = [
        ".env.local",
        ".npmrc",
        "configuration/settings.json",
        "credentials/fake-key",
        "data/main.sqlite",
        "node_modules/tiny/index.js",
        "backend/.env.production",
        "backend/node_modules/nested/index.js",
    ];
    for path in excluded_files {
        put(&source, path, b"disposable private state");
    }
    put(&root, "outside-private-state", b"disposable external value");
    symlink(root.join("outside-private-state"), source.join(".env")).unwrap();

    let digest = source_digest(&source, &control.control).unwrap();
    copy_source(&source, &copied, Path::new(""), &control.control).unwrap();
    assert_eq!(source_digest(&copied, &control.control).unwrap(), digest);
    for path in excluded_files.into_iter().chain([".env"]) {
        assert!(fs::symlink_metadata(copied.join(path)).is_err(), "{path}");
    }
    let original_file = source.join("backend/server.cjs");
    let copied_file = copied.join("backend/server.cjs");
    let original_metadata = fs::metadata(&original_file).unwrap();
    let copied_metadata = fs::metadata(&copied_file).unwrap();
    assert_ne!(original_metadata.ino(), copied_metadata.ino());
    assert_eq!(copied_metadata.nlink(), 1);
    assert_ne!(copied_metadata.mode() & 0o111, 0);
    assert_eq!(
        fs::read(copied.join("backend/config.js")).unwrap(),
        b"program configuration helper"
    );
    fs::write(&copied_file, b"candidate edited independently").unwrap();
    assert_eq!(fs::read(&original_file).unwrap(), b"source program");
    fs::write(&original_file, b"source edited independently").unwrap();
    assert_eq!(
        fs::read(&copied_file).unwrap(),
        b"candidate edited independently"
    );
    assert_eq!(
        fs::read(root.join("outside-private-state")).unwrap(),
        b"disposable external value"
    );
}

#[test]
fn program_digest_allows_internal_relative_npm_bin_links_and_hashes_their_payload() {
    let (_temp, root) = disposable_root();
    let control = LiveControl::new();
    put(
        &root,
        "node_modules/tiny/cli.js",
        b"first executable script",
    );
    fs::create_dir(root.join("node_modules/.bin")).unwrap();
    symlink("../tiny/cli.js", root.join("node_modules/.bin/tiny")).unwrap();
    let first = program_digest(&root, &control.control).unwrap();
    put(
        &root,
        "node_modules/tiny/cli.js",
        b"changed executable script",
    );
    let second = program_digest(&root, &control.control).unwrap();
    assert_ne!(first, second);
}

#[test]
fn program_digest_rejects_external_relative_and_all_absolute_symlinks() {
    let (_temp, root) = disposable_root();
    let program = root.join("program");
    let control = LiveControl::new();
    put(&program, "server.cjs", b"ordinary program");
    put(&root, "outside.cjs", b"disposable external program");
    for target in [
        PathBuf::from("../outside.cjs"),
        program.join("server.cjs"),
        root.join("outside.cjs"),
    ] {
        let link = program.join("unsafe-link");
        symlink(&target, &link).unwrap();
        assert_eq!(
            program_digest(&program, &control.control).unwrap_err().code,
            "NODE_INSTALL_INVALID"
        );
        fs::remove_file(link).unwrap();
    }
    assert_eq!(
        fs::read(root.join("outside.cjs")).unwrap(),
        b"disposable external program"
    );
    program_digest(&program, &control.control).unwrap();
}

#[test]
fn program_digest_rejects_third_party_node_addons() {
    let (_temp, root) = disposable_root();
    let control = LiveControl::new();
    put(&root, "server.cjs", b"ordinary program");
    for extension in ["node", "NODE"] {
        let relative = format!("node_modules/native/payload.{extension}");
        put(&root, &relative, b"disposable native payload");
        assert_eq!(
            program_digest(&root, &control.control).unwrap_err().code,
            "NODE_INSTALL_INVALID"
        );
        fs::remove_file(root.join(relative)).unwrap();
    }
    program_digest(&root, &control.control).unwrap();
}

fn configured_manifest() -> Value {
    json!({
        "app": {"id": "disposable-policy-app", "name": "Policy fixture", "version": "1.0.0"},
        "web": {"root": "web", "entry": "index.html", "routing": "hash"},
        "backend": {"root": "backend", "entry": "server.cjs", "nodeMajor": 22, "protocolVersion": 1},
        "configuration": {"version": 1, "fields": [{
            "name": "endpoint", "label": "Service URL", "type": "url", "required": true,
            "secret": false, "description": "Disposable test endpoint", "howToGet": ["Use the test endpoint"]
        }]}
    })
}

fn installed_target(manifest: Value) -> InstallTargetSnapshot {
    InstallTargetSnapshot {
        app_id: Uuid::new_v4().to_string(),
        registration_digest: Some("a".repeat(64)),
        generation: Some(format!("gen_{}", "1".repeat(32))),
        manifest: Some(manifest),
        configuration_revision: 7,
        external_registration: None,
        development_source: None,
    }
}

#[test]
fn compatible_code_update_keeps_app_identity_configuration_and_target_revision() {
    let previous = configured_manifest();
    let target = installed_target(previous.clone());
    let original = target.clone();
    let mut next = previous;
    next["app"]["version"] = json!("1.1.0");
    next["app"]["name"] = json!("Updated display name");
    next["backend"]["entry"] = json!("updated-server.cjs");
    compatible(&target, &next).unwrap();
    assert_eq!(target, original);
    next["app"]["id"] = json!("different-application");
    assert_eq!(
        compatible(&target, &next).unwrap_err().code,
        "NODE_INSTALL_APP_MISMATCH"
    );
    assert_eq!(target, original);
}

#[test]
fn compatible_rejects_any_configuration_declaration_change() {
    let previous = configured_manifest();
    let target = installed_target(previous.clone());
    let original = target.clone();
    for (field, value) in [
        ("required", json!(false)),
        ("secret", json!(true)),
        ("type", json!("string")),
        ("label", json!("A different label")),
        ("default", json!("https://example.test")),
    ] {
        let mut next = previous.clone();
        next["configuration"]["fields"][0][field] = value;
        assert_eq!(
            compatible(&target, &next).unwrap_err().code,
            "NODE_INSTALL_CONFIGURATION_INCOMPATIBLE"
        );
    }
    let mut next = previous;
    next.as_object_mut().unwrap().remove("configuration");
    assert_eq!(
        compatible(&target, &next).unwrap_err().code,
        "NODE_INSTALL_CONFIGURATION_INCOMPATIBLE"
    );
    assert_eq!(target, original);
}

#[test]
fn npm_space_scan_counts_private_cache_and_never_follows_links_or_reads_content() {
    let (_temp, root) = disposable_root();
    let operation = root.join("operation");
    put(&operation, "program/source.js", b"12345678");
    put(&operation, "npm-cache/archive", b"12345678");
    let mut live = LiveControl::new();
    live.control.npm_space.max_total = 15;
    assert_eq!(
        scan_npm_space(&operation, &live.control, |_| Ok(u64::MAX))
            .unwrap_err()
            .code,
        "NODE_INSTALL_SPACE_LIMIT"
    );
    live.control.npm_space.max_total = 512;
    live.control.npm_space.minimum_free = 100;
    assert_eq!(
        scan_npm_space(&operation, &live.control, |_| Ok(99))
            .unwrap_err()
            .code,
        "NODE_INSTALL_SPACE_INSUFFICIENT"
    );
    assert_eq!(
        scan_npm_space(&operation, &live.control, |_| Err(std::io::Error::other(
            "simulated disk query failure"
        )))
        .unwrap_err()
        .code,
        "NODE_INSTALL_SPACE_CHECK_FAILED"
    );
    let external = root.join("outside-evidence");
    put(&external, "not-part-of-the-installation", &[0; 1024]);
    symlink(&external, operation.join("npm-cache/external-link")).unwrap();
    symlink("missing-target", operation.join("npm-cache/dangling-link")).unwrap();
    // Even an outside link and a not-yet-created npm bin target are only counted
    // as link metadata; final program validation owns the stricter link policy.
    scan_npm_space(&operation, &live.control, |_| Ok(100)).unwrap();
    assert_eq!(
        fs::read(external.join("not-part-of-the-installation")).unwrap(),
        vec![0; 1024]
    );
}
