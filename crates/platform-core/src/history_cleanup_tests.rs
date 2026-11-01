use super::*;
use std::os::unix::fs::PermissionsExt;

fn candidate(root: &Path) -> Candidate {
    let canonical = root.canonicalize().unwrap();
    let root = canonical.as_path();
    let mut item = Candidate::new(
        "original_task_fixture".into(),
        "recoveryOriginal",
        "fixture",
        root.into(),
    );
    item.root_identity = Some(directory_identity(root).unwrap());

    item.parents = parent_identities(root).unwrap();
    item.entries = scan(root).unwrap();
    item.can_clean = true;
    item
}

#[test]
fn safe_unlink_never_follows_symlinks_and_reports_actual_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("owned");
    fs::create_dir_all(root.join("nested")).unwrap();
    let outside = temporary.path().join("outside");
    fs::write(&outside, "never remove target").unwrap();
    fs::write(root.join("nested/file"), b"12345").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
    let item = candidate(&root);
    let removed = remove_tree(&item).unwrap();
    assert_eq!(removed["completed"], true);
    assert_eq!(removed["removedLogicalBytes"], 5);
    assert_eq!(removed["removedFileCount"], 2);
    assert!(!root.exists());
    assert_eq!(fs::read_to_string(outside).unwrap(), "never remove target");
}

#[test]
fn changed_content_and_replaced_ancestors_are_retained() {
    let temporary = tempfile::tempdir().unwrap();
    let parent = temporary.path().join("parent");
    let root = parent.join("owned");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a"), "original").unwrap();
    let item = candidate(&root);
    fs::write(root.join("a"), "modified").unwrap();
    assert!(remove_tree(&item).is_err());
    assert_eq!(fs::read_to_string(root.join("a")).unwrap(), "modified");
    fs::rename(&parent, temporary.path().join("old-parent")).unwrap();
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("replacement"), "retain").unwrap();
    assert!(remove_tree(&item).is_err());
    assert_eq!(
        fs::read_to_string(root.join("replacement")).unwrap(),
        "retain"
    );
}

#[test]
fn new_unplanned_files_survive_partial_deletion_with_an_honest_receipt() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("owned");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("a"), "abc").unwrap();
    let item = candidate(&root);
    let failure = remove_tree_with_hook(&item, |_, path| {
        fs::write(path.join("new-user-file"), "new draft")?;
        Ok(())
    })
    .unwrap_err();
    assert_eq!(failure.details["cleanupResult"]["completed"], false);
    assert_eq!(failure.details["cleanupResult"]["removedLogicalBytes"], 3);
    assert!(root.exists());
    assert_eq!(
        fs::read_to_string(root.join("new-user-file")).unwrap(),
        "new draft"
    );
}

#[test]
fn special_files_and_root_symlinks_are_never_cleanup_candidates() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("owned");
    fs::create_dir(&root).unwrap();
    let fifo = c_name(&root.join("pipe")).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(scan(&root).is_err());
    let alias = temporary.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    assert!(scan(&alias).is_err());
}

#[test]
fn large_byte_counts_are_never_silently_coerced() {
    assert_eq!(
        wire_u64(Some(9_007_199_254_740_992)),
        json!({"$int64":"9007199254740992"})
    );
    assert_eq!(wire_u64(None), Value::Null);
}

#[test]
fn owned_dependency_removal_is_log_free_and_does_not_read_payload_contents() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary
        .path()
        .canonicalize()
        .unwrap()
        .join("disposable-node-modules");
    fs::create_dir_all(root.join("icons")).unwrap();
    for index in 0..4_000 {
        fs::write(
            root.join("icons").join(format!("icon-{index}.svg")),
            b"<svg/>",
        )
        .unwrap();
    }
    let unreadable = root.join("unreadable.js");
    fs::write(
        &unreadable,
        b"need not hash a dependency being permanently removed",
    )
    .unwrap();
    fs::set_permissions(&unreadable, fs::Permissions::from_mode(0o0)).unwrap();
    let expected = PersistedIdentity::capture(&root).unwrap();
    let started = std::time::Instant::now();
    let result = remove_owned_directory(&root, &expected).unwrap();
    eprintln!(
        "log-free permanent removal: 4001 files in {:?}",
        started.elapsed()
    );
    assert_eq!(result["removedFileCount"], 4_001);
    assert_eq!(result["completed"], true);
    assert!(!root.exists());
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 0);
}

#[test]
fn permanent_removal_never_adopts_a_replacement_after_binding_the_original_descriptor() {
    let temporary = tempfile::Builder::new()
        .prefix("disposable-bound-delete-")
        .tempdir()
        .unwrap();
    let base = temporary.path().canonicalize().unwrap();
    let root = base.join("owned");
    let moved = base.join("preserved-original");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("original.txt"), "original project material").unwrap();
    let expected = PersistedIdentity::capture(&root).unwrap();
    let result = remove_owned_directory_bound(&root, &expected, || {
        fs::rename(&root, &moved)?;
        fs::create_dir(&root)?;
        fs::write(root.join("replacement.txt"), "must survive")?;
        Ok(())
    });
    assert!(result.is_err());
    assert_eq!(
        fs::read_to_string(root.join("replacement.txt")).unwrap(),
        "must survive"
    );
    assert_eq!(
        fs::read_to_string(moved.join("original.txt")).unwrap(),
        "original project material"
    );
}
