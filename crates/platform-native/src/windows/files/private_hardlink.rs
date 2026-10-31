//! Repair only the admitted ordinary file object. This follows the original
//! chmod-on-opened-file contract; other names retain the same object and bytes.
use super::*;

pub fn open_owned_file_private_allowing_hard_links(
    path: &Path,
    expected: &FileIdentity,
) -> io::Result<File> {
    let _parent = pin_parent(path)?;
    let name = native_path(path)?;
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | READ_CONTROL | WRITE_DAC,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    reject_reparse(&file)?;
    let captured = identity(&file)?;
    if captured.directory || expected.directory || !expected.same_object(&captured) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Admitted ordinary file identity changed",
        ));
    }
    // Identity precedes any DACL change. No parent ACL or link-count admission.
    make_opened_object_private(&file, false)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn acl(path: &Path) -> Vec<u8> {
        let file = open_read_file_allowing_hard_links(path).unwrap();
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
        unsafe { std::slice::from_raw_parts(dacl.cast::<u8>(), (*dacl).AclSize as usize) }.to_vec()
    }
    #[test]
    fn actual_hardlink_names_keep_identity_and_bytes_after_bound_permission_repair() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("原件 甲");
        let second = root.path().join("原件 乙");
        std::fs::write(&first, b"disposable original").unwrap();
        std::fs::hard_link(&first, &second).unwrap();
        let expected = open_no_reparse(&first).unwrap().identity;
        let file = open_owned_file_private_allowing_hard_links(&first, &expected).unwrap();
        verify_private(&file).unwrap();
        assert!(expected.same_object(&identity(&file).unwrap()));
        drop(file);
        assert!(expected.same_object(&open_no_reparse(&second).unwrap().identity));
        assert_eq!(std::fs::read(first).unwrap(), b"disposable original");
        assert_eq!(std::fs::read(second).unwrap(), b"disposable original");
    }
    #[test]
    fn wrong_full_identity_preserves_foreign_acl_and_bytes() {
        let root = tempfile::tempdir().unwrap();
        let expected = root.path().join("admitted");
        let foreign = root.path().join("foreign");
        std::fs::write(&expected, b"admitted").unwrap();
        std::fs::write(&foreign, b"foreign").unwrap();
        let expected = open_no_reparse(&expected).unwrap().identity;
        let before = acl(&foreign);
        assert!(open_owned_file_private_allowing_hard_links(&foreign, &expected).is_err());
        assert_eq!(acl(&foreign), before);
        assert_eq!(std::fs::read(foreign).unwrap(), b"foreign");
    }
}
