//! Real NTFS/ACL fixtures under the repository's disposable evidence directory.
use super::*;
use std::{fs, os::windows::process::CommandExt};
use windows_sys::Win32::Security::{
    Authorization::SetSecurityInfo, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION,
};

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
