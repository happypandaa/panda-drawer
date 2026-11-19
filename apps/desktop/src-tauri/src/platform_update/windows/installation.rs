//! Bind the running image to the current user's Tauri NSIS registration.
use ::windows::{
    core::PCWSTR,
    Win32::{
        Foundation::ERROR_SUCCESS,
        Storage::FileSystem::{
            GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        },
        System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ},
    },
};
use platform_core::build_profile::{APP_IDENTIFIER, DISPLAY_NAME};
use platform_native::windows::files;
use std::{
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io,
    os::windows::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    ptr,
};

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Unexpected installed application identity",
    )
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

fn reg_string(key: &[u16], name: &str) -> io::Result<String> {
    let mut buf = vec![0u16; 32768];
    let mut size = (buf.len() * 2) as u32;
    let value = wide(OsStr::new(name));
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status.0 as i32));
    }
    let data = buf
        .get(..size as usize / 2)
        .and_then(|s| s.strip_suffix(&[0]))
        .ok_or_else(invalid)?;
    if data.is_empty() || data.contains(&0) {
        return Err(invalid());
    }
    String::from_utf16(data).map_err(|_| invalid())
}

/// Hold the checked object against replacement and modification until use by CreateProcess.
pub(super) fn pin_file(path: &Path) -> io::Result<File> {
    let checked = files::open_no_reparse(path)?;
    let pin = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let id = files::identity(&pin)?;
    if id.directory || id.links != 1 || !id.same_object(&checked.identity) {
        return Err(invalid());
    }
    Ok(pin)
}

fn image_version(path: &Path, expected: &str) -> io::Result<()> {
    let path = wide(path.as_os_str());
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(path.as_ptr()), None) };
    if size == 0 || size > 1024 * 1024 {
        return Err(invalid());
    }
    let mut bytes = vec![0u32; (size as usize).div_ceil(4)];
    unsafe { GetFileVersionInfoW(PCWSTR(path.as_ptr()), None, size, bytes.as_mut_ptr().cast()) }
        .map_err(io::Error::other)?;
    let query = |key: &str| {
        let key = wide(OsStr::new(key));
        let (mut pointer, mut length) = (ptr::null_mut(), 0);
        if !unsafe {
            VerQueryValueW(
                bytes.as_ptr().cast(),
                PCWSTR(key.as_ptr()),
                &mut pointer,
                &mut length,
            )
        }
        .as_bool()
            || pointer.is_null()
            || length == 0
        {
            return Err(invalid());
        }
        Ok((pointer.cast::<u16>(), length as usize))
    };
    let (translation, length) = query(r"\VarFileInfo\Translation")?;
    if length < 4 {
        return Err(invalid());
    }
    let (language, page) = unsafe { (*translation, *translation.add(1)) };
    for (name, value) in [("ProductName", DISPLAY_NAME), ("ProductVersion", expected)] {
        let (text, n) = query(&format!(r"\StringFileInfo\{language:04x}{page:04x}\{name}"))?;
        let actual = unsafe { std::slice::from_raw_parts(text, n) };
        if actual.strip_suffix(&[0]) != Some(value.encode_utf16().collect::<Vec<_>>().as_slice()) {
            return Err(invalid());
        }
    }
    Ok(())
}

pub(super) struct Installation {
    // Exact conventional path from NSIS. Its /D parameter does not accept quotes.
    pub directory: PathBuf,
    pub executable: PathBuf,
    pub directory_pin: files::PinnedDirectory,
    pub executable_pin: File,
}

impl Installation {
    pub fn inspect(actual: &Path, version: &str) -> io::Result<Self> {
        let key = wide(OsStr::new(&format!(
            r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{DISPLAY_NAME}"
        )));
        let folder = reg_string(&key, "InstallLocation")?;
        let folder = folder
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(&folder);
        let directory = PathBuf::from(folder);
        let name = reg_string(&key, "MainBinaryName")?;
        if !directory.is_absolute()
            || folder.contains('"')
            || directory.parent().is_none()
            || Path::new(&name).file_name() != Some(OsStr::new(&name))
            || actual.file_name() != Some(OsStr::new(&name))
            || reg_string(&key, "DisplayVersion")? != version
            || reg_string(&key, "DisplayName")? != DISPLAY_NAME
            || Some(reg_string(&key, "Publisher")?.as_str()) != APP_IDENTIFIER.split('.').nth(1)
        {
            return Err(invalid());
        }
        let directory_pin = files::pin_directory(&directory)?;
        let executable = directory.join(name);
        let executable_pin = pin_file(&executable)?;
        let id = files::identity(&executable_pin)?;
        if !id.same_object(&files::open_no_reparse(actual)?.identity) {
            return Err(invalid());
        }
        image_version(&executable, version)?;
        Ok(Self {
            directory,
            executable,
            directory_pin,
            executable_pin,
        })
    }

    pub fn nsis_directory_arg(&self) -> OsString {
        let mut argument = OsString::from("/D=");
        argument.push(&self.directory);
        argument
    }
}
