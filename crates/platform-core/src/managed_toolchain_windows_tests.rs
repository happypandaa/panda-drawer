//! Synthetic payload bytes exercise the real verifier and are never executed.
use super::*;
use serde_json::json;
use std::collections::BTreeMap;

fn sample() -> (tempfile::TempDir, PathBuf, String) {
    let temp = tempfile::Builder::new()
        .prefix("disposable-windows-managed-node-中文 space-")
        .tempdir()
        .unwrap();
    let root = temp.path().canonicalize().unwrap().join("development-node");
    let files = BTreeMap::from([
        ("LICENSE", "synthetic license"),
        ("node.exe", "synthetic node, never executed"),
        (
            "node_modules/npm/bin/npm-cli.js",
            "synthetic npm, never executed",
        ),
        (
            "node_modules/npm/bin/npx-cli.js",
            "synthetic npx, never executed",
        ),
        ("node_modules/npm/package.json", "{\"version\":\"11.19.1\"}"),
    ]);
    let mut hash = Sha256::new();
    let mut total = 0u64;
    for (name, bytes) in &files {
        let file = root.join(name);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, bytes).unwrap();
        hash.update(format!(
            "{name}\0{}\0{:x}\n",
            bytes.len(),
            Sha256::digest(bytes)
        ));
        total += bytes.len() as u64;
    }
    let source = json!({
        "schemaVersion":1,"nodeVersion":"24.22.0","npmVersion":"11.19.1",
        "platform":"win32","arch":"x64","archiveName":"synthetic.zip",
        "archiveSha256":"a".repeat(64),"executables":["node.exe"],
        "tree":{"rule":"sorted-utf8-path-nul-size-nul-sha256-lf-v1",
            "fileCount":files.len(),"totalBytes":total,"sha256":format!("{:x}",hash.finalize())}
    })
    .to_string();
    fs::write(root.join(METADATA_FILE), &source).unwrap();
    (temp, root, source)
}

#[test]
fn fixed_windows_release_matches_the_native_layout_and_architecture() {
    let source = release(RELEASE).unwrap();
    assert_eq!(source.platform, "win32");
    assert_eq!(source.arch, "x64");
    assert_eq!(source.executables, ["node.exe"]);
    assert_eq!(source.tree.file_count, 1928);
    let mac = include_str!("../../../scripts/development-node-release.json");
    assert_eq!(release(mac).err().unwrap().code, "MANAGED_NODE_INVALID");
}

#[test]
fn actual_windows_payload_verifier_relocates_and_keeps_read_only_files() {
    let (_temp, root, source) = sample();
    let selected = verify_sync(root.clone(), &source).unwrap();
    assert_eq!(selected.node, root.join("node.exe"));
    assert_eq!(selected.npm, root.join("node_modules/npm/bin/npm-cli.js"));
    let moved = root.with_file_name("moved 中文 space");
    fs::rename(&root, &moved).unwrap();
    let file = moved.join("node.exe");
    let original_permissions = fs::metadata(&file).unwrap().permissions();
    let mut permissions = original_permissions.clone();
    permissions.set_readonly(true);
    fs::set_permissions(&file, permissions).unwrap();
    verify_sync(moved.clone(), &source).unwrap();
    // Restore only the disposable fixture's original writable attribute.
    fs::set_permissions(&file, original_permissions).unwrap();
    assert_eq!(
        fs::read_to_string(file).unwrap(),
        "synthetic node, never executed"
    );
}

#[test]
fn changed_missing_and_additional_payload_files_are_rejected_without_repair() {
    for change in ["changed", "missing", "additional", "metadata"] {
        let (_temp, root, source) = sample();
        let npm = root.join("node_modules/npm/bin/npm-cli.js");
        match change {
            "changed" => fs::write(&npm, "retained changed bytes").unwrap(),
            "missing" => fs::remove_file(&npm).unwrap(),
            "additional" => fs::write(root.join("caller-file"), "retain").unwrap(),
            "metadata" => fs::write(root.join(METADATA_FILE), "{}").unwrap(),
            _ => unreachable!(),
        }
        assert!(
            verify_sync(root.clone(), &source).is_err(),
            "accepted {change}"
        );
        if change == "changed" {
            assert_eq!(fs::read_to_string(&npm).unwrap(), "retained changed bytes");
        }
        if change == "additional" {
            assert_eq!(
                fs::read_to_string(root.join("caller-file")).unwrap(),
                "retain"
            );
        }
    }
}
