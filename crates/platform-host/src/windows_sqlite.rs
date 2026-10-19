//! Internal, existing-WAL connection boundary. No project/API admission or
//! arbitrary SQL interface is exposed here. Initialization and workers follow
//! separately; all public Windows SQL routes remain closed.
#![deny(unsafe_op_in_unsafe_fn)]

use crate::{windows_sqlite_shm::ShmConnection, SqliteFileKind, SqliteFileSet};
use platform_project_format::ApiError;
use rusqlite::{
    ffi,
    hooks::{AuthAction, AuthContext, Authorization},
    Connection, OpenFlags,
};
use std::{
    ffi::{c_char, c_int, c_void, CStr, CString},
    fs::File,
    io::Read,
    mem::{align_of, size_of},
    os::windows::io::BorrowedHandle,
    panic::{catch_unwind, AssertUnwindSafe},
    ptr::{null, null_mut},
    sync::{
        atomic::{fence, Ordering},
        Mutex,
    },
    time::Duration,
};
use uuid::Uuid;
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};

type Result<T> = std::result::Result<T, ApiError>;
fn failure(code: &str, message: impl Into<String>) -> ApiError {
    ApiError::new(code, message).outcome("not_started")
}
fn workspace(error: crate::WorkspaceError) -> ApiError {
    failure(error.code, error.message)
}
fn sqlite(error: rusqlite::Error) -> ApiError {
    failure("DATABASE_SQLITE_ERROR", error.to_string())
}
fn sqlite_code(error: &ApiError) -> c_int {
    error
        .details
        .get("sqliteCode")
        .and_then(|v| v.as_i64())
        .and_then(|n| c_int::try_from(n).ok())
        .unwrap_or(ffi::SQLITE_IOERR)
}

struct Context {
    parent: *mut ffi::sqlite3_vfs,
    files: SqliteFileSet,
    paths: [CString; 3],
    journal: CString,
    last_failure: Mutex<Option<ApiError>>,
}
impl Context {
    fn verify(&self) -> Result<()> {
        self.files.verify_binding().map_err(workspace)
    }
    fn record(&self, error: ApiError) -> c_int {
        let code = sqlite_code(&error);
        if let Ok(mut last) = self.last_failure.lock() {
            *last = Some(error);
        }
        code
    }
    fn take_failure(&self) -> Option<ApiError> {
        self.last_failure
            .lock()
            .ok()
            .and_then(|mut last| last.take())
    }
    fn run(&self, f: impl FnOnce() -> Result<c_int>) -> c_int {
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(Ok(code)) => code,
            Ok(Err(error)) => self.record(error),
            Err(_) => self.record(failure(
                "DATABASE_VFS_PANIC",
                "Database callback failed; connection must be closed",
            )),
        }
    }
    fn index(kind: SqliteFileKind) -> usize {
        match kind {
            SqliteFileKind::Main => 0,
            SqliteFileKind::Wal => 1,
            SqliteFileKind::Shm => 2,
        }
    }
    unsafe fn kind(&self, name: *const c_char) -> Option<SqliteFileKind> {
        if name.is_null() {
            return None;
        }
        // SQLite owns this NUL-terminated filename for the duration of its call.
        let name = unsafe { CStr::from_ptr(name) };
        [
            SqliteFileKind::Main,
            SqliteFileKind::Wal,
            SqliteFileKind::Shm,
        ]
        .into_iter()
        .find(|kind| self.paths[Self::index(*kind)].as_c_str() == name)
    }
}

// SQLite allocates this header and an aligned native winFile in one block.
// The native object is never interpreted as a Rust/SQLite-private winFile.
#[repr(C)]
struct BoundFile {
    base: ffi::sqlite3_file,
    context: *const Context,
    shm: *mut ShmConnection,
    kind: SqliteFileKind,
}
const INNER_OFFSET: usize = size_of::<BoundFile>().div_ceil(8) * 8;
unsafe fn context<'a>(vfs: *mut ffi::sqlite3_vfs) -> &'a Context {
    // Registered allocations outlive every connection and sqlite3_file.
    unsafe { &*((*vfs).pAppData.cast::<Context>()) }
}
unsafe fn bound<'a>(file: *mut ffi::sqlite3_file) -> &'a mut BoundFile {
    unsafe { &mut *file.cast::<BoundFile>() }
}
unsafe fn inner(file: *mut ffi::sqlite3_file) -> *mut ffi::sqlite3_file {
    unsafe { file.cast::<u8>().add(INNER_OFFSET).cast() }
}
unsafe fn methods<'a>(file: *mut ffi::sqlite3_file) -> &'a ffi::sqlite3_io_methods {
    unsafe { &*((*inner(file)).pMethods) }
}

unsafe extern "C" fn open(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    out: *mut c_int,
) -> c_int {
    let ctx = unsafe { context(vfs) };
    ctx.run(|| {
        ctx.verify()?;
        let kind = unsafe { ctx.kind(name) }.ok_or_else(|| {
            failure(
                "DATABASE_VFS_PATH_REJECTED",
                "Only the registered main/WAL paths may be opened",
            )
        })?;
        let expected_type = match kind {
            SqliteFileKind::Main => ffi::SQLITE_OPEN_MAIN_DB,
            SqliteFileKind::Wal => ffi::SQLITE_OPEN_WAL,
            SqliteFileKind::Shm => {
                return Err(failure(
                    "DATABASE_VFS_PATH_REJECTED",
                    "SHM requires the handle-backed mapping layer",
                ))
            }
        };
        let allowed = ffi::SQLITE_OPEN_READWRITE
            | ffi::SQLITE_OPEN_CREATE
            | expected_type
            | ffi::SQLITE_OPEN_NOFOLLOW;
        if flags & !allowed != 0
            || flags & expected_type == 0
            || flags & ffi::SQLITE_OPEN_READWRITE == 0
        {
            return Err(failure(
                "DATABASE_VFS_FLAGS_REJECTED",
                format!("Unsupported native open flags: {flags:#x}"),
            ));
        }
        // Preserve the exact filename pointer: winFile retains it for its own
        // lifetime. Removing CREATE guarantees OPEN_EXISTING even during a race.
        let mut actual = 0;
        let rc = unsafe {
            ((*ctx.parent).xOpen.unwrap())(
                ctx.parent,
                name,
                inner(file),
                flags & !ffi::SQLITE_OPEN_CREATE,
                &mut actual,
            )
        };
        if rc != ffi::SQLITE_OK {
            return Ok(rc);
        }
        let admitted = (|| {
            if actual & ffi::SQLITE_OPEN_READWRITE == 0 {
                return Err(failure(
                    "DATABASE_READONLY_FALLBACK",
                    "SQLite fell back to a read-only file",
                ));
            }
            let native = unsafe { &*((*inner(file)).pMethods) };
            let mut handle: HANDLE = null_mut();
            let rc = unsafe {
                (native.xFileControl.unwrap())(
                    inner(file),
                    ffi::SQLITE_FCNTL_WIN32_GET_HANDLE,
                    (&mut handle as *mut HANDLE).cast(),
                )
            };
            if rc != ffi::SQLITE_OK || handle.is_null() || handle == INVALID_HANDLE_VALUE {
                return Err(failure(
                    "DATABASE_HANDLE_UNAVAILABLE",
                    "Win32 VFS did not provide its actual file handle",
                ));
            }
            // The native file owns this handle until its xClose returns.
            ctx.files
                .file(kind)
                .verify_native_handle(unsafe { BorrowedHandle::borrow_raw(handle) })
                .map_err(workspace)?;
            if kind == SqliteFileKind::Main {
                let mut persist = 1;
                let rc = unsafe {
                    (native.xFileControl.unwrap())(
                        inner(file),
                        ffi::SQLITE_FCNTL_PERSIST_WAL,
                        (&mut persist as *mut c_int).cast(),
                    )
                };
                persist = -1;
                let queried = unsafe {
                    (native.xFileControl.unwrap())(
                        inner(file),
                        ffi::SQLITE_FCNTL_PERSIST_WAL,
                        (&mut persist as *mut c_int).cast(),
                    )
                };
                if rc != ffi::SQLITE_OK || queried != ffi::SQLITE_OK || persist != 1 {
                    return Err(failure(
                        "DATABASE_CONFIGURATION_ERROR",
                        "Persistent WAL could not be established before reading SQLite",
                    ));
                }
            }
            ctx.verify()
        })();
        if let Err(error) = admitted {
            unsafe {
                ((*(*inner(file)).pMethods).xClose.unwrap())(inner(file));
                (*inner(file)).pMethods = null();
            }
            return Err(error);
        }
        unsafe {
            file.cast::<BoundFile>().write(BoundFile {
                base: ffi::sqlite3_file {
                    pMethods: &IO_METHODS,
                },
                context: ctx,
                shm: null_mut(),
                kind,
            });
            if !out.is_null() {
                *out = actual;
            }
        }
        Ok(ffi::SQLITE_OK)
    })
}
unsafe extern "C" fn delete(vfs: *mut ffi::sqlite3_vfs, _: *const c_char, _: c_int) -> c_int {
    unsafe { context(vfs) }.record(failure(
        "DATABASE_VFS_DELETE_REJECTED",
        "Existing-WAL connections never delete backing files",
    ))
}
unsafe extern "C" fn access(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    flags: c_int,
    out: *mut c_int,
) -> c_int {
    let ctx = unsafe { context(vfs) };
    ctx.run(|| {
        if out.is_null() {
            return Ok(ffi::SQLITE_IOERR_ACCESS);
        }
        unsafe {
            *out = 0;
        }
        ctx.verify()?;
        if !matches!(
            flags,
            ffi::SQLITE_ACCESS_EXISTS | ffi::SQLITE_ACCESS_READ | ffi::SQLITE_ACCESS_READWRITE
        ) {
            return Ok(ffi::SQLITE_IOERR_ACCESS);
        }
        if unsafe { ctx.kind(name) }.is_some() {
            unsafe {
                *out = 1;
            }
            return Ok(ffi::SQLITE_OK);
        }
        if !name.is_null() && unsafe { CStr::from_ptr(name) } == ctx.journal.as_c_str() {
            // Full directory validation above proved absence. Never hide a
            // retained journal or ask the native VFS to inspect arbitrary paths.
            return Ok(ffi::SQLITE_OK);
        }
        Err(failure(
            "DATABASE_VFS_PATH_REJECTED",
            "Unregistered path access rejected",
        ))
    })
}
unsafe extern "C" fn full_path(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    n: c_int,
    out: *mut c_char,
) -> c_int {
    let ctx = unsafe { context(vfs) };
    ctx.run(|| {
        ctx.verify()?;
        if unsafe { ctx.kind(name) } != Some(SqliteFileKind::Main) || out.is_null() || n <= 0 {
            return Ok(ffi::SQLITE_CANTOPEN);
        }
        let bytes = ctx.paths[0].as_bytes_with_nul();
        if bytes.len() > n as usize {
            return Ok(ffi::SQLITE_CANTOPEN);
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr().cast(), out, bytes.len());
        }
        Ok(ffi::SQLITE_OK)
    })
}

macro_rules! delegate_io {
    ($name:ident, $field:ident, $( $arg:ident : $ty:ty ),*) => {
        unsafe extern "C" fn $name(file: *mut ffi::sqlite3_file, $($arg:$ty),*) -> c_int {
            let ctx = unsafe { &*bound(file).context };
            ctx.run(|| {
                ctx.verify()?;
                let rc = unsafe { (methods(file).$field.unwrap())(inner(file), $($arg),*) };
                if rc == ffi::SQLITE_OK { ctx.verify()?; }
                Ok(rc)
            })
        }
    }
}
delegate_io!(read, xRead, output: *mut c_void, count: c_int, offset: i64);
delegate_io!(write, xWrite, input: *const c_void, count: c_int, offset: i64);
delegate_io!(truncate, xTruncate, size: i64);
delegate_io!(sync, xSync, flags: c_int);
delegate_io!(file_size, xFileSize, size: *mut i64);
delegate_io!(lock, xLock, level: c_int);
delegate_io!(check_reserved, xCheckReservedLock, output: *mut c_int);
// Unlock must remain possible after a failed admission check.
unsafe extern "C" fn unlock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    unsafe { (methods(file).xUnlock.unwrap())(inner(file), level) }
}
unsafe extern "C" fn file_control(
    file: *mut ffi::sqlite3_file,
    op: c_int,
    arg: *mut c_void,
) -> c_int {
    let ctx = unsafe { &*bound(file).context };
    ctx.run(|| {
        ctx.verify()?;
        if op == ffi::SQLITE_FCNTL_PERSIST_WAL
            && (arg.is_null() || unsafe { *arg.cast::<c_int>() } == 0)
        {
            return Err(failure(
                "DATABASE_CONFIGURATION_ERROR",
                "Persistent WAL cannot be disabled",
            ));
        }
        Ok(unsafe { (methods(file).xFileControl.unwrap())(inner(file), op, arg) })
    })
}
unsafe extern "C" fn sector_size(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe { (methods(file).xSectorSize.unwrap())(inner(file)) }
}
unsafe extern "C" fn device_characteristics(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe { (methods(file).xDeviceCharacteristics.unwrap())(inner(file)) }
}
unsafe fn shm<'a>(file: *mut ffi::sqlite3_file) -> Result<&'a mut ShmConnection> {
    let header = unsafe { bound(file) };
    let ctx = unsafe { &*header.context };
    if header.kind != SqliteFileKind::Main {
        return Err(failure(
            "DATABASE_SHM_REJECTED",
            "Only main database handles may map SHM",
        ));
    }
    if header.shm.is_null() {
        header.shm = Box::into_raw(Box::new(ShmConnection::open(&ctx.files)?));
    }
    Ok(unsafe { &mut *header.shm })
}
unsafe extern "C" fn shm_map(
    file: *mut ffi::sqlite3_file,
    region: c_int,
    size: c_int,
    extend: c_int,
    out: *mut *mut c_void,
) -> c_int {
    let ctx = unsafe { &*bound(file).context };
    ctx.run(|| {
        if out.is_null() {
            return Ok(ffi::SQLITE_IOERR_SHMMAP);
        }
        unsafe {
            *out = null_mut();
        }
        ctx.verify()?;
        let mapped = unsafe { shm(file)? }.map(region, size, extend != 0)?;
        ctx.verify()?;
        unsafe {
            *out = mapped;
        }
        Ok(ffi::SQLITE_OK)
    })
}
unsafe extern "C" fn shm_lock(
    file: *mut ffi::sqlite3_file,
    offset: c_int,
    count: c_int,
    flags: c_int,
) -> c_int {
    let ctx = unsafe { &*bound(file).context };
    ctx.run(|| {
        if flags & ffi::SQLITE_SHM_UNLOCK == 0 {
            ctx.verify()?;
        } else if unsafe { bound(file).shm.is_null() } {
            // Cleanup must not allocate/open a new SHM node merely to unlock.
            return Ok(ffi::SQLITE_OK);
        }
        unsafe { shm(file)? }.lock(offset, count, flags)?;
        Ok(ffi::SQLITE_OK)
    })
}
unsafe extern "C" fn shm_barrier(file: *mut ffi::sqlite3_file) {
    // No pointer/path access or fallible work is needed for a memory barrier.
    let header = unsafe { bound(file) };
    if !header.shm.is_null() {
        unsafe { &*header.shm }.barrier();
    } else {
        fence(Ordering::SeqCst);
    }
}
unsafe extern "C" fn shm_unmap(file: *mut ffi::sqlite3_file, delete_flag: c_int) -> c_int {
    let ctx = unsafe { &*bound(file).context };
    ctx.run(|| {
        let header = unsafe { bound(file) };
        if !header.shm.is_null() {
            let mut shm = unsafe { Box::from_raw(header.shm) };
            header.shm = null_mut();
            shm.close()?;
        }
        if delete_flag != 0 {
            return Err(failure(
                "DATABASE_VFS_DELETE_REJECTED",
                "SHM deletion is not permitted",
            ));
        }
        Ok(ffi::SQLITE_OK)
    })
}
unsafe extern "C" fn close(file: *mut ffi::sqlite3_file) -> c_int {
    let ctx = unsafe { &*bound(file).context };
    // Always release resources even if validation or an earlier callback failed.
    let shm_rc = unsafe { shm_unmap(file, 0) };
    let rc = unsafe { (methods(file).xClose.unwrap())(inner(file)) };
    unsafe {
        (*file).pMethods = null();
    }
    if shm_rc != ffi::SQLITE_OK {
        return shm_rc;
    }
    if rc != ffi::SQLITE_OK {
        return ctx.record(failure(
            "DATABASE_NATIVE_CLOSE_FAILED",
            format!("Native VFS close failed: {rc}"),
        ));
    }
    match ctx.verify() {
        Ok(()) => rc,
        Err(error) => ctx.record(error),
    }
}
static IO_METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 2,
    xClose: Some(close),
    xRead: Some(read),
    xWrite: Some(write),
    xTruncate: Some(truncate),
    xSync: Some(sync),
    xFileSize: Some(file_size),
    xLock: Some(lock),
    xUnlock: Some(unlock),
    xCheckReservedLock: Some(check_reserved),
    xFileControl: Some(file_control),
    xSectorSize: Some(sector_size),
    xDeviceCharacteristics: Some(device_characteristics),
    xShmMap: Some(shm_map),
    xShmLock: Some(shm_lock),
    xShmBarrier: Some(shm_barrier),
    xShmUnmap: Some(shm_unmap),
    xFetch: None,
    xUnfetch: None,
};

// Only non-filesystem callbacks are delegated with the original VFS pointer.
unsafe extern "C" fn randomness(vfs: *mut ffi::sqlite3_vfs, n: c_int, out: *mut c_char) -> c_int {
    let ctx = unsafe { context(vfs) };
    unsafe { ((*ctx.parent).xRandomness.unwrap())(ctx.parent, n, out) }
}
unsafe extern "C" fn sleep(vfs: *mut ffi::sqlite3_vfs, n: c_int) -> c_int {
    let ctx = unsafe { context(vfs) };
    unsafe { ((*ctx.parent).xSleep.unwrap())(ctx.parent, n) }
}
unsafe extern "C" fn current_time(vfs: *mut ffi::sqlite3_vfs, out: *mut f64) -> c_int {
    let ctx = unsafe { context(vfs) };
    unsafe { ((*ctx.parent).xCurrentTime.unwrap())(ctx.parent, out) }
}
unsafe extern "C" fn current_time64(vfs: *mut ffi::sqlite3_vfs, out: *mut i64) -> c_int {
    let ctx = unsafe { context(vfs) };
    unsafe { ((*ctx.parent).xCurrentTimeInt64.unwrap())(ctx.parent, out) }
}
unsafe extern "C" fn last_error(vfs: *mut ffi::sqlite3_vfs, n: c_int, out: *mut c_char) -> c_int {
    let ctx = unsafe { context(vfs) };
    unsafe { ((*ctx.parent).xGetLastError.unwrap())(ctx.parent, n, out) }
}
unsafe extern "C" fn dl_open(_: *mut ffi::sqlite3_vfs, _: *const c_char) -> *mut c_void {
    null_mut()
}
unsafe extern "C" fn dl_error(_: *mut ffi::sqlite3_vfs, n: c_int, out: *mut c_char) {
    if n > 0 && !out.is_null() {
        unsafe {
            *out = 0;
        }
    }
}
unsafe extern "C" fn dl_sym(
    _: *mut ffi::sqlite3_vfs,
    _: *mut c_void,
    _: *const c_char,
) -> Option<unsafe extern "C" fn(*mut ffi::sqlite3_vfs, *mut c_void, *const c_char)> {
    None
}
unsafe extern "C" fn dl_close(_: *mut ffi::sqlite3_vfs, _: *mut c_void) {}

struct Registration {
    vfs: Box<ffi::sqlite3_vfs>,
    context: Box<Context>,
    name: CString,
}
impl Registration {
    fn new(files: SqliteFileSet) -> Result<Self> {
        files.verify_binding().map_err(workspace)?;
        let initialized = unsafe { ffi::sqlite3_initialize() };
        if initialized != ffi::SQLITE_OK {
            return Err(failure(
                "DATABASE_VFS_UNAVAILABLE",
                "SQLite initialization failed",
            ));
        }
        let parent = unsafe { ffi::sqlite3_vfs_find(c"win32-longpath".as_ptr()) };
        if parent.is_null() || align_of::<BoundFile>() > 8 {
            return Err(failure(
                "DATABASE_VFS_UNAVAILABLE",
                "Required Win32 VFS is unavailable",
            ));
        }
        let parent_ref = unsafe { &*parent };
        if parent_ref.iVersion < 2
            || parent_ref.szOsFile <= 0
            || parent_ref.xOpen.is_none()
            || parent_ref.xRandomness.is_none()
            || parent_ref.xSleep.is_none()
            || parent_ref.xCurrentTime.is_none()
            || parent_ref.xCurrentTimeInt64.is_none()
            || parent_ref.xGetLastError.is_none()
        {
            return Err(failure(
                "DATABASE_VFS_UNAVAILABLE",
                "Win32 VFS contract is incomplete",
            ));
        }
        let path =
            |kind| {
                CString::new(files.file(kind).path().to_str().ok_or_else(|| {
                    failure("DATABASE_PATH_INVALID", "Database path is not Unicode")
                })?)
                .map_err(|_| failure("DATABASE_PATH_INVALID", "Database path contains NUL"))
            };
        let paths = [
            path(SqliteFileKind::Main)?,
            path(SqliteFileKind::Wal)?,
            path(SqliteFileKind::Shm)?,
        ];
        let journal = CString::new(format!("{}-journal", paths[0].to_str().unwrap())).unwrap();
        let mut context = Box::new(Context {
            parent,
            files,
            paths,
            journal,
            last_failure: Mutex::new(None),
        });
        let name = CString::new(format!("windows-bound-{}", Uuid::new_v4())).unwrap();
        let size = parent_ref
            .szOsFile
            .checked_add(INNER_OFFSET as c_int)
            .ok_or_else(|| failure("DATABASE_VFS_UNAVAILABLE", "VFS file allocation overflow"))?;
        let mut vfs = Box::new(ffi::sqlite3_vfs {
            iVersion: 2,
            szOsFile: size,
            mxPathname: parent_ref.mxPathname,
            pNext: null_mut(),
            zName: name.as_ptr(),
            pAppData: (&mut *context as *mut Context).cast(),
            xOpen: Some(open),
            xDelete: Some(delete),
            xAccess: Some(access),
            xFullPathname: Some(full_path),
            xDlOpen: Some(dl_open),
            xDlError: Some(dl_error),
            xDlSym: Some(dl_sym),
            xDlClose: Some(dl_close),
            xRandomness: Some(randomness),
            xSleep: Some(sleep),
            xCurrentTime: Some(current_time),
            xGetLastError: Some(last_error),
            xCurrentTimeInt64: Some(current_time64),
            xSetSystemCall: None,
            xGetSystemCall: None,
            xNextSystemCall: None,
        });
        let rc = unsafe { ffi::sqlite3_vfs_register(&mut *vfs, 0) };
        if rc != ffi::SQLITE_OK {
            return Err(failure(
                "DATABASE_VFS_UNAVAILABLE",
                "VFS registration failed",
            ));
        }
        Ok(Self { vfs, context, name })
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        unsafe {
            ffi::sqlite3_vfs_unregister(&mut *self.vfs);
        }
    }
}

/// A private connection, retained with its non-default VFS and all backing
/// leases. No raw connection or arbitrary SQL API is exported in this stage.
pub struct BoundSqliteDatabase {
    connection: Option<Connection>,
    registration: Option<Registration>,
}
impl BoundSqliteDatabase {
    pub fn open_existing(files: SqliteFileSet) -> Result<Self> {
        // The WAL-reset race is fixed from SQLite 3.51.3. A future dependency
        // change must not silently restore the unpatched engine for writes.
        if rusqlite::version_number() < 3_051_003 {
            return Err(failure(
                "DATABASE_ENGINE_UNSUPPORTED",
                "SQLite with the WAL-reset fix (3.51.3 or later) is required",
            )
            .detail("sqliteVersion", rusqlite::version()));
        }
        // Reject missing/empty/rollback/certain corrupt headers before SQLite
        // can initialize, recover or map anything. This is not a full integrity
        // proof; SQLite quick_check follows on the real WAL-backed connection.
        let mut main = File::from(
            files
                .open_readwrite_handle(SqliteFileKind::Main)
                .map_err(workspace)?,
        );
        let mut header = [0u8; 100];
        main.read_exact(&mut header)
            .map_err(|err| failure("DATABASE_INVALID_HEADER", err.to_string()))?;
        let page_size = u16::from_be_bytes([header[16], header[17]]) as u64;
        let page_size = if page_size == 1 { 65536 } else { page_size };
        let length = main
            .metadata()
            .map_err(|err| failure("DATABASE_METADATA_ERROR", err.to_string()))?
            .len();
        if &header[..16] != b"SQLite format 3\0"
            || header[18..20] != [2, 2]
            || !(512..=65536).contains(&page_size)
            || !page_size.is_power_of_two()
            || length < page_size
            || !length.is_multiple_of(page_size)
        {
            return Err(failure("DATABASE_INVALID_HEADER", "An existing, complete WAL database is required; no conversion or replacement was attempted"));
        }
        drop(main);
        let mut wal = File::from(
            files
                .open_readwrite_handle(SqliteFileKind::Wal)
                .map_err(workspace)?,
        );
        let wal_length = wal
            .metadata()
            .map_err(|err| failure("DATABASE_METADATA_ERROR", err.to_string()))?
            .len();
        crate::windows_sqlite_wal::validate_wal(&mut wal, page_size as u32)?;
        if wal
            .metadata()
            .map_err(|err| failure("DATABASE_METADATA_ERROR", err.to_string()))?
            .len()
            != wal_length
        {
            return Err(failure(
                "DATABASE_WAL_CHANGED",
                "WAL length changed during admission; no connection was opened",
            ));
        }
        drop(wal);
        let registration = Registration::new(files)?;
        let connection = Connection::open_with_flags_and_vfs(
            registration.context.files.file(SqliteFileKind::Main).path(),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            registration.name.as_c_str(),
        )
        .map_err(|err| {
            registration
                .context
                .take_failure()
                .unwrap_or_else(|| sqlite(err))
        })?;
        let database = Self {
            connection: Some(connection),
            registration: Some(registration),
        };
        match database.configure() {
            Ok(()) => Ok(database),
            Err(error) => match database.close() {
                Ok(()) => Err(error),
                Err(cleanup) => Err(error
                    .detail("closeError", serde_json::json!(cleanup))
                    .outcome("unknown")),
            },
        }
    }
    fn configure(&self) -> Result<()> {
        let conn = self.connection.as_ref().unwrap();
        let result = (|| {
            // Opening/diagnosing a bad database must not checkpoint WAL into
            // main as a side effect of error cleanup. Normal commits remain
            // durable in WAL; a future worker owns explicit checkpoint policy.
            let mut configured = 0;
            let rc = unsafe {
                ffi::sqlite3_db_config(
                    conn.handle(),
                    ffi::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
                    1,
                    &mut configured as *mut c_int,
                )
            };
            if rc != ffi::SQLITE_OK || configured != 1 {
                return Err(failure(
                    "DATABASE_CONFIGURATION_ERROR",
                    "Could not disable implicit checkpoint during close",
                ));
            }
            conn.busy_timeout(Duration::from_secs(5)).map_err(sqlite)?;
            let mode: String = conn
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .map_err(sqlite)?;
            if mode != "wal" {
                return Err(failure(
                    "DATABASE_CONFIGURATION_ERROR",
                    "Database is not already in WAL mode",
                ));
            }
            conn.execute_batch(
                "PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL; PRAGMA temp_store=MEMORY;",
            )
            .map_err(sqlite)?;
            for (query, expected) in [
                ("PRAGMA foreign_keys", 1),
                ("PRAGMA synchronous", 2),
                ("PRAGMA busy_timeout", 5000),
                ("PRAGMA temp_store", 2),
            ] {
                let actual: i64 = conn
                    .query_row(query, [], |row| row.get(0))
                    .map_err(sqlite)?;
                if actual != expected {
                    return Err(failure(
                        "DATABASE_CONFIGURATION_ERROR",
                        format!("{query} did not establish its required value"),
                    ));
                }
            }
            let check: String = conn
                .query_row("PRAGMA quick_check", [], |row| row.get(0))
                .map_err(sqlite)?;
            if check != "ok" {
                return Err(failure("DATABASE_CORRUPT", check));
            }
            self.verify_binding()?;
            // Defense in depth for this private experimental connection. URI
            // ATTACH may select another VFS, so a VFS alone is not a SQL policy.
            // The shared request executor must install its complete policy
            // before public SQL is ever admitted.
            conn.authorizer(Some(|context: AuthContext<'_>| match context.action {
                AuthAction::Attach { .. }
                | AuthAction::Detach { .. }
                | AuthAction::Unknown { .. } => Authorization::Deny,
                AuthAction::Function { function_name }
                    if matches!(
                        function_name.to_ascii_lowercase().as_str(),
                        "load_extension" | "readfile" | "writefile" | "edit"
                    ) =>
                {
                    Authorization::Deny
                }
                AuthAction::Pragma {
                    pragma_name,
                    pragma_value: Some(_),
                } if !matches!(
                    pragma_name.to_ascii_lowercase().as_str(),
                    "table_info"
                        | "table_xinfo"
                        | "foreign_key_list"
                        | "index_list"
                        | "index_info"
                        | "index_xinfo"
                ) =>
                {
                    Authorization::Deny
                }
                _ => Authorization::Allow,
            }))
            .map_err(sqlite)?;
            Ok(())
        })();
        result.map_err(|err| {
            self.registration
                .as_ref()
                .unwrap()
                .context
                .take_failure()
                .unwrap_or(err)
        })
    }
    pub fn verify_binding(&self) -> Result<()> {
        self.registration.as_ref().unwrap().context.verify()
    }
    fn close_connection(&mut self) -> Result<()> {
        if let Some(registration) = &self.registration {
            registration.context.take_failure();
        }
        if let Some(connection) = self.connection.take() {
            if let Err((connection, error)) = connection.close() {
                // A still-live sqlite3/statement can call the VFS later. Keep
                // every allocation and lease alive, never free behind it. The
                // private worker integration must report this shutdown failure.
                std::mem::forget(connection);
                std::mem::forget(self.registration.take());
                return Err(
                    ApiError::new("DATABASE_CLOSE_FAILED", error.to_string()).outcome("unknown")
                );
            }
        }
        let reported = self.registration.as_ref().and_then(|registration| {
            // SQLite intentionally ignores several xClose/xShmUnmap errors.
            // Observe our own callbacks, and validate the retained set once
            // more before releasing any registration/lease.
            registration
                .context
                .take_failure()
                .or_else(|| registration.context.verify().err())
        });
        self.registration.take();
        match reported {
            Some(error) => Err(error.outcome("unknown")),
            None => Ok(()),
        }
    }
    pub fn close(mut self) -> Result<()> {
        self.close_connection()
    }
}
impl Drop for BoundSqliteDatabase {
    fn drop(&mut self) {
        let _ = self.close_connection();
    }
}

#[cfg(test)]
#[path = "windows_sqlite_tests.rs"]
mod tests;
