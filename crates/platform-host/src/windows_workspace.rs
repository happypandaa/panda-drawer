//! Restricted Windows W2.1 storage primitives. These are independent of the
//! observational probe and disposable WebView cache. Opening a root does not
//! admit its contents: the caller must validate the complete W2.1 format and
//! registration before exposing a project. No recovery or deletion is provided.
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    ffi::{c_void, OsStr},
    fmt,
    fs::File,
    io::{Read, Write},
    mem::{offset_of, size_of},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
    ptr::{null, null_mut},
    sync::{Arc, Mutex},
};
use windows_sys::{
    Wdk::{
        Foundation::OBJECT_ATTRIBUTES,
        Storage::FileSystem::{
            FileRenameInformation, NtCreateFile, NtSetInformationFile, FILE_CREATE,
            FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
            FILE_RENAME_INFORMATION, FILE_SYNCHRONOUS_IO_NONALERT,
        },
    },
    Win32::{
        Foundation::{
            GetLastError, LocalFree, RtlNtStatusToDosError, ERROR_ALREADY_EXISTS,
            ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE, OBJ_CASE_INSENSITIVE,
            OBJ_DONT_REPARSE, UNICODE_STRING,
        },
        Security::{
            AclSizeInformation,
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetSecurityInfo, SE_FILE_OBJECT,
            },
            GetAce, GetAclInformation, GetSecurityDescriptorControl, GetTokenInformation,
            IsValidAcl, IsValidSecurityDescriptor, IsValidSid, TokenUser, ACCESS_ALLOWED_ACE,
            ACE_HEADER, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
            OWNER_SECURITY_INFORMATION, PSID, SECURITY_ATTRIBUTES, SE_DACL_PROTECTED, TOKEN_QUERY,
            TOKEN_USER,
        },
        Storage::FileSystem::{
            BusTypeAta, BusTypeNvme, BusTypeSata, CreateFileW, FileAttributeTagInfo,
            FileCaseSensitiveInfo, FileIdBothDirectoryInfo, FileIdBothDirectoryRestartInfo,
            FileIdInfo, FileStandardInfo, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
            GetVolumeInformationByHandleW, GetVolumeNameForVolumeMountPointW, QueryDosDeviceW,
            DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
            FILE_CASE_SENSITIVE_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_ID_BOTH_DIR_INFO, FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
            FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FILE_STANDARD_INFO, FILE_TRAVERSE, FILE_WRITE_DATA,
            IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING, READ_CONTROL, SYNCHRONIZE,
            VOLUME_NAME_GUID,
        },
        System::{
            Com::CoTaskMemFree,
            Ioctl::{
                PropertyStandardQuery, StorageDeviceProperty, IOCTL_STORAGE_GET_HOTPLUG_INFO,
                IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DEVICE_DESCRIPTOR, STORAGE_HOTPLUG_INFO,
                STORAGE_PROPERTY_QUERY, VOLUME_DISK_EXTENTS,
            },
            Threading::{CreateMutexExW, GetCurrentProcess, OpenProcessToken, MUTEX_ALL_ACCESS},
            IO::{DeviceIoControl, IO_STATUS_BLOCK},
        },
        UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath},
    },
};

const DIRECTORY_ACCESS: u32 =
    FILE_LIST_DIRECTORY | FILE_TRAVERSE | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE;
const DIRECTORY_SHARING: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE;
const MAX_ENTRIES: usize = 256;
const MAX_BYTES: usize = 8_000_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WindowsObjectIdentity {
    pub volume_serial: u64,
    pub file_id128: [u8; 16],
}

#[derive(Clone, Debug)]
pub struct WorkspaceError {
    pub code: &'static str,
    pub message: String,
}
impl fmt::Display for WorkspaceError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for WorkspaceError {}
type Result<T> = std::result::Result<T, WorkspaceError>;
fn error(code: &'static str, message: impl Into<String>) -> WorkspaceError {
    WorkspaceError {
        code,
        message: message.into(),
    }
}
fn os_error(code: &'static str) -> WorkspaceError {
    error(code, std::io::Error::last_os_error().to_string())
}
fn nt_result(status: i32, code: &'static str) -> Result<()> {
    if status < 0 {
        let win32 = unsafe { RtlNtStatusToDosError(status) };
        Err(error(
            code,
            format!(
                "NTSTATUS=0x{:08X}, Win32={win32}: {}",
                status as u32,
                std::io::Error::from_raw_os_error(win32 as i32)
            ),
        ))
    } else {
        Ok(())
    }
}

struct Handle(OwnedHandle);
impl Handle {
    unsafe fn own(raw: HANDLE) -> Self {
        Self(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
    fn raw(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
}
struct Allocation(*mut c_void);
impl Drop for Allocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}
struct Object {
    handle: Handle,
    identity: WindowsObjectIdentity,
    directory: bool,
}
impl Object {
    fn raw(&self) -> HANDLE {
        self.handle.raw()
    }
    fn verify(&self) -> Result<FILE_STANDARD_INFO> {
        let tag: FILE_ATTRIBUTE_TAG_INFO = info(self.raw(), FileAttributeTagInfo)?;
        let standard: FILE_STANDARD_INFO = info(self.raw(), FileStandardInfo)?;
        if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(error(
                "WORKSPACE_REPARSE_REJECTED",
                "重解析点不在 W2.1 范围内。",
            ));
        }
        if standard.Directory != self.directory || standard.DeletePending {
            return Err(error(
                "WORKSPACE_OBJECT_REJECTED",
                "对象类型改变或正在删除。",
            ));
        }
        if self.directory
            && info::<FILE_CASE_SENSITIVE_INFO>(self.raw(), FileCaseSensitiveInfo)?.Flags & 1 != 0
        {
            return Err(error(
                "WORKSPACE_CASE_SENSITIVE_REJECTED",
                "大小写敏感目录尚未开放。",
            ));
        }
        if !self.directory && standard.NumberOfLinks != 1 {
            return Err(error(
                "WORKSPACE_HARDLINK_REJECTED",
                "文件必须是无硬链接的独立普通文件。",
            ));
        }
        if identity(self.raw())? != self.identity {
            return Err(error(
                "WORKSPACE_IDENTITY_CHANGED",
                "打开对象的卷或文件身份已变化。",
            ));
        }
        Ok(standard)
    }
}
fn info<T: Default>(raw: HANDLE, class: i32) -> Result<T> {
    let mut value = T::default();
    if unsafe {
        GetFileInformationByHandleEx(
            raw,
            class,
            (&mut value as *mut T).cast(),
            size_of::<T>() as u32,
        )
    } == 0
    {
        return Err(os_error("WORKSPACE_METADATA_UNAVAILABLE"));
    }
    Ok(value)
}
fn identity(raw: HANDLE) -> Result<WindowsObjectIdentity> {
    let value: FILE_ID_INFO = info(raw, FileIdInfo)?;
    if value.FileId.Identifier == [0; 16] {
        return Err(error(
            "WORKSPACE_IDENTITY_UNAVAILABLE",
            "卷未提供完整文件身份。",
        ));
    }
    Ok(WindowsObjectIdentity {
        volume_serial: value.VolumeSerialNumber,
        file_id128: value.FileId.Identifier,
    })
}
fn object(handle: Handle, directory: bool) -> Result<Arc<Object>> {
    let value = Object {
        identity: identity(handle.raw())?,
        handle,
        directory,
    };
    value.verify()?;
    Ok(Arc::new(value))
}
fn verify_leaf_name(raw: HANDLE, expected: &str) -> Result<()> {
    let mut buffer = vec![0u16; 32768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            raw,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            VOLUME_NAME_GUID,
        )
    };
    if length == 0 || length >= buffer.len() as u32 {
        return Err(os_error("WORKSPACE_CANONICAL_NAME_UNAVAILABLE"));
    }
    let actual = String::from_utf16(&buffer[..length as usize]).map_err(|_| {
        error(
            "WORKSPACE_CANONICAL_NAME_UNAVAILABLE",
            "实际文件名不是有效 Unicode。",
        )
    })?;
    if actual.rsplit('\\').next() != Some(expected) {
        return Err(error(
            "WORKSPACE_NAME_ALIAS_REJECTED",
            "文件名大小写或短名称别名与明确路径片段不一致。",
        ));
    }
    Ok(())
}
fn wide(value: &OsStr) -> Result<Vec<u16>> {
    let mut value: Vec<u16> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(error("WORKSPACE_PATH_REJECTED", "路径含有空字符。"));
    }
    value.push(0);
    Ok(value)
}
fn component(value: &str) -> Result<Vec<u16>> {
    let units: Vec<u16> = value.encode_utf16().collect();
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || (stem.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        || ["COM¹", "COM²", "COM³", "LPT¹", "LPT²", "LPT³"].contains(&stem.as_str());
    if units.is_empty()
        || units.len() > 255
        || value == "."
        || value == ".."
        || reserved
        || matches!(units.last(), Some(32 | 46))
        || units
            .iter()
            .any(|unit| *unit < 32 || matches!(*unit, 34 | 42 | 47 | 58 | 60 | 62 | 63 | 92 | 124))
    {
        return Err(error(
            "WORKSPACE_COMPONENT_REJECTED",
            "仅接受一个普通相对路径片段。",
        ));
    }
    Ok(units)
}

fn sid_string(sid: PSID) -> Result<String> {
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        return Err(error("WORKSPACE_ACL_INVALID", "SID 结构无法识别。"));
    }
    let mut pointer = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut pointer) } == 0 {
        return Err(os_error("WORKSPACE_ACL_UNAVAILABLE"));
    }
    let allocation = Allocation(pointer.cast());
    let result = unsafe { text(pointer, 256) };
    drop(allocation);
    result
}
unsafe fn text(pointer: *const u16, limit: usize) -> Result<String> {
    if pointer.is_null() {
        return Err(error("WORKSPACE_SYSTEM_VALUE_INVALID", "系统文本为空。"));
    }
    let mut length = 0;
    while length < limit && unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    if length == limit {
        return Err(error(
            "WORKSPACE_SYSTEM_VALUE_INVALID",
            "系统文本超过上限。",
        ));
    }
    String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, length) }).map_err(|_| {
        error(
            "WORKSPACE_SYSTEM_VALUE_INVALID",
            "系统文本不是有效 Unicode。",
        )
    })
}
fn user_sid() -> Result<String> {
    let mut raw = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(os_error("WORKSPACE_USER_UNAVAILABLE"));
    }
    let token = unsafe { Handle::own(raw) };
    let mut size = 0;
    unsafe {
        GetTokenInformation(token.raw(), TokenUser, null_mut(), 0, &mut size);
    }
    if size == 0 || size > 65536 {
        return Err(error(
            "WORKSPACE_USER_UNAVAILABLE",
            "当前用户身份超出支持范围。",
        ));
    }
    let mut bytes = vec![0u64; (size as usize).div_ceil(8)];
    if unsafe {
        GetTokenInformation(
            token.raw(),
            TokenUser,
            bytes.as_mut_ptr().cast(),
            size,
            &mut size,
        )
    } == 0
    {
        return Err(os_error("WORKSPACE_USER_UNAVAILABLE"));
    }
    sid_string(unsafe { (*bytes.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}
fn descriptor(sid: &str, directory: bool) -> Result<Allocation> {
    let inherit = if directory { "OICI" } else { "" };
    let value =
        format!("O:{sid}D:P(A;{inherit};FA;;;{sid})(A;{inherit};FA;;;SY)(A;{inherit};FA;;;BA)");
    let value = wide(OsStr::new(&value))?;
    let mut raw = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            value.as_ptr(),
            1,
            &mut raw,
            null_mut(),
        )
    } == 0
    {
        return Err(os_error("WORKSPACE_PRIVATE_ACL_UNAVAILABLE"));
    }
    Ok(Allocation(raw))
}
fn owner_and_acl(raw: HANDLE, sid: &str, private: bool) -> Result<()> {
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let mut descriptor = null_mut();
    let flags = OWNER_SECURITY_INFORMATION
        | if private {
            DACL_SECURITY_INFORMATION
        } else {
            0
        };
    let status = unsafe {
        GetSecurityInfo(
            raw,
            SE_FILE_OBJECT,
            flags,
            &mut owner,
            null_mut(),
            if private { &mut dacl } else { null_mut() },
            null_mut(),
            &mut descriptor,
        )
    };
    let allocation = Allocation(descriptor);
    if status != 0 || descriptor.is_null() || unsafe { IsValidSecurityDescriptor(descriptor) } == 0
    {
        return Err(error(
            "WORKSPACE_PRIVATE_ACL_UNVERIFIED",
            "所有者或权限无法确认；未修改旧 ACL。",
        ));
    }
    if sid_string(owner)? != sid {
        return Err(error(
            "WORKSPACE_OWNER_REJECTED",
            "对象不属于当前用户；未修复所有者。",
        ));
    }
    if private {
        if dacl.is_null() || unsafe { IsValidAcl(dacl) } == 0 {
            return Err(error(
                "WORKSPACE_PRIVATE_ACL_UNVERIFIED",
                "私有 DACL 无法确认。",
            ));
        }
        let mut control = 0;
        let mut revision = 0;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
            || control & SE_DACL_PROTECTED == 0
        {
            return Err(error(
                "WORKSPACE_PRIVATE_ACL_UNVERIFIED",
                "W2.1 对象必须阻止继承其他身份的权限。",
            ));
        }
        let mut information = ACL_SIZE_INFORMATION::default();
        if unsafe {
            GetAclInformation(
                dacl,
                (&mut information as *mut ACL_SIZE_INFORMATION).cast(),
                size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        } == 0
        {
            return Err(os_error("WORKSPACE_PRIVATE_ACL_UNVERIFIED"));
        }
        if information.AceCount > 256 {
            return Err(error("WORKSPACE_ACL_UNSUPPORTED", "ACL 超出检查上限。"));
        }
        let mut user_allow = false;
        for index in 0..information.AceCount {
            let mut raw_ace = null_mut();
            if unsafe { GetAce(dacl, index, &mut raw_ace) } == 0 || raw_ace.is_null() {
                return Err(os_error("WORKSPACE_ACL_UNVERIFIED"));
            }
            let header = unsafe { &*raw_ace.cast::<ACE_HEADER>() };
            let sid_offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
            if !matches!(header.AceType, 0 | 1) || (header.AceSize as usize) < sid_offset + 8 {
                return Err(error(
                    "WORKSPACE_ACL_UNSUPPORTED",
                    "ACL 含有本阶段未支持的权限形式。",
                ));
            }
            let ace_sid = unsafe { raw_ace.cast::<u8>().add(sid_offset) };
            let count = unsafe { *ace_sid.add(1) } as usize;
            if sid_offset + 8 + count * 4 > header.AceSize as usize {
                return Err(error("WORKSPACE_ACL_INVALID", "ACL 长度无效。"));
            }
            let principal = sid_string(ace_sid.cast())?;
            if header.AceType == 0 {
                if principal != sid && principal != "S-1-5-18" && principal != "S-1-5-32-544" {
                    return Err(error(
                        "WORKSPACE_PRIVATE_ACL_REJECTED",
                        "旧 ACL 向其他身份授权；未进行权限修复。",
                    ));
                }
                if principal == sid {
                    user_allow = true;
                }
            }
        }
        if !user_allow {
            return Err(error(
                "WORKSPACE_PRIVATE_ACL_REJECTED",
                "ACL 未向当前用户明确授权。",
            ));
        }
    }
    drop(allocation);
    Ok(())
}

fn relative(
    parent: HANDLE,
    name: &str,
    directory: bool,
    create: bool,
    access: u32,
    sharing: u32,
    security: Option<&Allocation>,
) -> Result<Arc<Object>> {
    let mut units = component(name)?;
    let unicode = UNICODE_STRING {
        Length: (units.len() * 2) as u16,
        MaximumLength: (units.len() * 2) as u16,
        Buffer: units.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent,
        ObjectName: &unicode,
        Attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        SecurityDescriptor: security.map_or(null(), |value| value.0.cast()),
        SecurityQualityOfService: null(),
    };
    let mut raw = null_mut();
    let mut io = IO_STATUS_BLOCK::default();
    let status = unsafe {
        NtCreateFile(
            &mut raw,
            access,
            &attributes,
            &mut io,
            null(),
            if directory {
                FILE_ATTRIBUTE_DIRECTORY
            } else {
                FILE_ATTRIBUTE_NORMAL
            },
            sharing,
            if create { FILE_CREATE } else { FILE_OPEN },
            FILE_OPEN_REPARSE_POINT
                | FILE_SYNCHRONOUS_IO_NONALERT
                | if directory {
                    FILE_DIRECTORY_FILE
                } else {
                    FILE_NON_DIRECTORY_FILE
                },
            null(),
            0,
        )
    };
    if status == 0xC000_0034_u32 as i32 {
        return Err(error("WORKSPACE_NAME_MISSING", "指定的直接子对象不存在。"));
    }
    if status == 0xC000_0035_u32 as i32 {
        return Err(error(
            "WORKSPACE_ALREADY_EXISTS",
            "目标已存在；没有采用、截断或覆盖。",
        ));
    }
    nt_result(
        status,
        if create {
            "WORKSPACE_CREATE_NEW_FAILED"
        } else {
            "WORKSPACE_OPEN_FAILED"
        },
    )?;
    if raw.is_null() || raw == INVALID_HANDLE_VALUE {
        return Err(error("WORKSPACE_HANDLE_INVALID", "系统没有返回有效句柄。"));
    }
    let result = object(unsafe { Handle::own(raw) }, directory)?;
    verify_leaf_name(result.raw(), name)?;
    Ok(result)
}

fn known_folder() -> Result<PathBuf> {
    let mut pointer = null_mut();
    let status =
        unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, null_mut(), &mut pointer) };
    if status < 0 {
        return Err(error(
            "WORKSPACE_KNOWN_FOLDER_UNAVAILABLE",
            "无法定位固定 LocalAppData。",
        ));
    }
    let result = unsafe { text(pointer, 32768) };
    unsafe {
        CoTaskMemFree(pointer.cast());
    }
    result.map(PathBuf::from)
}
/// Locate the fixed business root without creating, opening, locking or
/// inspecting it. This is suitable for status display even while a store runs.
pub fn fixed_workspace_data_root() -> Result<PathBuf> {
    Ok(known_folder()?.join(crate::APP_IDENTIFIER))
}
fn drive_mapping(drive: u8) -> Result<String> {
    let name = wide(OsStr::new(&format!("{}:", drive as char)))?;
    let mut buffer = [0u16; 1024];
    if unsafe { QueryDosDeviceW(name.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } == 0 {
        return Err(os_error("WORKSPACE_VOLUME_ALIAS_UNVERIFIED"));
    }
    let mapping = unsafe { text(buffer.as_ptr(), buffer.len()) }?;
    if !mapping.starts_with("\\Device\\HarddiskVolume")
        || !mapping["\\Device\\HarddiskVolume".len()..]
            .bytes()
            .all(|b| b.is_ascii_digit())
        || mapping.len() == "\\Device\\HarddiskVolume".len()
    {
        return Err(error(
            "WORKSPACE_VOLUME_ALIAS_REJECTED",
            "映射盘、SUBST、网络或设备别名不在 W2.1 范围内。",
        ));
    }
    Ok(mapping)
}
fn control<T: Default>(raw: HANDLE, code: u32, input: Option<(*const c_void, usize)>) -> Result<T> {
    let mut output = T::default();
    let mut returned = 0;
    let (pointer, size) = input.unwrap_or((null(), 0));
    if unsafe {
        DeviceIoControl(
            raw,
            code,
            pointer,
            size as u32,
            (&mut output as *mut T).cast(),
            size_of::<T>() as u32,
            &mut returned,
            null_mut(),
        )
    } == 0
    {
        return Err(os_error("WORKSPACE_INTERNAL_VOLUME_UNVERIFIED"));
    }
    if returned < size_of::<T>() as u32 {
        return Err(error(
            "WORKSPACE_VOLUME_RESPONSE_INVALID",
            "存储设备信息不完整。",
        ));
    }
    Ok(output)
}
fn device_handle(path: &str) -> Result<Handle> {
    let name = wide(OsStr::new(path))?;
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(os_error("WORKSPACE_INTERNAL_VOLUME_UNVERIFIED"));
    }
    Ok(unsafe { Handle::own(raw) })
}
fn internal_volume(root: &Object, drive: u8) -> Result<()> {
    let mut filesystem = [0u16; 32];
    if unsafe {
        GetVolumeInformationByHandleW(
            root.raw(),
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    } == 0
    {
        return Err(os_error("WORKSPACE_FILESYSTEM_UNVERIFIED"));
    }
    if unsafe { text(filesystem.as_ptr(), filesystem.len()) }? != "NTFS" {
        return Err(error(
            "WORKSPACE_FILESYSTEM_UNSUPPORTED",
            "W2.1 只允许内置本地 NTFS。",
        ));
    }
    let drive_root = wide(OsStr::new(&format!("{}:\\", drive as char)))?;
    let mut guid = [0u16; 128];
    if unsafe {
        GetVolumeNameForVolumeMountPointW(drive_root.as_ptr(), guid.as_mut_ptr(), guid.len() as u32)
    } == 0
    {
        return Err(os_error("WORKSPACE_VOLUME_GUID_UNAVAILABLE"));
    }
    let volume_name = unsafe { text(guid.as_ptr(), guid.len()) }?;
    if !volume_name.starts_with("\\\\?\\Volume{") || !volume_name.ends_with("}\\") {
        return Err(error(
            "WORKSPACE_VOLUME_GUID_INVALID",
            "卷 GUID 路径无法确认。",
        ));
    }
    let mut actual = [0u16; 256];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            root.raw(),
            actual.as_mut_ptr(),
            actual.len() as u32,
            VOLUME_NAME_GUID,
        )
    };
    if length == 0
        || length >= actual.len() as u32
        || !unsafe { text(actual.as_ptr(), actual.len()) }?.eq_ignore_ascii_case(&volume_name)
    {
        return Err(error(
            "WORKSPACE_VOLUME_ALIAS_REJECTED",
            "盘符与实际卷身份不一致。",
        ));
    }
    let volume = device_handle(volume_name.trim_end_matches('\\'))?;
    let extents: VOLUME_DISK_EXTENTS =
        control(volume.raw(), IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, None)?;
    if extents.NumberOfDiskExtents != 1 {
        return Err(error(
            "WORKSPACE_VOLUME_LAYOUT_UNSUPPORTED",
            "多磁盘或未知卷布局尚未开放。",
        ));
    }
    let disk = device_handle(&format!(
        "\\\\.\\PhysicalDrive{}",
        extents.Extents[0].DiskNumber
    ))?;
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    // Descriptor strings are not read. A bounded buffer accommodates the variable
    // trailing bytes, then only the fixed header is interpreted.
    let mut bytes = vec![0u64; 8192 / 8];
    let mut returned = 0;
    if unsafe {
        DeviceIoControl(
            disk.raw(),
            IOCTL_STORAGE_QUERY_PROPERTY,
            (&query as *const STORAGE_PROPERTY_QUERY).cast(),
            size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            bytes.as_mut_ptr().cast(),
            8192,
            &mut returned,
            null_mut(),
        )
    } == 0
    {
        return Err(os_error("WORKSPACE_INTERNAL_VOLUME_UNVERIFIED"));
    }
    if returned < size_of::<STORAGE_DEVICE_DESCRIPTOR>() as u32 {
        return Err(error(
            "WORKSPACE_VOLUME_RESPONSE_INVALID",
            "设备描述不完整。",
        ));
    }
    let device = unsafe { &*bytes.as_ptr().cast::<STORAGE_DEVICE_DESCRIPTOR>() };
    let hotplug: STORAGE_HOTPLUG_INFO = control(disk.raw(), IOCTL_STORAGE_GET_HOTPLUG_INFO, None)?;
    if device.Size < size_of::<STORAGE_DEVICE_DESCRIPTOR>() as u32
        || device.Size > returned
        || !device_admitted(device.BusType, device.RemovableMedia, &hotplug)
    {
        return Err(error(
            "WORKSPACE_INTERNAL_VOLUME_REJECTED",
            "外置、可移除、虚拟或无法证明内置的卷尚未开放。",
        ));
    }
    Ok(())
}
fn device_admitted(bus: i32, removable: bool, hotplug: &STORAGE_HOTPLUG_INFO) -> bool {
    hotplug.Size == size_of::<STORAGE_HOTPLUG_INFO>() as u32
        && !removable
        && !hotplug.MediaRemovable
        && !hotplug.MediaHotplug
        && !hotplug.DeviceHotplug
        && [BusTypeAta, BusTypeSata, BusTypeNvme].contains(&bus)
}

struct Chain {
    objects: Vec<Arc<Object>>,
    drive: u8,
    mapping: String,
}
fn open_parent(path: &Path) -> Result<Chain> {
    let mut parts = path.components();
    let Some(Component::Prefix(prefix)) = parts.next() else {
        return Err(error(
            "WORKSPACE_PATH_REJECTED",
            "固定目录必须使用本机绝对盘符。",
        ));
    };
    let drive = match prefix.kind() {
        Prefix::Disk(drive) => drive.to_ascii_uppercase(),
        _ => {
            return Err(error(
                "WORKSPACE_PATH_REJECTED",
                "UNC、设备或替代命名空间尚未开放。",
            ))
        }
    };
    if !matches!(parts.next(), Some(Component::RootDir)) {
        return Err(error("WORKSPACE_PATH_REJECTED", "固定目录不是绝对路径。"));
    }
    let mapping = drive_mapping(drive)?;
    let root_name = wide(OsStr::new(&format!("\\\\?\\{}:\\", drive as char)))?;
    let raw = unsafe {
        CreateFileW(
            root_name.as_ptr(),
            DIRECTORY_ACCESS,
            DIRECTORY_SHARING,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(os_error("WORKSPACE_ANCESTOR_OPEN_FAILED"));
    }
    let first = object(unsafe { Handle::own(raw) }, true)?;
    internal_volume(&first, drive)?;
    let mut objects = vec![first];
    for part in parts {
        let Component::Normal(name) = part else {
            return Err(error("WORKSPACE_PATH_REJECTED", "固定目录包含非普通片段。"));
        };
        let name = name
            .to_str()
            .ok_or_else(|| error("WORKSPACE_PATH_REJECTED", "目录名不是有效 Unicode。"))?;
        let next = relative(
            objects.last().unwrap().raw(),
            name,
            true,
            false,
            DIRECTORY_ACCESS,
            DIRECTORY_SHARING,
            None,
        )
        .map_err(|mut failure| {
            failure.message = format!("打开固定祖先片段“{name}”失败：{}", failure.message);
            failure
        })?;
        if next.identity.volume_serial != objects[0].identity.volume_serial {
            return Err(error("WORKSPACE_VOLUME_CHANGED", "固定目录跨越了其他卷。"));
        }
        objects.push(next);
    }
    if drive_mapping(drive)? != mapping {
        return Err(error(
            "WORKSPACE_VOLUME_ALIAS_CHANGED",
            "打开过程中盘符映射已变化。",
        ));
    }
    Ok(Chain {
        objects,
        drive,
        mapping,
    })
}
fn reservation(sid: &str, label: &str) -> Result<Handle> {
    let security = descriptor(sid, false)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: security.0,
        bInheritHandle: 0,
    };
    let name = wide(OsStr::new(&format!(
        "Local\\{}-w2-workspace-{sid}-{label}",
        crate::APP_IDENTIFIER
    )))?;
    // Presence, rather than thread ownership, is the reservation. No release on
    // a different thread is required when the state moves into the desktop host.
    let raw = unsafe { CreateMutexExW(&attributes, name.as_ptr(), 0, MUTEX_ALL_ACCESS) };
    let existing = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if raw.is_null() {
        return Err(os_error("WORKSPACE_INSTANCE_LOCK_FAILED"));
    }
    let handle = unsafe { Handle::own(raw) };
    if existing {
        return Err(error(
            "WORKSPACE_INSTANCE_LOCKED",
            "另一个 W2.1 宿主已经持有此工作区。",
        ));
    }
    Ok(handle)
}

struct Context {
    path: PathBuf,
    sid: String,
    fresh: bool,
    ancestors: Chain,
    _reservation: Handle,
    operation: Mutex<()>,
}
impl Context {
    fn verify(&self) -> Result<()> {
        if drive_mapping(self.ancestors.drive)? != self.ancestors.mapping {
            return Err(error(
                "WORKSPACE_VOLUME_ALIAS_CHANGED",
                "盘符映射改变；当前工作区已阻断。",
            ));
        }
        for object in &self.ancestors.objects {
            object.verify()?;
        }
        owner_and_acl(
            self.ancestors.objects.last().unwrap().raw(),
            &self.sid,
            false,
        )
    }
}

/// A lifetime reservation plus a handle-rooted directory tree. All ancestry is
/// opened without delete sharing; children are published using relative handles.
/// Existing contents are candidates, never implicitly trusted or repaired.
pub struct WindowsWorkspace {
    root: WorkspaceDirectory,
}
#[derive(Clone)]
pub struct WorkspaceDirectory {
    context: Arc<Context>,
    lineage: Vec<Arc<Object>>,
    path: PathBuf,
}
#[derive(Clone, Debug)]
pub struct WorkspaceFile {
    pub bytes: Vec<u8>,
    pub identity: WindowsObjectIdentity,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceEntry {
    pub name: String,
    pub is_directory: bool,
}
/// Cross-session authority. The private ordinary lock file is never removed;
/// its exclusive Windows handle, rather than its presence, denotes ownership.
pub struct WorkspaceLock {
    _directory: WorkspaceDirectory,
    _file: Arc<Object>,
}

impl WindowsWorkspace {
    pub fn create_fixed() -> Result<Self> {
        Self::at(known_folder()?, true, "fixed")
    }
    /// Read-only candidate opening. The caller must validate marker, settings,
    /// complete expected contents and registration before using any project.
    pub fn open_fixed() -> Result<Self> {
        Self::at(known_folder()?, false, "fixed")
    }
    fn at(parent: PathBuf, create: bool, label: &str) -> Result<Self> {
        let sid = user_sid()?;
        let reservation = reservation(&sid, label)?;
        let ancestors = open_parent(&parent)?;
        owner_and_acl(ancestors.objects.last().unwrap().raw(), &sid, false)?;
        let security = descriptor(&sid, true)?;
        let root = relative(
            ancestors.objects.last().unwrap().raw(),
            crate::APP_IDENTIFIER,
            true,
            create,
            DIRECTORY_ACCESS | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY,
            DIRECTORY_SHARING,
            if create { Some(&security) } else { None },
        )
        .map_err(|failure| {
            if !create && failure.code == "WORKSPACE_NAME_MISSING" {
                error("FIXED_ROOT_MISSING", "固定业务根尚不存在；没有创建目录。")
            } else {
                failure
            }
        })?;
        owner_and_acl(root.raw(), &sid, true)?;
        if root.identity.volume_serial != ancestors.objects[0].identity.volume_serial {
            return Err(error(
                "WORKSPACE_VOLUME_CHANGED",
                "业务根不在已核验的卷上。",
            ));
        }
        let path = parent.join(crate::APP_IDENTIFIER);
        let context = Arc::new(Context {
            path: path.clone(),
            sid,
            fresh: create,
            ancestors,
            _reservation: reservation,
            operation: Mutex::new(()),
        });
        let root = WorkspaceDirectory {
            context,
            lineage: vec![root],
            path,
        };
        root.verify_binding()?;
        Ok(Self { root })
    }
    pub fn root(&self) -> WorkspaceDirectory {
        self.root.clone()
    }
    pub fn path(&self) -> &Path {
        self.root.path()
    }
    pub fn identity(&self) -> &WindowsObjectIdentity {
        self.root.identity()
    }
    /// Stable identities of the fixed path's ancestors, ordered from the volume
    /// root through LocalAppData. Persisting and comparing these is the caller's
    /// responsibility when admitting a completed sample after restart.
    pub fn ancestor_identities(&self) -> Vec<WindowsObjectIdentity> {
        self.root
            .context
            .ancestors
            .objects
            .iter()
            .map(|object| object.identity.clone())
            .collect()
    }
    pub fn verify_binding(&self) -> Result<()> {
        self.root.verify_binding()
    }
    #[cfg(test)]
    pub(crate) fn for_test_path(parent: PathBuf, create: bool) -> Result<Self> {
        let label = parent
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| error("WORKSPACE_TEST_LABEL_INVALID", "夹具标识无效。"))?
            .to_owned();
        Self::at(parent, create, &label)
    }
}

impl WorkspaceDirectory {
    fn object(&self) -> &Object {
        self.lineage.last().unwrap()
    }
    fn verify(&self) -> Result<()> {
        self.context.verify()?;
        for object in &self.lineage {
            object.verify()?;
            owner_and_acl(object.raw(), &self.context.sid, true)?;
            if object.identity.volume_serial != self.lineage[0].identity.volume_serial {
                return Err(error(
                    "WORKSPACE_VOLUME_CHANGED",
                    "工作区目录跨越了其他卷。",
                ));
            }
        }
        Ok(())
    }
    pub fn verify_binding(&self) -> Result<()> {
        let _operation = self
            .context
            .operation
            .lock()
            .map_err(|_| error("WORKSPACE_LOCK_POISONED", "工作区上次操作异常终止。"))?;
        self.verify()
    }
    pub fn identity(&self) -> &WindowsObjectIdentity {
        &self.object().identity
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn root_path(&self) -> &Path {
        &self.context.path
    }
    pub fn lock_file(&self, name: &str) -> Result<WorkspaceLock> {
        let _operation = self
            .context
            .operation
            .lock()
            .map_err(|_| error("WORKSPACE_LOCK_POISONED", "工作区上次操作异常终止。"))?;
        self.verify()?;
        let security = descriptor(&self.context.sid, false)?;
        let access =
            FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE;
        let file = match relative(
            self.object().raw(),
            name,
            false,
            self.context.fresh,
            access,
            0,
            if self.context.fresh {
                Some(&security)
            } else {
                None
            },
        ) {
            Ok(file) => file,
            Err(failure) if failure.code == "WORKSPACE_ALREADY_EXISTS" => {
                relative(self.object().raw(), name, false, false, access, 0, None)?
            }
            Err(failure) => return Err(failure),
        };
        owner_and_acl(file.raw(), &self.context.sid, true)?;
        if file.verify()?.EndOfFile != 0 {
            return Err(error(
                "WORKSPACE_LOCK_CONTENT_REJECTED",
                "实例锁不是预期的空文件；原内容已保留。",
            ));
        }
        self.verify()?;
        Ok(WorkspaceLock {
            _directory: self.clone(),
            _file: file,
        })
    }
    fn directory(&self, name: &str, create: bool) -> Result<Self> {
        let _operation = self
            .context
            .operation
            .lock()
            .map_err(|_| error("WORKSPACE_LOCK_POISONED", "工作区上次操作异常终止。"))?;
        self.verify()?;
        let security = descriptor(&self.context.sid, true)?;
        let next = relative(
            self.object().raw(),
            name,
            true,
            create,
            DIRECTORY_ACCESS | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY,
            DIRECTORY_SHARING,
            if create { Some(&security) } else { None },
        )?;
        owner_and_acl(next.raw(), &self.context.sid, true)?;
        let mut lineage = self.lineage.clone();
        lineage.push(next);
        let result = Self {
            context: self.context.clone(),
            lineage,
            path: self.path.join(name),
        };
        result.verify()?;
        Ok(result)
    }
    pub fn create_directory(&self, name: &str) -> Result<Self> {
        self.directory(name, true)
    }
    pub fn open_directory(&self, name: &str) -> Result<Self> {
        self.directory(name, false)
    }

    /// Create a new, private ordinary file, write and sync it. Failure retains
    /// partial evidence. This API never truncates, overwrites or deletes a file.
    pub fn write_new(&self, name: &str, bytes: &[u8]) -> Result<WindowsObjectIdentity> {
        if bytes.len() > MAX_BYTES {
            return Err(error(
                "WORKSPACE_FILE_TOO_LARGE",
                "文件超过 W2.1 写入上限。",
            ));
        }
        let _operation = self
            .context
            .operation
            .lock()
            .map_err(|_| error("WORKSPACE_LOCK_POISONED", "工作区上次操作异常终止。"))?;
        self.verify()?;
        let security = descriptor(&self.context.sid, false)?;
        let file = relative(
            self.object().raw(),
            name,
            false,
            true,
            FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
            FILE_SHARE_READ,
            Some(&security),
        )?;
        owner_and_acl(file.raw(), &self.context.sid, true)?;
        let duplicate = file
            .handle
            .0
            .try_clone()
            .map_err(|err| error("WORKSPACE_FILE_HANDLE_FAILED", err.to_string()))?;
        let mut writer = File::from(duplicate);
        writer
            .write_all(bytes)
            .and_then(|_| writer.flush())
            .and_then(|_| writer.sync_all())
            .map_err(|err| error("WORKSPACE_WRITE_FAILED", err.to_string()))?;
        let metadata = file.verify()?;
        if metadata.EndOfFile != bytes.len() as i64 {
            return Err(error(
                "WORKSPACE_WRITE_INCOMPLETE",
                "写入长度与预期不符；文件已保留。",
            ));
        }
        self.verify()?;
        Ok(file.identity.clone())
    }
    pub fn read_bounded(&self, name: &str, limit: usize) -> Result<WorkspaceFile> {
        if limit > MAX_BYTES {
            return Err(error(
                "WORKSPACE_READ_LIMIT_REJECTED",
                "读取上限超过 W2.1 范围。",
            ));
        }
        let _operation = self
            .context
            .operation
            .lock()
            .map_err(|_| error("WORKSPACE_LOCK_POISONED", "工作区上次操作异常终止。"))?;
        self.verify()?;
        let file = relative(
            self.object().raw(),
            name,
            false,
            false,
            FILE_READ_DATA | FILE_READ_ATTRIBUTES | READ_CONTROL | SYNCHRONIZE,
            FILE_SHARE_READ,
            None,
        )?;
        owner_and_acl(file.raw(), &self.context.sid, true)?;
        let before = file.verify()?;
        if before.EndOfFile < 0 || before.EndOfFile as u64 > limit as u64 {
            return Err(error("WORKSPACE_FILE_TOO_LARGE", "文件超过有界读取上限。"));
        }
        let duplicate = file
            .handle
            .0
            .try_clone()
            .map_err(|err| error("WORKSPACE_FILE_HANDLE_FAILED", err.to_string()))?;
        let reader = File::from(duplicate);
        let mut bytes = Vec::new();
        reader
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| error("WORKSPACE_READ_FAILED", err.to_string()))?;
        if bytes.len() > limit
            || file.verify()?.EndOfFile != before.EndOfFile
            || bytes.len() as i64 != before.EndOfFile
        {
            return Err(error("WORKSPACE_FILE_CHANGED", "读取过程中对象长度变化。"));
        }
        self.verify()?;
        Ok(WorkspaceFile {
            bytes,
            identity: file.identity.clone(),
        })
    }
    /// Publish the previously captured source object under an absent name.
    /// Callers first drop child handles, then supply the complete identity from
    /// preparation. A replacement source is retained without being renamed.
    /// No replacement flag, fallback copy, cleanup or target adoption is used.
    pub fn publish_noreplace_checked(
        &self,
        source: &str,
        target: &str,
        is_directory: bool,
        expected_source_identity: &WindowsObjectIdentity,
    ) -> Result<()> {
        if source.eq_ignore_ascii_case(target) {
            return Err(error(
                "WORKSPACE_PUBLICATION_NAME_REJECTED",
                "暂存名与发布名必须不同。",
            ));
        }
        let target_name = target;
        let target = component(target)?;
        component(source)?;
        let _operation = self
            .context
            .operation
            .lock()
            .map_err(|_| error("WORKSPACE_LOCK_POISONED", "工作区上次操作异常终止。"))?;
        self.verify()?;
        let source = relative(
            self.object().raw(),
            source,
            is_directory,
            false,
            DELETE
                | READ_CONTROL
                | FILE_READ_ATTRIBUTES
                | SYNCHRONIZE
                | if is_directory {
                    FILE_LIST_DIRECTORY
                } else {
                    FILE_READ_DATA
                },
            DIRECTORY_SHARING,
            None,
        )?;
        owner_and_acl(source.raw(), &self.context.sid, true)?;
        if &source.identity != expected_source_identity {
            return Err(error(
                "WORKSPACE_PUBLICATION_SOURCE_CHANGED",
                "暂存源与准备阶段的完整 Windows 身份不符；没有移动替代对象或创建发布目标。",
            ));
        }
        let length = offset_of!(FILE_RENAME_INFORMATION, FileName) + target.len() * 2;
        let mut storage = vec![
            0usize;
            length
                .max(size_of::<FILE_RENAME_INFORMATION>())
                .div_ceil(size_of::<usize>())
        ];
        let value = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        unsafe {
            (*value).Anonymous.ReplaceIfExists = false;
            (*value).RootDirectory = self.object().raw();
            (*value).FileNameLength = (target.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(
                target.as_ptr(),
                storage
                    .as_mut_ptr()
                    .cast::<u8>()
                    .add(offset_of!(FILE_RENAME_INFORMATION, FileName))
                    .cast::<u16>(),
                target.len(),
            );
        }
        let mut io = IO_STATUS_BLOCK::default();
        nt_result(
            unsafe {
                NtSetInformationFile(
                    source.raw(),
                    &mut io,
                    storage.as_ptr().cast(),
                    length as u32,
                    FileRenameInformation,
                )
            },
            "WORKSPACE_PUBLICATION_FAILED",
        )?;
        source.verify()?;
        verify_leaf_name(source.raw(), target_name)?;
        self.verify()?;
        Ok(())
    }
    pub fn list(&self) -> Result<Vec<WorkspaceEntry>> {
        let _operation = self
            .context
            .operation
            .lock()
            .map_err(|_| error("WORKSPACE_LOCK_POISONED", "工作区上次操作异常终止。"))?;
        self.verify()?;
        let mut entries = Vec::new();
        let mut names = HashSet::new();
        let mut restart = true;
        loop {
            let mut buffer = vec![0u64; 65536 / 8];
            let class = if restart {
                FileIdBothDirectoryRestartInfo
            } else {
                FileIdBothDirectoryInfo
            };
            restart = false;
            if unsafe {
                GetFileInformationByHandleEx(
                    self.object().raw(),
                    class,
                    buffer.as_mut_ptr().cast(),
                    65536,
                )
            } == 0
            {
                if unsafe { GetLastError() } == ERROR_NO_MORE_FILES {
                    break;
                }
                return Err(os_error("WORKSPACE_ENUMERATION_FAILED"));
            }
            let mut offset = 0usize;
            loop {
                let name_offset = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
                if offset + size_of::<FILE_ID_BOTH_DIR_INFO>() > 65536 {
                    return Err(error("WORKSPACE_ENUMERATION_INVALID", "目录数据不完整。"));
                }
                let item = unsafe {
                    &*buffer
                        .as_ptr()
                        .cast::<u8>()
                        .add(offset)
                        .cast::<FILE_ID_BOTH_DIR_INFO>()
                };
                let length = item.FileNameLength as usize;
                if length == 0
                    || length > 510
                    || !length.is_multiple_of(2)
                    || offset + name_offset + length > 65536
                {
                    return Err(error(
                        "WORKSPACE_ENUMERATION_INVALID",
                        "目录名称超过格式上限。",
                    ));
                }
                let units = unsafe {
                    std::slice::from_raw_parts(
                        buffer
                            .as_ptr()
                            .cast::<u8>()
                            .add(offset + name_offset)
                            .cast::<u16>(),
                        length / 2,
                    )
                };
                let name = String::from_utf16(units).map_err(|_| {
                    error("WORKSPACE_COMPONENT_REJECTED", "目录名不是有效 Unicode。")
                })?;
                if name != "." && name != ".." {
                    component(&name)?;
                    if entries.len() == MAX_ENTRIES {
                        return Err(error(
                            "WORKSPACE_ENUMERATION_LIMIT",
                            "目录数量超过 W2.1 上限。",
                        ));
                    }
                    if !names.insert(name.to_lowercase()) {
                        return Err(error(
                            "WORKSPACE_NAME_COLLISION",
                            "目录含有大小写冲突名称。",
                        ));
                    }
                    if item.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                        return Err(error(
                            "WORKSPACE_REPARSE_REJECTED",
                            "目录含有重解析点；未跟随。",
                        ));
                    }
                    entries.push(WorkspaceEntry {
                        name,
                        is_directory: item.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
                    });
                }
                if item.NextEntryOffset == 0 {
                    break;
                }
                let next = item.NextEntryOffset as usize;
                if next < name_offset + length || !next.is_multiple_of(8) || offset + next >= 65536
                {
                    return Err(error("WORKSPACE_ENUMERATION_INVALID", "目录偏移无效。"));
                }
                offset += next;
            }
        }
        self.verify()?;
        Ok(entries)
    }
}

#[cfg(test)]
#[path = "windows_workspace_tests.rs"]
mod tests;
