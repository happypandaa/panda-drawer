//! Budget observations may see Git's temporary links. Only the final, quiescent
//! scan grants publication, and it still rejects every non-independent file.
use super::*;
use std::{
    ffi::{CStr, CString},
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd},
};

fn names(directory: &File) -> Result<Vec<CString>, ApiError> {
    // fdopendir owns its duplicate; the original directory remains bound for
    // no-follow fstatat/openat operations throughout the scan.
    let descriptor = unsafe { libc::dup(directory.as_raw_fd()) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let pointer = unsafe { libc::fdopendir(descriptor) };
    if pointer.is_null() {
        unsafe {
            libc::close(descriptor);
        }
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
    let mut names = Vec::new();
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
        if names.len() >= 60_000 {
            return Err(limit());
        }
        names.push(name.to_owned());
    }
    Ok(names)
}
fn stat(directory: &File, name: &CStr) -> io::Result<libc::stat> {
    let mut metadata = std::mem::MaybeUninit::uninit();
    // This is the descriptor-relative equivalent of symlink_metadata: neither
    // a file link nor a directory link is followed to its target.
    if unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { metadata.assume_init() })
}
fn scan(
    directory: &File,
    bytes: &mut u64,
    count: &mut usize,
    final_scan: bool,
) -> Result<(), ApiError> {
    for name in names(directory)? {
        let metadata = match stat(directory, &name) {
            Ok(metadata) => metadata,
            Err(issue) if !final_scan && issue.kind() == io::ErrorKind::NotFound => continue,
            Err(issue) => return Err(issue.into()),
        };
        *count += 1;
        if *count > 60_000 {
            return Err(limit());
        }
        let kind = metadata.st_mode & libc::S_IFMT;
        if kind == libc::S_IFDIR {
            let descriptor = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if descriptor < 0 {
                let issue = io::Error::last_os_error();
                if !final_scan
                    && matches!(
                        issue.raw_os_error(),
                        Some(libc::ENOENT | libc::ENOTDIR | libc::ELOOP)
                    )
                {
                    continue;
                }
                return Err(issue.into());
            }
            // A successful openat transfers this fresh descriptor to File.
            let child = unsafe { File::from_raw_fd(descriptor) };
            let opened = child.metadata()?;
            if opened.dev() != metadata.st_dev as u64 || opened.ino() != metadata.st_ino {
                if final_scan {
                    return Err(invalid("获取目录在最终校验时被替换。"));
                }
                continue;
            }
            scan(&child, bytes, count, final_scan)?;
        } else {
            if final_scan && (kind != libc::S_IFREG || metadata.st_nlink != 1) {
                return Err(invalid("获取目录包含不支持的链接或特殊文件。"));
            }
            // A running Git may briefly hard-link pack files or probe symlinks.
            // Count each observed name and its own length, never read/follow it.
            *bytes = bytes
                .checked_add(u64::try_from(metadata.st_size).unwrap_or(0))
                .ok_or_else(limit)?;
            if *bytes > MAX_DISK {
                return Err(limit());
            }
        }
    }
    Ok(())
}
fn check(root: &Path, final_scan: bool) -> Result<(), ApiError> {
    let directory = crate::files::root(root)?;
    scan(&directory, &mut 0, &mut 0, final_scan)?;
    if fs2::available_space(root)? < MIN_FREE {
        return Err(error(
            "SOURCE_SPACE_INSUFFICIENT",
            "磁盘可用空间不足，源码获取已停止。",
        ));
    }
    Ok(())
}
pub(super) fn check_disk(root: &Path) -> Result<(), ApiError> {
    check(root, false)
}
pub(super) fn check_final_disk(root: &Path) -> Result<(), ApiError> {
    check(root, true)
}
