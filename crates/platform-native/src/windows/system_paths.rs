//! System installation directories reported by Win32, independent of the caller's environment.
use std::{ffi::OsString, io, os::windows::ffi::OsStringExt, path::PathBuf};
use windows_sys::Win32::System::SystemInformation::{
    GetSystemDirectoryW, GetSystemWindowsDirectoryW,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemPaths {
    pub system: PathBuf,
    pub windows: PathBuf,
}

impl SystemPaths {
    pub fn current() -> io::Result<Self> {
        Ok(Self {
            system: query_directory(|buffer, capacity| unsafe {
                GetSystemDirectoryW(buffer, capacity)
            })?,
            windows: query_directory(|buffer, capacity| unsafe {
                GetSystemWindowsDirectoryW(buffer, capacity)
            })?,
        })
    }
}

fn query_directory(mut query: impl FnMut(*mut u16, u32) -> u32) -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 260];
    loop {
        let capacity = u32::try_from(buffer.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "System directory is too long")
        })?;
        let length = query(buffer.as_mut_ptr(), capacity);
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        let length = length as usize;
        if length >= buffer.len() {
            // Insufficient-buffer results include the terminating NUL. An extra
            // unit also allows the next call to observe a changed system value.
            let capacity = length.checked_add(1).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "System directory is too long")
            })?;
            buffer.resize(capacity, 0);
            continue;
        }
        buffer.truncate(length);
        let path = PathBuf::from(OsString::from_wide(&buffer));
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Win32 returned a non-absolute system directory",
            ));
        }
        return Ok(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::ffi::OsStrExt;

    #[test]
    fn actual_system_directories_are_absolute_existing_installation_paths() {
        let paths = SystemPaths::current().unwrap();
        assert!(paths.system.is_absolute() && paths.system.is_dir());
        assert!(paths.windows.is_absolute() && paths.windows.is_dir());
        assert!(paths.system.join("cmd.exe").is_file());
    }

    #[test]
    fn query_grows_the_buffer_and_preserves_native_utf16() {
        let mut expected: Vec<u16> = "D:\\系统 中文\\".encode_utf16().collect();
        expected.extend(std::iter::repeat_n(u16::from(b'x'), 300));
        expected.push(0xd800);
        let mut calls = 0;
        let actual = query_directory(|buffer, capacity| {
            calls += 1;
            if capacity as usize <= expected.len() {
                return expected.len() as u32 + 1;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(expected.as_ptr(), buffer, expected.len());
                *buffer.add(expected.len()) = 0;
            }
            expected.len() as u32
        })
        .unwrap();
        assert_eq!(calls, 2);
        assert_eq!(
            actual.as_os_str().encode_wide().collect::<Vec<_>>(),
            expected
        );
    }

    #[test]
    fn failed_or_relative_system_queries_have_no_environment_fallback() {
        assert!(query_directory(|_, _| 0).is_err());
        assert!(query_directory(|buffer, _| {
            unsafe { *buffer = u16::from(b'x') };
            1
        })
        .is_err());
    }
}
