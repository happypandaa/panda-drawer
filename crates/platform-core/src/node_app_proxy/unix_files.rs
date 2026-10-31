//! Original Unix descriptor traversal.
use std::{
    ffi::CString,
    fs::File,
    io,
    path::{Component, Path},
};
fn open_at(directory: &File, name: &str, directory_only: bool) -> io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let name = CString::new(name).map_err(|_| io::ErrorKind::InvalidInput)?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory_only { libc::O_DIRECTORY } else { 0 };
    // Both descriptors and the component name remain alive throughout openat;
    // O_NOFOLLOW applies to each individual component, never just the final path.
    let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // A successful openat returns a fresh descriptor with sole ownership.
    let file = unsafe { File::from_raw_fd(descriptor) };
    let metadata = file.metadata()?;
    if (directory_only && !metadata.is_dir()) || (!directory_only && !metadata.is_file()) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    if !directory_only {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
    }
    Ok(file)
}
pub(super) fn open_root(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut directory = File::open("/")?;
    for component in path.components() {
        match component {
            Component::RootDir => (),
            Component::Normal(name) => {
                let name = name.to_str().ok_or(io::ErrorKind::InvalidInput)?;
                directory = open_at(&directory, name, true)?;
            }
            _ => return Err(io::ErrorKind::InvalidInput.into()),
        }
    }
    Ok(directory)
}
pub(super) fn open_file(root: &File, parts: &[String]) -> io::Result<File> {
    let (last, parents) = parts.split_last().ok_or(io::ErrorKind::InvalidInput)?;
    let mut directory = root.try_clone()?;
    for component in parents {
        directory = open_at(&directory, component, true)?;
    }
    open_at(&directory, last, false)
}
