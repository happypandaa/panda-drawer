//! Read-only facts about the volume of an actual opened directory. A fixed
//! drive is not proof of an internal device or permission to migrate a workspace.
use super::{checked, files, native_path};
use std::{io, os::windows::io::AsRawHandle, path::Path, ptr};
use windows_sys::Win32::Storage::FileSystem::{
    GetDiskFreeSpaceExW, GetDriveTypeW, GetVolumeInformationByHandleW, GetVolumePathNameW,
};

#[derive(Debug)]
pub struct VolumeInfo {
    pub filesystem: String,
    pub volume_serial: u64,
    pub available_bytes: u64,
    pub total_bytes: u64,
    /// The actual GetDriveTypeW result; unknown values remain unknown.
    pub drive_type: u32,
}

impl VolumeInfo {
    pub fn local(&self) -> Option<bool> {
        // GetDriveType values: removable=2, fixed=3, remote=4, CD=5, RAM=6.
        // A removable or fixed local volume can still be an external device.
        match self.drive_type {
            2 | 3 | 5 | 6 => Some(true),
            4 => Some(false),
            _ => None,
        }
    }
}

pub fn inspect(path: &Path) -> io::Result<VolumeInfo> {
    let directory = files::directory_file(path)?;
    let (actual, _scope) = files::directory_scope(&directory)?;
    let identity = files::identity(&directory)?;
    let mut filesystem = [0u16; 256];
    checked(unsafe {
        GetVolumeInformationByHandleW(
            directory.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    })?;
    let end = filesystem
        .iter()
        .position(|unit| *unit == 0)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Unterminated filesystem name")
        })?;
    let filesystem = String::from_utf16(&filesystem[..end])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid filesystem name"))?;
    if filesystem.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Missing filesystem name",
        ));
    }
    let actual = native_path(&actual)?;
    let mut root = vec![0u16; 32768];
    checked(unsafe { GetVolumePathNameW(actual.as_ptr(), root.as_mut_ptr(), root.len() as u32) })?;
    let drive_type = unsafe { GetDriveTypeW(root.as_ptr()) };
    let mut available_bytes = 0;
    let mut total_bytes = 0;
    checked(unsafe {
        GetDiskFreeSpaceExW(
            actual.as_ptr(),
            &mut available_bytes,
            &mut total_bytes,
            ptr::null_mut(),
        )
    })?;
    Ok(VolumeInfo {
        filesystem,
        volume_serial: identity.volume_serial,
        available_bytes,
        total_bytes,
        drive_type,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn observes_the_actual_volume_without_modifying_a_disposable_directory() {
        let temp = tempfile::Builder::new()
            .prefix("panda-disposable-volume-中文 space-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let file = root.join("retained.txt");
        fs::write(&file, b"retained source").unwrap();
        let before = files::open_no_reparse(&root).unwrap().identity;
        let volume = inspect(&root).unwrap();
        assert!(!volume.filesystem.is_empty());
        assert_eq!(volume.volume_serial, before.volume_serial);
        assert!(volume.total_bytes > 0);
        assert!(volume.available_bytes <= volume.total_bytes);
        assert!(before.same_object(&files::open_no_reparse(&root).unwrap().identity));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        assert_eq!(fs::read(file).unwrap(), b"retained source");
        eprintln!("Actual test volume: {volume:?}");
    }

    #[test]
    fn missing_paths_and_files_are_not_directory_volume_observations() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let missing = root.join("missing");
        assert!(inspect(&missing).is_err());
        assert!(!missing.exists());
        let file = root.join("ordinary.txt");
        fs::write(&file, b"retained").unwrap();
        assert!(inspect(&file).is_err());
        assert_eq!(fs::read(file).unwrap(), b"retained");
    }

    #[test]
    fn unknown_drive_types_do_not_supply_a_local_or_internal_claim() {
        let mut info = VolumeInfo {
            filesystem: String::new(),
            volume_serial: 0,
            available_bytes: 0,
            total_bytes: 0,
            drive_type: 0,
        };
        for raw in [0, 1, 7, u32::MAX] {
            info.drive_type = raw;
            assert_eq!(info.local(), None);
        }
        info.drive_type = 4;
        assert_eq!(info.local(), Some(false));
        for raw in [2, 3, 5, 6] {
            info.drive_type = raw;
            assert_eq!(info.local(), Some(true));
        }
    }
}
