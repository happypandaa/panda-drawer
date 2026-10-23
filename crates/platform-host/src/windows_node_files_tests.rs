//! Real NTFS/ACL fixtures under the repository's disposable evidence directory.
use super::*;
use std::{fs, os::windows::process::CommandExt};
use windows_sys::Win32::Security::{
    Authorization::SetSecurityInfo, GetSecurityDescriptorDacl, GetSecurityDescriptorLength,
    PROTECTED_DACL_SECURITY_INFORMATION,
};

fn acl_bytes(directory: &WindowsSourceDirectory) -> Vec<u8> {
    let mut security = null_mut();
    assert_eq!(
        unsafe {
            GetSecurityInfo(
                directory.object().raw(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut security,
            )
        },
        0
    );
    let allocation = Allocation(security.cast());
    let length = unsafe { GetSecurityDescriptorLength(security) } as usize;
    let bytes = unsafe { std::slice::from_raw_parts(security.cast::<u8>(), length) }.to_vec();
    drop(allocation);
    bytes
}

#[test]
fn external_create_new_uses_inherited_acl_and_preserves_existing_parent_permissions() {
    let _serial = serial();
    let root = fixture();
    let original = root.create_private_directory("ordinary-source").unwrap();
    let external = WindowsSourceDirectory::open(original.path()).unwrap();
    let before = acl_bytes(&external);
    let child = external.create_source_directory("web").unwrap();
    assert_eq!(before, acl_bytes(&external));
    assert!(!child.private);
    assert!(
        owner_and_acl(child.object().raw(), &user_sid().unwrap(), true).is_err(),
        "No private protected ACL is imposed on external children"
    );
    let (mut file, identity) = child.create_source_file("new.txt").unwrap();
    file.write_all(b"ordinary inherited source").unwrap();
    file.sync_all().unwrap();
    assert!(fs::OpenOptions::new()
        .write(true)
        .open(child.path().join("new.txt"))
        .is_err());
    assert!(fs::rename(
        child.path().join("new.txt"),
        child.path().join("replaced.txt")
    )
    .is_err());
    drop(file);
    let read = child.read_bounded("new.txt", 100).unwrap();
    assert_eq!(read.identity, identity);
    assert_eq!(read.bytes, b"ordinary inherited source");
    assert_eq!(before, acl_bytes(&external));
    assert!(external.create_source_directory("web").is_err());
    assert!(child.create_source_file("new.txt").is_err());
    assert_eq!(
        fs::read(child.path().join("new.txt")).unwrap(),
        b"ordinary inherited source"
    );
}

#[test]
fn external_created_files_are_retained_when_the_caller_cannot_complete_writing() {
    let _serial = serial();
    let root = fixture();
    let external = WindowsSourceDirectory::open(
        root.create_private_directory("partial-source")
            .unwrap()
            .path(),
    )
    .unwrap();
    let (mut file, _) = external.create_source_file("partial.txt").unwrap();
    file.write_all(b"retained partial bytes").unwrap();
    file.sync_all().unwrap();
    drop(file);
    assert_eq!(
        fs::read(external.path().join("partial.txt")).unwrap(),
        b"retained partial bytes"
    );
    assert!(external.create_source_file("partial.txt").is_err());
}

#[test]
fn create_private_in_never_adopts_existing_children_or_changes_parent_acl() {
    let _serial = serial();
    let root = fixture();
    let external = root.create_private_directory("temporary-parent").unwrap();
    let before = acl_bytes(&external);
    let child =
        WindowsSourceDirectory::create_private_in(external.path(), "disposable-development-new")
            .unwrap();
    assert_eq!(before, acl_bytes(&external));
    owner_and_acl(child.object().raw(), &user_sid().unwrap(), true).unwrap();
    child
        .write_new("retained.txt", b"private temporary bytes")
        .unwrap();
    assert!(WindowsSourceDirectory::create_private_in(
        external.path(),
        "disposable-development-new"
    )
    .is_err());
    assert!(WindowsSourceDirectory::create_private_in(external.path(), "../outside").is_err());
    assert_eq!(before, acl_bytes(&external));
    assert_eq!(
        fs::read(child.path().join("retained.txt")).unwrap(),
        b"private temporary bytes"
    );
}

#[test]
fn source_metadata_pins_reject_occupied_files_and_prevent_replacement_without_reading_contents() {
    let _serial = serial();
    let root = fixture();
    root.write_new("metadata-only.txt", b"artificial source text")
        .unwrap();
    let source = WindowsSourceDirectory::open(root.path()).unwrap();
    let writer = fs::OpenOptions::new()
        .write(true)
        .open(root.path().join("metadata-only.txt"))
        .unwrap();
    assert!(source.pin_entry("metadata-only.txt").is_err());
    drop(writer);
    let (pin, observed) = source.pin_entry("metadata-only.txt").unwrap();
    assert_eq!(observed.length, 22);
    assert!(!observed.is_directory);
    assert!(fs::OpenOptions::new()
        .write(true)
        .open(root.path().join("metadata-only.txt"))
        .is_err());
    assert!(fs::rename(
        root.path().join("metadata-only.txt"),
        root.path().join("changed.txt")
    )
    .is_err());
    pin.verify_binding().unwrap();
}

#[test]
fn source_entries_allow_bounded_large_sources_without_changing_legacy_list_limits() {
    let _serial = serial();
    let root = fixture();
    for index in 0..301 {
        fs::write(
            root.path().join(format!("source-{index:03}.txt")),
            b"public source metadata",
        )
        .unwrap();
    }
    let source = WindowsSourceDirectory::open(root.path()).unwrap();
    assert_eq!(source.list().unwrap_err().code, "NODE_ENUMERATION_LIMIT");
    let observed = source.source_entries(512).unwrap();
    assert_eq!(observed.len(), 301);
    assert!(source.source_entries(300).is_err());
    assert!(source.source_entries(0).is_err());
    assert!(source.source_entries(40_001).is_err());
    let expected = observed
        .iter()
        .find(|entry| entry.name == "source-200.txt")
        .unwrap();
    let (pin, actual) = source.pin_source_entry(expected).unwrap();
    assert_eq!(&actual, expected);
    assert!(fs::OpenOptions::new()
        .write(true)
        .open(root.path().join(&expected.name))
        .is_err());
    pin.verify_binding().unwrap();
}

#[test]
fn source_enumeration_can_exclude_junctions_but_selected_links_never_pin_or_read_targets() {
    let _serial = serial();
    let root = fixture();
    let outside = root.create_private_directory("retained-outside").unwrap();
    outside
        .write_new("secret", b"never enumerate this target")
        .unwrap();
    let source = root.create_private_directory("source-filter").unwrap();
    source.write_new("included.txt", b"allowed source").unwrap();
    let link = source.path().join(".git");
    let output = std::process::Command::new("cmd.exe")
        .creation_flags(0x08000000)
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(outside.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let observed = source.source_entries(512).unwrap();
    let excluded = observed.iter().find(|entry| entry.name == ".git").unwrap();
    assert!(excluded.reparse);
    assert!(source.pin_source_entry(excluded).is_err());
    assert!(source.list().is_err());
    let included = observed
        .iter()
        .find(|entry| entry.name == "included.txt")
        .unwrap();
    source.pin_source_entry(included).unwrap();
    assert_eq!(
        outside.read_bounded("secret", 128).unwrap().bytes,
        b"never enumerate this target"
    );
}

#[test]
fn source_pins_refuse_replaced_identity_changed_metadata_hardlinks_and_new_junctions() {
    let _serial = serial();
    let root = fixture();
    root.write_new("selected.txt", b"same bytes").unwrap();
    let source = WindowsSourceDirectory::open(root.path()).unwrap();
    let observed = source.source_entries(64).unwrap();
    let expected = observed
        .iter()
        .find(|entry| entry.name == "selected.txt")
        .unwrap();
    fs::rename(
        root.path().join("selected.txt"),
        root.path().join("retained-original.txt"),
    )
    .unwrap();
    root.write_new("selected.txt", b"same bytes").unwrap();
    assert!(source.pin_source_entry(expected).is_err());
    let observed = source.source_entries(64).unwrap();
    let expected = observed
        .iter()
        .find(|entry| entry.name == "selected.txt")
        .unwrap();
    fs::write(root.path().join("selected.txt"), b"changed length").unwrap();
    assert!(source.pin_source_entry(expected).is_err());
    let observed = source.source_entries(64).unwrap();
    let expected = observed
        .iter()
        .find(|entry| entry.name == "selected.txt")
        .unwrap();
    fs::hard_link(
        root.path().join("selected.txt"),
        root.path().join("retained-hardlink.txt"),
    )
    .unwrap();
    assert!(source.pin_source_entry(expected).is_err());
    let selected = root.create_private_directory("selected-directory").unwrap();
    let path = selected.path().to_owned();
    drop(selected);
    let observed = source.source_entries(64).unwrap();
    let expected = observed
        .iter()
        .find(|entry| entry.name == "selected-directory")
        .unwrap();
    fs::rename(&path, root.path().join("retained-directory")).unwrap();
    let target = root.create_private_directory("target-directory").unwrap();
    let output = std::process::Command::new("cmd.exe")
        .creation_flags(0x08000000)
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&path)
        .arg(target.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(source.pin_source_entry(expected).is_err());
}

#[test]
fn successful_native_creation_is_reported_before_any_failing_post_create_verification() {
    let _serial = serial();
    let root = fixture();
    let source = WindowsSourceDirectory::open(root.path()).unwrap();
    let mut created = false;
    let result = source.create_source_file_notified("partial.txt", || {
        created = true;
        fs::hard_link(
            root.path().join("partial.txt"),
            root.path().join("retained-created-hardlink"),
        )
        .unwrap();
    });
    assert!(result.is_err());
    assert!(created);
    assert!(root.path().join("partial.txt").is_file());
    assert!(root.path().join("retained-created-hardlink").is_file());
    let mut duplicate_notified = false;
    assert!(source
        .create_source_file_notified("partial.txt", || duplicate_notified = true)
        .is_err());
    assert!(
        !duplicate_notified,
        "EEXIST did not create or adopt an object"
    );
}

fn fixture() -> WindowsSourceDirectory {
    let artifacts = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(".test-artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    let path = artifacts.join(format!(
        "windows-workspace-disposable-nodefs-{}",
        uuid::Uuid::new_v4()
    ));
    create_private_fixture_directory(&path).unwrap();
    WindowsSourceDirectory::open_private(&path).unwrap()
}
fn serial() -> std::sync::MutexGuard<'static, ()> {
    crate::WINDOWS_FILESYSTEM_TESTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[test]
fn node_private_file_acl_changes_are_rejected_without_permission_repair() {
    let _serial = serial();
    let root = fixture();
    root.write_new("secret", b"artificial-private-key").unwrap();
    let sid = user_sid().unwrap();
    let writer = root
        .file(
            "secret",
            windows_sys::Win32::Storage::FileSystem::WRITE_DAC,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            false,
        )
        .unwrap();
    let sddl = wide(OsStr::new(&format!("D:P(A;;FA;;;{sid})(A;;GR;;;WD)"))).unwrap();
    let mut raw = null_mut();
    assert_ne!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut raw,
                null_mut(),
            )
        },
        0
    );
    let security = Allocation(raw);
    let (mut present, mut defaulted, mut dacl) = (0, 0, null_mut());
    assert_ne!(
        unsafe { GetSecurityDescriptorDacl(security.0, &mut present, &mut dacl, &mut defaulted) },
        0
    );
    assert_ne!(present, 0);
    assert_eq!(
        unsafe {
            SetSecurityInfo(
                writer.raw(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                null(),
            )
        },
        0
    );
    assert!(root.read_bounded("secret", 128).is_err());
    assert!(owner_and_acl(writer.raw(), &sid, true).is_err());
    assert_eq!(
        fs::read(root.path().join("secret")).unwrap(),
        b"artificial-private-key"
    );
}

#[test]
fn node_source_long_name_aliases_share_identity_and_namespaces_stay_restricted() {
    let _serial = serial();
    let root = fixture();
    let source = root.create_private_directory("SourceCase").unwrap();
    for alias in [
        source.path().to_string_lossy().to_lowercase(),
        source.path().to_string_lossy().replace('\\', "/"),
        source
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    ] {
        let opened = WindowsSourceDirectory::open(Path::new(&alias)).unwrap();
        assert_eq!(opened.identity(), source.identity());
        assert_eq!(opened.path(), source.path());
    }
    for path in [
        r"\\server\share\source",
        r"\\.\D:\source",
        r"\\?\UNC\server\share\source",
    ] {
        assert!(WindowsSourceDirectory::open(Path::new(path)).is_err());
    }
}

#[test]
fn node_source_handles_prevent_replacement_and_detect_new_hardlinks() {
    let _serial = serial();
    let root = fixture();
    let source = root.create_private_directory("public").unwrap();
    source
        .write_new("index.html", b"retained original page")
        .unwrap();
    let guard = source.pin_file("index.html").unwrap();
    assert!(fs::rename(source.path(), source.path().with_file_name("moved-public")).is_err());
    assert!(fs::rename(
        source.path().join("index.html"),
        source.path().join("moved.html")
    )
    .is_err());
    fs::hard_link(
        source.path().join("index.html"),
        root.path().join("retained-hardlink"),
    )
    .unwrap();
    assert!(guard.verify_binding().is_err());
    assert!(source.read_bounded("index.html", 128).is_err());
    assert_eq!(
        fs::read(root.path().join("retained-hardlink")).unwrap(),
        b"retained original page"
    );
}

#[test]
fn node_publish_and_delete_reject_external_write_handles() {
    let _serial = serial();
    let root = fixture();
    let identity = root
        .write_new("prepared", b"immutable prepared bytes")
        .unwrap();
    let writer = fs::OpenOptions::new()
        .write(true)
        .open(root.path().join("prepared"))
        .unwrap();
    assert!(root
        .publish_noreplace_checked("prepared", "committed", &identity)
        .is_err());
    assert!(root.remove_file_checked("prepared", &identity).is_err());
    assert!(!root.path().join("committed").exists());
    assert_eq!(
        fs::read(root.path().join("prepared")).unwrap(),
        b"immutable prepared bytes"
    );
    drop(writer);
    root.publish_noreplace_checked("prepared", "committed", &identity)
        .unwrap();
    assert_eq!(
        root.read_bounded("committed", 128).unwrap().identity,
        identity
    );
}

#[test]
fn node_publication_collision_and_replacement_identity_preserve_all_objects() {
    let _serial = serial();
    let root = fixture();
    let identity = root.write_new("prepared", b"prepared original").unwrap();
    root.write_new("committed", b"old revision").unwrap();
    assert!(root
        .publish_noreplace_checked("prepared", "committed", &identity)
        .is_err());
    assert_eq!(
        root.read_bounded("committed", 128).unwrap().bytes,
        b"old revision"
    );
    fs::rename(
        root.path().join("prepared"),
        root.path().join("prepared-retained"),
    )
    .unwrap();
    root.write_new("prepared", b"external replacement").unwrap();
    assert!(root
        .publish_noreplace_checked("prepared", "new-target", &identity)
        .is_err());
    assert!(root.remove_file_checked("prepared", &identity).is_err());
    assert_eq!(
        root.read_bounded("prepared", 128).unwrap().bytes,
        b"external replacement"
    );
    assert_eq!(
        root.read_bounded("prepared-retained", 128).unwrap().bytes,
        b"prepared original"
    );
    assert!(!root.path().join("new-target").exists());
}

#[test]
fn node_source_junctions_are_rejected_without_reading_or_rewriting_target() {
    let _serial = serial();
    let root = fixture();
    let outside = root.create_private_directory("outside").unwrap();
    outside
        .write_new("private.txt", b"outside retained")
        .unwrap();
    let source = root.create_private_directory("public").unwrap();
    let link = source.path().join("junction");
    let output = std::process::Command::new("cmd.exe")
        .creation_flags(0x08000000)
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(outside.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "cannot create disposable junction");
    assert!(source.open_directory("junction").is_err());
    assert!(WindowsSourceDirectory::open(&link).is_err());
    assert_eq!(
        outside.read_bounded("private.txt", 128).unwrap().bytes,
        b"outside retained"
    );
}
