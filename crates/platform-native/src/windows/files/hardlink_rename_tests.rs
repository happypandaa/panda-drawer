use super::*;
use std::fs;

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-hardlink-rename-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().canonicalize().unwrap();
    (temporary, root)
}

#[test]
fn moving_one_readonly_hardlink_preserves_external_name_identity_bytes_and_attributes() {
    let (_temporary, root) = fixture();
    let external = root.join("external");
    let source = root.join("owned");
    let target = root.join("published");
    fs::write(&external, b"external bytes").unwrap();
    fs::hard_link(&external, &source).unwrap();
    set_read_only(&external, true).unwrap();
    let expected = open_no_reparse(&source).unwrap().identity;
    assert_eq!(expected.links, 2);
    let external_handle = open_no_reparse(&external).unwrap();
    let source_handle = open_no_reparse(&source).unwrap();
    let observe = || {
        for file in [&external_handle.file, &source_handle.file] {
            assert!(file.metadata().unwrap().permissions().readonly());
            assert!(expected.same_object(&identity(file).unwrap()));
        }
    };
    rename_with(&source, &target, &expected, observe, observe).unwrap();
    assert!(!source.exists());
    for name in [&external, &target] {
        assert!(expected.same_object(&open_no_reparse(name).unwrap().identity));
        assert!(fs::metadata(name).unwrap().permissions().readonly());
        assert_eq!(fs::read(name).unwrap(), b"external bytes");
    }
    // Existing independent-file callers retain their original restriction.
    assert!(rename_file_noreplace_expected(&target, &root.join("rejected"), &expected).is_err());
    set_read_only(&external, false).unwrap();
}

#[test]
fn occupied_destination_and_replacement_before_open_preserve_both_files() {
    let (_temporary, root) = fixture();
    let source = root.join("owned");
    let target = root.join("occupied");
    fs::write(&source, b"original").unwrap();
    fs::write(&target, b"foreign destination").unwrap();
    let expected = open_no_reparse(&source).unwrap().identity;
    assert!(
        rename_file_noreplace_expected_allowing_hard_links(&source, &target, &expected).is_err()
    );
    assert_eq!(fs::read(&source).unwrap(), b"original");
    assert_eq!(fs::read(&target).unwrap(), b"foreign destination");
    fs::rename(&source, root.join("held")).unwrap();
    fs::write(&source, b"foreign source").unwrap();
    assert!(rename_file_noreplace_expected_allowing_hard_links(
        &source,
        &root.join("new"),
        &expected
    )
    .is_err());
    assert_eq!(fs::read(&source).unwrap(), b"foreign source");
    assert_eq!(fs::read(root.join("held")).unwrap(), b"original");
}

#[test]
fn opened_handle_moves_only_original_object_when_public_name_is_replaced() {
    let (_temporary, root) = fixture();
    let source = root.join("owned");
    let target = root.join("published");
    let held = root.join("held");
    fs::write(&source, b"original").unwrap();
    let expected = open_no_reparse(&source).unwrap().identity;
    rename_with(
        &source,
        &target,
        &expected,
        || {
            fs::rename(&source, &held).unwrap();
            fs::write(&source, b"foreign source").unwrap();
        },
        || {},
    )
    .unwrap();
    assert!(!held.exists());
    assert_eq!(fs::read(&source).unwrap(), b"foreign source");
    assert_eq!(fs::read(&target).unwrap(), b"original");
    assert!(expected.same_object(&open_no_reparse(&target).unwrap().identity));
}

#[test]
fn postmove_disappearance_is_unknown_and_preserves_exact_original() {
    let (_temporary, root) = fixture();
    let source = root.join("owned");
    let target = root.join("published");
    let held = root.join("held");
    fs::write(&source, b"original").unwrap();
    let expected = open_no_reparse(&source).unwrap().identity;
    let issue = rename_with(
        &source,
        &target,
        &expected,
        || {},
        || {
            fs::rename(&target, &held).unwrap();
        },
    )
    .unwrap_err();
    assert!(publication_outcome_unknown(&issue));
    assert_eq!(fs::read(&held).unwrap(), b"original");
    assert!(expected.same_object(&open_no_reparse(&held).unwrap().identity));
}

#[test]
fn complete_high_half_identity_and_no_reparse_are_required_without_touching_targets() {
    let (_temporary, root) = fixture();
    let source = root.join("owned");
    let target = root.join("published");
    fs::write(&source, b"original").unwrap();
    let mut expected = open_no_reparse(&source).unwrap().identity;
    expected.file_id[15] ^= 1;
    assert!(
        rename_file_noreplace_expected_allowing_hard_links(&source, &target, &expected).is_err()
    );
    assert_eq!(fs::read(&source).unwrap(), b"original");
    assert!(!target.exists());
    let expected = open_no_reparse(&source).unwrap().identity;
    let held = root.join("original-kept");
    fs::rename(&source, &held).unwrap();
    let victim = root.join("external-victim");
    fs::create_dir(&victim).unwrap();
    fs::write(victim.join("sentinel"), b"external target").unwrap();
    let result = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&source)
        .arg(&victim)
        .output()
        .unwrap();
    assert!(result.status.success());
    assert!(
        rename_file_noreplace_expected_allowing_hard_links(&source, &target, &expected).is_err()
    );
    assert!(fs::symlink_metadata(&source).is_ok());
    assert_eq!(
        fs::read(victim.join("sentinel")).unwrap(),
        b"external target"
    );
    assert_eq!(fs::read(&held).unwrap(), b"original");
    fs::remove_dir(&source).unwrap();
}
