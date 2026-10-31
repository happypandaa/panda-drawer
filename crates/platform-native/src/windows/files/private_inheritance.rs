//! A browser creates its own descendants. Its already-authorized private root
//! must pass the same current-user access to those descendants; no other
//! principal or ancestor ACL is changed here.
use super::*;

const CHILD_INHERITANCE: u8 = 0x01 | 0x02; // OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE

/// Enable current-user inheritance on this exact, already-private directory.
/// The caller supplies its previously bound complete identity. Returns true
/// only if its DACL needed a change; an already-correct DACL is not rewritten.
pub fn enable_private_directory_inheritance(
    path: &Path,
    expected: &FileIdentity,
) -> io::Result<bool> {
    if !expected.directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected a bound private directory",
        ));
    }
    let scope = pin_directory(path)?;
    let pinned = identity(&scope.file)?;
    if !pinned.directory || !expected.same_object(&pinned) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Private directory identity changed before inheritance preparation",
        ));
    }
    verify_private(&scope.file)?;
    let name = native_path(path)?;
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    reject_reparse(&file)?;
    let actual = identity(&file)?;
    if !actual.directory || !expected.same_object(&actual) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Opened private directory does not match its bound identity",
        ));
    }
    // Validate owner, protection, principal, rights and flags from one security
    // snapshot. A separate flag read must not bless a nonprivate replacement ACL.
    if private_flags(&file)? == CHILD_INHERITANCE {
        return Ok(false);
    }
    let security = inheritable_security()?;
    let (mut present, mut defaulted, mut dacl) = (0, 0, null_mut());
    unsafe {
        checked(GetSecurityDescriptorDacl(
            security.allocation.0,
            &mut present,
            &mut dacl,
            &mut defaulted,
        ))?;
    }
    if present == 0 || dacl.is_null() {
        return Err(io::Error::other("Private directory DACL is absent"));
    }
    let result = unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            dacl,
            null(),
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    verify_private(&file)?;
    if private_flags(&file)? != CHILD_INHERITANCE {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Private directory inheritance could not be confirmed",
        ));
    }
    Ok(true)
}

fn inheritable_security() -> io::Result<PrivateSecurity> {
    let sid = CurrentUser::load()?.text()?;
    let sddl = wide(OsStr::new(&format!("O:{sid}D:P(A;OICI;FA;;;{sid})")))?;
    let mut descriptor = null_mut();
    unsafe {
        checked(ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        ))?;
    }
    Ok(PrivateSecurity {
        allocation: LocalAllocation(descriptor),
    })
}

// The admission is exactly verify_private's existing policy. Keep the flags in
// that same snapshot rather than adding a broader private-directory policy.
fn private_flags(file: &File) -> io::Result<u8> {
    let current = CurrentUser::load()?;
    let (mut owner, mut dacl, mut descriptor) = (null_mut(), null_mut::<ACL>(), null_mut());
    let result = unsafe {
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
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let _allocation = LocalAllocation(descriptor);
    let (mut control, mut revision) = (0u16, 0);
    unsafe {
        checked(GetSecurityDescriptorControl(
            descriptor,
            &mut control,
            &mut revision,
        ))?;
    }
    if owner.is_null()
        || unsafe { EqualSid(owner, current.sid()) } == 0
        || dacl.is_null()
        || control & SE_DACL_PROTECTED == 0
        || unsafe { (*dacl).AceCount } != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Expected current-user ownership and a protected private DACL",
        ));
    }
    let mut raw = null_mut();
    unsafe {
        checked(GetAce(dacl, 0, &mut raw))?;
    }
    let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
    if ace.Header.AceType != ACCESS_ALLOWED_ACE_TYPE
        || ace.Mask & 0x001f_01ff != 0x001f_01ff
        || unsafe {
            EqualSid(
                (&ace.SidStart as *const u32).cast_mut().cast(),
                current.sid(),
            )
        } == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Expected one current-user full-control ACE",
        ));
    }
    Ok(ace.Header.AceFlags)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dacl_bytes(file: &File) -> Vec<u8> {
        let (mut dacl, mut descriptor) = (null_mut::<ACL>(), null_mut());
        let result = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(result, 0);
        let _allocation = LocalAllocation(descriptor);
        assert!(!dacl.is_null());
        unsafe { std::slice::from_raw_parts(dacl.cast::<u8>(), (*dacl).AclSize as usize).to_vec() }
    }
    fn inherited_current_user_only(file: &File) {
        let current = CurrentUser::load().unwrap();
        let (mut dacl, mut descriptor) = (null_mut::<ACL>(), null_mut());
        assert_eq!(
            unsafe {
                GetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    &mut dacl,
                    null_mut(),
                    &mut descriptor,
                )
            },
            0
        );
        let _allocation = LocalAllocation(descriptor);
        assert!(!dacl.is_null());
        assert_eq!(unsafe { (*dacl).AceCount }, 1);
        let mut raw = null_mut();
        unsafe {
            checked(GetAce(dacl, 0, &mut raw)).unwrap();
        }
        let ace = unsafe { &*raw.cast::<ACCESS_ALLOWED_ACE>() };
        assert_eq!(ace.Header.AceType, ACCESS_ALLOWED_ACE_TYPE);
        assert_eq!(ace.Mask & 0x001f_01ff, 0x001f_01ff);
        assert_ne!(ace.Header.AceFlags & 0x10, 0); // INHERITED_ACE
        assert_ne!(
            unsafe {
                EqualSid(
                    (&ace.SidStart as *const u32).cast_mut().cast(),
                    current.sid(),
                )
            },
            0
        );
    }
    #[test]
    fn selected_private_root_inherits_only_its_user_and_is_not_rewritten_on_reopen() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-private-inheritance-")
            .tempdir()
            .unwrap();
        let parent = open_no_reparse(temporary.path()).unwrap();
        let parent_acl = dacl_bytes(&parent.file);
        let root = temporary.path().join("selected-browser-root");
        create_private_directory(&root).unwrap();
        let selected = open_no_reparse(&root).unwrap();
        assert_eq!(private_flags(&selected.file).unwrap(), 0);
        assert!(enable_private_directory_inheritance(&root, &selected.identity).unwrap());
        assert_eq!(private_flags(&selected.file).unwrap(), CHILD_INHERITANCE);
        verify_private(&selected.file).unwrap();
        let inherited = root.join("ordinary-engine-file");
        std::fs::write(&inherited, b"disposable engine bytes").unwrap();
        inherited_current_user_only(&open_no_reparse(&inherited).unwrap().file);
        let inherited_directory = root.join("ordinary-engine-directory");
        std::fs::create_dir(&inherited_directory).unwrap();
        inherited_current_user_only(&open_no_reparse(&inherited_directory).unwrap().file);
        let secret = root.join("explicit-private-slot");
        let slot = create_private_file(&secret).unwrap();
        verify_private(&slot).unwrap();
        assert_eq!(private_flags(&slot).unwrap(), 0);
        let (root_acl, slot_acl) = (dacl_bytes(&selected.file), dacl_bytes(&slot));
        assert!(!enable_private_directory_inheritance(&root, &selected.identity).unwrap());
        assert_eq!(dacl_bytes(&selected.file), root_acl);
        assert_eq!(dacl_bytes(&slot), slot_acl);
        assert_eq!(dacl_bytes(&parent.file), parent_acl);
        assert_eq!(
            std::fs::read(inherited).unwrap(),
            b"disposable engine bytes"
        );
    }
    #[test]
    fn replacement_and_nonprivate_roots_are_preserved_without_acl_repair() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-private-inheritance-reject-")
            .tempdir()
            .unwrap();
        let root = temporary.path().join("selected");
        create_private_directory(&root).unwrap();
        let expected = open_no_reparse(&root).unwrap().identity;
        std::fs::rename(&root, temporary.path().join("held-original")).unwrap();
        create_private_directory(&root).unwrap();
        std::fs::write(root.join("sentinel"), b"retain replacement").unwrap();
        let replacement = open_no_reparse(&root).unwrap();
        let replacement_acl = dacl_bytes(&replacement.file);
        assert!(enable_private_directory_inheritance(&root, &expected).is_err());
        assert_eq!(dacl_bytes(&replacement.file), replacement_acl);
        assert_eq!(
            std::fs::read(root.join("sentinel")).unwrap(),
            b"retain replacement"
        );
        let ordinary = temporary.path().join("ordinary");
        std::fs::create_dir(&ordinary).unwrap();
        let ordinary = open_no_reparse(&ordinary).unwrap();
        let ordinary_acl = dacl_bytes(&ordinary.file);
        assert!(enable_private_directory_inheritance(
            &temporary.path().join("ordinary"),
            &ordinary.identity
        )
        .is_err());
        assert_eq!(dacl_bytes(&ordinary.file), ordinary_acl);
    }
}
