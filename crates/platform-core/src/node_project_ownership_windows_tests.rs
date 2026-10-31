//! Actual private-directory and writer-inventory checks on fresh disposable roots.
use super::*;
use crate::durable_writer::{require_all_clear, WriterOwner};
use platform_native::windows::files as native;
use std::process::Command;

fn fixture() -> (tempfile::TempDir, NodeProjectOwnership, String) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-node-ownership-private-中文 space-")
        .tempdir()
        .unwrap();
    let data = temporary.path().canonicalize().unwrap();
    let ownership = NodeProjectOwnership::new(data.clone(), data.join("unused-workspace"));
    (temporary, ownership, Uuid::new_v4().to_string())
}

// Inspect actual descriptor bytes without adding a Win32 dependency to core.
// Every supplied path is inside this test's newly-created disposable directory.
fn security(paths: &[&Path]) -> Vec<String> {
    let paths: Vec<_> = paths
        .iter()
        .map(|path| {
            let path = path.to_str().unwrap();
            // Windows PowerShell's provider accepts the ordinary representation
            // of these short, local disposable paths. Native checks retain the
            // original canonical path and full FileId throughout the test.
            path.strip_prefix(r"\\?\").unwrap_or(path)
        })
        .collect();
    let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
    let output = Command::new(system.join("System32/WindowsPowerShell/v1.0/powershell.exe"))
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "$ErrorActionPreference='Stop'; foreach ($p in (ConvertFrom-Json $env:PANDA_DISPOSABLE_NODE_OWNER_ACL_PATHS)) { [Convert]::ToBase64String((Get-Acl -LiteralPath $p).GetSecurityDescriptorBinaryForm()) }",
        ])
        .env(
            "PANDA_DISPOSABLE_NODE_OWNER_ACL_PATHS",
            serde_json::to_string(&paths).unwrap(),
        )
        // Let Windows PowerShell initialize its own built-in module paths,
        // rather than inheriting PowerShell 7's incompatible security module.
        .env_remove("PSModulePath")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Disposable descriptor inspection failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let values: Vec<_> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(values.len(), paths.len());
    values
}

#[test]
fn new_root_and_leaf_are_private_and_actual_writer_inventory_closes() {
    let (_temporary, ownership, id) = fixture();
    let parent = native::open_no_reparse(&ownership.data).unwrap();
    let parent_security = security(&[&ownership.data]);
    ownership.initialize(&id).unwrap();
    for path in [
        ownership.root(),
        ownership.directory(&id).unwrap(),
        ownership.path(&id).unwrap(),
    ] {
        native::verify_private(&native::open_no_reparse(&path).unwrap().file).unwrap();
    }
    require_all_clear(&ownership.data).unwrap();
    let owner = WriterOwner::node(&ownership.data, &id).unwrap();
    let mut writer = owner.begin("disposable node private-root check").unwrap();
    assert_eq!(
        require_all_clear(&ownership.data).unwrap_err().code,
        "PROJECT_WRITER_UNCONFIRMED"
    );
    // Merely updating ownership must not clear a real, unconfirmed writer.
    ownership.initialize(&id).unwrap();
    assert_eq!(
        require_all_clear(&ownership.data).unwrap_err().code,
        "PROJECT_WRITER_UNCONFIRMED"
    );
    writer.finish().unwrap();
    require_all_clear(&ownership.data).unwrap();
    assert!(parent
        .identity
        .same_object(&native::open_no_reparse(&ownership.data).unwrap().identity));
    assert_eq!(security(&[&ownership.data]), parent_security);
}

#[test]
fn existing_root_and_leaf_repair_keep_full_ids_parent_and_child_security_and_bytes() {
    let (_temporary, ownership, id) = fixture();
    let root = ownership.root();
    let leaf = ownership.directory(&id).unwrap();
    fs::create_dir(&root).unwrap();
    fs::create_dir(&leaf).unwrap();
    let child = leaf.join("retained-child");
    fs::create_dir(&child).unwrap();
    let payload = child.join("retained-payload");
    fs::write(&payload, b"preserve original disposable child bytes").unwrap();
    let sibling = ownership.data.join("unrelated-sibling");
    fs::create_dir(&sibling).unwrap();
    let parent = native::open_no_reparse(&ownership.data).unwrap();
    let root_before = native::open_no_reparse(&root).unwrap();
    let leaf_before = native::open_no_reparse(&leaf).unwrap();
    let child_before = native::open_no_reparse(&child).unwrap();
    let payload_before = native::open_no_reparse(&payload).unwrap();
    let sibling_before = native::open_no_reparse(&sibling).unwrap();
    let retained_security = security(&[&ownership.data, &child, &payload, &sibling]);
    assert!(native::verify_private(&root_before.file).is_err());
    assert_eq!(
        require_all_clear(&ownership.data).unwrap_err().details["retainedPath"],
        root.to_string_lossy().as_ref()
    );
    ownership.initialize(&id).unwrap();
    ownership.initialize(&id).unwrap();
    for (path, before) in [(&root, &root_before), (&leaf, &leaf_before)] {
        let actual = native::open_no_reparse(path).unwrap();
        assert!(before.identity.same_object(&actual.identity));
        native::verify_private(&actual.file).unwrap();
    }
    for (path, before) in [
        (&ownership.data, &parent),
        (&child, &child_before),
        (&payload, &payload_before),
        (&sibling, &sibling_before),
    ] {
        assert!(before
            .identity
            .same_object(&native::open_no_reparse(path).unwrap().identity));
    }
    assert_eq!(
        security(&[&ownership.data, &child, &payload, &sibling]),
        retained_security
    );
    assert_eq!(
        fs::read(payload).unwrap(),
        b"preserve original disposable child bytes"
    );
    require_all_clear(&ownership.data).unwrap();
}
