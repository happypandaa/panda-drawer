use super::*;
use platform_native::windows::{files as native, removal};
use std::{fs, io::Read};

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::Builder::new()
        .prefix("panda-disposable-workspace-copy-中文 space-")
        .tempdir()
        .unwrap();
    let root = temp.path().canonicalize().unwrap();
    (temp, root)
}

#[test]
fn only_actual_local_internal_ntfs_information_admits_migration() {
    require_supported_filesystem(&serde_json::json!({"type":"NTFS","local":true,"internal":true}))
        .unwrap();
    for info in [
        Value::Null,
        serde_json::json!({"type":"apfs","local":true,"internal":true}),
        serde_json::json!({"type":"NTFS","local":false,"internal":true}),
        serde_json::json!({"type":"NTFS","local":true,"internal":false}),
        serde_json::json!({"type":"NTFS","local":true,"internal":null}),
    ] {
        assert!(require_supported_filesystem(&info).is_err());
    }
}

#[test]
fn real_long_unicode_readonly_and_hardlinked_source_copy_matches_without_touching_originals() {
    let (_temp, root) = fixture();
    let source = root.join("原工作区 source");
    native::create_private_directory(&source).unwrap();
    let mut deep = source.clone();
    for _ in 0..7 {
        deep = deep.join("中文 文件夹 forty space");
        fs::create_dir(&deep).unwrap();
    }
    let file = deep.join("只读对象 git");
    fs::write(&file, b"disposable source bytes").unwrap();
    let mut permissions = fs::metadata(&file).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&file, permissions).unwrap();
    fs::hard_link(&file, source.join("source hard link")).unwrap();
    let before = native::open_no_reparse(&file).unwrap().identity;
    fs::create_dir(source.join("excluded")).unwrap();
    fs::write(source.join("excluded/retained"), b"excluded bytes").unwrap();
    let exclusions = BTreeSet::from([PathBuf::from("excluded")]);
    let expected = collect(&source, &exclusions).unwrap();
    let target = root.join("新工作区 target");
    copy_entries(&source, &target, &expected).unwrap();
    assert_eq!(collect(&source, &exclusions).unwrap(), expected);
    assert_eq!(collect(&target, &exclusions).unwrap(), expected);
    assert!(!target.join("excluded").exists());
    assert!(before.same_object(&native::open_no_reparse(&file).unwrap().identity));
    assert_eq!(fs::read(file).unwrap(), b"disposable source bytes");
    assert_eq!(
        native::open_no_reparse(&target.join("source hard link"))
            .unwrap()
            .identity
            .links,
        1
    );
    assert!(copy_entries(&source, &target, &expected).is_err());
}

#[test]
fn junction_copy_preserves_link_itself_without_copying_or_entering_its_external_target() {
    let (_temp, root) = fixture();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    let external = root.join("external original");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("retained"), b"outside author bytes").unwrap();
    let link = source.join("junction");
    let status = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&external)
        .output()
        .unwrap();
    assert!(status.status.success());
    let original = native::open_no_reparse(&external).unwrap().identity;
    let entries = collect(&source, &BTreeSet::new()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].kind, "symlink");
    let target = root.join("target");
    copy_entries(&source, &target, &entries).unwrap();
    assert_eq!(collect(&target, &BTreeSet::new()).unwrap(), entries);
    assert_eq!(
        fs::read_link(target.join("junction")).unwrap(),
        fs::read_link(&link).unwrap()
    );
    assert_eq!(
        fs::read(external.join("retained")).unwrap(),
        b"outside author bytes"
    );
    assert!(original.same_object(&native::open_no_reparse(&external).unwrap().identity));
    for path in [link, target.join("junction")] {
        let id = removal::inspect(&path).unwrap().identity;
        removal::remove(&path, &id).unwrap();
    }
}

#[test]
fn copy_rejects_changed_source_and_traversal_and_never_overwrites_the_target() {
    let (_temp, root) = fixture();
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("source.txt"), b"original").unwrap();
    let entries = collect(&source, &BTreeSet::new()).unwrap();
    fs::write(source.join("source.txt"), b"changed!").unwrap();
    let target = root.join("candidate");
    assert!(copy_entries(&source, &target, &entries).is_err());
    assert_eq!(fs::read(source.join("source.txt")).unwrap(), b"changed!");
    let mut invalid = entries;
    invalid[0].path = "../outside".into();
    assert!(copy_entries(&source, &root.join("candidate2"), &invalid).is_err());
    assert!(!root.join("outside").exists());
}

#[test]
fn phase_log_is_a_real_private_flushed_append_and_unsynced_query_preserves_source() {
    let (_temp, root) = fixture();
    require_unsynced(&root).unwrap();
    let path = root.join("phase.log");
    append_phase_log(&path, "inventory").unwrap();
    append_phase_log(&path, "copying").unwrap();
    let mut file = native::open_private_file(&path, false, false).unwrap();
    let mut bytes = String::new();
    file.read_to_string(&mut bytes).unwrap();
    assert_eq!(bytes, "inventory\ncopying\n");
    native::verify_private(&file).unwrap();
    let info = filesystem(&root).unwrap();
    assert_eq!(info["type"], "NTFS");
    assert!(info["availableBytes"].as_u64().is_some());
}
