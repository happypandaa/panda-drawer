use super::*;
use std::os::windows::fs::OpenOptionsExt;

fn temporary() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("panda-disposable-owned-removal-")
        .tempdir()
        .unwrap()
}

#[test]
fn real_owned_tree_with_unicode_long_paths_and_readonly_files_is_absent() {
    let tmp = temporary();
    let root = tmp.path().canonicalize().unwrap();
    let selected = root.join("selected");
    fs::create_dir(&selected).unwrap();
    let mut deep = selected.clone();
    for index in 0..14 {
        deep.push(format!("中文 space directory {index}"));
    }
    fs::create_dir_all(&deep).unwrap();
    let file = deep.join("只读.txt");
    fs::write(&file, b"disposable payload").unwrap();
    native::set_read_only(&file, true).unwrap();
    fs::create_dir(selected.join("empty")).unwrap();
    let keep = root.join("outside-sentinel");
    fs::write(&keep, b"preserve sibling").unwrap();
    let identity = PersistedIdentity::capture(&selected).unwrap();
    let result = remove(&selected, &identity).unwrap();
    assert_eq!(result["completed"], true);
    assert_eq!(result["removedLogicalBytes"], 18);
    assert_eq!(result["removedFileCount"], 1);
    assert!(!selected.exists());
    assert_eq!(fs::read(&keep).unwrap(), b"preserve sibling");
}

#[test]
fn replaced_root_or_leaf_never_authorizes_deleting_the_replacement() {
    let tmp = temporary();
    let root = tmp.path().canonicalize().unwrap();
    let selected = root.join("selected");
    fs::create_dir(&selected).unwrap();
    fs::write(selected.join("payload"), b"owned original").unwrap();
    let identity = PersistedIdentity::capture(&selected).unwrap();
    fs::rename(&selected, root.join("retained-original")).unwrap();
    fs::create_dir(&selected).unwrap();
    fs::write(selected.join("payload"), b"replacement").unwrap();
    assert!(remove(&selected, &identity).is_err());
    assert_eq!(fs::read(selected.join("payload")).unwrap(), b"replacement");
    let identity = PersistedIdentity::capture(&selected).unwrap();
    let error = remove_with_hook(&selected, &identity, || {
        fs::rename(selected.join("payload"), selected.join("renamed"))?;
        fs::write(selected.join("payload"), b"new external payload")?;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.details["cleanupResult"]["completed"], false);
    assert_eq!(
        fs::read(selected.join("payload")).unwrap(),
        b"new external payload"
    );
    assert_eq!(fs::read(selected.join("renamed")).unwrap(), b"replacement");
}

#[test]
fn persistent_open_file_reports_partial_failure_and_later_retry_finishes() {
    let tmp = temporary();
    let selected = tmp.path().canonicalize().unwrap().join("selected");
    fs::create_dir(&selected).unwrap();
    fs::write(selected.join("a-occupied"), b"keep while open").unwrap();
    fs::write(selected.join("z-removable"), b"remove first").unwrap();
    let identity = PersistedIdentity::capture(&selected).unwrap();
    let open = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(selected.join("a-occupied"))
        .unwrap();
    let started = std::time::Instant::now();
    let error = remove(&selected, &identity).unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(8));
    assert_eq!(error.details["reason"], "file_in_use");
    assert_eq!(error.details["cleanupResult"]["removedFileCount"], 1);
    assert_eq!(error.details["cleanupResult"]["countsAreLowerBounds"], true);
    assert!(!selected.join("z-removable").exists());
    assert_eq!(
        fs::read(selected.join("a-occupied")).unwrap(),
        b"keep while open"
    );
    drop(open);
    assert_eq!(remove(&selected, &identity).unwrap()["completed"], true);
    assert!(!selected.exists());
}

#[test]
fn unplanned_content_is_preserved_and_not_counted_as_removed() {
    let tmp = temporary();
    let selected = tmp.path().canonicalize().unwrap().join("selected");
    fs::create_dir(&selected).unwrap();
    fs::write(selected.join("planned"), b"disposable planned").unwrap();
    let identity = PersistedIdentity::capture(&selected).unwrap();
    let error = remove_with_hook(&selected, &identity, || {
        fs::write(selected.join("unplanned"), b"preserve late content")?;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.details["cleanupResult"]["completed"], false);
    assert_eq!(
        fs::read(selected.join("unplanned")).unwrap(),
        b"preserve late content"
    );
    assert!(!selected.join("planned").exists());
}

#[test]
fn an_owned_hardlink_does_not_claim_the_external_original() {
    let tmp = temporary();
    let root = tmp.path().canonicalize().unwrap();
    let selected = root.join("selected");
    fs::create_dir(&selected).unwrap();
    let original = root.join("external-original");
    fs::write(&original, b"preserve original").unwrap();
    fs::hard_link(&original, selected.join("owned-link")).unwrap();
    native::set_read_only(&original, true).unwrap();
    let identity = PersistedIdentity::capture(&selected).unwrap();
    assert_eq!(remove(&selected, &identity).unwrap()["completed"], true);
    assert!(!selected.exists());
    assert_eq!(fs::read(&original).unwrap(), b"preserve original");
    assert!(fs::metadata(&original).unwrap().permissions().readonly());
    native::set_read_only(&original, false).unwrap();
}

#[test]
fn directory_junction_is_unlinked_without_entering_the_external_target() {
    let tmp = temporary();
    let root = tmp.path().canonicalize().unwrap();
    let selected = root.join("selected");
    let external = root.join("external-original");
    fs::create_dir(&selected).unwrap();
    fs::create_dir(&external).unwrap();
    fs::write(external.join("sentinel"), b"preserve junction target").unwrap();
    let external_identity = PersistedIdentity::capture(&external).unwrap();
    let cmd = std::env::var_os("COMSPEC")
        .expect("Windows command processor for disposable junction fixture");
    let status = std::process::Command::new(cmd)
        .args(["/d", "/c", "mklink", "/J"])
        .arg(selected.join("junction"))
        .arg(&external)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let identity = PersistedIdentity::capture(&selected).unwrap();
    assert_eq!(remove(&selected, &identity).unwrap()["completed"], true);
    assert!(!selected.exists());
    assert!(external_identity.matches_path(&external).unwrap());
    assert_eq!(
        fs::read(external.join("sentinel")).unwrap(),
        b"preserve junction target"
    );
}
