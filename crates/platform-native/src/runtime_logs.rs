//! Private runtime log files. Rotation policy belongs to the caller; native
//! creation, no-link traversal and ownership checks belong here.
use std::{
    fs::{File, Metadata},
    io,
};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::private_current;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::private_current;

pub fn validate_file(_file: &File, metadata: &Metadata) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o7777 != 0o600
        {
            return Err(io::Error::other("unsafe runtime log inode"));
        }
    }
    #[cfg(windows)]
    {
        use crate::windows::files;
        let identity = files::identity(_file)?;
        if !metadata.is_file() || identity.directory || identity.links != 1 {
            return Err(io::Error::other("unsafe runtime log file"));
        }
        files::verify_private(_file)?;
    }
    Ok(())
}
