//! Descriptor-bound directory observation. No paths or data roots are discovered.
use std::{ffi::OsStr, io};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
pub use unix::{child_directory, inspect, stamp, Names};
#[cfg(windows)]
pub use windows::{child_directory, inspect, stamp, Names};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObjectIdentity {
    Unix {
        device: u64,
        inode: u64,
    },
    Windows {
        volume_serial: u64,
        file_id: [u8; 16],
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stamp {
    pub identity: ObjectIdentity,
    // Native facts remain native: no invented Unix modes or inode values.
    pub(crate) change: Change,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    #[cfg(unix)]
    Unix {
        length: u64,
        mode: u32,
        modified: i64,
        modified_ns: i64,
        changed: i64,
        changed_ns: i64,
    },
    #[cfg(windows)]
    Windows {
        length: u64,
        attributes: u32,
        modified_100ns: i64,
        changed_100ns: i64,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Directory,
    Regular,
    Other,
}
pub struct Entry {
    pub kind: Kind,
    pub identity: Option<ObjectIdentity>,
}
pub fn valid_name(name: &OsStr) -> io::Result<()> {
    if name.is_empty()
        || std::path::Path::new(name).components().count() != 1
        || !matches!(
            std::path::Path::new(name).components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected one ordinary directory entry",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write};

    #[test]
    fn stamps_distinguish_actual_content_changes_and_same_name_replacement() {
        let temp = tempfile::Builder::new()
            .prefix("panda-disposable-stamp-中文 space-")
            .tempdir()
            .unwrap();
        let path = temp.path().canonicalize().unwrap().join("disposable.txt");
        fs::write(&path, b"initial").unwrap();
        let held = fs::File::open(&path).unwrap();
        let before = stamp(&held).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap()
            .write_all(b"a changed length")
            .unwrap();
        let changed = stamp(&held).unwrap();
        assert_eq!(before.identity, changed.identity);
        assert_ne!(before, changed);
        fs::rename(&path, path.with_file_name("retained-original.txt")).unwrap();
        fs::write(&path, b"a changed length").unwrap();
        let replacement = stamp(&fs::File::open(&path).unwrap()).unwrap();
        assert_ne!(replacement.identity, changed.identity);
    }
}
