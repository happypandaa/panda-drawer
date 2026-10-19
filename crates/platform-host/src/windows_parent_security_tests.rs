use super::*;
use std::{
    fs,
    os::windows::{
        ffi::OsStrExt,
        fs::MetadataExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::null,
};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Security::{
        Authorization::{
            ConvertSecurityDescriptorToStringSecurityDescriptorW,
            ConvertStringSecurityDescriptorToSecurityDescriptorW,
        },
        GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    },
    Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    },
    System::{
        Com::CoCreateGuid,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

const USER: &str = "S-1-5-21-1-2-3-1001";
const OTHER: &str = "S-1-5-21-1-2-3-1002";

#[test]
fn only_current_system_or_builtin_administrators_may_own_a_system_parent() {
    for owner in [USER, SYSTEM, ADMINISTRATORS] {
        owner_rule(owner, USER).unwrap();
    }
    for owner in [OTHER, "S-1-1-0", "S-1-5-32-545"] {
        assert_eq!(
            owner_rule(owner, USER).unwrap_err().code,
            "SYSTEM_PARENT_OWNER_REJECTED"
        );
    }
}

#[test]
fn foreign_grants_are_restricted_to_read_only_bits_including_all_unknown_bits() {
    for mask in [0, READ_ONLY, 0x8000_0000, 0x2000_0000] {
        ace_rule(0, 0, mask, OTHER, USER).unwrap();
    }
    for bit in 0..32 {
        let mask = 1u32 << bit;
        if mask & READ_ONLY == 0 {
            assert_eq!(
                ace_rule(0, 0, mask, OTHER, USER).unwrap_err().code,
                "SYSTEM_PARENT_ACL_REJECTED"
            );
        }
    }
    for sid in [USER, SYSTEM, ADMINISTRATORS] {
        ace_rule(0, 0, u32::MAX, sid, USER).unwrap();
    }
}

#[test]
fn parent_inherit_only_and_deny_do_not_grant_access_but_unknown_aces_stay_closed() {
    ace_rule(0, INHERIT_ONLY | 3, u32::MAX, OTHER, USER).unwrap();
    ace_rule(1, 0, u32::MAX, OTHER, USER).unwrap();
    ace_rule(0, 0x10, READ_ONLY, OTHER, USER).unwrap();
    for kind in [2, 5, 9, 255] {
        assert_eq!(
            ace_rule(kind, INHERIT_ONLY, 0, USER, USER)
                .unwrap_err()
                .code,
            "SYSTEM_PARENT_ACL_UNSUPPORTED"
        );
    }
    assert_eq!(
        ace_rule(0, 0x20, 0, USER, USER).unwrap_err().code,
        "SYSTEM_PARENT_ACL_UNSUPPORTED"
    );
}

fn wide(path: &std::ffi::OsStr) -> Vec<u16> {
    path.encode_wide().chain(Some(0)).collect()
}
fn current_sid() -> String {
    let mut raw = null_mut();
    assert_ne!(
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) },
        0
    );
    let token = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut needed = 0;
    unsafe {
        GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut needed);
    }
    assert!((size_of::<TOKEN_USER>() as u32..=65536).contains(&needed));
    let mut storage = vec![0u64; (needed as usize).div_ceil(8)];
    assert_ne!(
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                storage.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        },
        0
    );
    sid_text(unsafe { (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid }).unwrap()
}
fn fixture(extra_ace: &str) -> (PathBuf, OwnedHandle, String) {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let artifacts = repository.join(".test-artifacts");
    fs::create_dir_all(&artifacts).unwrap();
    assert_eq!(
        fs::symlink_metadata(&artifacts).unwrap().file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT,
        0
    );
    let mut guid = windows_sys::core::GUID::default();
    assert!(unsafe { CoCreateGuid(&mut guid) } >= 0);
    let path = artifacts.join(format!(
        "windows-system-parent-disposable-{:08x}{:04x}{:04x}{}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ));
    let user = current_sid();
    let sddl = wide(std::ffi::OsStr::new(&format!(
        "O:{user}D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA){extra_ace}"
    )));
    let mut descriptor = null_mut();
    assert_ne!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                null_mut(),
            )
        },
        0
    );
    let allocation = Allocation(descriptor);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let name = wide(path.as_os_str());
    assert_ne!(unsafe { CreateDirectoryW(name.as_ptr(), &attributes) }, 0);
    drop(allocation);
    fs::write(
        path.join("retained-evidence.txt"),
        b"new disposable ACL evidence",
    )
    .unwrap();
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            READ_CONTROL | FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    assert_ne!(raw, INVALID_HANDLE_VALUE);
    (path, unsafe { OwnedHandle::from_raw_handle(raw) }, user)
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
    let mut text = null_mut();
    let mut length = 0;
    assert_ne!(
        unsafe {
            ConvertSecurityDescriptorToStringSecurityDescriptorW(
                descriptor,
                1,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut text,
                &mut length,
            )
        },
        0
    );
    let converted = Allocation(text.cast());
    assert!(length > 0 && length < 32768);
    let result =
        String::from_utf16(unsafe { std::slice::from_raw_parts(text, length as usize - 1) })
            .unwrap();
    drop(converted);
    drop(allocation);
    result
}

#[test]
fn native_parent_allows_foreign_read_or_inherit_only_without_rewriting_acl_or_bytes() {
    let _serial = crate::WINDOWS_FILESYSTEM_TESTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for ace in ["(A;;GR;;;WD)", "(A;OICIIO;FA;;;WD)"] {
        let (path, handle, user) = fixture(ace);
        let before = acl_text(handle.as_raw_handle());
        let verified = verify_system_parent(handle.as_raw_handle(), &user).unwrap();
        assert_eq!(verified.owner_sid, user);
        assert_eq!(acl_text(handle.as_raw_handle()), before);
        assert_eq!(
            fs::read(path.join("retained-evidence.txt")).unwrap(),
            b"new disposable ACL evidence"
        );
    }
}

#[test]
fn native_parent_rejects_foreign_mutation_without_repairing_acl_or_bytes() {
    let _serial = crate::WINDOWS_FILESYSTEM_TESTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for ace in ["(A;;0x00000040;;;WD)", "(A;;WD;;;WD)", "(A;;GW;;;WD)"] {
        let (path, handle, user) = fixture(ace);
        let before = acl_text(handle.as_raw_handle());
        assert_eq!(
            verify_system_parent(handle.as_raw_handle(), &user)
                .unwrap_err()
                .code,
            "SYSTEM_PARENT_ACL_REJECTED"
        );
        assert_eq!(acl_text(handle.as_raw_handle()), before);
        assert_eq!(
            fs::read(path.join("retained-evidence.txt")).unwrap(),
            b"new disposable ACL evidence"
        );
    }
}
