//! Removal of one already owned name. Reparse leaves are opened themselves;
//! traversal never follows them. No ACL or single-link admission is added.
use super::{checked, files, native_path};
use std::{
    fs::File,
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle},
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Storage::FileSystem::{
        CreateFileW, FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, OPEN_EXISTING,
    },
};

pub struct Entry {
    pub file: File,
    pub identity: files::FileIdentity,
    _parent: files::PinnedDirectory,
}

/// Open without acquiring deletion or content-write authority. This observes
/// reparse leaves without following their targets, like lstat in a Unix scan.
pub fn inspect(path: &Path) -> io::Result<Entry> {
    open(path, false)
}

fn open(path: &Path, deleting: bool) -> io::Result<Entry> {
    let parent = files::pin_directory(path.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "Removal entry has no parent")
    })?)?;
    let name = native_path(path)?;
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_READ_ATTRIBUTES
                | if deleting {
                    0x0001_0000 | FILE_WRITE_ATTRIBUTES
                } else {
                    0
                },
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
    let identity = files::identity(&file)?;
    Ok(Entry {
        file,
        identity,
        _parent: parent,
    })
}

/// The DELETE handle and complete FileId bind the operation to the selected
/// object. A shared hard link is not rejected, and its read-only attribute is
/// restored through the same handle after unlinking this name.
pub fn remove(path: &Path, expected: &files::FileIdentity) -> io::Result<()> {
    let entry = open(path, true)?;
    if !expected.same_object(&entry.identity) || expected.directory != entry.identity.directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Removal entry changed",
        ));
    }
    let permissions = entry.file.metadata()?.permissions();
    let read_only = permissions.readonly();
    if read_only {
        let mut writable = permissions.clone();
        // This module is compiled only on Windows: this toggles the native
        // FILE_ATTRIBUTE_READONLY bit and does not grant Unix write modes.
        #[allow(clippy::permissions_set_readonly_false)]
        writable.set_readonly(false);
        entry.file.set_permissions(writable)?;
    }
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    let result = unsafe {
        checked(SetFileInformationByHandle(
            entry.file.as_raw_handle(),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        ))
    };
    // A failed removal must not leave the admitted object writable. Likewise,
    // unrelated hard-link names retain their original attribute after success.
    if read_only && (result.is_err() || entry.identity.links > 1) {
        entry.file.set_permissions(permissions)?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::windows::fs::OpenOptionsExt};

    #[test]
    fn ordinary_read_only_names_and_empty_directories_are_actually_removed() {
        let tmp = tempfile::Builder::new()
            .prefix("panda-disposable-native-remove-")
            .tempdir()
            .unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let path = root.join("中文 readonly.txt");
        fs::write(&path, b"disposable").unwrap();
        files::set_read_only(&path, true).unwrap();
        let id = inspect(&path).unwrap().identity;
        remove(&path, &id).unwrap();
        assert!(!path.exists());
        let directory = root.join("empty");
        fs::create_dir(&directory).unwrap();
        let id = inspect(&directory).unwrap().identity;
        remove(&directory, &id).unwrap();
        assert!(!directory.exists());
    }

    #[test]
    fn replacements_and_open_files_are_preserved() {
        let tmp = tempfile::Builder::new()
            .prefix("panda-disposable-native-remove-bound-")
            .tempdir()
            .unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let path = root.join("owned");
        fs::write(&path, b"original").unwrap();
        let id = inspect(&path).unwrap().identity;
        fs::rename(&path, root.join("retained")).unwrap();
        fs::write(&path, b"replacement").unwrap();
        assert!(remove(&path, &id).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"replacement");
        let id = inspect(&path).unwrap().identity;
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert_eq!(remove(&path, &id).unwrap_err().raw_os_error(), Some(32));
        drop(lock);
        remove(&path, &id).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn unlinking_a_read_only_hard_link_preserves_the_other_name_and_attribute() {
        let tmp = tempfile::Builder::new()
            .prefix("panda-disposable-native-remove-hardlink-")
            .tempdir()
            .unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let original = root.join("external-original");
        let owned = root.join("owned-name");
        fs::write(&original, b"preserved original").unwrap();
        fs::hard_link(&original, &owned).unwrap();
        files::set_read_only(&original, true).unwrap();
        let id = inspect(&owned).unwrap().identity;
        remove(&owned, &id).unwrap();
        assert!(!owned.exists());
        assert_eq!(fs::read(&original).unwrap(), b"preserved original");
        assert!(fs::metadata(&original).unwrap().permissions().readonly());
        files::set_read_only(&original, false).unwrap();
    }
}
