use super::{Change, Entry, Kind, ObjectIdentity, Stamp};
use std::{
    ffi::{CStr, CString, OsStr, OsString},
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::MetadataExt,
        },
    },
};

pub fn stamp(file: &File) -> io::Result<Stamp> {
    let m = file.metadata()?;
    Ok(Stamp {
        identity: ObjectIdentity::Unix {
            device: m.dev(),
            inode: m.ino(),
        },
        change: Change::Unix {
            length: m.len(),
            mode: m.mode(),
            modified: m.mtime(),
            modified_ns: m.mtime_nsec(),
            changed: m.ctime(),
            changed_ns: m.ctime_nsec(),
        },
    })
}
fn c(name: &OsStr) -> io::Result<CString> {
    super::valid_name(name)?;
    CString::new(name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "NUL in directory entry"))
}
pub fn child_directory(parent: &File, name: &OsStr) -> io::Result<File> {
    let name = c(name)?;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
pub struct Names(*mut libc::DIR);
impl Names {
    pub fn open(directory: &File) -> io::Result<Self> {
        let fd = unsafe { libc::fcntl(directory.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let pointer = unsafe { libc::fdopendir(fd) };
        if pointer.is_null() {
            let error = io::Error::last_os_error();
            unsafe {
                libc::close(fd);
            }
            return Err(error);
        }
        unsafe {
            libc::rewinddir(pointer);
        }
        Ok(Self(pointer))
    }
    pub fn next_name(&mut self) -> io::Result<Option<OsString>> {
        loop {
            #[cfg(target_os = "macos")]
            unsafe {
                *libc::__error() = 0;
            }
            #[cfg(not(target_os = "macos"))]
            unsafe {
                *libc::__errno_location() = 0;
            }
            let entry = unsafe { libc::readdir(self.0) };
            if entry.is_null() {
                let error = io::Error::last_os_error();
                return if error.raw_os_error().unwrap_or(0) == 0 {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
            let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if bytes != b"." && bytes != b".." {
                return Ok(Some(OsString::from_vec(bytes.to_vec())));
            }
        }
    }
}
impl Drop for Names {
    fn drop(&mut self) {
        unsafe {
            libc::closedir(self.0);
        }
    }
}
pub fn inspect(directory: &File, name: &OsStr) -> io::Result<Entry> {
    let name = c(name)?;
    let mut value = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            value.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let value = unsafe { value.assume_init() };
    let kind = match value.st_mode & libc::S_IFMT {
        libc::S_IFDIR => Kind::Directory,
        libc::S_IFREG => Kind::Regular,
        _ => Kind::Other,
    };
    Ok(Entry {
        kind,
        identity: Some(ObjectIdentity::Unix {
            device: value.st_dev as u64,
            inode: value.st_ino,
        }),
    })
}
