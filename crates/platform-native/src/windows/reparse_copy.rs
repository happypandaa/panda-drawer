//! Copy an existing symbolic-link/junction object without entering its target.
//! FSCTL_GET/SET_REPARSE_POINT copy the original Microsoft link data; a new
//! ordinary leaf is created exclusively and retained on any uncertain outcome.
//! https://learn.microsoft.com/windows/win32/fileio/reparse-point-operations
use super::{checked, files, removal};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    },
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{GENERIC_READ, GENERIC_WRITE},
    Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    },
    System::{
        Ioctl::{FSCTL_GET_REPARSE_POINT, FSCTL_SET_REPARSE_POINT},
        IO::DeviceIoControl,
    },
};
const MOUNT_POINT: u32 = 0xa0000003;
const SYMLINK: u32 = 0xa000000c;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn data(file: &File) -> io::Result<Vec<u8>> {
    let mut buffer = vec![0u8; 16 * 1024];
    let mut length = 0;
    checked(unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_GET_REPARSE_POINT,
            ptr::null(),
            0,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            &mut length,
            ptr::null_mut(),
        )
    })?;
    buffer.truncate(length as usize);
    if buffer.len() < 8
        || u16::from_le_bytes(buffer[4..6].try_into().unwrap()) as usize + 8 != buffer.len()
    {
        return Err(invalid("Invalid link reparse data length"));
    }
    let tag = u32::from_le_bytes(buffer[..4].try_into().unwrap());
    if ![MOUNT_POINT, SYMLINK].contains(&tag) {
        return Err(invalid("Unsupported special reparse object"));
    }
    Ok(buffer)
}

pub fn copy(source: &Path, destination: &Path, expected_target: &Path) -> io::Result<()> {
    let _source_parent = files::pin_directory(
        source
            .parent()
            .ok_or_else(|| invalid("Link source has no parent"))?,
    )?;
    let _target_parent = files::pin_directory(
        destination
            .parent()
            .ok_or_else(|| invalid("Link copy has no parent"))?,
    )?;
    let original = removal::inspect(source)?;
    if !super::super::filesystem::is_reparse(&original.file.metadata()?)
        || fs::read_link(source)? != expected_target
    {
        return Err(invalid("Source link changed"));
    }
    let buffer = data(&original.file)?;
    let directory = original.file.metadata()?.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0;
    let output = if directory {
        files::create_private_directory(destination)?;
        OpenOptions::new()
            .access_mode(GENERIC_READ | GENERIC_WRITE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(destination)?
    } else {
        files::create_private_file(destination)?
    };
    let created = files::identity(&output)?;
    let mut length = 0;
    checked(unsafe {
        DeviceIoControl(
            output.as_raw_handle(),
            FSCTL_SET_REPARSE_POINT,
            buffer.as_ptr().cast(),
            buffer.len() as u32,
            ptr::null_mut(),
            0,
            &mut length,
            ptr::null_mut(),
        )
    })?;
    let mut permissions = output.metadata()?.permissions();
    permissions.set_readonly(original.file.metadata()?.permissions().readonly());
    output.set_permissions(permissions)?;
    if !created.same_object(&removal::inspect(destination)?.identity)
        || fs::read_link(destination)? != expected_target
        || data(&original.file)? != buffer
        || !original
            .identity
            .same_object(&removal::inspect(source)?.identity)
    {
        return Err(invalid("Link identity or data changed while copying"));
    }
    Ok(())
}
