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
        CreateFileW, FileDispositionInfoEx, SetFileInformationByHandle,
        FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
        FILE_DISPOSITION_INFO_EX, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
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
            FILE_READ_ATTRIBUTES | if deleting { 0x0001_0000 } else { 0 },
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
/// object. Ignore the read-only attribute for this deletion request without
/// changing it on the file or any of its other hard-link names. Standard
/// delete-pending semantics remain in effect; success is not an absence receipt.
pub fn remove(path: &Path, expected: &files::FileIdentity) -> io::Result<()> {
    remove_observing(path, expected, |_| {})
}

fn remove_observing(
    path: &Path,
    expected: &files::FileIdentity,
    mut observe: impl FnMut(&File),
) -> io::Result<()> {
    let entry = open(path, true)?;
    if !expected.same_object(&entry.identity) || expected.directory != entry.identity.directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Removal entry changed",
        ));
    }
    observe(&entry.file);
    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE,
    };
    let result = unsafe {
        checked(SetFileInformationByHandle(
            entry.file.as_raw_handle(),
            FileDispositionInfoEx,
            (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        ))
    };
    observe(&entry.file);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::windows::fs::{MetadataExt, OpenOptionsExt},
    };

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
        files::set_read_only(&path, true).unwrap();
        let id = inspect(&path).unwrap().identity;
        let lock = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert_eq!(remove(&path, &id).unwrap_err().raw_os_error(), Some(32));
        assert!(fs::metadata(&path).unwrap().permissions().readonly());
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
        let original_handle = inspect(&original).unwrap();
        let owned_handle = inspect(&owned).unwrap();
        let original_attributes = original_handle.file.metadata().unwrap().file_attributes();
        let mut observations = 0;
        remove_observing(&owned, &id, |deleting| {
            observations += 1;
            for file in [&original_handle.file, &owned_handle.file, deleting] {
                let metadata = file.metadata().unwrap();
                assert!(metadata.permissions().readonly());
                assert_eq!(metadata.file_attributes(), original_attributes);
                assert!(id.same_object(&files::identity(file).unwrap()));
            }
        })
        .unwrap();
        assert_eq!(observations, 2);
        // Normal disposition can remain pending while these observers hold the
        // file. They are released before testing the selected name's absence.
        drop(owned_handle);
        drop(original_handle);
        assert!(!owned.exists());
        assert_eq!(fs::read(&original).unwrap(), b"preserved original");
        assert!(fs::metadata(&original).unwrap().permissions().readonly());
        files::set_read_only(&original, false).unwrap();
    }

    #[test]
    fn nonempty_readonly_directory_failure_preserves_attributes_and_contents() {
        let tmp = tempfile::Builder::new()
            .prefix("panda-disposable-native-remove-nonempty-")
            .tempdir()
            .unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let directory = root.join("readonly-nonempty");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("sentinel"), b"preserve nonempty content").unwrap();
        files::set_read_only(&directory, true).unwrap();
        let opened = inspect(&directory).unwrap();
        let attributes = opened.file.metadata().unwrap().file_attributes();
        let mut observations = 0;
        let error = remove_observing(&directory, &opened.identity, |deleting| {
            observations += 1;
            for file in [&opened.file, deleting] {
                assert_eq!(file.metadata().unwrap().file_attributes(), attributes);
                assert!(file.metadata().unwrap().permissions().readonly());
            }
        })
        .unwrap_err();
        assert_eq!(observations, 2);
        assert_eq!(error.raw_os_error(), Some(145));
        assert!(opened
            .identity
            .same_object(&inspect(&directory).unwrap().identity));
        assert_eq!(
            fs::read(directory.join("sentinel")).unwrap(),
            b"preserve nonempty content"
        );
        assert_eq!(
            fs::metadata(&directory).unwrap().file_attributes(),
            attributes
        );
        drop(opened);
        files::set_read_only(&directory, false).unwrap();
    }

    #[test]
    fn a_hardlink_added_after_open_keeps_its_original_readonly_attribute() {
        let tmp = tempfile::Builder::new()
            .prefix("panda-disposable-native-remove-late-hardlink-")
            .tempdir()
            .unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let owned = root.join("owned-name");
        let external = root.join("late-external-name");
        fs::write(&owned, b"preserve late hardlink").unwrap();
        files::set_read_only(&owned, true).unwrap();
        let identity = inspect(&owned).unwrap().identity;
        assert_eq!(identity.links, 1);
        let mut external_handle = None;
        let mut observations = 0;
        remove_observing(&owned, &identity, |deleting| {
            if observations == 0 {
                // The deletion handle and its identity have already been
                // captured. Link count is not a stable no-sharing guarantee.
                fs::hard_link(&owned, &external).unwrap();
                external_handle = Some(inspect(&external).unwrap());
            }
            observations += 1;
            for file in [&external_handle.as_ref().unwrap().file, deleting] {
                assert!(file.metadata().unwrap().permissions().readonly());
                assert!(identity.same_object(&files::identity(file).unwrap()));
            }
        })
        .unwrap();
        assert_eq!(observations, 2);
        drop(external_handle);
        assert!(!owned.exists());
        assert_eq!(fs::read(&external).unwrap(), b"preserve late hardlink");
        assert!(fs::metadata(&external).unwrap().permissions().readonly());
        files::set_read_only(&external, false).unwrap();
    }

    #[test]
    fn junction_name_is_removed_without_changing_or_entering_its_target() {
        let tmp = tempfile::Builder::new()
            .prefix("panda-disposable-native-remove-junction-")
            .tempdir()
            .unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let target = root.join("external-target");
        let junction = root.join("owned-junction");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("sentinel"), b"preserve external target").unwrap();
        let target_identity = files::open_no_reparse(&target).unwrap().identity;
        let attributes = fs::metadata(&target).unwrap().file_attributes();
        let command = super::super::system_paths::SystemPaths::current()
            .unwrap()
            .system
            .join("cmd.exe");
        let created = std::process::Command::new(command)
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            created.status.success(),
            "{}",
            String::from_utf8_lossy(&created.stderr)
        );
        let identity = inspect(&junction).unwrap().identity;
        remove(&junction, &identity).unwrap();
        assert!(
            matches!(fs::symlink_metadata(&junction), Err(error) if error.kind() == io::ErrorKind::NotFound)
        );
        assert!(target_identity.same_object(&files::open_no_reparse(&target).unwrap().identity));
        assert_eq!(
            fs::read(target.join("sentinel")).unwrap(),
            b"preserve external target"
        );
        assert_eq!(fs::metadata(&target).unwrap().file_attributes(), attributes);
    }
}
