//! Descriptor-relative file access. Every directory and file is opened without
//! following symlinks, including the supplied root, so path validation is not a
//! check-then-open security boundary.
use super::{info, FileInfo};
use crate::ApiError;
use std::os::unix::fs::MetadataExt;
use std::{
    ffi::CString,
    fs::File,
    io,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::ffi::OsStrExt,
    path::{Component, Path},
};

pub(crate) fn error(error: io::Error) -> ApiError {
    if error.raw_os_error() == Some(libc::ELOOP) {
        return invalid();
    }
    let code = match error.kind() {
        io::ErrorKind::NotFound => "FILE_NOT_FOUND",
        io::ErrorKind::AlreadyExists => "FILE_EXISTS",
        io::ErrorKind::PermissionDenied => "FILE_ACCESS_DENIED",
        _ => "FILE_IO_ERROR",
    };
    ApiError::new(code, error.to_string())
}
fn invalid() -> ApiError {
    ApiError::new(
        "INVALID_PATH",
        "Path must be a relative, non-empty file path without dot segments or symlinks",
    )
}
fn cstr(bytes: &[u8]) -> Result<CString, ApiError> {
    CString::new(bytes).map_err(|_| invalid())
}
fn opened(fd: libc::c_int) -> Result<File, ApiError> {
    if fd < 0 {
        Err(error(io::Error::last_os_error()))
    } else {
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}
fn open_at(dir: &File, name: &[u8], flags: libc::c_int) -> Result<File, ApiError> {
    let name = cstr(name)?;
    opened(unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    })
}
pub(crate) fn root(path: &Path) -> Result<File, ApiError> {
    if !path.is_absolute() {
        return Err(invalid());
    }
    let mut dir = opened(unsafe {
        libc::open(
            c"/".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    })?;
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => {
                dir = open_at(&dir, name.as_bytes(), libc::O_RDONLY | libc::O_DIRECTORY)?
            }
            _ => return Err(invalid()),
        }
    }
    Ok(dir)
}
pub(crate) fn parts(path: &str) -> Result<Vec<&str>, ApiError> {
    if path.is_empty()
        || path.len() > 2048
        || path.contains('\\')
        || path.chars().any(char::is_control)
    {
        return Err(invalid());
    }
    let parts: Vec<_> = path.split('/').collect();
    if parts
        .iter()
        .any(|p| p.is_empty() || *p == "." || *p == ".." || p.len() > 255)
    {
        return Err(invalid());
    }
    Ok(parts)
}
pub(crate) fn parent(root: &File, path: &str, create: bool) -> Result<(File, String), ApiError> {
    let parts = parts(path)?;
    let mut dir = root.try_clone().map_err(error)?;
    for part in &parts[..parts.len() - 1] {
        if create {
            let name = cstr(part.as_bytes())?;
            let status = unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) };
            if status != 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
                return Err(error(io::Error::last_os_error()));
            }
        }
        dir = open_at(&dir, part.as_bytes(), libc::O_RDONLY | libc::O_DIRECTORY)?;
    }
    Ok((dir, parts.last().unwrap().to_string()))
}
pub(crate) fn read(root: &File, path: &str) -> Result<File, ApiError> {
    let (dir, name) = parent(root, path, false)?;
    let file = open_at(&dir, name.as_bytes(), libc::O_RDONLY | libc::O_NONBLOCK)?;
    let metadata = file.metadata().map_err(error)?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(invalid());
    }
    Ok(file)
}
pub(crate) fn create(root: &File, path: &str) -> Result<File, ApiError> {
    let (dir, name) = parent(root, path, true)?;
    open_at(
        &dir,
        name.as_bytes(),
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
    )
}
pub(crate) fn discard_temporary(root: &File, path: &str) {
    if !path.starts_with(".upload-") || parts(path).is_err() || path.contains('/') {
        return;
    }
    if let Ok(name) = cstr(path.as_bytes()) {
        unsafe {
            libc::unlinkat(root.as_raw_fd(), name.as_ptr(), 0);
        }
    }
}
pub(crate) fn remove(root: &File, path: &str) -> Result<(), ApiError> {
    let (dir, name) = parent(root, path, false)?;
    let _file = read(root, path)?;
    let name = cstr(name.as_bytes())?;
    if unsafe { libc::unlinkat(dir.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(error(io::Error::last_os_error()));
    }
    dir.sync_all().map_err(|e| error(e).outcome("committed"))
}
pub(crate) fn publish(
    staging: &File,
    temporary: &str,
    root: &File,
    path: &str,
    overwrite: bool,
) -> Result<(), ApiError> {
    let (dir, name) = parent(root, path, true)?;
    // Even overwrite refuses a symlink/device/directory target.
    match read(root, path) {
        Ok(_) if !overwrite => {
            return Err(ApiError::new(
                "FILE_EXISTS",
                "An attachment already exists at this path",
            ))
        }
        Ok(_) => {}
        Err(e) if e.code == "FILE_NOT_FOUND" => {}
        Err(e) => return Err(e),
    }
    let name = cstr(name.as_bytes())?;
    let temporary = cstr(temporary.as_bytes())?;
    if overwrite {
        if unsafe {
            libc::renameat(
                staging.as_raw_fd(),
                temporary.as_ptr(),
                dir.as_raw_fd(),
                name.as_ptr(),
            )
        } != 0
        {
            return Err(error(io::Error::last_os_error()));
        }
    } else {
        // linkat is atomic and refuses an existing target; a concurrent writer
        // cannot be overwritten between the check above and publication.
        if unsafe {
            libc::linkat(
                staging.as_raw_fd(),
                temporary.as_ptr(),
                dir.as_raw_fd(),
                name.as_ptr(),
                0,
            )
        } != 0
        {
            return Err(error(io::Error::last_os_error()));
        }
        if unsafe { libc::unlinkat(staging.as_raw_fd(), temporary.as_ptr(), 0) } != 0 {
            return Err(error(io::Error::last_os_error()).outcome("committed"));
        }
    }
    dir.sync_all().map_err(|e| error(e).outcome("committed"))?;
    staging
        .sync_all()
        .map_err(|e| error(e).outcome("committed"))
}
pub(crate) fn list(root_path: &Path, prefix: &str) -> Result<Vec<FileInfo>, ApiError> {
    if !prefix.is_empty() {
        parts(prefix.trim_end_matches('/'))?;
    }
    let root_fd = root(root_path)?;
    let mut queue = vec![(root_path.to_path_buf(), String::new())];
    let mut output = Vec::new();
    let mut count = 0;
    while let Some((directory, relative)) = queue.pop() {
        // Enumeration yields names only; actual opens remain descriptor-relative.
        for entry in std::fs::read_dir(directory).map_err(error)? {
            let entry = entry.map_err(error)?;
            count += 1;
            if count > 10000 {
                return Err(ApiError::new(
                    "FILE_LIST_LIMIT_EXCEEDED",
                    "Attachment listing exceeds 10,000 entries; use narrower folders",
                ));
            }
            let name = entry.file_name().into_string().map_err(|_| invalid())?;
            let path = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            let metadata = entry.metadata().map_err(error)?;
            if metadata.is_dir() {
                let (dir, leaf) = parent(&root_fd, &path, false)?;
                let _checked = open_at(&dir, leaf.as_bytes(), libc::O_RDONLY | libc::O_DIRECTORY)?;
                if prefix.starts_with(&format!("{path}/")) || path.starts_with(prefix) {
                    queue.push((entry.path(), path));
                }
            } else if path.starts_with(prefix) {
                output.push(info(&path, &read(&root_fd, &path)?)?);
            }
        }
    }
    output.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[test]
    fn refuses_traversal_symlinks_and_preserves_original() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let canonical = tmp.path().canonicalize().unwrap();
        let rootfd = root(&canonical).unwrap();
        let mut original = create(&rootfd, "a/original.txt").unwrap();
        original.write_all(b"old").unwrap();
        assert!(read(&rootfd, "../secret").is_err());
        assert!(read(&rootfd, "a//original.txt").is_err());
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();
        assert!(create(&rootfd, "link/new").is_err());
        std::os::unix::fs::symlink(tmp.path().join("a/original.txt"), tmp.path().join("alias"))
            .unwrap();
        assert!(read(&rootfd, "alias").is_err());
        create(&rootfd, "temporary")
            .unwrap()
            .write_all(b"new")
            .unwrap();
        assert!(publish(&rootfd, "temporary", &rootfd, "a/original.txt", false).is_err());
        assert_eq!(
            std::fs::read(tmp.path().join("a/original.txt")).unwrap(),
            b"old"
        );
        publish(&rootfd, "temporary", &rootfd, "a/original.txt", true).unwrap();
        assert_eq!(
            std::fs::read(tmp.path().join("a/original.txt")).unwrap(),
            b"new"
        );
    }
}
