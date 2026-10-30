//! Read-only file facts for tool verification and metadata revisions. Unix permissions remain Unix
//! permissions; Windows uses real file IDs/attributes and ordinary read access.
use std::{
    borrow::Cow,
    fs::{File, Metadata},
    io,
    path::{Component, Path},
};

#[derive(Clone, Debug)]
pub struct Facts {
    metadata: Metadata,
    #[cfg(windows)]
    stamp: crate::directory::Stamp,
    #[cfg(windows)]
    links: u64,
}

impl Facts {
    pub fn capture(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                metadata: std::fs::symlink_metadata(path)?,
            })
        }
        #[cfg(windows)]
        {
            let opened = crate::windows::files::open_no_reparse(path)?;
            Self::from_file(&opened.file)
        }
    }
    pub fn from_file(file: &File) -> io::Result<Self> {
        Ok(Self {
            metadata: file.metadata()?,
            #[cfg(windows)]
            stamp: crate::directory::stamp(file)?,
            #[cfg(windows)]
            links: u64::from(crate::windows::files::identity(file)?.links),
        })
    }
    /// Explicit selections follow package-manager links, unlike no-follow
    /// bundled/control files. Preserve that existing policy boundary.
    pub fn capture_following(path: &Path) -> io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                metadata: std::fs::metadata(path)?,
            })
        }
        #[cfg(windows)]
        {
            Self::from_file(&crate::windows::files::following_metadata_file(path)?)
        }
    }
    pub fn is_file(&self) -> bool {
        self.metadata.is_file()
    }
    pub fn is_dir(&self) -> bool {
        self.metadata.is_dir()
    }
    pub fn is_reparse(&self) -> bool {
        crate::filesystem::is_reparse(&self.metadata)
    }
    pub fn len(&self) -> u64 {
        self.metadata.len()
    }
    pub fn is_empty(&self) -> bool {
        self.metadata.len() == 0
    }
    pub fn modified(&self) -> io::Result<std::time::SystemTime> {
        self.metadata.modified()
    }
    /// The existing transient snapshot stamp deliberately excludes link count;
    /// callers keep their separate admission/checking rule for that fact.
    pub fn stamp(&self) -> crate::directory::Stamp {
        #[cfg(unix)]
        {
            crate::directory::metadata_stamp(&self.metadata)
        }
        #[cfg(windows)]
        {
            self.stamp.clone()
        }
    }
    pub fn links(&self) -> u64 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            self.metadata.nlink()
        }
        #[cfg(windows)]
        {
            self.links
        }
    }
    pub fn same_object(&self, other: &Self) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            self.metadata.dev() == other.metadata.dev()
                && self.metadata.ino() == other.metadata.ino()
        }
        #[cfg(windows)]
        {
            self.stamp.identity == other.stamp.identity
        }
    }
    /// Existing Unix revision bytes, or the complete native Windows identity.
    /// These bytes invalidate editors; they never authorize a file operation.
    pub fn identity_revision_bytes(&self) -> Vec<u8> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            [
                self.metadata.dev().to_le_bytes(),
                self.metadata.ino().to_le_bytes(),
            ]
            .concat()
        }
        #[cfg(windows)]
        {
            let crate::directory::ObjectIdentity::Windows {
                volume_serial,
                file_id,
            } = &self.stamp.identity
            else {
                unreachable!("Windows file facts always carry a Windows identity");
            };
            [volume_serial.to_le_bytes().as_slice(), file_id.as_slice()].concat()
        }
    }
    pub fn content_revision_bytes(&self) -> Vec<u8> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            [
                self.metadata.len().to_le_bytes(),
                self.metadata.ino().to_le_bytes(),
                self.metadata.mtime().to_le_bytes(),
                self.metadata.mtime_nsec().to_le_bytes(),
            ]
            .concat()
        }
        #[cfg(windows)]
        {
            let crate::directory::Change::Windows {
                length,
                attributes,
                modified_100ns,
                changed_100ns,
            } = &self.stamp.change;
            [
                self.identity_revision_bytes(),
                length.to_le_bytes().to_vec(),
                attributes.to_le_bytes().to_vec(),
                modified_100ns.to_le_bytes().to_vec(),
                changed_100ns.to_le_bytes().to_vec(),
            ]
            .concat()
        }
    }
    /// The existing registration policy checks identity, size and modification
    /// time only. Do not strengthen it with mode, ctime or link-count changes.
    pub fn same_modified_file(&self, other: &Self) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            self.same_object(other)
                && self.len() == other.len()
                && self.metadata.mtime() == other.metadata.mtime()
                && self.metadata.mtime_nsec() == other.metadata.mtime_nsec()
        }
        #[cfg(windows)]
        {
            let crate::directory::Change::Windows {
                length,
                modified_100ns,
                ..
            } = &self.stamp.change;
            let crate::directory::Change::Windows {
                length: other_length,
                modified_100ns: other_modified,
                ..
            } = &other.stamp.change;
            self.same_object(other) && length == other_length && modified_100ns == other_modified
        }
    }
    /// Dependency input readers additionally observe Unix mode. Windows has no
    /// Unix execute bits; compare the real read-only permission alongside type.
    /// Neither host adds ctime or link-count checks to this change policy.
    pub fn same_modified_permissions(&self, other: &Self) -> bool {
        if !self.same_modified_file(other) {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            self.metadata.mode() == other.metadata.mode()
        }
        #[cfg(windows)]
        {
            self.is_file() == other.is_file()
                && self.is_dir() == other.is_dir()
                && self.metadata.permissions().readonly() == other.metadata.permissions().readonly()
        }
    }
    /// Migration metadata observes change time as well as modification time,
    /// without introducing a separate permission comparison into that policy.
    pub fn same_modified_and_changed(&self, other: &Self) -> bool {
        if !self.same_modified_file(other) {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            self.metadata.ctime() == other.metadata.ctime()
                && self.metadata.ctime_nsec() == other.metadata.ctime_nsec()
        }
        #[cfg(windows)]
        {
            let crate::directory::Change::Windows { changed_100ns, .. } = &self.stamp.change;
            let crate::directory::Change::Windows {
                changed_100ns: other_changed,
                ..
            } = &other.stamp.change;
            changed_100ns == other_changed
        }
    }
    pub fn unchanged(&self, other: &Self) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            self.same_object(other)
                && self.len() == other.len()
                && self.metadata.mode() == other.metadata.mode()
                && self.links() == other.links()
                && self.metadata.mtime() == other.metadata.mtime()
                && self.metadata.mtime_nsec() == other.metadata.mtime_nsec()
                && self.metadata.ctime() == other.metadata.ctime()
                && self.metadata.ctime_nsec() == other.metadata.ctime_nsec()
        }
        #[cfg(windows)]
        {
            self.stamp == other.stamp && self.links == other.links && self.len() == other.len()
        }
    }
    /// Equivalent host policy, without inventing mode bits on NTFS. Windows
    /// opening/reading and later executable probes check their real access.
    pub fn permissions_valid(&self, executable_or_directory: bool) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let modes = if executable_or_directory {
                [0o555, 0o755]
            } else {
                [0o444, 0o644]
            };
            modes.contains(&(self.metadata.mode() & 0o7777))
        }
        #[cfg(windows)]
        {
            let _ = executable_or_directory;
            !self.is_reparse() && (self.is_file() || self.is_dir())
        }
    }
}

pub fn open_read(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
    }
    #[cfg(windows)]
    {
        crate::windows::files::open_read_file(path)
    }
}

/// Windows has no Unix executable/set-ID bits. The actual owned version probe
/// decides whether an ordinary selected file is executable on that host.
pub fn executable_flags_allowed(facts: &Facts) -> bool {
    if !facts.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        facts.metadata.mode() & 0o111 != 0 && facts.metadata.mode() & 0o6000 == 0
    }
    #[cfg(windows)]
    {
        true
    }
}

pub fn absolute_path_valid(path: &Path) -> bool {
    let ordinary = path.is_absolute()
        && path
            .to_str()
            .is_some_and(|value| !value.chars().any(char::is_control))
        && !path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir));
    #[cfg(unix)]
    {
        ordinary && path.to_str().is_some_and(|value| !value.contains('\\'))
    }
    #[cfg(windows)]
    {
        ordinary
    }
}

pub fn relative_name(path: &Path) -> Option<Cow<'_, str>> {
    #[cfg(unix)]
    {
        path.to_str().map(Cow::Borrowed)
    }
    #[cfg(windows)]
    {
        let parts = path
            .components()
            .map(|part| match part {
                Component::Normal(name) => name.to_str(),
                _ => None,
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Cow::Owned(parts.join("/")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{Duration, SystemTime},
    };

    #[test]
    fn facts_detect_real_timestamp_changes_and_same_name_replacement() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-tool-facts-中文 space-")
            .tempdir()
            .unwrap();
        let path = temporary.path().canonicalize().unwrap().join("tool.dat");
        fs::write(&path, b"disposable resource bytes").unwrap();
        let before = Facts::capture(&path).unwrap();
        let held = open_read(&path).unwrap();
        assert_eq!(before.stamp(), crate::directory::stamp(&held).unwrap());
        assert!(before.unchanged(&Facts::from_file(&held).unwrap()));
        assert_eq!(before.links(), 1);
        let date = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let writer = fs::OpenOptions::new().write(true).open(&path).unwrap();
        writer
            .set_times(fs::FileTimes::new().set_modified(date))
            .unwrap();
        drop(writer);
        let changed = Facts::capture(&path).unwrap();
        assert!(before.same_object(&changed));
        assert!(!before.unchanged(&changed));
        fs::rename(&path, path.with_file_name("retained-tool.dat")).unwrap();
        fs::write(&path, b"disposable resource bytes").unwrap();
        assert!(!before.same_object(&Facts::capture(&path).unwrap()));
        assert_eq!(
            fs::read(path.with_file_name("retained-tool.dat")).unwrap(),
            b"disposable resource bytes"
        );
    }

    #[test]
    fn absolute_unicode_paths_and_manifest_relative_names_keep_their_native_meaning() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-tool-path-中文 space-")
            .tempdir()
            .unwrap();
        assert!(absolute_path_valid(
            &temporary.path().canonicalize().unwrap()
        ));
        assert!(!absolute_path_valid(Path::new("relative/tool")));
        assert_eq!(
            relative_name(
                Path::new("lib")
                    .join("node_modules/npm/bin/npm-cli.js")
                    .as_path()
            )
            .unwrap(),
            "lib/node_modules/npm/bin/npm-cli.js"
        );
    }

    #[test]
    fn registration_change_policy_does_not_add_link_count_or_change_time_checks() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-registration-facts-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("registration.json");
        fs::write(&path, b"disposable metadata").unwrap();
        let before = Facts::capture(&path).unwrap();
        fs::hard_link(&path, root.join("second-name")).unwrap();
        let linked = Facts::capture(&path).unwrap();
        assert_eq!(linked.links(), 2);
        assert!(before.same_modified_file(&linked));
        assert!(before.same_modified_permissions(&linked));
        assert!(!before.unchanged(&linked));
        fs::write(
            &path,
            b"changed disposable metadata with a different length",
        )
        .unwrap();
        assert!(!before.same_modified_file(&Facts::capture(&path).unwrap()));
        fs::rename(&path, root.join("retained-original")).unwrap();
        fs::write(&path, b"disposable metadata").unwrap();
        assert!(!before.same_modified_file(&Facts::capture(&path).unwrap()));
    }

    #[test]
    fn dependency_change_policy_observes_permissions_without_requiring_change_time() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-dependency-permissions-")
            .tempdir()
            .unwrap();
        let path = temporary.path().canonicalize().unwrap().join("input.json");
        fs::write(&path, b"disposable dependency input").unwrap();
        let original_permissions = fs::metadata(&path).unwrap().permissions();
        let before = Facts::capture(&path).unwrap();
        let mut read_only = original_permissions.clone();
        read_only.set_readonly(true);
        fs::set_permissions(&path, read_only).unwrap();
        let after = Facts::capture(&path).unwrap();
        // Restore the disposable file before assertions so failed assertions
        // cannot leave a read-only fixture behind on Windows.
        fs::set_permissions(&path, original_permissions).unwrap();
        assert!(before.same_modified_file(&after));
        assert!(!before.same_modified_permissions(&after));
        assert_eq!(fs::read(path).unwrap(), b"disposable dependency input");
    }
}
