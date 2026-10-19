//! Real Windows primitives, exclusively inside new named disposable paths.
//! No test initializes the actual application data root or removes evidence.
use super::*;
use std::{fs, process::Command, sync::MutexGuard};
use windows_sys::Win32::{
    Foundation::GENERIC_WRITE,
    Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW,
    Storage::FileSystem::{BusTypeUnknown, BusTypeUsb, BusTypeVirtual, CreateDirectoryW},
    System::{Com::CoCreateGuid, Ioctl::FSCTL_SET_REPARSE_POINT},
};

fn serial() -> MutexGuard<'static, ()> {
    crate::WINDOWS_FILESYSTEM_TESTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
fn fixture() -> PathBuf {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let artifacts = repository.join(".test-artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    use std::os::windows::fs::MetadataExt;
    assert_eq!(
        fs::symlink_metadata(&artifacts).unwrap().file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT,
        0
    );
    let mut guid = windows_sys::core::GUID::default();
    assert!(unsafe { CoCreateGuid(&mut guid) } >= 0);
    let name = format!(
        "windows-workspace-disposable-{:08x}{:04x}{:04x}{}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let path = artifacts.join(name);
    fs::create_dir(&path).unwrap();
    path
}
fn code<T>(value: Result<T>) -> &'static str {
    match value {
        Ok(_) => panic!("operation unexpectedly succeeded"),
        Err(failure) => failure.code,
    }
}
fn acl_text(raw: HANDLE) -> String {
    let mut descriptor = null_mut();
    assert_eq!(
        unsafe {
            GetSecurityInfo(
                raw,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor,
            )
        },
        0
    );
    let allocation = Allocation(descriptor);
    let mut pointer = null_mut();
    assert_ne!(
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                1,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut pointer,
                null_mut(),
            )
        },
        0
    );
    let converted = Allocation(pointer.cast());
    let text = unsafe { text(pointer, 32768) }.unwrap();
    drop(converted);
    drop(allocation);
    text
}
fn junction(link: &Path, target: &Path) {
    let status = Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "cannot create disposable junction: {}",
        String::from_utf8_lossy(&status.stderr)
    );
}

fn canonical_fixture_path(raw: HANDLE) -> String {
    let mut buffer = vec![0u16; 32768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            raw,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            VOLUME_NAME_GUID,
        )
    };
    assert!(length > 0 && length < buffer.len() as u32);
    String::from_utf16(&buffer[..length as usize]).unwrap()
}

fn absolute_fixture_directory(path: &Path) -> Arc<Object> {
    let path = wide(path.as_os_str()).unwrap();
    let raw = unsafe {
        CreateFileW(
            path.as_ptr(),
            DIRECTORY_ACCESS,
            DIRECTORY_SHARING,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    assert_ne!(raw, INVALID_HANDLE_VALUE);
    object(unsafe { Handle::own(raw) }, true).unwrap()
}

#[test]
fn known_folder_exact_binding_succeeds_or_redirect_is_rejected_without_initialization() {
    let _serial = serial();
    let local = known_folder().unwrap();
    let mut guid = windows_sys::core::GUID::default();
    assert!(unsafe { CoCreateGuid(&mut guid) } >= 0);
    let name = format!(
        "local-projects-dev-w2-disposable-{:08x}{:04x}{:04x}{}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let parent = local.join(&name);
    // Create only a unique diagnostic sibling. A redirected Win32 create may
    // land elsewhere; determine the actual binding before trying the backend.
    fs::create_dir(&parent).unwrap();
    let ancestors = open_parent(&local).unwrap();
    let fixed_parent = ancestors.objects.last().unwrap();
    let absolute_leaf = absolute_fixture_directory(&parent);
    let canonical_parent = canonical_fixture_path(fixed_parent.raw());
    let canonical_leaf = canonical_fixture_path(absolute_leaf.raw());
    let expected_leaf = format!("{}\\{name}", canonical_parent.trim_end_matches('\\'));
    eprintln!("KnownFolder canonical parent: {canonical_parent}");
    eprintln!("Win32 disposable canonical leaf: {canonical_leaf}");
    let before_acl = acl_text(absolute_leaf.raw());
    let outcome = WindowsWorkspace::for_test_path(parent.clone(), true);
    if canonical_leaf != expected_leaf {
        let failure = match outcome {
            Ok(_) => panic!("redirected KnownFolder fixture was incorrectly admitted"),
            Err(failure) => failure,
        };
        eprintln!("Redirect rejected ({failure}); this demonstrates rejection, not support for this launch context.");
        assert!(!parent.join(crate::APP_IDENTIFIER).exists());
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
        assert_eq!(acl_text(absolute_leaf.raw()), before_acl);
        absolute_leaf.verify().unwrap();
        return;
    }

    let workspace = outcome.unwrap();
    assert_eq!(workspace.path(), parent.join(crate::APP_IDENTIFIER));
    assert_eq!(
        workspace.ancestor_identities().last(),
        Some(&absolute_leaf.identity)
    );
    let root = workspace.root();
    owner_and_acl(root.object().raw(), &user_sid().unwrap(), true).unwrap();
    let identity = root
        .write_new(
            "relative-write.txt",
            b"strict KnownFolder disposable evidence",
        )
        .unwrap();
    let file = root.read_bounded("relative-write.txt", 100).unwrap();
    assert_eq!(file.identity, identity);
    assert_eq!(file.bytes, b"strict KnownFolder disposable evidence");
    assert_eq!(acl_text(absolute_leaf.raw()), before_acl);
    root.verify_binding().unwrap();
    workspace.verify_binding().unwrap();
    eprintln!("Unredirected KnownFolder binding supports strict fixture creation and write.");
    // Retain the uniquely named fixture; never initialize the actual app root.
}
#[test]
fn missing_candidate_is_not_initialized_and_only_missing_has_missing_code() {
    let _serial = serial();
    let parent = fixture();
    assert_eq!(
        code(WindowsWorkspace::for_test_path(parent.clone(), false)),
        "FIXED_ROOT_MISSING"
    );
    assert!(!parent.join(crate::APP_IDENTIFIER).exists());
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 0);
    fs::write(
        parent.join(crate::APP_IDENTIFIER),
        b"retained incompatible evidence",
    )
    .unwrap();
    assert_ne!(
        code(WindowsWorkspace::for_test_path(parent.clone(), false)),
        "FIXED_ROOT_MISSING"
    );
    assert_eq!(
        fs::read(parent.join(crate::APP_IDENTIFIER)).unwrap(),
        b"retained incompatible evidence"
    );
}

#[test]
fn fresh_root_is_private_and_second_create_never_adopts_it() {
    let _serial = serial();
    let parent = fixture();
    let workspace = WindowsWorkspace::for_test_path(parent.clone(), true).unwrap();
    let root = workspace.root();
    let before_acl = acl_text(root.object().raw());
    owner_and_acl(root.object().raw(), &user_sid().unwrap(), true).unwrap();
    let identity = root.identity().clone();
    assert_ne!(identity.file_id128, [0; 16]);
    root.write_new("marker.json", b"new disposable marker")
        .unwrap();
    drop(root);
    drop(workspace);
    assert_eq!(
        code(WindowsWorkspace::for_test_path(parent.clone(), true)),
        "WORKSPACE_ALREADY_EXISTS"
    );
    let reopened = WindowsWorkspace::for_test_path(parent, false).unwrap();
    assert_eq!(reopened.identity(), &identity);
    assert_eq!(acl_text(reopened.root.object().raw()), before_acl);
    assert_eq!(
        reopened
            .root()
            .read_bounded("marker.json", 100)
            .unwrap()
            .bytes,
        b"new disposable marker"
    );
}

#[test]
fn writes_and_publication_are_create_new_and_preserve_existing_bytes() {
    let _serial = serial();
    let workspace = WindowsWorkspace::for_test_path(fixture(), true).unwrap();
    let root = workspace.root();
    let expected = root.write_new("original.json", b"original").unwrap();
    assert_eq!(
        code(root.write_new("original.json", b"replacement")),
        "WORKSPACE_ALREADY_EXISTS"
    );
    let staged_identity = root.write_new("staged.json", b"staged").unwrap();
    assert!(root
        .publish_noreplace_checked("staged.json", "original.json", false, &staged_identity)
        .is_err());
    assert_eq!(
        root.read_bounded("original.json", 100).unwrap().identity,
        expected
    );
    assert_eq!(
        root.read_bounded("original.json", 100).unwrap().bytes,
        b"original"
    );
    assert_eq!(
        root.read_bounded("staged.json", 100).unwrap().bytes,
        b"staged"
    );
    root.publish_noreplace_checked("staged.json", "published.json", false, &staged_identity)
        .unwrap();
    assert_eq!(
        code(root.read_bounded("staged.json", 100)),
        "WORKSPACE_NAME_MISSING"
    );
    assert_eq!(
        root.read_bounded("published.json", 100).unwrap().bytes,
        b"staged"
    );
}

#[test]
fn child_atomic_publication_works_while_root_and_ancestors_cannot_be_replaced() {
    let _serial = serial();
    let parent = fixture();
    let workspace = WindowsWorkspace::for_test_path(parent.clone(), true).unwrap();
    let root = workspace.root();
    let staging = root.create_directory("staging").unwrap();
    staging.write_new("sample.txt", b"sample").unwrap();
    let identity = staging.identity().clone();
    drop(staging);
    root.publish_noreplace_checked("staging", "sample", true, &identity)
        .unwrap();
    let published = root.open_directory("sample").unwrap();
    assert_eq!(published.identity(), &identity);
    let file_identity = published
        .write_new("before.txt", b"child atomic publication")
        .unwrap();
    published
        .publish_noreplace_checked("before.txt", "after.txt", false, &file_identity)
        .unwrap();
    assert_eq!(
        published.read_bounded("after.txt", 100).unwrap().bytes,
        b"child atomic publication"
    );
    assert!(fs::rename(workspace.path(), parent.join("substituted-root")).is_err());
    let renamed_parent = parent.with_extension("retained-renamed");
    assert!(fs::rename(&parent, &renamed_parent).is_err());
    workspace.verify_binding().unwrap();
    drop(published);
    drop(root);
    drop(workspace);
    fs::rename(&parent, &renamed_parent).unwrap();
}

#[test]
fn replacement_staging_source_is_retained_without_moving_it_or_creating_target() {
    let _serial = serial();
    let workspace = WindowsWorkspace::for_test_path(fixture(), true).unwrap();
    let root = workspace.root();
    let staging = root.create_directory("staging").unwrap();
    staging
        .write_new("payload.txt", b"prepared original evidence")
        .unwrap();
    let expected = staging.identity().clone();
    drop(staging);

    // Exercise the actual gap after preparation drops its directory handle.
    fs::rename(
        root.path().join("staging"),
        root.path().join("retained-original"),
    )
    .unwrap();
    let replacement = root.create_directory("staging").unwrap();
    replacement
        .write_new("payload.txt", b"replacement must not be published")
        .unwrap();
    let replacement_identity = replacement.identity().clone();
    assert_ne!(expected, replacement_identity);
    drop(replacement);

    assert_eq!(
        code(root.publish_noreplace_checked("staging", "published", true, &expected)),
        "WORKSPACE_PUBLICATION_SOURCE_CHANGED"
    );
    assert!(!root.path().join("published").exists());
    let original = root.open_directory("retained-original").unwrap();
    assert_eq!(original.identity(), &expected);
    assert_eq!(
        original.read_bounded("payload.txt", 100).unwrap().bytes,
        b"prepared original evidence"
    );
    let replacement = root.open_directory("staging").unwrap();
    assert_eq!(replacement.identity(), &replacement_identity);
    assert_eq!(
        replacement.read_bounded("payload.txt", 100).unwrap().bytes,
        b"replacement must not be published"
    );
}

#[test]
fn lock_file_is_kernel_exclusive_and_never_created_on_existing_candidate() {
    let _serial = serial();
    let parent = fixture();
    let workspace = WindowsWorkspace::for_test_path(parent.clone(), true).unwrap();
    let root = workspace.root();
    let lock = root.lock_file("instance.lock").unwrap();
    assert!(File::open(root.path().join("instance.lock")).is_err());
    assert!(root.lock_file("instance.lock").is_err());
    drop(lock);
    let lock = root.lock_file("instance.lock").unwrap();
    drop(lock);
    drop(root);
    drop(workspace);
    let reopened = WindowsWorkspace::for_test_path(parent, false).unwrap();
    let lock = reopened.root().lock_file("instance.lock").unwrap();
    assert_eq!(
        code(reopened.root().lock_file("unknown.lock")),
        "WORKSPACE_NAME_MISSING"
    );
    assert!(!reopened.path().join("unknown.lock").exists());
    drop(lock);
}

#[test]
fn named_reservation_is_lifetime_bound_without_thread_ownership() {
    let _serial = serial();
    let parent = fixture();
    let workspace = WindowsWorkspace::for_test_path(parent.clone(), true).unwrap();
    assert_eq!(
        code(WindowsWorkspace::for_test_path(parent.clone(), false)),
        "WORKSPACE_INSTANCE_LOCKED"
    );
    let root = workspace.root();
    drop(workspace);
    assert_eq!(
        code(WindowsWorkspace::for_test_path(parent.clone(), false)),
        "WORKSPACE_INSTANCE_LOCKED"
    );
    std::thread::spawn(move || drop(root)).join().unwrap();
    WindowsWorkspace::for_test_path(parent, false).unwrap();
}

#[test]
fn existing_writer_hardlinks_and_size_bounds_are_rejected() {
    let _serial = serial();
    let workspace = WindowsWorkspace::for_test_path(fixture(), true).unwrap();
    let root = workspace.root();
    root.write_new("sample.txt", b"retained content").unwrap();
    let writer = fs::OpenOptions::new()
        .write(true)
        .open(root.path().join("sample.txt"))
        .unwrap();
    assert!(root.read_bounded("sample.txt", 100).is_err());
    drop(writer);
    assert_eq!(
        code(root.read_bounded("sample.txt", 2)),
        "WORKSPACE_FILE_TOO_LARGE"
    );
    assert_eq!(
        root.read_bounded("sample.txt", 100).unwrap().bytes,
        b"retained content"
    );
    fs::hard_link(
        root.path().join("sample.txt"),
        root.path().join("alias.txt"),
    )
    .unwrap();
    assert_eq!(
        code(root.read_bounded("sample.txt", 100)),
        "WORKSPACE_HARDLINK_REJECTED"
    );
    assert_eq!(
        code(root.read_bounded("alias.txt", 100)),
        "WORKSPACE_HARDLINK_REJECTED"
    );
    assert_eq!(
        fs::read(root.path().join("sample.txt")).unwrap(),
        b"retained content"
    );
}

#[test]
fn junctions_are_not_followed_for_root_ancestry_or_children() {
    let _serial = serial();
    let parent = fixture();
    let target = parent.join("retained-outside");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("evidence.txt"), b"never follow").unwrap();
    let parent_link = parent.join("junction-parent");
    junction(&parent_link, &target);
    assert!(WindowsWorkspace::for_test_path(parent_link, true).is_err());
    assert!(!target.join(crate::APP_IDENTIFIER).exists());
    let workspace = WindowsWorkspace::for_test_path(parent.clone(), true).unwrap();
    let root = workspace.root();
    junction(&root.path().join("junction-child"), &target);
    assert!(root.open_directory("junction-child").is_err());
    assert!(root.create_directory("junction-child").is_err());
    assert_eq!(code(root.list()), "WORKSPACE_REPARSE_REJECTED");
    assert_eq!(
        fs::read(target.join("evidence.txt")).unwrap(),
        b"never follow"
    );
    assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
}

#[test]
fn in_place_reparse_conversion_cannot_redirect_handle_relative_creation() {
    let _serial = serial();
    let parent = fixture();
    let outside = parent.join("retained-reparse-target");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("evidence.txt"), b"not an output directory").unwrap();
    let workspace = WindowsWorkspace::for_test_path(parent, true).unwrap();
    let empty = workspace
        .root()
        .create_directory("empty-directory")
        .unwrap();
    let name = wide(empty.path().as_os_str()).unwrap();
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        // A native sharing denial is also a valid protection. Unlike a skipped
        // test, the ordinary operation below must still succeed in this branch.
        empty
            .write_new("normal.txt", b"ordinary owned directory")
            .unwrap();
    } else {
        let writer = unsafe { Handle::own(raw) };
        let substitute: Vec<u16> = format!("\\??\\{}", outside.display())
            .encode_utf16()
            .collect();
        let printable: Vec<u16> = outside.as_os_str().encode_wide().collect();
        let path_units = substitute.len() + 1 + printable.len() + 1;
        let mut buffer = vec![0u8; 16 + path_units * 2];
        buffer[0..4].copy_from_slice(&0xA000_0003_u32.to_le_bytes()); // IO_REPARSE_TAG_MOUNT_POINT
        buffer[4..6].copy_from_slice(&((8 + path_units * 2) as u16).to_le_bytes());
        buffer[10..12].copy_from_slice(&((substitute.len() * 2) as u16).to_le_bytes());
        buffer[12..14].copy_from_slice(&(((substitute.len() + 1) * 2) as u16).to_le_bytes());
        buffer[14..16].copy_from_slice(&((printable.len() * 2) as u16).to_le_bytes());
        for (index, unit) in substitute
            .iter()
            .chain(std::iter::once(&0))
            .chain(printable.iter())
            .chain(std::iter::once(&0))
            .enumerate()
        {
            buffer[16 + index * 2..18 + index * 2].copy_from_slice(&unit.to_le_bytes());
        }
        let mut returned = 0;
        let converted = unsafe {
            DeviceIoControl(
                writer.raw(),
                FSCTL_SET_REPARSE_POINT,
                buffer.as_ptr().cast(),
                buffer.len() as u32,
                null_mut(),
                0,
                &mut returned,
                null_mut(),
            )
        } != 0;
        drop(writer);
        if converted {
            assert_eq!(
                code(empty.write_new("blocked.txt", b"must not follow")),
                "WORKSPACE_REPARSE_REJECTED"
            );
            let security = descriptor(&user_sid().unwrap(), false).unwrap();
            // Deliberately exercise the syscall after a conversion, without the
            // normal prior verify, as the strongest possible timing window.
            let _anchored = relative(
                empty.object().raw(),
                "anchored.txt",
                false,
                true,
                FILE_READ_DATA
                    | FILE_WRITE_DATA
                    | FILE_READ_ATTRIBUTES
                    | READ_CONTROL
                    | SYNCHRONIZE,
                FILE_SHARE_READ,
                Some(&security),
            );
            assert!(!outside.join("anchored.txt").exists());
            assert!(!outside.join("blocked.txt").exists());
        } else {
            empty
                .write_new("normal.txt", b"ordinary owned directory")
                .unwrap();
        }
    }
    assert_eq!(
        fs::read(outside.join("evidence.txt")).unwrap(),
        b"not an output directory"
    );
    assert_eq!(fs::read_dir(outside).unwrap().count(), 1);
}

#[test]
fn permissive_old_root_acl_is_rejected_without_repair_or_new_lock_file() {
    let _serial = serial();
    let parent = fixture();
    let sid = user_sid().unwrap();
    let sddl = wide(OsStr::new(&format!(
        "O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;GR;;;WD)"
    )))
    .unwrap();
    let mut raw_security = null_mut();
    assert_ne!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut raw_security,
                null_mut(),
            )
        },
        0
    );
    let security = Allocation(raw_security);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.0,
        bInheritHandle: 0,
    };
    let root_path = parent.join(crate::APP_IDENTIFIER);
    let name = wide(root_path.as_os_str()).unwrap();
    assert_ne!(unsafe { CreateDirectoryW(name.as_ptr(), &attributes) }, 0);
    fs::write(
        root_path.join("original.txt"),
        b"retained incompatible root",
    )
    .unwrap();
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            DIRECTORY_ACCESS,
            DIRECTORY_SHARING,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    assert_ne!(raw, INVALID_HANDLE_VALUE);
    let handle = unsafe { Handle::own(raw) };
    let before = acl_text(handle.raw());
    assert_eq!(
        code(WindowsWorkspace::for_test_path(parent, false)),
        "WORKSPACE_PRIVATE_ACL_REJECTED"
    );
    assert_eq!(acl_text(handle.raw()), before);
    assert_eq!(
        fs::read(root_path.join("original.txt")).unwrap(),
        b"retained incompatible root"
    );
    assert!(!root_path.join("instance.lock").exists());
}

#[test]
fn relative_components_reject_escape_dos_aliases_and_alternate_streams() {
    for name in [
        "",
        ".",
        "..",
        "a/b",
        "a\\b",
        "C:",
        "a:stream",
        "trailing.",
        "trailing ",
        "CON",
        "con.txt",
        "COM1.json",
        "LPT9",
        "COM¹.txt",
        "CONIN$",
        "x\0y",
    ] {
        assert!(component(name).is_err(), "accepted {name:?}");
    }
    for name in [
        "project.json",
        ".creating-prj_a",
        "main.sqlite",
        "sample project",
    ] {
        component(name).unwrap();
    }
}

#[test]
fn only_nonremovable_nonhotplug_supported_physical_buses_are_admitted() {
    let mut hotplug = STORAGE_HOTPLUG_INFO {
        Size: size_of::<STORAGE_HOTPLUG_INFO>() as u32,
        ..Default::default()
    };
    for bus in [BusTypeAta, BusTypeSata, BusTypeNvme] {
        assert!(device_admitted(bus, false, &hotplug));
    }
    for bus in [BusTypeUsb, BusTypeVirtual, BusTypeUnknown] {
        assert!(!device_admitted(bus, false, &hotplug));
    }
    assert!(!device_admitted(BusTypeNvme, true, &hotplug));
    hotplug.DeviceHotplug = true;
    assert!(!device_admitted(BusTypeNvme, false, &hotplug));
    hotplug.DeviceHotplug = false;
    hotplug.MediaHotplug = true;
    assert!(!device_admitted(BusTypeSata, false, &hotplug));
}

#[test]
fn full_native_identity_roundtrips_without_unix_fields_or_truncation() {
    let identity = WindowsObjectIdentity {
        volume_serial: u64::MAX,
        file_id128: [255; 16],
    };
    let value = serde_json::to_value(&identity).unwrap();
    assert_eq!(value["volumeSerial"], serde_json::json!(u64::MAX));
    assert_eq!(value["fileId128"], serde_json::json!(vec![255; 16]));
    assert!(value.get("device").is_none());
    assert!(value.get("inode").is_none());
    assert_eq!(
        serde_json::from_value::<WindowsObjectIdentity>(value).unwrap(),
        identity
    );
}
