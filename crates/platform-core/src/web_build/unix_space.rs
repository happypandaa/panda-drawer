//! Descriptor-bound budget traversal never follows npm's transient links.
use super::{limit, MAX_ENTRIES, MAX_FILE, MAX_TOTAL};
use crate::ApiError;
use std::{
    ffi::{CStr, CString},
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::Path,
};
fn names(directory: &File) -> Result<Vec<CString>, ApiError> {
    let descriptor = unsafe { libc::dup(directory.as_raw_fd()) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let pointer = unsafe { libc::fdopendir(descriptor) };
    if pointer.is_null() {
        unsafe {
            libc::close(descriptor);
        };
        return Err(io::Error::last_os_error().into());
    }
    struct Stream(*mut libc::DIR);
    impl Drop for Stream {
        fn drop(&mut self) {
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let stream = Stream(pointer);
    let mut result = vec![];
    loop {
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = 0;
        }
        #[cfg(not(target_os = "macos"))]
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(stream.0) };
        if entry.is_null() {
            let issue = io::Error::last_os_error();
            if issue.raw_os_error().unwrap_or(0) != 0 {
                return Err(issue.into());
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        if result.len() >= MAX_ENTRIES {
            return Err(limit());
        }
        result.push(name.to_owned());
    }
    Ok(result)
}
fn scan(directory: &File, depth: usize, usage: &mut (usize, u64)) -> Result<(), ApiError> {
    if depth > 128 {
        return Err(limit());
    }
    for name in names(directory)? {
        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            let issue = io::Error::last_os_error();
            if issue.kind() == io::ErrorKind::NotFound {
                continue;
            }
            return Err(issue.into());
        }
        let metadata = unsafe { metadata.assume_init() };
        usage.0 = usage.0.checked_add(1).ok_or_else(limit)?;
        if usage.0 > MAX_ENTRIES {
            return Err(limit());
        }
        if metadata.st_mode & libc::S_IFMT == libc::S_IFDIR {
            let descriptor = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if descriptor < 0 {
                let issue = io::Error::last_os_error();
                if matches!(
                    issue.raw_os_error(),
                    Some(libc::ENOENT | libc::ENOTDIR | libc::ELOOP)
                ) {
                    continue;
                }
                return Err(issue.into());
            }
            let child = unsafe { File::from_raw_fd(descriptor) };
            let opened = child.metadata()?;
            if opened.dev() != metadata.st_dev as u64 || opened.ino() != metadata.st_ino {
                continue;
            }
            scan(&child, depth + 1, usage)?;
        } else {
            let size = u64::try_from(metadata.st_size).unwrap_or(0);
            if metadata.st_mode & libc::S_IFMT == libc::S_IFREG && size > MAX_FILE {
                return Err(limit());
            }
            usage.1 = usage.1.checked_add(size).ok_or_else(limit)?;
            if usage.1 > MAX_TOTAL {
                return Err(limit());
            }
        }
    }
    Ok(())
}
pub(super) fn visit(path: &Path, usage: &mut (usize, u64)) -> Result<(), ApiError> {
    let directory = match crate::files::root(path) {
        Ok(directory) => directory,
        Err(issue) if issue.code == "FILE_NOT_FOUND" => return Ok(()),
        Err(issue) => return Err(issue),
    };
    scan(&directory, 0, usage)
}
