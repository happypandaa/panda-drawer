use super::protocol::*;
use super::verify_installed_resources;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
};

struct Sample {
    _temp: tempfile::TempDir,
    root: PathBuf,
    trusted: TrustedContext,
    manifest: Manifest,
}

fn policy_bytes() -> Vec<u8> {
    serde_json::to_vec(&json!({"schemaVersion":1,
        "signingProfile":"macos-developer-id-hardened-runtime-v1", "expectedTeamId":"TESTTEAM01",
        "nodeEntitlementsSha256":digest(b"synthetic-node-entitlements-no-signing-proof"),
        "gitEntitlementsSha256":digest(b"synthetic-git-entitlements-no-signing-proof")}))
    .unwrap()
}

fn source_tree(files: &BTreeMap<String, Vec<u8>>) -> serde_json::Value {
    let mut h = sha2::Sha256::new();
    let mut size = 0u64;
    use sha2::Digest;
    for (name, bytes) in files {
        h.update(format!("{name}\0{}\0{}\n", bytes.len(), digest(bytes)));
        size += bytes.len() as u64;
    }
    json!({"rule":"sorted-utf8-path-nul-size-nul-sha256-lf-v1", "fileCount":files.len(),"totalBytes":size,"sha256":format!("{:x}",h.finalize())})
}

fn write_tree(root: &Path, files: &BTreeMap<String, Vec<u8>>, executables: &[&str]) {
    fs::create_dir_all(root).unwrap();
    for (name, bytes) in files {
        let path = root.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if executables.contains(&name.as_str()) {
                0o755
            } else {
                0o644
            }),
        )
        .unwrap();
    }
    fn directories(path: &Path) {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        for child in fs::read_dir(path).unwrap() {
            let path = child.unwrap().path();
            if path.is_dir() {
                directories(&path);
            }
        }
    }
    directories(root);
}

fn capture(
    root: &Path,
    kind: PayloadKind,
    trusted: &TrustedContext,
    original: &BTreeMap<String, Vec<u8>>,
) -> Payload {
    fn visit(
        root: &Path,
        dir: &Path,
        kind: PayloadKind,
        trusted: &TrustedContext,
        original: &BTreeMap<String, Vec<u8>>,
        entries: &mut Vec<Entry>,
    ) {
        for child in fs::read_dir(dir).unwrap() {
            let path = child.unwrap().path();
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            let info = fs::symlink_metadata(&path).unwrap();
            let mode = info.permissions().mode() & 0o7777;
            if info.is_dir() {
                entries.push(Entry::Directory {
                    path: relative,
                    mode,
                });
                visit(root, &path, kind, trusted, original, entries);
            } else {
                let bytes = fs::read(&path).unwrap();
                let is_macho = bytes.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
                    || bytes.starts_with(&[0xca, 0xfe, 0xba, 0xbe]);
                let (signature, source) = if is_macho {
                    let before = original.get(&relative).unwrap_or(&bytes);
                    (
                        Some(SignatureBinding {
                            team_id: trusted.policy.expected_team_id.clone().unwrap(),
                            identifier: trusted.expected_identifier(kind, &relative),
                            hardened_runtime: true,
                            entitlements_sha256: if kind == PayloadKind::DevelopmentGit {
                                trusted.policy.git_entitlements_sha256.clone().unwrap()
                            } else {
                                trusted.policy.node_entitlements_sha256.clone().unwrap()
                            },
                        }),
                        Some(SourceFile {
                            size: before.len() as u64,
                            sha256: digest(before),
                        }),
                    )
                } else {
                    (None, None)
                };
                entries.push(Entry::File {
                    path: relative,
                    mode,
                    size: bytes.len() as u64,
                    sha256: digest(&bytes),
                    signature,
                    source,
                });
            }
        }
    }
    let mut entries = Vec::new();
    visit(root, root, kind, trusted, original, &mut entries);
    fn path(entry: &Entry) -> &str {
        match entry {
            Entry::Directory { path, .. } | Entry::File { path, .. } => path,
        }
    }
    entries.sort_by(|a, b| path(a).cmp(path(b)));
    let (file_count, total_bytes) =
        entries
            .iter()
            .fold((0, 0), |(count, total), entry| match entry {
                Entry::File { size, .. } => (count + 1, total + size),
                _ => (count, total),
            });
    let mut payload = Payload {
        root: kind.root().into(),
        root_mode: fs::metadata(root).unwrap().permissions().mode() & 0o7777,
        tool_version: if kind == PayloadKind::DevelopmentGit {
            "2.56.0"
        } else {
            "24.22.0"
        }
        .into(),
        npm_version: (kind == PayloadKind::DevelopmentNode).then(|| "11.19.1".into()),
        entry_count: entries.len(),
        file_count,
        total_bytes,
        sha256: String::new(),
        entries,
    };
    payload.sha256 = tree_digest(&payload).unwrap();
    payload
}

fn sample() -> Sample {
    let temp = tempfile::Builder::new()
        .prefix("disposable-synthetic-signed-tools-")
        .tempdir()
        .unwrap();
    let root = temp.path().canonicalize().unwrap().join("resources");
    fs::create_dir(&root).unwrap();
    fn synthetic_macho(label: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for word in [
            0xfeed_facfu32,
            0x0100_000c,
            0,
            2,
            1,
            24,
            0,
            0,
            0x32,
            24,
            1,
            0x000f_0000,
            0x000f_0000,
            0,
        ] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(label);
        bytes
    }
    let old = synthetic_macho(b"synthetic original NEVER EXECUTED");
    let signed = synthetic_macho(b"synthetic changed bytes NOT A SIGNATURE");
    let license = b"synthetic test license".to_vec();
    let runtime_source=serde_json::to_vec(&json!({"schemaVersion":1,"version":"24.22.0","platform":"darwin","arch":"arm64", "source":{"url":"https://example.invalid/synthetic-never-downloaded","sha256":digest(b"synthetic source archive")}, "binary":{"size":old.len(),"sha256":digest(&old)},"license":{"sha256":digest(&license)}})).unwrap();
    let mut node_old: BTreeMap<String, Vec<u8>> = BTreeMap::from([
        ("bin/node".into(), old.clone()),
        ("bin/npm".into(), b"synthetic npm".to_vec()),
        ("bin/npx".into(), b"synthetic npx".to_vec()),
        (
            "lib/node_modules/npm/bin/npm-cli.js".into(),
            b"synthetic npm JS".to_vec(),
        ),
        ("lib/node_modules/npm/package.json".into(), b"{}".to_vec()),
    ]);
    let node_execs = [
        "bin/node",
        "bin/npm",
        "bin/npx",
        "lib/node_modules/npm/bin/npm-cli.js",
    ];
    let node_source=serde_json::to_vec(&json!({"schemaVersion":1,"nodeVersion":"24.22.0","npmVersion":"11.19.1","platform":"darwin","arch":"arm64","tree":source_tree(&node_old),"executables":node_execs})).unwrap();
    let mut git_old: BTreeMap<String, Vec<u8>> = BTreeMap::from([
        ("bin/git".into(), old.clone()),
        ("libexec/git-core/git".into(), old.clone()),
        ("libexec/git-core/git-remote-https".into(), old.clone()),
        (
            "share/git-core/templates/description".into(),
            b"synthetic template".to_vec(),
        ),
    ]);
    let git_execs = [
        "bin/git",
        "libexec/git-core/git",
        "libexec/git-core/git-remote-https",
    ];
    let git_source=serde_json::to_vec(&json!({"schemaVersion":1,"gitVersion":"2.56.0","platform":"darwin","arch":"arm64","tree":source_tree(&git_old),"executables":git_execs})).unwrap();
    let trusted = TrustedContext::new(TrustInputs {
        profile: include_bytes!("../../../build-profiles/production.json"),
        target: "aarch64-apple-darwin",
        signing_policy: &policy_bytes(),
        node_runtime: &runtime_source,
        development_node: &node_source,
        development_git: &git_source,
    })
    .unwrap();
    let runtime = BTreeMap::from([
        ("bin/node".into(), signed.clone()),
        ("LICENSE".into(), license),
        (
            "runtime.json".into(),
            runtime_metadata_bytes(&serde_json::from_slice(&runtime_source).unwrap()).unwrap(),
        ),
    ]);
    write_tree(&root.join("node-runtime"), &runtime, &["bin/node"]);
    let mut node = node_old.clone();
    node.insert("bin/node".into(), signed.clone());
    node.insert("development-node.json".into(), node_source);
    write_tree(&root.join("development-node"), &node, &node_execs);
    let mut git = git_old.clone();
    for path in git_execs {
        git.insert(path.into(), signed.clone());
    }
    git.insert("development-git.json".into(), git_source);
    write_tree(&root.join("development-git"), &git, &git_execs);
    let runtime_old = BTreeMap::from([("bin/node".into(), old)]);
    let payloads = Payloads {
        node_runtime: capture(
            &root.join("node-runtime"),
            PayloadKind::NodeRuntime,
            &trusted,
            &runtime_old,
        ),
        development_node: capture(
            &root.join("development-node"),
            PayloadKind::DevelopmentNode,
            &trusted,
            &node_old,
        ),
        development_git: capture(
            &root.join("development-git"),
            PayloadKind::DevelopmentGit,
            &trusted,
            &git_old,
        ),
    };
    node_old.clear();
    git_old.clear();
    let manifest = Manifest {
        schema_version: 1,
        digest_rule_version: 1,
        profile: trusted.profile.clone(),
        signing_profile: trusted.policy.signing_profile.clone(),
        signing_policy_sha256: trusted.signing_policy_sha256.clone(),
        expected_team_id: trusted.policy.expected_team_id.clone().unwrap(),
        source_anchors: trusted.source_anchors.clone(),
        payloads,
    };
    Sample {
        _temp: temp,
        root,
        trusted,
        manifest,
    }
}

fn parsed(sample: &Sample) -> Manifest {
    parse(
        &serde_json::to_vec(&sample.manifest).unwrap(),
        &sample.trusted,
    )
    .unwrap()
}

#[test]
fn synthetic_signed_resource_bindings_roundtrip_all_three_trees_without_executing_anything() {
    let s = sample();
    let manifest = parsed(&s);
    verify_resources(&s.root, &manifest).unwrap();
    assert_eq!(
        runtime_node_digest(&manifest).unwrap(),
        digest(&fs::read(s.root.join("node-runtime/bin/node")).unwrap())
    );
    // No signature or transformation proof is claimed by the fixture.
    assert!(!crate::build_profile::info()["releaseReady"]
        .as_bool()
        .unwrap());
}

#[test]
fn signed_resource_schema_profile_signing_and_source_overrides_are_rejected() {
    let s = sample();
    let original = serde_json::to_value(&s.manifest).unwrap();
    for (pointer, value) in [
        ("/schemaVersion", json!(0)),
        ("/digestRuleVersion", json!(99)),
        ("/profile/version", json!("0.0.0")),
        ("/profile/target", json!("x86_64-apple-darwin")),
        ("/profile/minimumSystemVersion", json!("14.0")),
        ("/profile/sha256", json!("0".repeat(64))),
        ("/expectedTeamId", json!("OTHERMAN01")),
        ("/signingPolicySha256", json!("0".repeat(64))),
        ("/sourceAnchors/developmentGit", json!("0".repeat(64))),
        ("/payloads/nodeRuntime/root", json!("/absolute/stage")),
        ("/payloads/developmentNode/toolVersion", json!("99.0.0")),
    ] {
        let mut v = original.clone();
        *v.pointer_mut(pointer).unwrap() = value;
        assert!(
            parse(&serde_json::to_vec(&v).unwrap(), &s.trusted).is_err(),
            "{pointer}"
        );
    }
    let mut extra = original.clone();
    extra["payloads"]["unexpected"] = json!({});
    assert!(parse(&serde_json::to_vec(&extra).unwrap(), &s.trusted).is_err());
    let mut extra = original;
    extra["payloads"]["nodeRuntime"]["entries"][0]["unknown"] = json!(true);
    assert!(parse(&serde_json::to_vec(&extra).unwrap(), &s.trusted).is_err());
    assert!(parse(&vec![b' '; MAX_MANIFEST_BYTES + 1], &s.trusted).is_err());
    let duplicate = serde_json::to_string(&s.manifest).unwrap().replacen(
        "\"schemaVersion\":1",
        "\"schemaVersion\":1,\"schemaVersion\":1",
        1,
    );
    assert!(parse(duplicate.as_bytes(), &s.trusted).is_err());
}

#[test]
fn signed_resource_path_type_budget_and_collision_variants_are_rejected() {
    for path in [
        "../escape",
        "/absolute",
        "bin\\node",
        "bin/../node",
        "BIN/node",
        "bin/nodé",
        "bin/node\u{301}",
        "bin/node/child",
    ] {
        let s = sample();
        let mut m = s.manifest.clone();
        m.payloads.node_runtime.entries.push(Entry::File {
            path: path.into(),
            mode: 0o644,
            size: 0,
            sha256: digest(b""),
            signature: None,
            source: None,
        });
        m.payloads.node_runtime.entry_count += 1;
        m.payloads.node_runtime.file_count += 1;
        m.payloads.node_runtime.entries.sort_by_key(|e| match e {
            Entry::File { path, .. } | Entry::Directory { path, .. } => path.clone(),
        });
        m.payloads.node_runtime.sha256 = tree_digest(&m.payloads.node_runtime).unwrap();
        assert!(
            parse(&serde_json::to_vec(&m).unwrap(), &s.trusted).is_err(),
            "{path}"
        );
    }
    let s = sample();
    let mut v = serde_json::to_value(&s.manifest).unwrap();
    v["payloads"]["nodeRuntime"]["entries"][0]["type"] = json!("symlink");
    assert!(parse(&serde_json::to_vec(&v).unwrap(), &s.trusted).is_err());
    let mut m = s.manifest.clone();
    m.payloads.node_runtime.total_bytes = u64::MAX;
    assert!(parse(&serde_json::to_vec(&m).unwrap(), &s.trusted).is_err());
    let mut m = s.manifest.clone();
    m.payloads.node_runtime.entries.push(Entry::Directory {
        path: "BIN".into(),
        mode: 0o755,
    });
    m.payloads
        .node_runtime
        .entries
        .sort_by_key(|entry| match entry {
            Entry::Directory { path, .. } | Entry::File { path, .. } => path.clone(),
        });
    m.payloads.node_runtime.entry_count += 1;
    m.payloads.node_runtime.sha256 = tree_digest(&m.payloads.node_runtime).unwrap();
    let error = parse(&serde_json::to_vec(&m).unwrap(), &s.trusted).unwrap_err();
    assert!(error.contains("case collisions"));
    let mut m = s.manifest.clone();
    m.payloads
        .node_runtime
        .entries
        .push(m.payloads.node_runtime.entries[0].clone());
    m.payloads.node_runtime.entry_count += 1;
    m.payloads.node_runtime.sha256 = tree_digest(&m.payloads.node_runtime).unwrap();
    assert!(parse(&serde_json::to_vec(&m).unwrap(), &s.trusted).is_err());
}

#[test]
fn signed_resource_unsigned_changes_cannot_be_hidden_by_new_postsign_hashes() {
    let s = sample();
    let mut m = s.manifest.clone();
    for entry in &mut m.payloads.development_node.entries {
        if let Entry::File { path, sha256, .. } = entry {
            if path == "bin/npm" {
                *sha256 = digest(b"replacement");
            }
        }
    }
    m.payloads.development_node.sha256 = tree_digest(&m.payloads.development_node).unwrap();
    assert!(parse(&serde_json::to_vec(&m).unwrap(), &s.trusted).is_err());
    let mut m = s.manifest.clone();
    for entry in &mut m.payloads.development_git.entries {
        if let Entry::File {
            source: Some(old), ..
        } = entry
        {
            old.sha256 = "0".repeat(64);
            break;
        }
    }
    m.payloads.development_git.sha256 = tree_digest(&m.payloads.development_git).unwrap();
    assert!(parse(&serde_json::to_vec(&m).unwrap(), &s.trusted).is_err());
}

#[test]
fn signed_resource_disk_mutations_missing_metadata_and_extra_files_fail_closed() {
    for mutation in [
        "bytes",
        "metadata",
        "extra",
        "missing",
        "empty-directory",
        "mode",
    ] {
        let s = sample();
        let manifest = parsed(&s);
        match mutation {
            "bytes" => fs::write(s.root.join("node-runtime/bin/node"), b"corrupted").unwrap(),
            "metadata" => fs::write(
                s.root.join("development-node/development-node.json"),
                b"{\"tree\":{\"sha256\":\"replacement\"}}",
            )
            .unwrap(),
            "extra" => fs::write(s.root.join("development-git/extra.json"), b"ignored").unwrap(),
            "missing" => fs::remove_file(s.root.join("development-git/bin/git")).unwrap(),
            "empty-directory" => {
                fs::create_dir(s.root.join("development-node/extra-empty")).unwrap()
            }
            "mode" => fs::set_permissions(
                s.root.join("node-runtime/bin/node"),
                fs::Permissions::from_mode(0o777),
            )
            .unwrap(),
            _ => unreachable!(),
        }
        assert!(verify_resources(&s.root, &manifest).is_err(), "{mutation}");
    }
}

#[test]
fn signed_resource_symlinks_hardlinks_and_parent_links_are_rejected() {
    for mode in ["symlink", "hardlink", "root-parent"] {
        let s = sample();
        let manifest = parsed(&s);
        let node = s.root.join("node-runtime/bin/node");
        if mode == "root-parent" {
            let alias = s._temp.path().canonicalize().unwrap().join("alias");
            symlink(&s.root, &alias).unwrap();
            assert!(verify_resources(&alias, &manifest).is_err());
            continue;
        }
        let held = s.root.join("held-node");
        fs::rename(&node, &held).unwrap();
        if mode == "symlink" {
            symlink(&held, &node).unwrap();
        } else {
            fs::hard_link(&held, &node).unwrap();
        }
        assert!(verify_resources(&s.root, &manifest).is_err(), "{mode}");
    }
}

#[test]
#[cfg(not(feature = "production-resource-anchors"))]
fn signed_resource_missing_compiled_anchors_never_read_an_explicit_resource_path() {
    let error = verify_installed_resources(Path::new(
        "/nonexistent/disposable-production-no-resource-read",
    ))
    .unwrap_err();
    assert_eq!(error.code, crate::build_profile::PRODUCTION_RESOURCE_ERROR);
    assert_eq!(error.details["operationOutcome"], "not_started");
}

#[test]
#[ignore = "explicit disposable copied-tool fixture; digest checks only, no signing proof"]
fn export_disposable_signed_resource_binding_fixture() {
    let input = std::env::var_os("PANDA_DRAWER_TEST_SOURCE_RESOURCES")
        .expect("explicit read-only prepared resource input required");
    let input = PathBuf::from(input).canonicalize().unwrap();
    let temp = tempfile::Builder::new()
        .prefix("disposable-signed-resource-bindings-")
        .tempdir()
        .unwrap();
    let resources = temp.path().canonicalize().unwrap().join("resources");
    fs::create_dir(&resources).unwrap();
    fn copy_tree(from: &Path, to: &Path) {
        let info = fs::symlink_metadata(from).unwrap();
        assert!(info.is_dir() && !info.file_type().is_symlink());
        fs::create_dir(to).unwrap();
        for item in fs::read_dir(from).unwrap() {
            let path = item.unwrap().path();
            let dest = to.join(path.file_name().unwrap());
            let info = fs::symlink_metadata(&path).unwrap();
            if info.is_dir() && !info.file_type().is_symlink() {
                copy_tree(&path, &dest);
            } else {
                assert!(info.is_file() && !info.file_type().is_symlink());
                fs::copy(&path, &dest).unwrap();
                fs::set_permissions(&dest, info.permissions()).unwrap();
            }
        }
        fs::set_permissions(to, info.permissions()).unwrap();
    }
    for name in ["node-runtime", "development-node", "development-git"] {
        copy_tree(&input.join(name), &resources.join(name));
    }
    let policy = policy_bytes();
    let trusted = TrustedContext::new(TrustInputs {
        profile: include_bytes!("../../../build-profiles/production.json"),
        target: "aarch64-apple-darwin",
        signing_policy: &policy,
        node_runtime: include_bytes!("../../../scripts/node-runtime-release.json"),
        development_node: include_bytes!("../../../scripts/development-node-release.json"),
        development_git: include_bytes!("../../../scripts/development-git-release.json"),
    })
    .unwrap();
    let empty = BTreeMap::new();
    let manifest = Manifest {
        schema_version: 1,
        digest_rule_version: 1,
        profile: trusted.profile.clone(),
        signing_profile: trusted.policy.signing_profile.clone(),
        signing_policy_sha256: trusted.signing_policy_sha256.clone(),
        expected_team_id: trusted.policy.expected_team_id.clone().unwrap(),
        source_anchors: trusted.source_anchors.clone(),
        payloads: Payloads {
            node_runtime: capture(
                &resources.join("node-runtime"),
                PayloadKind::NodeRuntime,
                &trusted,
                &empty,
            ),
            development_node: capture(
                &resources.join("development-node"),
                PayloadKind::DevelopmentNode,
                &trusted,
                &empty,
            ),
            development_git: capture(
                &resources.join("development-git"),
                PayloadKind::DevelopmentGit,
                &trusted,
                &empty,
            ),
        },
    };
    let bytes = serde_json::to_vec_pretty(&manifest).unwrap();
    let manifest = parse(&bytes, &trusted).unwrap();
    verify_resources(&resources, &manifest).unwrap();
    fs::write(
        resources.join(".disposable-signed-resource-protocol-fixture"),
        b"digest bindings only; no signature or transformation evidence",
    )
    .unwrap();
    let base = temp.keep().canonicalize().unwrap();
    fs::write(base.join("synthetic-signing-policy.json"), &policy).unwrap();
    fs::write(base.join("compiled-tool-manifest.json"), &bytes).unwrap();
    println!("DISPOSABLE_SIGNED_RESOURCE_FIXTURE={}", base.display());
}

#[test]
#[cfg(feature = "production-resource-anchors")]
#[ignore = "requires explicitly generated disposable binding fixture, never a real installation"]
fn compiled_signed_resource_fixture_verifies_and_rejects_disk_changes_without_runtime_admission() {
    let root = PathBuf::from(std::env::var_os("PANDA_DRAWER_TEST_COMPILED_RESOURCE_ROOT").unwrap());
    assert_eq!(
        fs::read(root.join(".disposable-signed-resource-protocol-fixture")).unwrap(),
        b"digest bindings only; no signature or transformation evidence"
    );
    let value = verify_installed_resources(&root).unwrap();
    assert_eq!(value["digestBindingsVerified"], true);
    assert_eq!(value["signatureEvidenceVerified"], false);
    assert_eq!(value["sourceTransformationVerified"], false);
    assert_eq!(value["releaseReady"], false);
    assert_eq!(
        crate::build_profile::info()["resourceAnchors"],
        "compiled-signed-tools-v1"
    );
    let extra = root.join("development-node/disposable-attacker.json");
    assert!(!extra.exists());
    fs::write(&extra, b"{\"sha256\":\"replacement\"}").unwrap();
    let rejected = verify_installed_resources(&root).is_err();
    fs::remove_file(&extra).unwrap();
    assert!(rejected);
    verify_installed_resources(&root).unwrap();
    let error = crate::platform_support::require_runtime().unwrap_err();
    assert_eq!(error.code, crate::build_profile::PRODUCTION_PROOF_ERROR);
    assert!(crate::RuntimePaths::initialize().is_err());
}

#[test]
fn signed_resource_macho_target_and_minimum_os_are_checked_even_with_fresh_hashes() {
    for mutation in ["intel", "minimum-16", "missing-minimum", "invalid-command"] {
        let s = sample();
        let path = s.root.join("node-runtime/bin/node");
        let mut bytes = fs::read(&path).unwrap();
        let (offset, value): (usize, u32) = match mutation {
            "intel" => (4, 0x0100_0007),
            "minimum-16" => (44, 0x0010_0000),
            "missing-minimum" => (32, 0x31),
            "invalid-command" => (36, 1),
            _ => unreachable!(),
        };
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        fs::write(&path, &bytes).unwrap();
        let mut m = s.manifest.clone();
        for entry in &mut m.payloads.node_runtime.entries {
            if let Entry::File { path, sha256, .. } = entry {
                if path == "bin/node" {
                    *sha256 = digest(&bytes);
                }
            }
        }
        m.payloads.node_runtime.sha256 = tree_digest(&m.payloads.node_runtime).unwrap();
        let m = parse(&serde_json::to_vec(&m).unwrap(), &s.trusted).unwrap();
        assert!(verify_resources(&s.root, &m).is_err(), "{mutation}");
    }
}

#[test]
fn signed_resource_policy_cannot_infer_team_or_permissions_and_signature_fields_bind_exactly() {
    let inputs = |policy: &[u8]| {
        TrustedContext::new(TrustInputs {
            profile: include_bytes!("../../../build-profiles/production.json"),
            target: "aarch64-apple-darwin",
            signing_policy: policy,
            node_runtime: include_bytes!("../../../scripts/node-runtime-release.json"),
            development_node: include_bytes!("../../../scripts/development-node-release.json"),
            development_git: include_bytes!("../../../scripts/development-git-release.json"),
        })
    };
    let missing_policy = serde_json::to_vec(&json!({"schemaVersion":1,"signingProfile":"macos-developer-id-hardened-runtime-v1","expectedTeamId":null,"nodeEntitlementsSha256":null,"gitEntitlementsSha256":null})).unwrap();
    assert!(inputs(&missing_policy).is_err());
    let mut policy: serde_json::Value = serde_json::from_slice(&policy_bytes()).unwrap();
    policy["expectedTeamId"] = json!("short");
    assert!(inputs(&serde_json::to_vec(&policy).unwrap()).is_err());
    for mutation in ["team", "permissions", "identifier", "runtime"] {
        let s = sample();
        let mut m = s.manifest.clone();
        for entry in &mut m.payloads.node_runtime.entries {
            if let Entry::File {
                signature: Some(sig),
                ..
            } = entry
            {
                match mutation {
                    "team" => sig.team_id = "OTHERTEAM1".into(),
                    "permissions" => sig.entitlements_sha256 = "0".repeat(64),
                    "identifier" => sig.identifier = "arbitrary.signer.identifier".into(),
                    "runtime" => sig.hardened_runtime = false,
                    _ => unreachable!(),
                }
            }
        }
        m.payloads.node_runtime.sha256 = tree_digest(&m.payloads.node_runtime).unwrap();
        assert!(
            parse(&serde_json::to_vec(&m).unwrap(), &s.trusted).is_err(),
            "{mutation}"
        );
    }
}

#[test]
fn signed_resource_digest_rule_has_a_frozen_cross_language_vector() {
    let payload = Payload {
        root: "node-runtime".into(),
        root_mode: 493,
        tool_version: "vector".into(),
        npm_version: None,
        entry_count: 2,
        file_count: 1,
        total_bytes: 4,
        sha256: String::new(),
        entries: vec![
            Entry::Directory {
                path: "bin".into(),
                mode: 493,
            },
            Entry::File {
                path: "bin/node".into(),
                mode: 493,
                size: 4,
                sha256: "0".repeat(64),
                signature: None,
                source: None,
            },
        ],
    };
    // Independently calculated with Python json.dumps(separators=(',', ':')).
    assert_eq!(
        tree_digest(&payload).unwrap(),
        "fddefd6c6ee7747f75b1d095cddb761e30d0c759e6516ad82f48f7bead2b38f7"
    );
}

#[test]
fn signed_resource_counts_depth_and_file_size_budgets_reject_before_disk_io() {
    let s = sample();
    let mut m = s.manifest.clone();
    m.payloads.node_runtime.entries = vec![
        Entry::Directory {
            path: "dir".into(),
            mode: 493
        };
        30_001
    ];
    m.payloads.node_runtime.entry_count = 30_001;
    assert!(parse(&serde_json::to_vec(&m).unwrap(), &s.trusted).is_err());
    let mut m = s.manifest.clone();
    m.payloads.node_runtime.entries.push(Entry::Directory {
        path: vec!["d"; 65].join("/"),
        mode: 493,
    });
    m.payloads.node_runtime.entry_count += 1;
    m.payloads.node_runtime.sha256 = tree_digest(&m.payloads.node_runtime).unwrap();
    assert!(parse(&serde_json::to_vec(&m).unwrap(), &s.trusted).is_err());
    let mut m = s.manifest.clone();
    for entry in &mut m.payloads.node_runtime.entries {
        if let Entry::File { size, .. } = entry {
            *size = 180 * 1024 * 1024 + 1;
            break;
        }
    }
    m.payloads.node_runtime.sha256 = tree_digest(&m.payloads.node_runtime).unwrap();
    assert!(parse(&serde_json::to_vec(&m).unwrap(), &s.trusted).is_err());
}

#[test]
fn signed_resource_compile_input_paths_cannot_inject_cargo_directives() {
    for path in [
        "relative/stage",
        "/tmp/../stage",
        "/tmp/disposable\ncargo:rustc-env=OVERRIDE=value",
        "/tmp/disposable\rstage",
        "/tmp/disposable\\stage",
        "/tmp/disposable\0stage",
    ] {
        assert!(validate_input_path(Path::new(path)).is_err(), "{path:?}");
    }
    // User-selected installation folders may contain ordinary Unicode/spaces.
    assert!(validate_input_path(Path::new("/Users/example/普通目录/staged resources")).is_ok());
}

#[test]
fn signed_resource_manifest_input_is_bounded_ordinary_nonlinked_and_not_executable() {
    let s = sample();
    let root = s._temp.path().canonicalize().unwrap();
    let path = root.join("manifest.json");
    let bytes = serde_json::to_vec(&s.manifest).unwrap();
    fs::write(&path, &bytes).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(read_manifest(&path).unwrap(), bytes);
    let alias = root.join("alias.json");
    symlink(&path, &alias).unwrap();
    assert!(read_manifest(&alias).is_err());
    fs::remove_file(alias).unwrap();
    let hard = root.join("hard.json");
    fs::hard_link(&path, &hard).unwrap();
    assert!(read_manifest(&path).is_err());
    fs::remove_file(hard).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(read_manifest(&path).is_err());
    let oversized = root.join("oversized.json");
    let file = fs::File::create(&oversized).unwrap();
    file.set_len(MAX_MANIFEST_BYTES as u64 + 1).unwrap();
    assert!(read_manifest(&oversized).is_err());
}

#[test]
fn signed_resource_runtime_metadata_cannot_replace_the_fixed_original_projection() {
    let s = sample();
    let mut m = s.manifest.clone();
    for entry in &mut m.payloads.node_runtime.entries {
        if let Entry::File { path, sha256, .. } = entry {
            if path == "runtime.json" {
                *sha256 = digest(b"replacement hash metadata");
            }
        }
    }
    m.payloads.node_runtime.sha256 = tree_digest(&m.payloads.node_runtime).unwrap();
    assert!(parse(&serde_json::to_vec(&m).unwrap(), &s.trusted)
        .unwrap_err()
        .contains("source projection"));
    let source: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../scripts/node-runtime-release.json"))
            .unwrap();
    let bytes = runtime_metadata_bytes(&source).unwrap();
    // Independently frozen against the existing Node preparer's 556-byte output.
    assert_eq!(bytes.len(), 556);
    assert_eq!(
        digest(&bytes),
        "8c02210436c122effe5bad2be3503d82b02c1593decb345e3ae25bdade1c79f3"
    );
}
