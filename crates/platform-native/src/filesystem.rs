//! Shared file operations at the platform boundary. Unix branches retain the
//! existing directory modes, no-follow flags, rename and directory fsync.
use std::{
    fs::{File, Metadata},
    io,
    path::Path,
};

pub fn path_components(path: &Path) -> Vec<std::path::PathBuf> {
    let mut current = std::path::PathBuf::new();
    let mut result = Vec::new();
    for component in path.components() {
        current.push(component);
        // C: alone means the drive's current directory. Inspect C:\ only once
        // RootDir joins its prefix. Preserve ordinary relative traversal too.
        if !matches!(component, std::path::Component::Prefix(_)) {
            result.push(current.clone());
        }
    }
    result
}

pub fn is_reparse(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(unix)]
    {
        metadata.file_type().is_symlink()
    }
}

pub fn create_private_directory(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        crate::windows::files::create_private_directory(path)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path)
    }
}

pub fn find_reparse_component(
    path: &Path,
    allow_missing: bool,
) -> io::Result<Option<std::path::PathBuf>> {
    for component in path_components(path) {
        match std::fs::symlink_metadata(&component) {
            Ok(metadata) if is_reparse(&metadata) => return Ok(Some(component)),
            Ok(_) => (),
            Err(error) if allow_missing && error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

pub fn make_directory_private(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        crate::windows::files::make_directory_private(path)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
}

pub fn directory_is_private(path: &Path) -> io::Result<bool> {
    #[cfg(windows)]
    {
        let opened = crate::windows::files::open_no_reparse(path)?;
        if !opened.identity.directory {
            return Ok(false);
        }
        match crate::windows::files::verify_private(&opened.file) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => Ok(false),
            Err(error) => Err(error),
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(std::fs::metadata(path)?.permissions().mode() & 0o077 == 0)
    }
}

pub fn private_directory_requirement() -> &'static str {
    #[cfg(windows)]
    {
        "current-user access only"
    }
    #[cfg(unix)]
    {
        "mode 0700"
    }
}

pub fn open_private_file(path: &Path, create: bool, create_new: bool) -> io::Result<File> {
    #[cfg(windows)]
    {
        crate::windows::files::open_private_file(path, create, create_new)
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .write(create || create_new)
            .create(create)
            .create_new(create_new)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
    }
}

pub fn replace_file(source: &Path, destination: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        crate::windows::files::rename_file(source, destination, true)
    }
    #[cfg(unix)]
    {
        std::fs::rename(source, destination)
    }
}

pub fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        let opened = crate::windows::files::open_no_reparse(path)?;
        if !opened.identity.directory {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "Expected a directory to synchronize",
            ));
        }
        crate::windows::files::sync_directory(&opened.file)
    }
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{Read, Write},
    };

    #[test]
    fn private_file_publication_replaces_closed_metadata_with_new_bytes() {
        let temp = tempfile::Builder::new()
            .prefix("panda-private-fs-中文 space-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let directory = root.join("disposable-private");
        create_private_directory(&directory).unwrap();
        assert!(directory_is_private(&directory).unwrap());
        make_directory_private(&directory).unwrap();
        assert!(directory_is_private(&directory).unwrap());
        let destination = directory.join("settings.json");
        let mut old = open_private_file(&destination, false, true).unwrap();
        old.write_all(b"old disposable settings").unwrap();
        old.sync_all().unwrap();
        drop(old);
        let stage = directory.join(".write-disposable");
        let mut file = open_private_file(&stage, false, true).unwrap();
        file.write_all(b"new disposable settings").unwrap();
        file.sync_all().unwrap();
        // Match the runtime publisher: its writing handle stays alive until
        // publication and directory synchronization have completed.
        replace_file(&stage, &destination).unwrap();
        sync_directory(&directory).unwrap();
        drop(file);
        assert!(!stage.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"new disposable settings");
        let mut reopened = open_private_file(&destination, false, false).unwrap();
        let mut bytes = Vec::new();
        reopened.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"new disposable settings");
        assert!(open_private_file(&destination, false, true).is_err());
        assert!(open_private_file(&directory.join("missing"), false, false).is_err());
        assert!(!directory.join("missing").exists());
        drop(reopened);
    }

    #[test]
    fn component_inventory_never_treats_a_drive_prefix_as_an_absolute_directory() {
        let temp = tempfile::Builder::new()
            .prefix("panda-path-chain-")
            .tempdir()
            .unwrap();
        let path = temp.path().canonicalize().unwrap().join("disposable-child");
        let chain = path_components(&path);
        assert!(!chain.is_empty());
        assert_eq!(chain.last().unwrap(), &path);
        assert!(chain.iter().all(|entry| entry.is_absolute()));
        assert_eq!(chain.first().unwrap().parent(), None);
    }
}
