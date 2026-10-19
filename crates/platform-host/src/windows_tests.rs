//! Real Windows objects, exclusively in new disposable repository artifacts.
use super::*;
use std::{fs, os::windows::fs::MetadataExt, sync::MutexGuard};

// The probe intentionally takes short non-sharing ancestor handles. Independent
// fixtures still share volume/repository ancestors, so serialize this module's
// filesystem tests instead of pretending those native locks are independent.
fn serial() -> MutexGuard<'static, ()> {
    crate::WINDOWS_FILESYSTEM_TESTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn fixture() -> (PathBuf, String) {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let parent = repository.join(".test-artifacts");
    fs::create_dir_all(&parent).unwrap();
    assert_eq!(
        fs::symlink_metadata(&parent).unwrap().file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT,
        0
    );
    let path = parent.join(format!(
        "windows-host-disposable-{}",
        new_guid_name().unwrap()
    ));
    let user = current_user_sid().unwrap();
    private_directory(&path, &user).unwrap();
    (path, user)
}

fn world_readable_directory(parent: &Path, user: &str) -> PathBuf {
    let root = parent.join("world-readable");
    let sddl = format!("O:{user}D:P(A;OICI;FA;;;{user})(A;OICI;GR;;;WD)");
    let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut descriptor = null_mut();
    assert_ne!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                text.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            )
        },
        0
    );
    let allocation = LocalAllocation(descriptor);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let name = wide(&root).unwrap();
    assert_ne!(unsafe { CreateDirectoryW(name.as_ptr(), &attributes) }, 0);
    drop(allocation);
    root
}

fn acl_text(path: &Path) -> String {
    use windows_sys::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
    let object = open_object(path, true).unwrap();
    let flags = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut descriptor = null_mut();
    assert_eq!(
        unsafe {
            GetSecurityInfo(
                object.handle.raw(),
                SE_FILE_OBJECT,
                flags,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor,
            )
        },
        0
    );
    let allocation = LocalAllocation(descriptor);
    let mut text = null_mut();
    assert_ne!(
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                1,
                flags,
                &mut text,
                null_mut(),
            )
        },
        0
    );
    let string_allocation = LocalAllocation(text.cast());
    let value = unsafe { terminated_text(text, 65536).unwrap() };
    drop(string_allocation);
    drop(allocation);
    value
}

#[test]
fn missing_fixture_root_is_not_initialized() {
    let _serial = serial();
    let (parent, user) = fixture();
    let absent = parent.join("never-created");
    let error = inspect_existing(&absent, &user).err().unwrap();
    assert!(error.missing);
    assert!(!absent.exists());
    assert_eq!(fs::read_dir(parent).unwrap().count(), 0);
}

#[test]
fn private_root_metadata_leaves_control_bytes_and_runtime_token_untouched() {
    let _serial = serial();
    let (root, user) = fixture();
    let marker = b"synthetic marker; deliberately not parsed";
    let settings = b"synthetic settings; deliberately not parsed";
    fs::write(root.join(".environment.json"), marker).unwrap();
    fs::write(root.join("settings.json"), settings).unwrap();
    fs::create_dir(root.join("workspace")).unwrap();
    fs::create_dir(root.join("runtime")).unwrap();
    fs::write(root.join("runtime/token"), b"synthetic-disposable-token").unwrap();
    let counts = inspect_existing(&root, &user).unwrap();
    assert!(counts
        .iter()
        .any(|count| count.kind == "environmentMarker" && count.count == 1));
    assert!(counts
        .iter()
        .any(|count| count.kind == "workspaceEntries" && count.count == 0));
    assert!(counts
        .iter()
        .any(|count| count.kind == "runtimeDirectory" && count.count == 1));
    assert_eq!(fs::read(root.join(".environment.json")).unwrap(), marker);
    assert_eq!(fs::read(root.join("settings.json")).unwrap(), settings);
    assert_eq!(
        fs::read(root.join("runtime/token")).unwrap(),
        b"synthetic-disposable-token"
    );
    assert_eq!(fs::read_dir(&root).unwrap().count(), 4);
}

#[test]
fn known_metadata_hard_links_are_rejected_without_altering_either_name() {
    let _serial = serial();
    let (root, user) = fixture();
    let file = root.join(".environment.json");
    fs::write(&file, b"hard-linked synthetic evidence").unwrap();
    fs::hard_link(&file, root.join("alias")).unwrap();
    let error = inspect_existing(&root, &user).err().unwrap();
    assert_eq!(error.code, "HARD_LINK_REJECTED");
    assert_eq!(fs::read(file).unwrap(), b"hard-linked synthetic evidence");
    assert_eq!(
        fs::read(root.join("alias")).unwrap(),
        b"hard-linked synthetic evidence"
    );
}

#[test]
fn ancestor_handles_deny_renaming_the_observed_directory() {
    let _serial = serial();
    let (root, _) = fixture();
    let held = open_chain(&root).unwrap();
    let replacement = root.with_file_name(format!(
        "{}-moved",
        root.file_name().unwrap().to_string_lossy()
    ));
    assert!(fs::rename(&root, &replacement).is_err());
    assert!(root.is_dir());
    assert!(!replacement.exists());
    drop(held);
    fs::rename(&root, &replacement).unwrap();
}

#[test]
fn an_existing_metadata_writer_prevents_a_trusted_snapshot() {
    let _serial = serial();
    let (root, user) = fixture();
    let file = root.join("settings.json");
    fs::write(&file, b"unchanged synthetic settings").unwrap();
    let writer = fs::OpenOptions::new().write(true).open(&file).unwrap();
    let error = inspect_existing(&root, &user).err().unwrap();
    assert_eq!(error.code, "OBJECT_OPEN_DENIED");
    assert!(!error.missing);
    drop(writer);
    assert_eq!(fs::read(file).unwrap(), b"unchanged synthetic settings");
}

#[test]
fn permissive_fixture_acl_is_rejected_and_not_repaired() {
    let _serial = serial();
    let (parent, user) = fixture();
    let root = world_readable_directory(&parent, &user);
    let before = acl_text(&root);
    for _ in 0..2 {
        let error = inspect_existing(&root, &user).err().unwrap();
        assert_eq!(error.code, "PRIVATE_ACL_REJECTED");
    }
    assert!(
        before == acl_text(&root),
        "The probe changed its fixture's ACL"
    );
    assert_eq!(fs::read_dir(root).unwrap().count(), 0);
}

#[test]
fn a_known_directory_junction_is_rejected_without_following_its_payload() {
    let _serial = serial();
    use std::os::windows::process::CommandExt;
    let (root, user) = fixture();
    let (target, _) = fixture();
    fs::write(target.join("synthetic-evidence"), b"must remain unchanged").unwrap();
    let link = root.join("workspace");
    // mklink /J creates a junction for owned disposable directories without the
    // symbolic-link privilege. Only our generated paths enter this test shell.
    for path in [&link, &target] {
        assert!(path.to_string_lossy().chars().all(
            |ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '\\' | '/' | '.' | '-' | '_')
        ));
    }
    let command = format!("mklink /J {} {}", link.display(), target.display());
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", &command])
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Could not create the disposable junction fixture"
    );
    let error = inspect_existing(&root, &user).err().unwrap();
    assert_eq!(error.code, "EVIDENCE_TYPE_REJECTED");
    assert_eq!(
        fs::read(target.join("synthetic-evidence")).unwrap(),
        b"must remain unchanged"
    );
}

#[test]
fn shallow_enumeration_and_metadata_sizes_are_bounded() {
    let _serial = serial();
    let (root, user) = fixture();
    let file = fs::File::create(root.join("settings.json")).unwrap();
    file.set_len(METADATA_LIMIT as u64 + 1).unwrap();
    drop(file);
    assert_eq!(
        inspect_existing(&root, &user).err().unwrap().code,
        "METADATA_LIMIT"
    );
    let (many, user) = fixture();
    for index in 0..=ENTRY_LIMIT {
        fs::write(many.join(format!("synthetic-{index}")), []).unwrap();
    }
    assert_eq!(
        inspect_existing(&many, &user).err().unwrap().code,
        "ENUMERATION_LIMIT"
    );
}

#[test]
fn webview_profiles_are_fresh_private_and_never_adopt_existing_directories() {
    let _serial = serial();
    fn requires_send_sync<T: Send + Sync>() {}
    requires_send_sync::<WebviewCache>();
    let (parent, user) = fixture();
    let cache = cache_under(&parent).unwrap();
    let second = cache_under(&parent).unwrap();
    assert_ne!(cache.path(), second.path());
    assert!(cache.path().starts_with(&parent));
    assert!(cache
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("local-projects-dev-w1-webview-"));
    for profile in [&cache, &second] {
        let chain = open_chain(profile.path()).unwrap();
        private_acl(chain.last().unwrap(), &user).unwrap();
    }
    let preserved = cache.path().join("synthetic-existing-browser-state");
    fs::write(&preserved, b"do not adopt or reset").unwrap();
    assert_eq!(
        private_directory(cache.path(), &user).err().unwrap().code,
        "CACHE_CREATE_REJECTED"
    );
    assert_eq!(fs::read(preserved).unwrap(), b"do not adopt or reset");
}

#[test]
fn webview_profile_creation_rejects_a_reparse_parent_without_creating_in_its_target() {
    use std::os::windows::process::CommandExt;
    let _serial = serial();
    let (root, _) = fixture();
    let (target, _) = fixture();
    let link = root.join("synthetic-cache-parent-link");
    for path in [&link, &target] {
        assert!(path.to_string_lossy().chars().all(
            |ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '\\' | '/' | '.' | '-' | '_')
        ));
    }
    let command = format!("mklink /J {} {}", link.display(), target.display());
    let output = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", &command])
        .creation_flags(0x08000000)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "Could not create the disposable parent junction"
    );
    assert_eq!(
        cache_under(&link).err().unwrap().code,
        "REPARSE_POINT_REJECTED"
    );
    assert_eq!(fs::read_dir(target).unwrap().count(), 0);
}

#[test]
fn webview_profile_releases_creation_locks_for_atomic_file_and_directory_renames() {
    let _serial = serial();
    let (parent, _) = fixture();
    let cache = cache_under(&parent).unwrap();
    let original = cache.path().join("synthetic-cache-tmp");
    let published = cache.path().join("synthetic-cache-published");
    fs::write(&original, b"atomic browser file").unwrap();
    fs::rename(&original, &published)
        .expect("Creation-time checks left a lock blocking an atomic child-file rename");
    assert_eq!(fs::read(&published).unwrap(), b"atomic browser file");
    let directory = cache.path().join("synthetic-cache-directory-tmp");
    let renamed = cache.path().join("synthetic-cache-directory-published");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("child"), b"atomic browser directory").unwrap();
    fs::rename(&directory, &renamed)
        .expect("Creation-time checks left a lock blocking a child-directory rename");
    assert_eq!(
        fs::read(renamed.join("child")).unwrap(),
        b"atomic browser directory"
    );
    let writer = fs::OpenOptions::new().write(true).open(&published).unwrap();
    drop(writer);
    // Owning the profile path is not a lifetime directory lock or same-user
    // filesystem sandbox. Normal browser and other application renames work.
    let renamed_profile = parent.join("synthetic-renamed-profile");
    fs::rename(cache.path(), &renamed_profile).unwrap();
    let renamed_parent = parent.with_file_name(format!(
        "{}-moved",
        parent.file_name().unwrap().to_string_lossy()
    ));
    fs::rename(&parent, &renamed_parent).unwrap();
}

#[test]
fn cache_profile_is_private_without_repairing_its_readable_owned_parent() {
    let _serial = serial();
    let (base, user) = fixture();
    let parent = world_readable_directory(&base, &user);
    let existing = parent.join("synthetic-existing-data");
    fs::write(&existing, b"preserve the parent and existing file").unwrap();
    let before = acl_text(&parent);
    let cache = cache_under(&parent).unwrap();
    assert!(
        before == acl_text(&parent),
        "Cache creation changed the parent's ACL"
    );
    assert_eq!(
        fs::read(&existing).unwrap(),
        b"preserve the parent and existing file"
    );
    {
        let directory = open_object(cache.path(), true).unwrap();
        // The inherited Everyone ACE on the parent must not reach the profile.
        private_acl(&directory, &user).unwrap();
    }
    let child = cache.path().join("synthetic-tmp");
    let renamed = cache.path().join("synthetic-published");
    fs::write(&child, b"browser atomic write").unwrap();
    fs::rename(&child, &renamed).unwrap();
    let child_directory = cache.path().join("synthetic-directory-tmp");
    let final_directory = cache.path().join("synthetic-directory-published");
    fs::create_dir(&child_directory).unwrap();
    fs::rename(&child_directory, &final_directory).unwrap();
    assert_eq!(fs::read(renamed).unwrap(), b"browser atomic write");
    assert!(before == acl_text(&parent));
    assert_eq!(fs::read_dir(&parent).unwrap().count(), 2);
}

#[test]
fn owner_check_rejects_a_different_trusted_expected_sid_without_changing_acl() {
    let _serial = serial();
    let (root, user) = fixture();
    let before = acl_text(&root);
    let different = if user == "S-1-5-18" {
        "S-1-5-19"
    } else {
        "S-1-5-18"
    };
    let object = open_object(&root, true).unwrap();
    let failure = system_parent_security(&object, different).err().unwrap();
    assert_eq!(failure.code, "CACHE_PARENT_OWNER_REJECTED");
    assert!(failure
        .message
        .contains(&format!("expectedSid={different}")));
    assert!(failure.message.contains(&format!("observedSid={user}")));
    assert!(failure.message.contains("actualObject="));
    assert!(failure
        .message
        .contains(root.file_name().unwrap().to_str().unwrap()));
    drop(object);
    assert!(before == acl_text(&root));
}
