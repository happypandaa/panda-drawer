//! Windows x64 primitives. Business code calls the platform facade, never Win32.
pub mod files;
pub mod process;

use std::{
    ffi::OsStr,
    io,
    os::windows::ffi::OsStrExt,
    path::{Component, Path},
};

pub(crate) fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut result: Vec<_> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in native argument",
        ));
    }
    result.push(0);
    Ok(result)
}

pub(crate) fn native_path(path: &Path) -> io::Result<Vec<u16>> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected an absolute path without traversal",
        ));
    }
    // Extended paths work without LongPathsEnabled and preserve Unicode. Avoid
    // converting an OsStr through UTF-8, which would corrupt unpaired surrogates.
    let raw: Vec<u16> = path.as_os_str().encode_wide().collect();
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
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "NUL in native path",
        ));
    }
    Ok(extended.into_iter().chain([0]).collect())
}

pub(crate) fn checked(result: i32) -> io::Result<()> {
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
