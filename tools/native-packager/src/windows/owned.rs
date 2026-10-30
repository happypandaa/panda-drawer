//! Empty-directory removal is bound to a DELETE handle and the complete ID.
//! This is developer staging cleanup, never application/project deletion.
use super::*;
use std::{
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle},
    },
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Storage::FileSystem::{
        CreateFileW, FileDispositionInfo, SetFileInformationByHandle, DELETE,
        FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    },
};

pub(super) fn remove_directory(path: &Path, expected: &native::FileIdentity) -> Result<()> {
    let _parent = native::pin_directory(path.parent().ok_or_else(|| invalid("Parent missing"))?)?;
    // Keep native UTF-16, including unpaired surrogates in an existing parent.
    let normalized: PathBuf = path.components().collect();
    let raw: Vec<u16> = normalized.as_os_str().encode_wide().collect();
    let extended: Vec<u16> = if raw.starts_with(&[92, 92, 63, 92]) {
        raw
    } else if raw.starts_with(&[92, 92]) {
        "\\\\?\\UNC\\"
            .encode_utf16()
            .chain(raw.into_iter().skip(2))
            .collect()
    } else {
        "\\\\?\\".encode_utf16().chain(raw).collect()
    };
    if extended.contains(&0) {
        return Err(invalid("NUL in staging path"));
    }
    let name: Vec<u16> = extended.into_iter().chain(Some(0)).collect();
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            DELETE | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    let actual = native::identity(&file)?;
    if !actual.directory
        || !actual.same_object(expected)
        || platform_native::filesystem::is_reparse(&file.metadata()?)
    {
        return Err(invalid("Staging directory identity changed; retained"));
    }
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
