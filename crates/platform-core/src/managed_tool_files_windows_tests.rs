// Actual temporary Windows filesystem checks; synthetic bytes are never run
// and do not prove a bundled executable or release anchor is valid.
use super::*;
use std::io::Write;

#[test]
fn actual_read_only_tool_access_accepts_unicode_paths_and_manifest_separators() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-managed-tool-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let chain = directory_chain(&root).unwrap();
    assert!(chain
        .iter()
        .all(|(path, facts)| path.is_absolute() && facts.is_dir()));
    let path = root.join("synthetic-tool.dat");
    fs::write(&path, b"synthetic resource, never executed").unwrap();
    let before = Metadata::capture(&path).unwrap();
    let mut file = open_ordinary(&path, &before, false).unwrap();
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"synthetic resource, never executed");
    assert!(unchanged(&before, &Metadata::from_file(&file).unwrap()));
    let child = Path::new("lib").join("nested/package.dat");
    let name = native_tools::relative_name(&child).unwrap();
    assert_eq!(name, "lib/nested/package.dat");
    assert!(relative_valid(&name));
}

#[test]
fn a_hardlinked_tool_file_is_rejected_without_changing_its_bytes() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-tool-hardlink-")
        .tempdir()
        .unwrap();
    let original = temporary.path().join("retained-original.dat");
    let link = temporary.path().join("disposable-link.dat");
    fs::write(&original, b"retained disposable bytes").unwrap();
    fs::hard_link(&original, &link).unwrap();
    let before = Metadata::capture(&link).unwrap();
    assert_eq!(before.links(), 2);
    assert!(open_ordinary(&link, &before, false).is_err());
    assert_eq!(fs::read(&original).unwrap(), b"retained disposable bytes");
}

#[test]
fn a_tool_directory_junction_is_rejected_at_the_leaf_and_in_ancestors() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-tool-junction-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    let foreign = root.join("disposable-target");
    fs::create_dir_all(foreign.join("child")).unwrap();
    let link = root.join("disposable-junction");
    let result = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&foreign)
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(directory_chain(&link).is_err());
    assert!(directory_chain(&link.join("child")).is_err());
    fs::remove_dir(link).unwrap();
    assert!(foreign.join("child").is_dir());
}

#[test]
fn a_file_replaced_after_selection_does_not_match_the_captured_tool_identity() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-tool-replacement-")
        .tempdir()
        .unwrap();
    let path = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("selected.dat");
    File::create(&path)
        .unwrap()
        .write_all(b"original disposable bytes")
        .unwrap();
    let before = Metadata::capture(&path).unwrap();
    fs::rename(&path, path.with_file_name("retained-original.dat")).unwrap();
    fs::write(&path, b"replacement disposable bytes").unwrap();
    assert!(open_ordinary(&path, &before, false).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"replacement disposable bytes");
}
