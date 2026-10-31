//! Handle-bound identity, no-reparse access and private creation. Parent ACLs
//! are not audited or changed; protection is applied only to objects we create.
#[path = "files/private_inheritance.rs"]
mod private_inheritance;
pub use private_inheritance::enable_private_directory_inheritance;
#[cfg(test)]
#[path = "files/private_creation_tests.rs"]
mod private_creation_tests;

use super::{checked, native_path, normalized_path, wide};
use std::{
    ffi::{c_void, OsStr, OsString},
    fs::File,
    io,
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStringExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Wdk::Storage::FileSystem::NtSetSecurityObject;
use windows_sys::Win32::{
    Foundation::{
        LocalFree, RtlNtStatusToDosError, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    },
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
        },
        EqualSid, GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
        GetTokenInformation, TokenUser, ACCESS_ALLOWED_ACE, ACL, DACL_SECURITY_INFORMATION,
        OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        PSID, SECURITY_ATTRIBUTES, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
    },
    Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, FileAttributeTagInfo, FileDispositionInfo, FileIdInfo,
        FileRenameInfo, FileStandardInfo, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
        MoveFileExW, SetFileInformationByHandle, CREATE_NEW, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_DISPOSITION_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FILE_WRITE_ATTRIBUTES,
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, OPEN_ALWAYS, OPEN_EXISTING,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

const READ_CONTROL: u32 = 0x0002_0000;
const DELETE: u32 = 0x0001_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    pub volume_serial: u64,
    pub file_id: [u8; 16],
    pub links: u32,
    pub directory: bool,
}
impl FileIdentity {
    pub fn same_object(&self, other: &Self) -> bool {
        self.volume_serial == other.volume_serial && self.file_id == other.file_id
    }
}

pub fn identity(file: &File) -> io::Result<FileIdentity> {
    let handle = file.as_raw_handle();
    let mut id: FILE_ID_INFO = unsafe { zeroed() };
    let mut standard: FILE_STANDARD_INFO = unsafe { zeroed() };
    unsafe {
        checked(GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        ))?;
        checked(GetFileInformationByHandleEx(
            handle,
            FileStandardInfo,
            (&mut standard as *mut FILE_STANDARD_INFO).cast(),
            size_of::<FILE_STANDARD_INFO>() as u32,
        ))?;
    }
    Ok(FileIdentity {
        volume_serial: id.VolumeSerialNumber,
        file_id: id.FileId.Identifier,
        links: standard.NumberOfLinks,
        directory: standard.Directory,
    })
}

fn reject_reparse(file: &File) -> io::Result<()> {
    let mut info: FILE_ATTRIBUTE_TAG_INFO = unsafe { zeroed() };
    unsafe {
        checked(GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileAttributeTagInfo,
            (&mut info as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
            size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        ))?;
    }
    if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Reparse points are not allowed at this file boundary",
        ));
    }
    Ok(())
}

fn open_component(path: &Path, ancestors: bool) -> io::Result<File> {
    let name = native_path(path)?;
    // An ancestor stays pinned against rename while a checked operation owns it.
    // This is the Windows equivalent of retained descriptor-relative traversal.
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | if ancestors { 0 } else { FILE_SHARE_DELETE };
    // Attribute-only handles do not pin a directory against modern POSIX-style
    // rename. Directory read access participates in the sharing contract.
    let access = FILE_READ_ATTRIBUTES
        | if ancestors {
            FILE_LIST_DIRECTORY
        } else {
            READ_CONTROL
        };
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            access,
            share,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    reject_reparse(&file)?;
    Ok(file)
}

/// Keeps every ancestor handle alive. No system-parent ownership/ACL admission
/// is imposed: only the existing no-link and object-identity boundary is enforced.
pub struct CheckedPath {
    pub file: File,
    pub identity: FileIdentity,
    _ancestors: Vec<File>,
}
pub fn open_no_reparse(path: &Path) -> io::Result<CheckedPath> {
    let normalized = normalized_path(path)?;
    let path = normalized.as_path();
    let mut ancestors = Vec::new();
    let mut chain: Vec<_> = path.ancestors().skip(1).collect();
    chain.reverse();
    for ancestor in chain {
        let file = open_component(ancestor, true)?;
        if !identity(&file)?.directory {
            return Err(io::Error::new(
                io::ErrorKind::NotADirectory,
                "An ancestor is not a directory",
            ));
        }
        ancestors.push(file);
    }
    let file = open_component(path, false)?;
    let captured = identity(&file)?;
    Ok(CheckedPath {
        file,
        identity: captured,
        _ancestors: ancestors,
    })
}

struct CurrentUser {
    _token: OwnedHandle,
    bytes: Vec<usize>,
}
impl CurrentUser {
    fn load() -> io::Result<Self> {
        let mut raw: HANDLE = null_mut();
        unsafe {
            checked(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw))?;
        }
        let token = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut length = 0u32;
        unsafe {
            GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut length);
        }
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut bytes = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        unsafe {
            checked(GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                bytes.as_mut_ptr().cast(),
                length,
                &mut length,
            ))?;
        }
        Ok(Self {
            _token: token,
            bytes,
        })
    }
    fn sid(&self) -> PSID {
        unsafe { (*(self.bytes.as_ptr().cast::<TOKEN_USER>())).User.Sid }
    }
    fn text(&self) -> io::Result<String> {
        let mut result = null_mut();
        unsafe {
            checked(ConvertSidToStringSidW(self.sid(), &mut result))?;
        }
        let allocation = LocalAllocation(result.cast());
        let mut length = 0;
        unsafe {
            while *result.add(length) != 0 {
                length += 1;
            }
        }
        let text = String::from_utf16(unsafe { std::slice::from_raw_parts(result, length) })
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "Invalid account SID"));
        drop(allocation);
        text
    }
}
struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

struct PrivateSecurity {
    allocation: LocalAllocation,
}
impl PrivateSecurity {
    fn new() -> io::Result<Self> {
        Self::with_ace_flags("")
    }
    fn new_directory() -> io::Result<Self> {
        Self::with_ace_flags("OICI")
    }
    fn with_ace_flags(flags: &str) -> io::Result<Self> {
        let sid = CurrentUser::load()?.text()?;
        let sddl = wide(OsStr::new(&format!("O:{sid}D:P(A;{flags};FA;;;{sid})")))?;
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        unsafe {
            checked(ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            ))?;
        }
        Ok(Self {
            allocation: LocalAllocation(descriptor),
        })
    }
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.allocation.0,
            bInheritHandle: 0,
        }
    }
}

fn pin_parent(path: &Path) -> io::Result<(CheckedPath, File)> {
    let normalized = normalized_path(path)?;
    let path = normalized.as_path();
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Object has no parent"))?;
    let checked = open_no_reparse(parent)?;
    let pinned = open_component(parent, true)?;
    if !checked.identity.directory || !checked.identity.same_object(&identity(&pinned)?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Parent directory changed before private creation",
        ));
    }
    Ok((checked, pinned))
}

/// Pins the directory itself as well as its ancestors, retaining a separate
/// attribute/security handle so callers can validate only their owned object.
pub struct PinnedDirectory {
    pub file: File,
    _scope: CheckedPath,
    _pin: File,
}
pub fn pin_directory(path: &Path) -> io::Result<PinnedDirectory> {
    let checked = open_no_reparse(path)?;
    let pinned = open_component(path, true)?;
    if !checked.identity.directory || !checked.identity.same_object(&identity(&pinned)?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Directory changed before it could be pinned",
        ));
    }
    Ok(PinnedDirectory {
        file: checked.file.try_clone()?,
        _scope: checked,
        _pin: pinned,
    })
}

/// The returned descriptor pins its directory against rename. Each later
/// relative operation resolves this actual handle afresh, then pins its scope.
pub fn directory_file(path: &Path) -> io::Result<File> {
    let directory = pin_directory(path)?;
    Ok(directory._pin)
}

pub fn directory_scope(file: &File) -> io::Result<(PathBuf, PinnedDirectory)> {
    let expected = identity(file)?;
    if !expected.directory {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "Expected a directory handle",
        ));
    }
    let mut buffer = vec![0u16; 32768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            0,
        )
    };
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize >= buffer.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Directory path exceeds the Windows namespace limit",
        ));
    }
    buffer.truncate(length as usize);
    let path = PathBuf::from(OsString::from_wide(&buffer));
    let scope = pin_directory(&path)?;
    if !expected.same_object(&identity(&scope.file)?) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Directory path no longer identifies its handle",
        ));
    }
    Ok((path, scope))
}

pub fn open_read_file(path: &Path) -> io::Result<File> {
    open_read_file_with_policy(path, true)
}

/// A no-link read does not by itself prohibit hard links. Only callers whose
/// existing contract requires single-link objects use open_read_file instead.
pub fn open_read_file_allowing_hard_links(path: &Path) -> io::Result<File> {
    open_read_file_with_policy(path, false)
}

fn open_read_file_with_policy(path: &Path, single_link: bool) -> io::Result<File> {
    let _scope = pin_parent(path)?;
    let name = native_path(path)?;
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    reject_reparse(&file)?;
    let info = identity(&file)?;
    if info.directory || (single_link && info.links != 1) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "File does not satisfy the requested ordinary-file read policy",
        ));
    }
    Ok(file)
}

/// Delete the actual, checked object through its handle. A replacement between
/// selection and opening is rejected without unlinking the replacement.
pub fn remove_file(path: &Path, expected: &FileIdentity) -> io::Result<()> {
    remove_file_with_link_policy(path, expected, true)
}

/// Removing one explicitly owned name does not require taking ownership of
/// other names for the same object. Use only where the existing host policy
/// admits hard links; callers needing independent files keep remove_file.
pub fn remove_file_allowing_hard_links(path: &Path, expected: &FileIdentity) -> io::Result<()> {
    remove_file_with_link_policy(path, expected, false)
}

fn remove_file_with_link_policy(
    path: &Path,
    expected: &FileIdentity,
    require_single_link: bool,
) -> io::Result<()> {
    let _scope = pin_parent(path)?;
    let name = native_path(path)?;
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            DELETE | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    reject_reparse(&file)?;
    let captured = identity(&file)?;
    if captured.directory
        || (require_single_link && captured.links != 1)
        || !expected.same_object(&captured)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "File changed before deletion",
        ));
    }
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    unsafe {
        checked(SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        ))
    }
}

pub fn create_private_directory(path: &Path) -> io::Result<()> {
    let _parent = pin_parent(path)?;
    // Future ordinary child creations retain this same user's access. The
    // protected root still inherits no access from its external parent.
    let security = PrivateSecurity::new_directory()?;
    let name = native_path(path)?;
    unsafe {
        checked(CreateDirectoryW(name.as_ptr(), &security.attributes()))?;
    }
    let created = open_no_reparse(path)?;
    verify_private(&created.file)
}

/// Apply the existing private-directory policy to an empty directory explicitly
/// admitted by startup. Do not change an owner or inspect system-parent DACLs.
pub fn make_directory_private(path: &Path) -> io::Result<()> {
    make_object_private(path, true)
}

/// Apply the file counterpart of the private-directory policy to an admitted
/// platform file. This changes its DACL, never its owner or parent permissions.
pub fn make_file_private(path: &Path) -> io::Result<()> {
    make_object_private(path, false)
}

fn make_object_private(path: &Path, directory: bool) -> io::Result<()> {
    let _parent = pin_parent(path)?;
    let name = native_path(path)?;
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            READ_CONTROL | WRITE_DAC | FILE_READ_ATTRIBUTES,
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
    make_opened_object_private(&file, directory)
}

fn make_opened_object_private(file: &File, directory: bool) -> io::Result<()> {
    reject_reparse(file)?;
    if identity(file)?.directory != directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Admitted object has an unexpected file type",
        ));
    }
    // Existing private directories already satisfy this operation's policy.
    // Keep their chosen inheritance flags and every descendant's security.
    // File/credential repairs continue through the original path below.
    if directory && verify_private(file).is_ok() {
        return Ok(());
    }
    let current = CurrentUser::load()?;
    let mut owner = null_mut();
    let mut descriptor = null_mut();
    let result = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let _owner_descriptor = LocalAllocation(descriptor);
    if owner.is_null() || unsafe { EqualSid(owner, current.sid()) } == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Object does not belong to the current user",
        ));
    }
    let security = PrivateSecurity::new()?;
    if directory {
        // SetSecurityInfo propagates inheritance changes into existing child
        // objects. Update only this already-open directory's DACL instead,
        // preserving descendant security and the existing HANDLE/pin policy.
        // User-mode callers use NtSetSecurityObject, as documented by Microsoft:
        // https://learn.microsoft.com/windows-hardware/drivers/ddi/ntifs/nf-ntifs-zwsetsecurityobject
        let status = unsafe {
            NtSetSecurityObject(
                file.as_raw_handle(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                security.allocation.0,
            )
        };
        if status < 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { RtlNtStatusToDosError(status) } as i32,
            ));
        }
        return verify_private(file);
    }
    let mut present = 0;
    let mut defaulted = 0;
    let mut dacl = null_mut();
    unsafe {
        checked(GetSecurityDescriptorDacl(
            security.allocation.0,
            &mut present,
            &mut dacl,
            &mut defaulted,
        ))?;
    }
    if present == 0 || dacl.is_null() {
        return Err(io::Error::other("Private object DACL is absent"));
    }
    let result = unsafe {
        SetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            dacl,
            null(),
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    verify_private(file)
}

pub fn create_private_file(path: &Path) -> io::Result<File> {
    open_private_file(path, false, true)
}

/// Atomically creates a private file whose original handle can publish it.
pub fn create_private_file_for_publication(path: &Path) -> io::Result<File> {
    open_private_file_with_access(path, false, true, DELETE)
}

/// Repair permissions only on this admitted, current-user ordinary file. The
/// actual opened file blocks rename while its DACL is checked and repaired.
/// Newly created callers supply the original ID before any permission change.
pub fn open_owned_file_private(
    path: &Path,
    write: bool,
    expected: Option<&FileIdentity>,
) -> io::Result<File> {
    let _parent = pin_parent(path)?;
    let name = native_path(path)?;
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | READ_CONTROL | WRITE_DAC | if write { GENERIC_WRITE } else { 0 },
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    reject_reparse(&file)?;
    let captured = identity(&file)?;
    if captured.directory
        || captured.links != 1
        || expected.is_some_and(|expected| !expected.same_object(&captured))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Admitted private file changed or is not an independent file",
        ));
    }
    make_opened_object_private(&file, false)?;
    Ok(file)
}

/// Apply the real Windows read-only attribute to the opened object. Attribute
/// access is independent of file-content write access, including for files
/// already read-only. File::set_permissions uses SetFileInformationByHandle:
/// https://doc.rust-lang.org/std/fs/struct.File.html#method.set_permissions
pub fn set_read_only(path: &Path, read_only: bool) -> io::Result<()> {
    let _scope = pin_parent(path)?;
    let name = native_path(path)?;
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES,
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
    reject_reparse(&file)?;
    let mut permissions = file.metadata()?.permissions();
    permissions.set_readonly(read_only);
    file.set_permissions(permissions)
}

pub fn open_private_file(path: &Path, create: bool, create_new: bool) -> io::Result<File> {
    open_private_file_with_access(path, create, create_new, 0)
}

fn open_private_file_with_access(
    path: &Path,
    create: bool,
    create_new: bool,
    extra_access: u32,
) -> io::Result<File> {
    let _parent = pin_parent(path)?;
    let security = PrivateSecurity::new()?;
    let name = native_path(path)?;
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ
                | READ_CONTROL
                | extra_access
                | if create || create_new {
                    GENERIC_WRITE
                } else {
                    0
                },
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &security.attributes(),
            if create_new {
                CREATE_NEW
            } else if create {
                OPEN_ALWAYS
            } else {
                OPEN_EXISTING
            },
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    reject_reparse(&file)?;
    verify_private(&file)?;
    Ok(file)
}

/// Attribute-only access that intentionally follows links, used solely where
/// local-tool selection already permits package-manager indirection.
pub fn following_metadata_file(path: &Path) -> io::Result<File> {
    let name = native_path(path)?;
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_handle(raw) })
}

pub fn rename_file(source: &Path, destination: &Path, replace: bool) -> io::Result<()> {
    rename_file_with_hook(source, destination, replace, || {})
}

fn rename_file_with_hook(
    source: &Path,
    destination: &Path,
    replace: bool,
    after_move: impl FnOnce(),
) -> io::Result<()> {
    let _source_parent = pin_parent(source)?;
    let _destination_parent = pin_parent(destination)?;
    let source_identity = open_no_reparse(source)?;
    if source_identity.identity.directory {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "File replacement cannot rename a directory",
        ));
    }
    match open_no_reparse(destination) {
        Ok(target) if target.identity.directory => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "File replacement cannot overwrite a directory",
            ))
        }
        Ok(_) => (),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let from = native_path(source)?;
    let to = native_path(destination)?;
    unsafe {
        checked(MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_WRITE_THROUGH
                | if replace {
                    MOVEFILE_REPLACE_EXISTING
                } else {
                    0
                },
        ))?;
    }
    after_move();
    confirm_publication(destination, &source_identity.identity)
}

/// Publish a directory only into an absent name, including when an existing
/// destination is empty. A move across volumes is never turned into a copy.
pub fn rename_directory_noreplace(source: &Path, destination: &Path) -> io::Result<()> {
    rename_directory_with_hook(source, destination, || {})
}

/// Publish the exact owned directory, never a same-name replacement. The source
/// DELETE handle remains bound to its full identity through the native rename.
/// Destination-parent handles pin the absolute target namespace throughout.
pub fn rename_directory_noreplace_expected(
    source: &Path,
    destination: &Path,
    expected: &FileIdentity,
) -> io::Result<()> {
    if !expected.directory {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "Expected a directory identity",
        ));
    }
    rename_expected_directory_with_hook(source, destination, expected, || {}, || {})
}

/// Publish an independently owned ordinary file through its exact DELETE
/// handle. This is the file counterpart of the owned-directory publication.
pub fn rename_file_noreplace_expected(
    source: &Path,
    destination: &Path,
    expected: &FileIdentity,
) -> io::Result<()> {
    if expected.directory || expected.links != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected an independent file identity",
        ));
    }
    rename_expected_directory_with_hook(source, destination, expected, || {}, || {})
}

fn rename_expected_directory_with_hook(
    source: &Path,
    destination: &Path,
    expected: &FileIdentity,
    before_move: impl FnOnce(),
    after_move: impl FnOnce(),
) -> io::Result<()> {
    let _source_parent = pin_parent(source)?;
    let _destination_parent = pin_parent(destination)?;
    let from = native_path(source)?;
    let raw = unsafe {
        CreateFileW(
            from.as_ptr(),
            DELETE | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = unsafe { File::from_raw_handle(raw) };
    reject_reparse(&file)?;
    let captured = identity(&file)?;
    if captured.directory != expected.directory
        || (!captured.directory && captured.links != 1)
        || !expected.same_object(&captured)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Owned object changed before publication",
        ));
    }
    before_move();
    rename_opened_handle(&file, destination, false)?;
    after_move();
    confirm_publication(destination, expected)
}

/// For an admitted private file created with DELETE access, native success is
/// the commit point. No fallible observation follows the successful rename;
/// transactional callers must not roll back referenced material afterward.
pub fn rename_private_file_handle(
    file: &File,
    destination: &Path,
    replace: bool,
) -> io::Result<()> {
    let _parent = pin_parent(destination)?;
    reject_reparse(file)?;
    let captured = identity(file)?;
    if captured.directory || captured.links != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected an independent private file",
        ));
    }
    verify_private(file)?;
    rename_opened_handle(file, destination, replace)
}

fn rename_opened_handle(file: &File, destination: &Path, replace: bool) -> io::Result<()> {
    let name = native_path(destination)?;
    // The variable-length Win32 structure must retain its native alignment.
    // Leave RootDirectory null; the complete UTF-16
    // destination is protected by the pinned directory/ancestor handles above.
    let size = size_of::<FILE_RENAME_INFO>() + name.len() * size_of::<u16>();
    let mut buffer = vec![0usize; size.div_ceil(size_of::<usize>())];
    let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*information).Anonymous.ReplaceIfExists = replace;
        (*information).FileNameLength = ((name.len() - 1) * size_of::<u16>()) as u32;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            (*information).FileName.as_mut_ptr(),
            name.len(),
        );
    }
    unsafe {
        checked(SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfo,
            information.cast(),
            size as u32,
        ))
    }
}

fn rename_directory_with_hook(
    source: &Path,
    destination: &Path,
    after_move: impl FnOnce(),
) -> io::Result<()> {
    let source = normalized_path(source)?;
    let destination = normalized_path(destination)?;
    let source = source.as_path();
    let destination = destination.as_path();
    let _source_parent = pin_parent(source)?;
    let _destination_parent = pin_parent(destination)?;
    let source_identity = open_no_reparse(source)?;
    if !source_identity.identity.directory {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "Expected an ordinary directory candidate",
        ));
    }
    match std::fs::symlink_metadata(destination) {
        Ok(_) => {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Directory publication cannot replace an existing destination",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    let from = native_path(source)?;
    let to = native_path(destination)?;
    unsafe {
        checked(MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        ))?;
    }
    after_move();
    confirm_publication(destination, &source_identity.identity)
}

fn confirm_publication(destination: &Path, expected: &FileIdentity) -> io::Result<()> {
    // The move already returned success. Every subsequent inspection failure,
    // including NotFound/sharing/access errors, is an unknown post-commit
    // outcome. Preserve the underlying I/O cause, never classify it as a clean
    // pre-commit failure that could authorize rollback of related material.
    let published = open_no_reparse(destination).map_err(publication_unknown)?;
    if !expected.same_object(&published.identity) {
        return Err(publication_unknown(io::Error::new(
            io::ErrorKind::InvalidData,
            "Publication identity is unconfirmed after the move",
        )));
    }
    Ok(())
}

#[derive(Debug)]
struct UnconfirmedPublication(io::Error);
impl std::fmt::Display for UnconfirmedPublication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Publication outcome is unknown after the move: {}",
            self.0
        )
    }
}
impl std::error::Error for UnconfirmedPublication {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}
fn publication_unknown(cause: io::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, UnconfirmedPublication(cause))
}

/// Read-only counterpart of Unix uid ownership checks. This does not require
/// or repair a DACL, inheritance policy, link count, or any system ancestor.
pub fn verify_current_user_owner(file: &File) -> io::Result<()> {
    let current = CurrentUser::load()?;
    let mut owner = null_mut();
    let mut descriptor = null_mut();
    let result = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let _allocation = LocalAllocation(descriptor);
    if owner.is_null() || unsafe { EqualSid(owner, current.sid()) } == 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Object does not belong to the current user",
        ));
    }
    Ok(())
}

pub fn verify_private(file: &File) -> io::Result<()> {
    let current = CurrentUser::load()?;
    let mut owner = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor = null_mut();
    let result = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let _allocation = LocalAllocation(descriptor);
    let mut control = 0u16;
    let mut revision = 0;
    unsafe {
        checked(GetSecurityDescriptorControl(
            descriptor,
            &mut control,
            &mut revision,
        ))?;
    }
    let valid = unsafe {
        !owner.is_null()
            && EqualSid(owner, current.sid()) != 0
            && !dacl.is_null()
            && control & SE_DACL_PROTECTED != 0
            && (*dacl).AceCount == 1
    };
    if !valid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Expected current-user ownership and a protected private DACL",
        ));
    }
    let mut raw_ace = null_mut();
    unsafe {
        checked(GetAce(dacl, 0, &mut raw_ace))?;
    }
    let ace = unsafe { &*raw_ace.cast::<ACCESS_ALLOWED_ACE>() };
    if ace.Header.AceType != ACCESS_ALLOWED_ACE_TYPE
        || ace.Mask & 0x001f_01ff != 0x001f_01ff
        || unsafe {
            EqualSid(
                (&ace.SidStart as *const u32).cast_mut().cast(),
                current.sid(),
            )
        } == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Expected one current-user full-control ACE",
        ));
    }
    Ok(())
}

/// Windows cannot fsync directory handles. File contents are flushed with
/// File::sync_all / FlushFileBuffers; namespace crash recovery uses journals.
pub fn sync_directory(_directory: &File) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use std::{
        fs,
        io::{Read, Seek, SeekFrom, Write},
    };

    #[test]
    fn private_creation_identity_and_replacement_use_real_handles() {
        let temp = tempfile::Builder::new()
            .prefix("panda-native-中文 space-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let private = root.join("disposable-私有 folder");
        create_private_directory(&private).unwrap();
        let path = private.join("secret.json");
        let mut file = create_private_file(&path).unwrap();
        verify_private(&file).unwrap();
        file.write_all(b"artificial-test-data").unwrap();
        file.sync_all().unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"artificial-test-data");
        let first = identity(&file).unwrap();
        assert_eq!(first.file_id.len(), 16);
        assert_eq!(first.links, 1);
        assert!(!first.directory);
        assert!(first.same_object(&open_no_reparse(&path).unwrap().identity));
        assert!(create_private_file(&path).is_err());
        fs::rename(&path, private.join("retained.json")).unwrap();
        let replacement = create_private_file(&path).unwrap();
        assert!(!first.same_object(&identity(&replacement).unwrap()));
        drop(replacement);
        drop(file);
    }

    #[test]
    fn ordinary_inherited_acl_is_not_reported_as_private() {
        let temp = tempfile::Builder::new()
            .prefix("panda-native-中文 space-")
            .tempdir()
            .unwrap();
        let path = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("disposable-public.txt");
        fs::write(&path, b"disposable").unwrap();
        assert!(verify_private(&open_no_reparse(&path).unwrap().file).is_err());
    }

    #[test]
    fn failed_file_inspection_after_a_real_move_is_an_unknown_outcome() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-file-postcommit-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("candidate");
        let destination = root.join("destination");
        let retained = root.join("retained-published-file");
        fs::write(&source, b"published disposable bytes").unwrap();
        fs::write(&destination, b"old disposable bytes").unwrap();
        let expected = open_no_reparse(&source).unwrap().identity;
        let error = rename_file_with_hook(&source, &destination, true, || {
            // Deterministically change the public name after the real native
            // move, before its observation; no foreign/non-test data is used.
            fs::rename(&destination, &retained).unwrap();
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let cause = error
            .get_ref()
            .unwrap()
            .downcast_ref::<UnconfirmedPublication>()
            .unwrap();
        assert_eq!(cause.0.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("outcome is unknown"));
        assert!(!source.exists());
        assert!(!destination.exists());
        assert!(expected.same_object(&open_no_reparse(&retained).unwrap().identity));
        assert_eq!(fs::read(retained).unwrap(), b"published disposable bytes");
    }

    #[test]
    fn failed_directory_inspection_after_a_real_move_is_an_unknown_outcome() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-directory-postcommit-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("candidate");
        let destination = root.join("destination");
        let retained = root.join("retained-published-directory");
        fs::create_dir(&source).unwrap();
        fs::write(
            source.join("payload"),
            b"published disposable directory bytes",
        )
        .unwrap();
        let expected = open_no_reparse(&source).unwrap().identity;
        let error = rename_directory_with_hook(&source, &destination, || {
            fs::rename(&destination, &retained).unwrap();
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<UnconfirmedPublication>()
                .unwrap()
                .0
                .kind(),
            io::ErrorKind::NotFound
        );
        assert!(!source.exists());
        assert!(!destination.exists());
        assert!(expected.same_object(&open_no_reparse(&retained).unwrap().identity));
        assert_eq!(
            fs::read(retained.join("payload")).unwrap(),
            b"published disposable directory bytes"
        );
    }

    #[test]
    fn expected_directory_publication_never_moves_a_same_name_replacement() {
        for replace_after_open in [false, true] {
            let temporary = tempfile::Builder::new()
                .prefix("panda-disposable-bound-publication-")
                .tempdir()
                .unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let source = root.join("owned 中文 stage");
            let retained = root.join("renamed-owned-stage");
            let destination = root.join("published 中文 payload");
            fs::create_dir(&source).unwrap();
            fs::write(source.join("payload"), b"owned disposable bytes").unwrap();
            let expected = open_no_reparse(&source).unwrap().identity;
            let replace = || {
                fs::rename(&source, &retained).unwrap();
                fs::create_dir(&source).unwrap();
                fs::write(source.join("foreign"), b"preserve external replacement").unwrap();
            };
            let result = if replace_after_open {
                rename_expected_directory_with_hook(
                    &source,
                    &destination,
                    &expected,
                    replace,
                    || {},
                )
            } else {
                replace();
                rename_directory_noreplace_expected(&source, &destination, &expected)
            };
            assert_eq!(
                fs::read(source.join("foreign")).unwrap(),
                b"preserve external replacement"
            );
            if replace_after_open {
                result.unwrap();
                assert!(!retained.exists());
                assert!(expected.same_object(&open_no_reparse(&destination).unwrap().identity));
                assert_eq!(
                    fs::read(destination.join("payload")).unwrap(),
                    b"owned disposable bytes"
                );
            } else {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("changed before publication"));
                assert!(!destination.exists());
                assert!(expected.same_object(&open_no_reparse(&retained).unwrap().identity));
            }
        }
    }

    #[test]
    fn expected_directory_publication_preserves_occupied_targets_and_reports_unknown_commit() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("candidate");
        let destination = root.join("occupied");
        let retained = root.join("retained-published-directory");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("payload"), b"owned disposable bytes").unwrap();
        fs::create_dir(&destination).unwrap();
        let expected = open_no_reparse(&source).unwrap().identity;
        let occupied = open_no_reparse(&destination).unwrap().identity;
        assert!(rename_directory_noreplace_expected(&source, &destination, &expected).is_err());
        assert!(occupied.same_object(&open_no_reparse(&destination).unwrap().identity));
        assert!(expected.same_object(&open_no_reparse(&source).unwrap().identity));
        fs::remove_dir(&destination).unwrap();
        let error = rename_expected_directory_with_hook(
            &source,
            &destination,
            &expected,
            || {},
            || fs::rename(&destination, &retained).unwrap(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Publication outcome is unknown"));
        assert!(expected.same_object(&open_no_reparse(&retained).unwrap().identity));
        assert_eq!(
            fs::read(retained.join("payload")).unwrap(),
            b"owned disposable bytes"
        );
    }

    #[test]
    fn expected_file_publication_uses_the_owned_handle_and_never_overwrites_a_target() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("download.partial");
        let retained = root.join("retained-owned.partial");
        let destination = root.join("fixed-archive.zip");
        fs::write(&source, b"verified disposable archive").unwrap();
        fs::write(&destination, b"occupied caller archive").unwrap();
        let expected = open_no_reparse(&source).unwrap().identity;
        assert!(rename_file_noreplace_expected(&source, &destination, &expected).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"occupied caller archive");
        fs::remove_file(&destination).unwrap();
        rename_expected_directory_with_hook(
            &source,
            &destination,
            &expected,
            || {
                fs::rename(&source, &retained).unwrap();
                fs::write(&source, b"preserve foreign replacement").unwrap();
            },
            || {},
        )
        .unwrap();
        assert!(expected.same_object(&open_no_reparse(&destination).unwrap().identity));
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"verified disposable archive"
        );
        assert_eq!(fs::read(&source).unwrap(), b"preserve foreign replacement");
        assert!(!retained.exists());
        assert!(
            rename_file_noreplace_expected(&source, &root.join("unrelated"), &expected).is_err()
        );
        assert_eq!(fs::read(source).unwrap(), b"preserve foreign replacement");
    }

    fn security_bytes(path: &Path) -> Vec<u8> {
        let opened = open_no_reparse(path).unwrap();
        let mut descriptor = null_mut();
        let result = unsafe {
            GetSecurityInfo(
                opened.file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(result, 0);
        let allocation = LocalAllocation(descriptor);
        let length =
            unsafe { windows_sys::Win32::Security::GetSecurityDescriptorLength(descriptor) };
        assert!(length > 0);
        let bytes = unsafe { std::slice::from_raw_parts(descriptor.cast::<u8>(), length as usize) }
            .to_vec();
        drop(allocation);
        bytes
    }

    #[test]
    fn admitted_directory_gets_private_access_without_changing_parent_security() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-empty-root-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let target = root.join("admitted-empty-directory");
        fs::create_dir(&target).unwrap();
        let parent_before = security_bytes(&root);
        assert!(verify_private(&open_no_reparse(&target).unwrap().file).is_err());
        make_directory_private(&target).unwrap();
        verify_private(&open_no_reparse(&target).unwrap().file).unwrap();
        assert_eq!(security_bytes(&root), parent_before);
        create_private_file(&target.join("disposable-marker.json"))
            .unwrap()
            .write_all(b"disposable marker")
            .unwrap();
        make_directory_private(&target).unwrap();
        assert_eq!(
            fs::read(target.join("disposable-marker.json")).unwrap(),
            b"disposable marker"
        );
        assert_eq!(security_bytes(&root), parent_before);
    }

    #[test]
    fn directory_protection_rejects_files_and_junctions_without_changing_their_targets() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-root-permission-boundary-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let file = root.join("retained-file.txt");
        fs::write(&file, b"retained disposable bytes").unwrap();
        let before = security_bytes(&file);
        assert!(make_directory_private(&file).is_err());
        assert_eq!(security_bytes(&file), before);
        assert_eq!(fs::read(&file).unwrap(), b"retained disposable bytes");
        let target = root.join("retained-directory");
        fs::create_dir(&target).unwrap();
        let before = security_bytes(&target);
        let link = root.join("disposable-junction");
        let result = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(result.status.success());
        assert!(make_directory_private(&link).is_err());
        assert_eq!(security_bytes(&target), before);
        fs::remove_dir(link).unwrap();
    }

    #[test]
    fn occupied_replacement_keeps_both_files_and_succeeds_after_the_handle_closes() {
        use std::{fs::OpenOptions, io::Write, os::windows::fs::OpenOptionsExt};
        let temp = tempfile::Builder::new()
            .prefix("panda-native-occupied-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join(".write-disposable");
        let destination = root.join("disposable-settings.json");
        create_private_file(&source)
            .unwrap()
            .write_all(b"new disposable bytes")
            .unwrap();
        create_private_file(&destination)
            .unwrap()
            .write_all(b"old disposable bytes")
            .unwrap();
        let blocker = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&destination)
            .unwrap();
        let error = rename_file(&source, &destination, true).unwrap_err();
        assert!(matches!(error.raw_os_error(), Some(5 | 32)));
        assert_eq!(fs::read(&source).unwrap(), b"new disposable bytes");
        drop(blocker);
        assert_eq!(fs::read(&destination).unwrap(), b"old disposable bytes");
        rename_file(&source, &destination, true).unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"new disposable bytes");
    }

    #[test]
    fn private_creation_parent_cannot_be_renamed_during_the_operation() {
        let temp = tempfile::Builder::new()
            .prefix("panda-native-中文 space-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let parent = root.join("disposable-parent");
        let moved = root.join("disposable-moved-parent");
        fs::create_dir(&parent).unwrap();
        let guard = pin_parent(&parent.join("new-file")).unwrap();
        assert!(fs::rename(&parent, &moved).is_err());
        assert!(parent.is_dir());
        drop(guard);
        fs::rename(&parent, &moved).unwrap();
        assert!(moved.is_dir());
    }

    #[test]
    fn long_unicode_paths_work_without_registry_changes() {
        let temp = tempfile::Builder::new()
            .prefix("panda-native-中文 space-")
            .tempdir()
            .unwrap();
        let mut path = temp.path().canonicalize().unwrap();
        for number in 0..14 {
            path = path.join(format!("disposable-中文 long component {number}"));
            create_private_directory(&path).unwrap();
        }
        assert!(path.as_os_str().encode_wide().count() > 260);
        let file_path = path.join("sample.txt");
        let file = create_private_file(&file_path).unwrap();
        assert!(identity(&file)
            .unwrap()
            .same_object(&open_no_reparse(&file_path).unwrap().identity));
    }

    #[test]
    fn junctions_are_rejected_at_the_leaf_and_in_ancestors() {
        let temp = tempfile::Builder::new()
            .prefix("panda-native-中文 space-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let target = root.join("disposable-target");
        let link = root.join("disposable-junction");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("sample.txt"), b"disposable").unwrap();
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Could not create the disposable junction fixture"
        );
        assert!(open_no_reparse(&link).is_err());
        assert!(open_no_reparse(&link.join("sample.txt")).is_err());
        let mut raw = root.as_os_str().to_os_string();
        raw.push("\\disposable-junction/sample.txt");
        let mixed = PathBuf::from(raw);
        assert!(open_no_reparse(&mixed).is_err());
        assert!(open_read_file_allowing_hard_links(&mixed).is_err());
        let mut raw = root.as_os_str().to_os_string();
        raw.push("\\disposable-junction/not-created.txt");
        assert!(create_private_file(&PathBuf::from(raw)).is_err());
        assert!(!target.join("not-created.txt").exists());
        assert_eq!(
            crate::filesystem::find_reparse_component(&link, false).unwrap(),
            Some(link.clone())
        );
        assert_eq!(
            crate::filesystem::find_reparse_component(&link.join("sample.txt"), false).unwrap(),
            Some(link.clone())
        );
        assert_eq!(
            crate::filesystem::find_reparse_component(&target.join("missing/child"), true).unwrap(),
            None
        );
        assert!(
            crate::filesystem::find_reparse_component(&target.join("missing/child"), false)
                .is_err()
        );
        fs::remove_dir(&link).unwrap();
        assert_eq!(fs::read(target.join("sample.txt")).unwrap(), b"disposable");
    }

    #[test]
    fn explicit_owned_name_removal_does_not_add_an_independent_file_requirement() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-linked-name-removal-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let path = root.join("owned-name");
        let retained = root.join("retained-name");
        std::fs::write(&path, b"disposable linked bytes").unwrap();
        std::fs::hard_link(&path, &retained).unwrap();
        let expected = open_no_reparse(&path).unwrap();
        assert!(remove_file(&path, &expected.identity).is_err());
        assert!(path.exists());
        remove_file_allowing_hard_links(&path, &expected.identity).unwrap();
        drop(expected);
        assert!(!path.exists());
        assert_eq!(
            std::fs::read(&retained).unwrap(),
            b"disposable linked bytes"
        );
        assert_eq!(open_no_reparse(&retained).unwrap().identity.links, 1);
    }
}
