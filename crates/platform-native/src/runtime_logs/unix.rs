//! Existing descriptor-relative Unix log rotation, moved without policy changes.
use std::{fs::File, path::Path};

pub fn private_current(control: &Path, limit: u64) -> std::io::Result<File> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::MetadataExt},
        },
        path::Component,
    };
    fn c(value: &[u8]) -> std::io::Result<CString> {
        CString::new(value).map_err(|_| std::io::Error::other("invalid log path"))
    }
    fn opened(fd: i32) -> std::io::Result<File> {
        if fd < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }
    let mut directory = opened(unsafe {
        libc::open(
            c(b"/")?.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    })?;
    if !control.is_absolute() {
        return Err(std::io::Error::other("relative log path"));
    }
    for part in control.components() {
        match part {
            Component::RootDir => (),
            Component::Normal(name) => {
                directory = opened(unsafe {
                    libc::openat(
                        directory.as_raw_fd(),
                        c(name.as_bytes())?.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                })?;
            }
            _ => return Err(std::io::Error::other("noncanonical log path")),
        }
    }
    let logs = c(b"logs")?;
    if unsafe { libc::mkdirat(directory.as_raw_fd(), logs.as_ptr(), 0o700) } != 0
        && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
    {
        return Err(std::io::Error::last_os_error());
    }
    let directory = opened(unsafe {
        libc::openat(
            directory.as_raw_fd(),
            logs.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    })?;
    let metadata = directory.metadata()?;
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o7777 != 0o700 {
        return Err(std::io::Error::other("unsafe log directory"));
    }
    let current = c(b"current.log")?;
    let previous = c(b"previous.log")?;
    // Open every existing target without following links or blocking on FIFOs,
    // and inspect the inode before rename/removal. The directory FD pins scope.
    let check = |name: &CString| -> std::io::Result<bool> {
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            return Ok(false);
        }
        let file = opened(fd)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o7777 != 0o600
            || metadata.len() > limit
        {
            return Err(std::io::Error::other("unsafe log file"));
        }
        Ok(true)
    };
    let has_current = check(&current)?;
    let has_previous = check(&previous)?;
    if has_current {
        if has_previous
            && unsafe { libc::unlinkat(directory.as_raw_fd(), previous.as_ptr(), 0) } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        if unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                current.as_ptr(),
                directory.as_raw_fd(),
                previous.as_ptr(),
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    opened(unsafe {
        libc::openat(
            directory.as_raw_fd(),
            current.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    })
}
