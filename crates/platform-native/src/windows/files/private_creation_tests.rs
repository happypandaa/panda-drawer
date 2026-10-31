//! Ordinary subprocess descendants must keep the same current-user access as
//! their newly-created private parent. These fixtures never use product paths.
use super::*;
use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const CHILD: &str =
    "windows::files::private_creation_tests::private_directory_external_child_process";
const ROOT_VARIABLE: &str = "PANDA_DISPOSABLE_PRIVATE_CREATION_FIXTURE";
const CONTENT: &[u8] = b"ordinary disposable child output";

struct Access {
    bytes: Vec<u8>,
    protected: bool,
    flags: u8,
}

fn current_user_only(file: &File) -> Access {
    let current = CurrentUser::load().unwrap();
    let (mut owner, mut dacl, mut descriptor) = (null_mut(), null_mut::<ACL>(), null_mut());
    assert_eq!(
        unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        },
        0
    );
    let _allocation = LocalAllocation(descriptor);
    let (mut control, mut revision) = (0u16, 0);
    unsafe {
        checked(GetSecurityDescriptorControl(
            descriptor,
            &mut control,
            &mut revision,
        ))
        .unwrap();
    }
    assert!(!owner.is_null());
    assert_ne!(unsafe { EqualSid(owner, current.sid()) }, 0);
    assert!(!dacl.is_null());
    assert_eq!(unsafe { (*dacl).AceCount }, 1);
    let mut raw = null_mut();
    unsafe { checked(GetAce(dacl, 0, &mut raw)).unwrap() };
    let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
    assert_eq!(ace.Header.AceType, ACCESS_ALLOWED_ACE_TYPE);
    assert_eq!(ace.Mask, 0x001f_01ff);
    assert_ne!(
        unsafe {
            EqualSid(
                (&ace.SidStart as *const u32).cast_mut().cast(),
                current.sid(),
            )
        },
        0
    );
    Access {
        bytes: unsafe { std::slice::from_raw_parts(dacl.cast::<u8>(), (*dacl).AclSize as usize) }
            .to_vec(),
        protected: control & SE_DACL_PROTECTED != 0,
        flags: ace.Header.AceFlags,
    }
}

fn descriptor_bytes(file: &File) -> Vec<u8> {
    let mut descriptor = null_mut();
    assert_eq!(
        unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
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
    let _allocation = LocalAllocation(descriptor);
    let size = unsafe { windows_sys::Win32::Security::GetSecurityDescriptorLength(descriptor) };
    assert!(size > 0);
    unsafe { std::slice::from_raw_parts(descriptor.cast::<u8>(), size as usize).to_vec() }
}

#[test]
fn newly_created_private_directory_passes_only_its_user_to_real_subprocess_descendants() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-private-creation-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().join(".platform");
    let parent = open_no_reparse(temporary.path()).unwrap();
    let parent_before = descriptor_bytes(&parent.file);
    create_private_directory(&root).unwrap();
    let created = open_no_reparse(&root).unwrap();
    let access = current_user_only(&created.file);
    assert!(access.protected);
    assert_eq!(access.flags, 0x01 | 0x02);
    verify_private(&created.file).unwrap();

    let log_path = temporary.path().join("disposable-child.log");
    let log = File::create(&log_path).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            CHILD,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ROOT_VARIABLE, &root)
        // libtest's terminal probe can contact a root-relative UNC terminfo
        // path. This test-only environment change does not affect product jobs.
        .env_remove("TERM")
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.is_some_and(|status| status.success()),
        "Real private-directory child failed: {status:?}: {}",
        std::fs::read_to_string(log_path).unwrap()
    );
    for relative in ["tasks", "tasks/disposable-task", "cache", "cache/npm"] {
        let inherited = open_no_reparse(&root.join(relative)).unwrap();
        assert!(inherited.identity.directory);
        let access = current_user_only(&inherited.file);
        assert!(!access.protected);
        assert_eq!(access.flags, 0x01 | 0x02 | 0x10);
    }
    for relative in ["tasks/disposable-task/output.txt", "cache/npm/result.txt"] {
        let path = root.join(relative);
        assert_eq!(std::fs::read(&path).unwrap(), CONTENT);
        let inherited = open_no_reparse(&path).unwrap();
        assert!(!inherited.identity.directory);
        let access = current_user_only(&inherited.file);
        assert!(!access.protected);
        assert_eq!(access.flags, 0x10);
        std::fs::write(&path, b"parent can update disposable child").unwrap();
    }
    assert_eq!(descriptor_bytes(&parent.file), parent_before);
    assert_eq!(current_user_only(&created.file).bytes, access.bytes);
}

#[test]
#[ignore = "Exact finite subprocess fixture, launched only with a new disposable root"]
fn private_directory_external_child_process() {
    let root = PathBuf::from(std::env::var_os(ROOT_VARIABLE).expect("Missing disposable root"));
    assert_eq!(root.file_name().unwrap(), ".platform");
    assert!(root
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("panda-disposable-private-creation-"));
    std::fs::create_dir_all(root.join("tasks/disposable-task")).unwrap();
    std::fs::create_dir_all(root.join("cache/npm")).unwrap();
    for relative in ["tasks/disposable-task/output.txt", "cache/npm/result.txt"] {
        let path = root.join(relative);
        std::fs::write(&path, CONTENT).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), CONTENT);
    }
}

#[test]
fn occupied_directory_and_existing_leaf_policy_keep_their_original_security() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-private-creation-preserve-")
        .tempdir()
        .unwrap();
    let root = temporary.path().join("new-private-directory");
    create_private_directory(&root).unwrap();
    let secret = create_private_file(&root.join("explicit-credential-slot")).unwrap();
    let secret_before = current_user_only(&secret);
    assert!(secret_before.protected);
    assert_eq!(secret_before.flags, 0);
    verify_private(&secret).unwrap();
    let existing = root.join("existing-child");
    std::fs::create_dir(&existing).unwrap();
    std::fs::write(existing.join("sentinel"), CONTENT).unwrap();
    let root_handle = open_no_reparse(&root).unwrap();
    let child_handle = open_no_reparse(&existing).unwrap();
    let root_before = descriptor_bytes(&root_handle.file);
    let child_before = descriptor_bytes(&child_handle.file);
    assert_eq!(
        create_private_directory(&root).unwrap_err().kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(descriptor_bytes(&root_handle.file), root_before);
    assert_eq!(descriptor_bytes(&child_handle.file), child_before);
    assert_eq!(current_user_only(&secret).bytes, secret_before.bytes);
    assert_eq!(std::fs::read(existing.join("sentinel")).unwrap(), CONTENT);

    // The existing selected-leaf operation retains its original non-inheriting
    // policy; this change affects only newly-created directory construction.
    let selected = temporary.path().join("existing-selected-empty-leaf");
    std::fs::create_dir(&selected).unwrap();
    make_directory_private(&selected).unwrap();
    let selected = open_no_reparse(&selected).unwrap();
    let access = current_user_only(&selected.file);
    assert!(access.protected);
    assert_eq!(access.flags, 0);
    assert_eq!(current_user_only(&secret).bytes, secret_before.bytes);
}

struct PreservedObject {
    path: PathBuf,
    identity: FileIdentity,
    security: Vec<u8>,
    content: Option<Vec<u8>>,
}

fn native_descriptor_bytes(file: &File) -> Vec<u8> {
    use windows_sys::Wdk::Storage::FileSystem::NtQuerySecurityObject;
    let information = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut needed = 0;
    unsafe {
        NtQuerySecurityObject(
            file.as_raw_handle(),
            information,
            null_mut(),
            0,
            &mut needed,
        );
    }
    assert!(needed > 0);
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    let status = unsafe {
        NtQuerySecurityObject(
            file.as_raw_handle(),
            information,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * size_of::<usize>()) as u32,
            &mut needed,
        )
    };
    assert!(status >= 0, "NtQuerySecurityObject returned {status:#x}");
    // GetSecurityInfo can relabel inherited ACEs in its returned descriptor
    // when the parent's inheritance flags change. Compare the object's actual
    // native descriptor, including all its original control and ACE bytes.
    unsafe { std::slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), needed as usize) }.to_vec()
}

impl PreservedObject {
    fn capture(path: &Path) -> Self {
        let opened = open_no_reparse(path).unwrap();
        Self {
            path: path.to_path_buf(),
            identity: opened.identity.clone(),
            security: native_descriptor_bytes(&opened.file),
            content: (!opened.identity.directory).then(|| std::fs::read(path).unwrap()),
        }
        // No handle survives this snapshot. Later comparisons reopen each
        // original name and verify its complete captured identity again.
    }

    fn assert_unchanged(&self) {
        let opened = open_no_reparse(&self.path).unwrap();
        assert_eq!(opened.identity, self.identity, "{}", self.path.display());
        assert_eq!(
            native_descriptor_bytes(&opened.file),
            self.security,
            "{}",
            self.path.display()
        );
        if let Some(content) = &self.content {
            assert_eq!(std::fs::read(&self.path).unwrap(), *content);
        }
    }
}

#[test]
fn selected_leaf_privacy_preserves_inherited_descendant_security_identity_and_content() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-private-leaf-preserve-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().join("new-private-parent");
    create_private_directory(&root).unwrap();
    let selected = root.join("ordinary-selected-leaf");
    std::fs::create_dir(&selected).unwrap();
    let child = selected.join("ordinary-child.txt");
    std::fs::write(&child, CONTENT).unwrap();
    let nested = selected.join("ordinary-child-directory");
    std::fs::create_dir(&nested).unwrap();
    let grandchild = nested.join("ordinary-grandchild.txt");
    std::fs::write(&grandchild, CONTENT).unwrap();
    let sibling = root.join("ordinary-sibling.txt");
    std::fs::write(&sibling, CONTENT).unwrap();
    let credential = root.join("explicit-credential-slot");
    drop(create_private_file(&credential).unwrap());
    std::fs::write(&credential, CONTENT).unwrap();

    let selected_identity = {
        let opened = open_no_reparse(&selected).unwrap();
        let access = current_user_only(&opened.file);
        assert!(!access.protected);
        assert_eq!(access.flags, 0x01 | 0x02 | 0x10);
        assert!(verify_private(&opened.file).is_err());
        opened.identity.clone()
    };
    for (path, flags) in [(&child, 0x10), (&nested, 0x01 | 0x02 | 0x10)] {
        let opened = open_no_reparse(path).unwrap();
        let access = current_user_only(&opened.file);
        assert!(!access.protected);
        assert_eq!(access.flags, flags);
    }
    let preserved = [
        temporary.path(),
        &root,
        &child,
        &nested,
        &grandchild,
        &sibling,
        &credential,
    ]
    .into_iter()
    .map(PreservedObject::capture)
    .collect::<Vec<_>>();

    // This existing platform pin must not become a new permission-repair
    // admission. The original WRITE_DAC handle still updates this exact leaf.
    let selected_pin = pin_directory(&selected).unwrap();
    make_directory_private(&selected).unwrap();
    {
        let opened = open_no_reparse(&selected).unwrap();
        assert_eq!(opened.identity, selected_identity);
        verify_private(&opened.file).unwrap();
        let access = current_user_only(&opened.file);
        assert!(access.protected);
        assert_eq!(access.flags, 0);
    }
    assert_eq!(identity(&selected_pin.file).unwrap(), selected_identity);
    for object in &preserved {
        object.assert_unchanged();
    }
    let private_selected = PreservedObject::capture(&selected);
    make_directory_private(&selected).unwrap();
    private_selected.assert_unchanged();
    for object in &preserved {
        object.assert_unchanged();
    }
}

#[test]
fn directory_repair_without_write_dac_reports_access_denied_and_preserves_every_object() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-private-directory-no-write-dac-")
        .tempdir()
        .unwrap();
    let root = temporary.path().join("new-private-parent");
    create_private_directory(&root).unwrap();
    let selected = root.join("ordinary-selected-leaf");
    std::fs::create_dir(&selected).unwrap();
    let child = selected.join("ordinary-child.txt");
    std::fs::write(&child, CONTENT).unwrap();
    let preserved = [temporary.path(), &root, &selected, &child]
        .into_iter()
        .map(PreservedObject::capture)
        .collect::<Vec<_>>();
    let readonly = open_no_reparse(&selected).unwrap();
    assert!(verify_private(&readonly.file).is_err());
    let error = make_opened_object_private(&readonly.file, true).unwrap_err();
    assert_eq!(error.raw_os_error(), Some(5));
    for object in &preserved {
        object.assert_unchanged();
    }
}

#[test]
fn private_directory_rechecks_keep_inheritance_and_descendants_while_it_is_pinned() {
    let temporary = tempfile::Builder::new()
        .prefix("panda-disposable-private-directory-recheck-中文 space-")
        .tempdir()
        .unwrap();
    let root = temporary.path().join("new-private-directory");
    create_private_directory(&root).unwrap();
    let child_directory = root.join("ordinary-child-directory");
    std::fs::create_dir(&child_directory).unwrap();
    let child = child_directory.join("ordinary-child.txt");
    std::fs::write(&child, CONTENT).unwrap();
    let sibling = temporary.path().join("ordinary-sibling.txt");
    std::fs::write(&sibling, CONTENT).unwrap();
    let credential = root.join("explicit-credential-slot");
    drop(create_private_file(&credential).unwrap());
    std::fs::write(&credential, CONTENT).unwrap();
    let preserved = [
        temporary.path(),
        &root,
        &child_directory,
        &child,
        &sibling,
        &credential,
    ]
    .into_iter()
    .map(PreservedObject::capture)
    .collect::<Vec<_>>();

    let pinned = pin_directory(&root).unwrap();
    for _ in 0..3 {
        make_directory_private(&root).unwrap();
        verify_private(&pinned.file).unwrap();
        let access = current_user_only(&pinned.file);
        assert!(access.protected);
        assert_eq!(access.flags, 0x01 | 0x02);
        for object in &preserved {
            object.assert_unchanged();
        }
    }
}
