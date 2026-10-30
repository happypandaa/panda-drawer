//! Read-only workspace admission. Neither fixed-drive type nor a familiar
//! folder name supplies internal-device or cloud-sync evidence.
use super::{files, native_path, normalized_path, volume};
use std::{
    ffi::OsString,
    fs::{self, File},
    io,
    mem::size_of,
    os::windows::{ffi::OsStringExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
    sync::{mpsc, OnceLock},
};
use windows::Storage::Provider::StorageProviderSyncRootManager;
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
use windows_sys::Win32::{
    Foundation::{ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT, ERROR_NOT_A_CLOUD_FILE},
    Globalization::{CompareStringOrdinal, CSTR_EQUAL},
    Storage::{
        CloudFilters::{
            CfGetSyncRootInfoByHandle, CF_SYNC_ROOT_BASIC_INFO, CF_SYNC_ROOT_INFO_BASIC,
        },
        FileSystem::{
            GetFinalPathNameByHandleW, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY,
            FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, VOLUME_NAME_GUID,
        },
    },
};

fn unknown(message: impl ToString) -> io::Error {
    io::Error::other(message.to_string())
}
fn hresult_from_win32(code: u32) -> i32 {
    (0x8007_0000u32 | code) as i32
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CloudMembership {
    Registered { sync_root_file_id: i64 },
    NotRegistered,
}
fn cloud_membership(file: &File) -> io::Result<CloudMembership> {
    let mut information = CF_SYNC_ROOT_BASIC_INFO::default();
    let mut length = 0;
    let result = unsafe {
        CfGetSyncRootInfoByHandle(
            file.as_raw_handle(),
            CF_SYNC_ROOT_INFO_BASIC,
            (&mut information as *mut CF_SYNC_ROOT_BASIC_INFO).cast(),
            size_of::<CF_SYNC_ROOT_BASIC_INFO>() as u32,
            &mut length,
        )
    };
    if result == 0 {
        if length as usize != size_of::<CF_SYNC_ROOT_BASIC_INFO>() {
            return Err(unknown(
                "Cloud Filter sync-root response size is unconfirmed",
            ));
        }
        return Ok(CloudMembership::Registered {
            sync_root_file_id: information.SyncRootFileId,
        });
    }
    if result == hresult_from_win32(ERROR_NOT_A_CLOUD_FILE)
        || result == hresult_from_win32(ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT)
    {
        return Ok(CloudMembership::NotRegistered);
    }
    Err(unknown(format!(
        "Cloud Filter sync-root query is unconfirmed (HRESULT 0x{:08x})",
        result as u32
    )))
}

struct Apartment;
impl Apartment {
    fn enter() -> io::Result<Self> {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|error| {
            unknown(format!(
                "Sync-root WinRT apartment initialization failed: {error}"
            ))
        })?;
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}
fn registered_roots_in_apartment() -> io::Result<Vec<PathBuf>> {
    let roots = StorageProviderSyncRootManager::GetCurrentSyncRoots().map_err(|error| {
        unknown(format!(
            "System registered sync-root enumeration failed: {error}"
        ))
    })?;
    let count = roots.Size().map_err(unknown)?;
    let mut result = Vec::new();
    for index in 0..count {
        let information = roots.GetAt(index).map_err(unknown)?;
        let folder = information.Path().map_err(unknown)?;
        let path = folder.Path().map_err(unknown)?;
        if path.is_empty() {
            return Err(unknown("A system registered sync-root path is empty"));
        }
        result.push(PathBuf::from(OsString::from_wide(&path)));
    }
    Ok(result)
}

type SyncRootReply = mpsc::SyncSender<io::Result<Vec<PathBuf>>>;
fn registered_roots_on_worker() -> io::Result<Vec<PathBuf>> {
    // One owned MTA lasts for the process, rather than racing WinRT activation
    // against teardown of the last transient MTA on another querying thread.
    // Tauri STA and Tokio callers keep their own apartments. Initialization,
    // query and disconnected-worker errors remain explicit unknown results.
    static WORKER: OnceLock<io::Result<mpsc::Sender<SyncRootReply>>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (sender, receiver) = mpsc::channel::<SyncRootReply>();
        std::thread::Builder::new()
            .name("workspace-sync-roots".into())
            .spawn(move || {
                let apartment = Apartment::enter();
                for reply in receiver {
                    let result = match &apartment {
                        Ok(_) => registered_roots_in_apartment(),
                        Err(error) => Err(unknown(error)),
                    };
                    let _ = reply.send(result);
                }
            })?;
        Ok(sender)
    });
    let worker = worker.as_ref().map_err(unknown)?;
    let (reply, result) = mpsc::sync_channel(1);
    worker
        .send(reply)
        .map_err(|_| unknown("System sync-root query worker is unavailable"))?;
    result
        .recv()
        .map_err(|_| unknown("System sync-root query worker did not return a result"))?
}
fn within(path: &Path, root: &Path) -> io::Result<bool> {
    let mut path = native_path(path)?;
    path.pop();
    let mut root = native_path(root)?;
    root.pop();
    if path.len() < root.len() {
        return Ok(false);
    }
    let root_length = i32::try_from(root.len())
        .map_err(|_| unknown("Sync-root path exceeds native comparison range"))?;
    let equal =
        unsafe { CompareStringOrdinal(path.as_ptr(), root_length, root.as_ptr(), root_length, 1) };
    if equal == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(equal == CSTR_EQUAL
        && (path.len() == root.len()
            || root.last() == Some(&u16::from(b'\\'))
            || path.get(root.len()) == Some(&u16::from(b'\\'))))
}

fn normalized_volume_path(file: &File) -> io::Result<PathBuf> {
    if !files::identity(file)?.directory {
        return Err(unknown(
            "A registered synchronization root is not a directory",
        ));
    }
    let mut units = vec![0u16; 32768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            units.as_mut_ptr(),
            units.len() as u32,
            VOLUME_NAME_GUID,
        )
    };
    if length == 0 {
        return Err(unknown(format!(
            "Synchronization-root final volume path is unconfirmed: {}",
            io::Error::last_os_error()
        )));
    }
    if length as usize >= units.len() {
        return Err(unknown(
            "Synchronization-root final path exceeds native range",
        ));
    }
    units.truncate(length as usize);
    Ok(PathBuf::from(OsString::from_wide(&units)))
}

fn require_outside_registered_roots(selected: &File, roots: &[PathBuf]) -> io::Result<()> {
    // Resolve both sides through actual directory HANDLEs and the same volume
    // GUID namespace. DOS aliases, junctions and mounted-volume names are not
    // evidence that an OS-registered root is unrelated to this directory.
    let actual = normalized_volume_path(selected)?;
    for root in roots {
        // This is a system-reported root, not a candidate user workspace. Let
        // Windows resolve its registered alias; retain its actual directory
        // against rename without admitting or changing owner/ancestor ACLs.
        let directory = File::options()
            .access_mode(FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(root)
            .map_err(|error| {
                unknown(format!(
                    "Registered synchronization-root resolution is unconfirmed: {error}"
                ))
            })?;
        let root = normalized_volume_path(&directory)?;
        if within(&actual, &root)? {
            return Err(unknown(
                "Workspace is inside a system registered synchronization root",
            ));
        }
    }
    Ok(())
}

/// Only absence from both native inventories proves no registered sync-root
/// membership. No provider path names, credentials or project content are read.
pub fn require_unsynced(path: &Path) -> io::Result<()> {
    let selected = files::pin_directory(path)?;
    if matches!(
        cloud_membership(&selected.file)?,
        CloudMembership::Registered { .. }
    ) {
        return Err(unknown(
            "Workspace is inside a registered Cloud Files synchronization root",
        ));
    }
    require_outside_registered_roots(&selected.file, &registered_roots_on_worker()?)
}

pub fn require_local_internal_ntfs(info: &volume::VolumeInfo) -> io::Result<()> {
    if !info.filesystem.eq_ignore_ascii_case("NTFS") {
        return Err(unknown(format!(
            "Workspace requires NTFS; actual filesystem is {}",
            info.filesystem
        )));
    }
    if info.local() != Some(true) {
        return Err(unknown(format!(
            "Workspace requires a local disk; native drive type is {}",
            info.drive_type
        )));
    }
    match info.internal {
        Some(true)=>Ok(()),
        Some(false)=>Err(unknown("Workspace requires an internal disk; native device membership identifies an external disk")),
        None=>Err(unknown(format!("Workspace internal-disk membership is unconfirmed: {}",info.internal_diagnostic.as_deref().unwrap_or("no positive native membership evidence")))),
    }
}

/// Checks only volume policy for an existing workspace or the nearest existing ordinary ancestor of a
/// not-yet-created default. This never creates, empties or adopts the directory.
/// Callers recheck the actual workspace after private creation before commit.
/// Sync policy is deliberately separate: Mac applies it to migration targets,
/// not to the default workspace on first launch or every subsequent restart.
pub fn require_workspace_volume(path: &Path) -> io::Result<volume::VolumeInfo> {
    let path = normalized_path(path)?;
    let mut candidate = path.as_path();
    loop {
        match fs::symlink_metadata(candidate) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                candidate = candidate.parent().ok_or_else(|| {
                    unknown("Workspace has no existing absolute directory ancestor")
                })?
            }
            Err(error) => return Err(error),
        }
    }
    let _scope = files::pin_directory(candidate)?;
    let info = volume::inspect(candidate)?;
    require_local_internal_ntfs(&info)?;
    Ok(info)
}

#[cfg(test)]
#[path = "workspace/tests.rs"]
mod tests;
