//! Handle-bound Win32 WAL-index mappings and locks.
//!
//! This implements only the four SQLite shared-memory methods. It does not
//! open paths, create/adopt sidecars, delete files, or implement main/WAL I/O.
//! The caller must use the same registered file set for its native main/WAL
//! handles and keep its SQLite connection serialized on its worker thread.
use crate::{SqliteFileKind, SqliteFileSet};
use platform_project_format::ApiError;
use rusqlite::ffi;
use std::{
    collections::HashSet,
    ffi::c_void,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{null, null_mut},
    sync::{
        atomic::{fence, Ordering},
        Mutex, OnceLock,
    },
};
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_LOCK_VIOLATION, HANDLE},
    Storage::FileSystem::{
        GetFileSizeEx, LockFileEx, SetEndOfFile, SetFilePointerEx, UnlockFileEx, FILE_BEGIN,
        LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    },
    System::{
        Memory::{
            CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE,
            MEMORY_MAPPED_VIEW_ADDRESS, PAGE_READWRITE,
        },
        SystemInformation::{GetSystemInfo, SYSTEM_INFO},
        IO::OVERLAPPED,
    },
};

type Result<T> = std::result::Result<T, ApiError>;
type FileKey = (u64, [u8; 16]);

// These offsets are the stock Win32 WAL lock protocol, not application locks.
// SQLite's WalCkptInfo has eight lock bytes starting at byte 120; the Win32
// dead-man-switch byte follows them at byte 128.
const SHM_BASE: u32 = 120;
const SHM_DMS: u32 = SHM_BASE + 8;
const SHM_LOCK_COUNT: i32 = 8;

fn failure(code: &str, message: &str, sqlite_code: i32) -> ApiError {
    ApiError::new(code, message).detail("sqliteCode", sqlite_code)
}

fn os_failure(operation: &str, sqlite_code: i32) -> ApiError {
    let error = std::io::Error::last_os_error();
    failure(
        "DATABASE_SHM_IO",
        &format!("SQLite 共享内存{operation}失败：{error}"),
        sqlite_code,
    )
    .detail("win32Code", error.raw_os_error())
}

fn binding_failure(error: crate::WorkspaceError, sqlite_code: i32) -> ApiError {
    failure(error.code, &error.message, sqlite_code)
}

/// Explicitly reject a second custom node for the same physical SHM object.
/// Stock SQLite clients, including another connection in this process, still
/// coordinate through the standard Windows byte-range locks on their own
/// handles. The registry does not attempt to emulate their private node list.
struct Reservation(FileKey);

fn reservations() -> &'static Mutex<HashSet<FileKey>> {
    static ACTIVE: OnceLock<Mutex<HashSet<FileKey>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashSet::new()))
}

impl Reservation {
    fn acquire(key: FileKey) -> Result<Self> {
        let mut active = reservations().lock().map_err(|_| {
            failure(
                "DATABASE_SHM_REGISTRY_POISONED",
                "共享内存登记锁异常；未开启另一个节点。",
                ffi::SQLITE_IOERR_SHMOPEN,
            )
        })?;
        if active.contains(&key) {
            return Err(failure(
                "DATABASE_BUSY",
                "同一共享内存文件已有平台连接。",
                ffi::SQLITE_BUSY,
            ));
        }
        active.try_reserve(1).map_err(|_| {
            failure(
                "DATABASE_SHM_ALLOCATION_FAILED",
                "无法登记共享内存连接。",
                ffi::SQLITE_NOMEM,
            )
        })?;
        active.insert(key);
        Ok(Self(key))
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        // A poisoned registry stays closed to new connections. Cleanup must
        // never panic, especially when invoked from an SQLite FFI callback.
        if let Ok(mut active) = reservations().lock() {
            active.remove(&self.0);
        }
    }
}

struct Region {
    view: MEMORY_MAPPED_VIEW_ADDRESS,
    _mapping: OwnedHandle,
    shift: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct MappingLayout {
    end: u64,
    offset: u64,
    shift: usize,
    length: usize,
}

fn mapping_layout(region: i32, size: i32, granularity: u64) -> Result<MappingLayout> {
    let invalid = || {
        failure(
            "DATABASE_SHM_MAP_INVALID",
            "共享内存区域、尺寸或系统对齐值无效。",
            ffi::SQLITE_IOERR_SHMMAP,
        )
    };
    if region < 0 || size <= 0 || granularity == 0 {
        return Err(invalid());
    }
    let start = (region as u64)
        .checked_mul(size as u64)
        .ok_or_else(invalid)?;
    let end = start.checked_add(size as u64).ok_or_else(invalid)?;
    if end > i64::MAX as u64 {
        return Err(invalid());
    }
    let shift = usize::try_from(start % granularity).map_err(|_| invalid())?;
    let length = (size as usize).checked_add(shift).ok_or_else(invalid)?;
    if length > isize::MAX as usize {
        return Err(invalid());
    }
    Ok(MappingLayout {
        end,
        offset: start - shift as u64,
        shift,
        length,
    })
}

#[derive(Debug)]
struct LockRequest {
    offset: u32,
    count: u32,
    mask: u16,
    exclusive: bool,
    unlock: bool,
}

fn lock_request(offset: i32, count: i32, flags: i32) -> Result<LockRequest> {
    let unlock = flags == (ffi::SQLITE_SHM_UNLOCK | ffi::SQLITE_SHM_SHARED)
        || flags == (ffi::SQLITE_SHM_UNLOCK | ffi::SQLITE_SHM_EXCLUSIVE);
    let exclusive = flags == (ffi::SQLITE_SHM_UNLOCK | ffi::SQLITE_SHM_EXCLUSIVE)
        || flags == (ffi::SQLITE_SHM_LOCK | ffi::SQLITE_SHM_EXCLUSIVE);
    let valid_flags = unlock
        || flags == (ffi::SQLITE_SHM_LOCK | ffi::SQLITE_SHM_SHARED)
        || flags == (ffi::SQLITE_SHM_LOCK | ffi::SQLITE_SHM_EXCLUSIVE);
    if !valid_flags
        || offset < 0
        || count <= 0
        || count > SHM_LOCK_COUNT
        || offset > SHM_LOCK_COUNT - count
        || (!exclusive && count != 1)
    {
        return Err(failure(
            "DATABASE_SHM_LOCK_INVALID",
            "共享内存锁范围或模式无效。",
            ffi::SQLITE_IOERR_SHMLOCK,
        ));
    }
    Ok(LockRequest {
        offset: SHM_BASE + offset as u32,
        count: count as u32,
        mask: (((1u16 << count) - 1) << offset),
        exclusive,
        unlock,
    })
}

fn lock_handle(handle: HANDLE, offset: u32, count: u32, exclusive: bool) -> Result<()> {
    let mut overlapped = OVERLAPPED::default();
    // SAFETY: OVERLAPPED's offset union is initialized before this synchronous,
    // nonblocking call. The owned handle remains open throughout the call.
    overlapped.Anonymous.Anonymous.Offset = offset;
    let flags = LOCKFILE_FAIL_IMMEDIATELY
        | if exclusive {
            LOCKFILE_EXCLUSIVE_LOCK
        } else {
            0
        };
    // SAFETY: the handle is a live, synchronous NtCreateFile result; the
    // stack OVERLAPPED is valid until this non-overlapped operation returns.
    if unsafe { LockFileEx(handle, flags, 0, count, 0, &mut overlapped) } != 0 {
        Ok(())
    } else if unsafe { GetLastError() } == ERROR_LOCK_VIOLATION {
        Err(failure(
            "DATABASE_BUSY",
            "共享内存锁正由其他 SQLite 连接占用。",
            ffi::SQLITE_BUSY,
        ))
    } else {
        Err(os_failure("加锁", ffi::SQLITE_IOERR_SHMLOCK))
    }
}

fn unlock_handle(handle: HANDLE, offset: u32, count: u32) -> Result<()> {
    let mut overlapped = OVERLAPPED::default();
    // SAFETY: the same live handle, offset and range are used as for locking.
    overlapped.Anonymous.Anonymous.Offset = offset;
    if unsafe { UnlockFileEx(handle, 0, count, 0, &mut overlapped) } != 0 {
        Ok(())
    } else {
        Err(os_failure("解锁", ffi::SQLITE_IOERR_SHMLOCK))
    }
}

fn truncate(handle: HANDLE, size: u64) -> Result<()> {
    // SAFETY: this is the node's private mapping/DMS handle. It is not shared
    // with another thread or used by stock main/WAL I/O; all callers serialize
    // its file pointer and validate the size before this conversion.
    if unsafe { SetFilePointerEx(handle, size as i64, null_mut(), FILE_BEGIN) } == 0 {
        return Err(os_failure("定位", ffi::SQLITE_IOERR_SHMSIZE));
    }
    if unsafe { SetEndOfFile(handle) } == 0 {
        return Err(os_failure("调整尺寸", ffi::SQLITE_IOERR_SHMSIZE));
    }
    Ok(())
}

/// One serialized connection to an existing, registered WAL-index file.
/// Every mapping remains stable until `close`; returned pointers must never
/// outlive that call. The type deliberately contains raw views and has no
/// unsafe Send/Sync implementation. Construct it on its SQLite worker thread.
pub(crate) struct ShmConnection {
    group: Option<SqliteFileSet>,
    reservation: Option<Reservation>,
    mapping_handle: Option<OwnedHandle>,
    locking_handle: Option<OwnedHandle>,
    regions: Vec<Option<Region>>,
    region_size: Option<i32>,
    granularity: u64,
    shared_mask: u16,
    exclusive_mask: u16,
    closing: bool,
    closed: bool,
}

impl ShmConnection {
    pub(crate) fn open(group: &SqliteFileSet) -> Result<Self> {
        group
            .verify_binding()
            .map_err(|e| binding_failure(e, ffi::SQLITE_IOERR_SHMOPEN))?;
        let identity = group.file(SqliteFileKind::Shm).identity();
        let reservation = Reservation::acquire((identity.volume_serial, identity.file_id128))?;
        // Two separate relative opens are required: a duplicated handle is not
        // a replacement for an independently opened connection-lock handle.
        let mapping_handle = group
            .open_readwrite_handle(SqliteFileKind::Shm)
            .map_err(|e| binding_failure(e, ffi::SQLITE_IOERR_SHMOPEN))?;
        let locking_handle = group
            .open_readwrite_handle(SqliteFileKind::Shm)
            .map_err(|e| binding_failure(e, ffi::SQLITE_IOERR_SHMOPEN))?;
        let mut system_info = SYSTEM_INFO::default();
        // SAFETY: GetSystemInfo writes the provided initialized structure.
        unsafe { GetSystemInfo(&mut system_info) };
        if system_info.dwAllocationGranularity == 0 {
            return Err(failure(
                "DATABASE_SHM_ALIGNMENT_INVALID",
                "系统没有提供共享内存映射对齐值。",
                ffi::SQLITE_IOERR_SHMMAP,
            ));
        }
        let handle = mapping_handle.as_raw_handle();
        match lock_handle(handle, SHM_DMS, 1, true) {
            Ok(()) => {
                // Stock Win32's DMS protocol: exclusive means there are no
                // other SHM clients. Only this branch may reset the wal-index;
                // the main database and WAL are never truncated here.
                let resize = group
                    .verify_binding()
                    .map_err(|e| binding_failure(e, ffi::SQLITE_IOERR_SHMOPEN))
                    .and_then(|()| truncate(handle, 0));
                let unlock = unlock_handle(handle, SHM_DMS, 1);
                resize?;
                unlock?;
            }
            Err(error) if error.details["sqliteCode"] == ffi::SQLITE_BUSY => {}
            Err(error) => return Err(error),
        }
        // Hold DMS SHARED until all views are unmapped and this node closes.
        // If another initializer temporarily holds EXCLUSIVE, return BUSY;
        // the VFS/SQLite caller controls retries, never this module.
        lock_handle(handle, SHM_DMS, 1, false)?;
        Ok(Self {
            group: Some(group.clone()),
            reservation: Some(reservation),
            mapping_handle: Some(mapping_handle),
            locking_handle: Some(locking_handle),
            regions: Vec::new(),
            region_size: None,
            granularity: u64::from(system_info.dwAllocationGranularity),
            shared_mask: 0,
            exclusive_mask: 0,
            closing: false,
            closed: false,
        })
    }

    fn verify_open(&self, sqlite_code: i32) -> Result<()> {
        if self.closing || self.closed {
            return Err(failure(
                "DATABASE_SHM_CLOSED",
                "共享内存连接正在关闭或已关闭。",
                sqlite_code,
            ));
        }
        match self.group.as_ref() {
            Some(group) => group
                .verify_binding()
                .map_err(|e| binding_failure(e, sqlite_code)),
            None => Err(failure(
                "DATABASE_SHM_CLOSED",
                "共享内存文件保护组已释放。",
                sqlite_code,
            )),
        }
    }

    pub(crate) fn map(&mut self, region: i32, size: i32, extend: bool) -> Result<*mut c_void> {
        self.verify_open(ffi::SQLITE_IOERR_SHMMAP)?;
        let layout = mapping_layout(region, size, self.granularity)?;
        if self.region_size.is_some_and(|existing| existing != size) {
            return Err(failure(
                "DATABASE_SHM_MAP_INVALID",
                "同一共享内存连接不能变更区域尺寸。",
                ffi::SQLITE_IOERR_SHMMAP,
            ));
        }
        let index = region as usize;
        if let Some(Some(mapped)) = self.regions.get(index) {
            // SAFETY: the view remains live until close, and shift is bounded
            // by the successfully mapped length in mapping_layout.
            return Ok(unsafe { mapped.view.Value.cast::<u8>().add(mapped.shift).cast() });
        }
        let handle = self
            .mapping_handle
            .as_ref()
            .ok_or_else(|| {
                failure(
                    "DATABASE_SHM_CLOSED",
                    "共享内存句柄已关闭。",
                    ffi::SQLITE_IOERR_SHMMAP,
                )
            })?
            .as_raw_handle();
        let mut file_size = 0i64;
        // SAFETY: the live verified handle and writable i64 are valid.
        if unsafe { GetFileSizeEx(handle, &mut file_size) } == 0 {
            return Err(os_failure("读取尺寸", ffi::SQLITE_IOERR_SHMSIZE));
        }
        if file_size < 0 {
            return Err(failure(
                "DATABASE_SHM_SIZE_INVALID",
                "共享内存文件尺寸无效。",
                ffi::SQLITE_IOERR_SHMSIZE,
            ));
        }
        if (file_size as u64) < layout.end && !extend {
            return Ok(null_mut());
        }
        if self.regions.len() <= index {
            self.regions
                .try_reserve(index + 1 - self.regions.len())
                .map_err(|_| {
                    failure(
                        "DATABASE_SHM_ALLOCATION_FAILED",
                        "无法保存共享内存映射。",
                        ffi::SQLITE_NOMEM,
                    )
                })?;
            self.regions.resize_with(index + 1, || None);
        }
        // SAFETY: unnamed mapping is created only from the verified file
        // handle. PAGE_READWRITE with a larger maximum extends the file without
        // moving existing views. SetEndOfFile while earlier views are mapped is
        // deliberately avoided; SQLite retains their pointers until unmap.
        let raw_mapping = unsafe {
            CreateFileMappingW(
                handle,
                null(),
                PAGE_READWRITE,
                (layout.end >> 32) as u32,
                layout.end as u32,
                null(),
            )
        };
        if raw_mapping.is_null() {
            return Err(os_failure("创建映射", ffi::SQLITE_IOERR_SHMMAP));
        }
        // SAFETY: CreateFileMappingW returned a new owned, non-null handle.
        let mapping = unsafe { OwnedHandle::from_raw_handle(raw_mapping) };
        // SAFETY: offset is aligned to allocation granularity and the full
        // view fits within this file mapping. No executable permission is used.
        let view = unsafe {
            MapViewOfFile(
                mapping.as_raw_handle(),
                FILE_MAP_READ | FILE_MAP_WRITE,
                (layout.offset >> 32) as u32,
                layout.offset as u32,
                layout.length,
            )
        };
        if view.Value.is_null() {
            return Err(os_failure("映射视图", ffi::SQLITE_IOERR_SHMMAP));
        }
        // SAFETY: the requested region is contained in the mapped view. The
        // Region now owns its mapping handle and retains the original base
        // address for UnmapViewOfFile, not the shifted pointer returned to C.
        let pointer = unsafe { view.Value.cast::<u8>().add(layout.shift).cast() };
        self.regions[index] = Some(Region {
            view,
            _mapping: mapping,
            shift: layout.shift,
        });
        self.region_size = Some(size);
        Ok(pointer)
    }

    pub(crate) fn lock(&mut self, offset: i32, count: i32, flags: i32) -> Result<()> {
        let request = lock_request(offset, count, flags)?;
        // Cleanup must still be able to unlock after an ACL/identity check
        // fails. Acquiring any new lock requires the complete group check.
        if !request.unlock {
            self.verify_open(ffi::SQLITE_IOERR_SHMLOCK)?;
        }
        let handle = self
            .locking_handle
            .as_ref()
            .ok_or_else(|| {
                failure(
                    "DATABASE_SHM_CLOSED",
                    "共享内存锁句柄已关闭。",
                    ffi::SQLITE_IOERR_SHMLOCK,
                )
            })?
            .as_raw_handle();
        let held = if request.exclusive {
            self.exclusive_mask
        } else {
            self.shared_mask
        };
        if request.unlock {
            // Match stock winShmLock's harmless no-op for a range on which
            // this connection holds no lock (for example repeated cleanup).
            if (self.shared_mask | self.exclusive_mask) & request.mask == 0 {
                return Ok(());
            }
            if held & request.mask != request.mask {
                return Err(failure(
                    "DATABASE_SHM_LOCK_INVALID",
                    "解锁范围与当前持有的共享内存锁不符。",
                    ffi::SQLITE_IOERR_SHMLOCK,
                ));
            }
            unlock_handle(handle, request.offset, request.count)?;
            self.shared_mask &= !request.mask;
            self.exclusive_mask &= !request.mask;
        } else {
            if !request.exclusive && self.shared_mask & request.mask == request.mask {
                return Ok(());
            }
            if (self.shared_mask | self.exclusive_mask) & request.mask != 0 {
                return Err(failure(
                    "DATABASE_SHM_LOCK_INVALID",
                    "共享内存锁必须先解锁才能切换模式。",
                    ffi::SQLITE_IOERR_SHMLOCK,
                ));
            }
            lock_handle(handle, request.offset, request.count, request.exclusive)?;
            if request.exclusive {
                self.exclusive_mask |= request.mask;
            } else {
                self.shared_mask |= request.mask;
            }
        }
        Ok(())
    }

    pub(crate) fn barrier(&self) {
        // A hardware/compiler fence is required for the wal-index's paired
        // volatile header reads/writes in SQLite, including other processes.
        // It does not flush a durable database and is not a replacement for
        // the byte-range locking protocol.
        fence(Ordering::SeqCst);
    }

    /// Unmap views and release locks; never delete/truncate a file on close.
    /// The outer VFS must use PERSIST_WAL and retain the registered sidecars.
    /// Callers must stop using all pointers before invoking this method.
    pub(crate) fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closing = true;
        let mut first_error = None;
        for slot in self.regions.iter_mut().rev() {
            if let Some(mapped) = slot.as_ref() {
                // SAFETY: SQLite no longer uses any returned pointer. This is
                // the original, live MapViewOfFile base and is unmapped once.
                if unsafe { UnmapViewOfFile(mapped.view) } == 0 {
                    if first_error.is_none() {
                        first_error = Some(os_failure("解除映射", ffi::SQLITE_IOERR_SHMMAP));
                    }
                } else {
                    // Closing the section handle follows successful unmapping.
                    slot.take();
                }
            }
        }
        if let Some(error) = first_error {
            // Retain DMS, locks and reservation while a view may still be live.
            // A subsequent close may retry cleanup, but map/lock stays closed.
            return Err(error);
        }
        self.regions.clear();
        self.locking_handle.take();
        self.shared_mask = 0;
        self.exclusive_mask = 0;
        self.mapping_handle.take();
        self.group.take();
        self.reservation.take();
        self.closed = true;
        Ok(())
    }
}

impl Drop for ShmConnection {
    fn drop(&mut self) {
        if self.close().is_err() {
            // An OS unmap failure must not release DMS and allow a new node to
            // truncate a file that still has a live view. Leak only this failed
            // node's protected resources until process exit; leave its registry
            // reservation occupied. No panic or path-based repair is attempted.
            std::mem::forget(std::mem::take(&mut self.regions));
            std::mem::forget(self.locking_handle.take());
            std::mem::forget(self.mapping_handle.take());
            std::mem::forget(self.group.take());
            std::mem::forget(self.reservation.take());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappings_align_to_allocation_granularity_without_losing_region_offset() {
        assert_eq!(
            mapping_layout(1, 32768, 65536).unwrap(),
            MappingLayout {
                end: 65536,
                offset: 0,
                shift: 32768,
                length: 65536,
            }
        );
        assert_eq!(mapping_layout(2, 32768, 65536).unwrap().offset, 65536);
        assert_eq!(mapping_layout(3, 32768, 65536).unwrap().shift, 32768);
        assert!(mapping_layout(-1, 32768, 65536).is_err());
        assert!(mapping_layout(0, 0, 65536).is_err());
        assert!(mapping_layout(0, 32768, 0).is_err());
    }

    #[test]
    fn lock_requests_match_the_stock_eight_byte_protocol() {
        let request = lock_request(0, 8, ffi::SQLITE_SHM_LOCK | ffi::SQLITE_SHM_EXCLUSIVE).unwrap();
        assert_eq!(request.offset, 120);
        assert_eq!(request.mask, 255);
        assert!(lock_request(7, 1, ffi::SQLITE_SHM_LOCK | ffi::SQLITE_SHM_SHARED).is_ok());
        for (offset, count) in [(-1, 1), (0, 0), (0, 9), (8, 1), (7, 2), (i32::MAX, 1)] {
            assert!(lock_request(
                offset,
                count,
                ffi::SQLITE_SHM_LOCK | ffi::SQLITE_SHM_EXCLUSIVE
            )
            .is_err());
        }
        assert!(lock_request(0, 2, ffi::SQLITE_SHM_LOCK | ffi::SQLITE_SHM_SHARED).is_err());
        assert!(lock_request(0, 1, ffi::SQLITE_SHM_LOCK).is_err());
        assert!(lock_request(0, 1, 0).is_err());
    }

    #[test]
    fn a_second_custom_node_is_rejected_until_the_first_reservation_releases() {
        let key = (0x73686d5f74657374, *b"shm-singleton-01");
        let first = Reservation::acquire(key).unwrap();
        let error = match Reservation::acquire(key) {
            Err(error) => error,
            Ok(_) => panic!("duplicate SHM node was admitted"),
        };
        assert_eq!(error.details["sqliteCode"], ffi::SQLITE_BUSY);
        drop(first);
        assert!(Reservation::acquire(key).is_ok());
    }
}
