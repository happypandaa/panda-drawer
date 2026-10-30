//! Shared file operations at the platform boundary. Unix branches retain the
//! existing directory modes, no-follow flags, rename and directory fsync.
use std::{
    fs::{File, Metadata},
    io,
    path::Path,
};

/// Lexical shape only: this does not inspect or claim a directory. Filesystem,
/// volume and ownership checks remain with the caller's existing I/O boundary.
pub fn is_explicit_non_root_absolute_path(path: &Path) -> bool {
    if !path.is_absolute() || path.parent().is_none() {
        return false;
    }
    #[cfg(unix)]
    {
        !path.components().any(|part| {
            !matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
    }
    #[cfg(windows)]
    {
        // Both Disk and VerbatimDisk (returned by canonicalize) carry a prefix.
        // A prefix alone does not grant local-volume or filesystem permission.
        !path.components().any(|part| {
            !matches!(
                part,
                std::path::Component::Prefix(_)
                    | std::path::Component::RootDir
                    | std::path::Component::Normal(_)
            )
        })
    }
}

pub fn path_components(path: &Path) -> Vec<std::path::PathBuf> {
    #[cfg(windows)]
    let normalized = crate::windows::separator_path(path);
    #[cfg(windows)]
    let path = normalized.as_path();
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

/// Preserve each existing Unix reader's flags at the platform boundary. On
/// Windows ordinary-file handles are non-inheritable and reject reparse points;
/// a separate caller policy decides whether hard links are admitted.
#[derive(Clone, Copy)]
pub enum ReadOptions {
    NoLinks,
    NoLinksNonblocking,
    NoLinksCloseOnExec,
    NoLinksNonblockingCloseOnExec,
}

pub fn open_read(path: impl AsRef<Path>, options: ReadOptions) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let flags = libc::O_NOFOLLOW
            | if matches!(
                options,
                ReadOptions::NoLinksNonblocking | ReadOptions::NoLinksNonblockingCloseOnExec
            ) {
                libc::O_NONBLOCK
            } else {
                0
            }
            | if matches!(
                options,
                ReadOptions::NoLinksCloseOnExec | ReadOptions::NoLinksNonblockingCloseOnExec
            ) {
                libc::O_CLOEXEC
            } else {
                0
            };
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(flags)
            .open(path)
    }
    #[cfg(windows)]
    {
        let _ = options;
        crate::windows::files::open_read_file_allowing_hard_links(path.as_ref())
    }
}

/// Private creation policies remain explicit, including the original Unix
/// no-follow choice. Windows creates a protected current-user DACL atomically.
pub fn create_new_private_file(path: impl AsRef<Path>, no_links: bool) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        if no_links {
            options.custom_flags(libc::O_NOFOLLOW);
        }
        options.open(path)
    }
    #[cfg(windows)]
    {
        let _ = no_links;
        crate::windows::files::create_private_file(path.as_ref())
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
    fn explicit_path_shape_accepts_a_canonical_root_child_without_creating_it() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-explicit-path-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let candidate = root.join("disposable-generated-中文 space");
        assert!(!candidate.exists());
        assert!(is_explicit_non_root_absolute_path(&candidate));
        assert!(!candidate.exists());
        assert!(fs::read_dir(&root).unwrap().next().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn explicit_path_shape_accepts_windows_prefixes_without_adding_volume_admission() {
        for path in [
            r"C:\中文 space\project",
            r"\\?\C:\中文 space\project",
            r"\\server\share\project",
            r"\\?\UNC\server\share\project",
            r"C:\folder\.\project",
        ] {
            assert!(
                is_explicit_non_root_absolute_path(Path::new(path)),
                "{path}"
            );
        }
        for path in [
            "",
            r"relative\project",
            r"C:relative\project",
            r"\rooted\project",
            r"C:\",
            r"\\?\C:\",
            r"C:\folder\..\project",
            r"\\?\C:\folder\..\project",
        ] {
            assert!(
                !is_explicit_non_root_absolute_path(Path::new(path)),
                "{path}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn explicit_path_shape_preserves_the_existing_unix_component_policy() {
        for path in [
            "/tmp/中文 space/project",
            "/tmp/./project",
            "/missing/project",
        ] {
            assert!(
                is_explicit_non_root_absolute_path(Path::new(path)),
                "{path}"
            );
        }
        for path in ["", "/", "relative/project", "/tmp/../project"] {
            assert!(
                !is_explicit_non_root_absolute_path(Path::new(path)),
                "{path}"
            );
        }
    }

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

    #[test]
    fn no_link_read_does_not_add_a_hard_link_admission_rule() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-read-policy-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("control.json");
        let mut file = create_new_private_file(&path, false).unwrap();
        file.write_all(b"disposable linked bytes").unwrap();
        file.sync_all().unwrap();
        std::fs::hard_link(&path, root.join("second-name")).unwrap();
        for policy in [
            ReadOptions::NoLinks,
            ReadOptions::NoLinksNonblocking,
            ReadOptions::NoLinksCloseOnExec,
            ReadOptions::NoLinksNonblockingCloseOnExec,
        ] {
            let mut reader = open_read(&path, policy).unwrap();
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"disposable linked bytes");
        }
        assert!(create_new_private_file(&path, true).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"disposable linked bytes");
    }
}
