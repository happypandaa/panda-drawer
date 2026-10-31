//! Snapshot flushing is separate from its read-only verification. Unix keeps
//! the original drop/open/fsync sequence; Windows flushes the original RW file.
use std::{
    fs::File,
    io,
    path::{Path, PathBuf},
};

pub(super) fn sqlite_companion(source: &Path, suffix: &str) -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from(format!("{}{suffix}", source.display()))
    }
    #[cfg(windows)]
    {
        let mut path = source.as_os_str().to_os_string();
        path.push(suffix);
        PathBuf::from(path)
    }
}

pub(super) struct SnapshotFile {
    #[cfg(windows)]
    file: File,
    #[cfg(windows)]
    _name: File,
    #[cfg(windows)]
    _parent: platform_native::windows::files::PinnedDirectory,
}
impl SnapshotFile {
    pub(super) fn retain(created: File, path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let _ = path;
            drop(created);
            Ok(Self {})
        }
        #[cfg(windows)]
        {
            use platform_native::windows::files as native;
            use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};
            let parent = native::pin_directory(path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Snapshot file needs a parent")
            })?)?;
            // SQLite uses this exact name while the original writable file is
            // retained. A real read handle (attribute-only handles do not
            // impose Windows share-delete exclusion) denies namespace changes,
            // without new write access or any parent/file DACL policy.
            let name = OpenOptions::new()
                .read(true)
                .share_mode(3)
                .custom_flags(0x0020_0000)
                .open(path)?;
            let actual = native::identity(&name)?;
            if actual.directory
                || platform_native::filesystem::is_reparse(&name.metadata()?)
                || !native::identity(&created)?.same_object(&actual)
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "New SQLite snapshot name changed before use",
                ));
            }
            Ok(Self {
                file: created,
                _name: name,
                _parent: parent,
            })
        }
    }
    pub(super) fn sync(self, path: &Path) -> io::Result<()> {
        #[cfg(unix)]
        {
            File::open(path)?.sync_all()
        }
        #[cfg(windows)]
        {
            let _ = path;
            self.file.sync_all()
        }
    }
}

#[cfg(all(test, windows))]
#[path = "backups_file_io/windows_tests.rs"]
mod tests;
