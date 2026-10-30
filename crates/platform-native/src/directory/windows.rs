use super::{Change, Entry, Kind, ObjectIdentity, Stamp};
use crate::windows::{checked, files};
use std::{
    ffi::{OsStr, OsString},
    fs::{File, ReadDir},
    io,
    mem::{size_of, zeroed},
    os::windows::io::AsRawHandle,
};
use windows_sys::Win32::Storage::FileSystem::{
    FileBasicInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO,
};

fn object(value: files::FileIdentity) -> ObjectIdentity {
    ObjectIdentity::Windows {
        volume_serial: value.volume_serial,
        file_id: value.file_id,
    }
}
pub fn stamp(file: &File) -> io::Result<Stamp> {
    let identity = object(files::identity(file)?);
    let mut basic: FILE_BASIC_INFO = unsafe { zeroed() };
    unsafe {
        checked(GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        ))?;
    }
    Ok(Stamp {
        identity,
        change: Change::Windows {
            length: file.metadata()?.len(),
            attributes: basic.FileAttributes,
            modified_100ns: basic.LastWriteTime,
            changed_100ns: basic.ChangeTime,
        },
    })
}
pub fn child_directory(parent: &File, name: &OsStr) -> io::Result<File> {
    super::valid_name(name)?;
    let (path, _scope) = files::directory_scope(parent)?;
    files::directory_file(&path.join(name))
}
pub struct Names {
    entries: ReadDir,
    _scope: files::PinnedDirectory,
}
impl Names {
    pub fn open(directory: &File) -> io::Result<Self> {
        let (path, scope) = files::directory_scope(directory)?;
        Ok(Self {
            entries: std::fs::read_dir(path)?,
            _scope: scope,
        })
    }
    pub fn next_name(&mut self) -> io::Result<Option<OsString>> {
        self.entries
            .next()
            .map(|entry| entry.map(|entry| entry.file_name()))
            .transpose()
    }
}
pub fn inspect(directory: &File, name: &OsStr) -> io::Result<Entry> {
    super::valid_name(name)?;
    let (path, _scope) = files::directory_scope(directory)?;
    let path = path.join(name);
    let metadata = std::fs::symlink_metadata(&path)?;
    if crate::filesystem::is_reparse(&metadata) || (!metadata.is_dir() && !metadata.is_file()) {
        return Ok(Entry {
            kind: Kind::Other,
            identity: None,
        });
    }
    let file = files::open_no_reparse(&path)?;
    Ok(Entry {
        kind: if file.identity.directory {
            Kind::Directory
        } else {
            Kind::Regular
        },
        identity: Some(object(file.identity)),
    })
}
