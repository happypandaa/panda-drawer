//! Windows metadata-only inspection. Never initializes the application root,
//! reads record contents, follows settings, or opens project databases/tokens.
use crate::{EvidenceCount, HostCheck, ProbeReport, APP_IDENTIFIER};
use std::{
    ffi::{c_void, OsString},
    mem::{offset_of, size_of},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{
        LocalFree, ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, ERROR_PATH_NOT_FOUND, HANDLE,
        INVALID_HANDLE_VALUE,
    },
    Security::{
        AclSizeInformation,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SE_FILE_OBJECT,
        },
        GetAce, GetAclInformation, GetTokenInformation, IsValidAcl, IsValidSecurityDescriptor,
        IsValidSid, TokenUser, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL_SIZE_INFORMATION,
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSID, SECURITY_ATTRIBUTES,
        TOKEN_QUERY, TOKEN_USER,
    },
    Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, FileAttributeTagInfo, FileIdBothDirectoryInfo,
        FileIdBothDirectoryRestartInfo, FileIdInfo, FileStandardInfo, GetDriveTypeW,
        GetFileInformationByHandle, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
        GetVolumeInformationByHandleW, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_BOTH_DIR_INFO, FILE_ID_INFO, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_READ, FILE_STANDARD_INFO, OPEN_EXISTING,
        READ_CONTROL, VOLUME_NAME_GUID,
    },
    System::{
        Com::{CoCreateGuid, CoTaskMemFree},
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
    UI::Shell::{FOLDERID_LocalAppData, SHGetKnownFolderPath},
};

const ENTRY_LIMIT: usize = 256;
const METADATA_LIMIT: i64 = 1024 * 1024;
const ENUM_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug)]
struct ProbeError {
    code: &'static str,
    message: String,
    missing: bool,
}
type Result<T> = std::result::Result<T, ProbeError>;

fn blocked(code: &'static str, message: &str) -> ProbeError {
    ProbeError {
        code,
        message: message.into(),
        missing: false,
    }
}
fn last_error(code: &'static str) -> ProbeError {
    let number = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
    ProbeError {
        code,
        message: format!("Windows 检查未完成，系统错误 {number}。原目录保持不变。"),
        missing: code == "OBJECT_OPEN_DENIED"
            && matches!(number, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND),
    }
}

struct Handle(OwnedHandle);
impl Handle {
    fn new(raw: HANDLE) -> Self {
        // Both callers just received a successful owning Win32 handle.
        Self(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
    fn raw(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
}
struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObjectIdentity {
    volume: u64,
    file: [u8; 16],
}
struct Opened {
    handle: Handle,
    identity: ObjectIdentity,
    index: u64,
    size: i64,
}

fn wide(path: &Path) -> Result<Vec<u16>> {
    let text = path.as_os_str();
    let mut value: Vec<u16> = text.encode_wide().collect();
    if value.contains(&0) {
        return Err(blocked("INVALID_PATH", "路径含有不支持的字符。"));
    }
    // The paths admitted by open_chain are drive-rooted, never UNC/devices.
    if !value.starts_with(&[92, 92, 63, 92]) {
        let mut prefix: Vec<u16> = r"\\?\".encode_utf16().collect();
        prefix.extend(value);
        value = prefix;
    }
    value.push(0);
    Ok(value)
}

fn information<T: Default>(handle: HANDLE, class: i32) -> Result<T> {
    let mut value = T::default();
    if unsafe {
        GetFileInformationByHandleEx(
            handle,
            class,
            (&mut value as *mut T).cast(),
            size_of::<T>() as u32,
        )
    } == 0
    {
        return Err(last_error("OBJECT_METADATA_UNAVAILABLE"));
    }
    Ok(value)
}

fn open_object(path: &Path, directory: bool) -> Result<Opened> {
    let name = wide(path)?;
    // Sharing restrictions must be backed by a real read/list access request;
    // metadata-only access did not enforce them in native Windows tests.
    // FILE_READ_DATA is never followed by ReadFile or any content read.
    let access = FILE_READ_ATTRIBUTES
        | READ_CONTROL
        | if directory {
            FILE_LIST_DIRECTORY
        } else {
            FILE_READ_DATA
        };
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            access,
            // Only held for the bounded probe or cache creation, never for a
            // running WebView: this sharing mode also blocks child renames.
            FILE_SHARE_READ,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(last_error("OBJECT_OPEN_DENIED"));
    }
    let handle = Handle::new(raw);
    let tag: FILE_ATTRIBUTE_TAG_INFO = information(raw, FileAttributeTagInfo)?;
    if tag.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(blocked(
            "REPARSE_POINT_REJECTED",
            "检查路径含有重解析点，未跟随或修改。",
        ));
    }
    let standard: FILE_STANDARD_INFO = information(raw, FileStandardInfo)?;
    if standard.Directory != directory || standard.DeletePending {
        return Err(blocked(
            "OBJECT_TYPE_REJECTED",
            "检查对象类型不符或正在删除。",
        ));
    }
    if !directory && standard.NumberOfLinks != 1 {
        return Err(blocked(
            "HARD_LINK_REJECTED",
            "控制文件含有硬链接，未读取其内容。",
        ));
    }
    let id: FILE_ID_INFO = information(raw, FileIdInfo)?;
    let mut basic = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(raw, &mut basic) } == 0 {
        return Err(last_error("OBJECT_IDENTITY_UNAVAILABLE"));
    }
    Ok(Opened {
        handle,
        identity: ObjectIdentity {
            volume: id.VolumeSerialNumber,
            file: id.FileId.Identifier,
        },
        index: ((basic.nFileIndexHigh as u64) << 32) | basic.nFileIndexLow as u64,
        size: standard.EndOfFile,
    })
}

fn component_ok(name: &std::ffi::OsStr) -> bool {
    let units: Vec<u16> = name.encode_wide().collect();
    !units.is_empty()
        && !matches!(units.last(), Some(32 | 46))
        && !units
            .iter()
            .any(|unit| *unit < 32 || matches!(*unit, 34 | 42 | 47 | 58 | 60 | 62 | 63 | 92 | 124))
}

/// Keep every ancestor open without write/delete sharing until inspection ends.
/// Metadata is queried on each opened object, not inferred from canonical text.
fn open_chain(path: &Path) -> Result<Vec<Opened>> {
    let mut parts = path.components();
    let Some(Component::Prefix(prefix)) = parts.next() else {
        return Err(blocked(
            "PATH_NAMESPACE_REJECTED",
            "只检查本机盘符下的固定目录。",
        ));
    };
    let drive = match prefix.kind() {
        Prefix::Disk(drive) | Prefix::VerbatimDisk(drive) => drive,
        _ => {
            return Err(blocked(
                "PATH_NAMESPACE_REJECTED",
                "网络、设备和别名路径不在本次检查范围。",
            ))
        }
    };
    if !matches!(parts.next(), Some(Component::RootDir)) {
        return Err(blocked(
            "PATH_NAMESPACE_REJECTED",
            "检查路径必须从盘符根目录开始。",
        ));
    }
    let mut current = PathBuf::from(format!("{}:\\", drive as char));
    let root_wide: Vec<u16> = current.as_os_str().encode_wide().chain(Some(0)).collect();
    // DRIVE_FIXED is 3. This establishes a fixed local volume, not a physical
    // internal-disk claim (USB enclosures can also report a fixed volume).
    if unsafe { GetDriveTypeW(root_wide.as_ptr()) } != 3 {
        return Err(blocked(
            "VOLUME_NOT_FIXED_LOCAL",
            "当前卷不是本次支持的固定本地卷。",
        ));
    }
    let first = open_object(&current, true)?;
    let mut filesystem = [0u16; 32];
    if unsafe {
        GetVolumeInformationByHandleW(
            first.handle.raw(),
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
        return Err(last_error("FILESYSTEM_UNAVAILABLE"));
    }
    let end = filesystem
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(filesystem.len());
    if String::from_utf16_lossy(&filesystem[..end]) != "NTFS" {
        return Err(blocked(
            "FILESYSTEM_UNSUPPORTED",
            "本次只检查 NTFS 固定本地卷。",
        ));
    }
    let mut handles = vec![first];
    for part in parts {
        let Component::Normal(name) = part else {
            return Err(blocked(
                "PATH_COMPONENT_REJECTED",
                "检查路径含有不支持的相对片段。",
            ));
        };
        if !component_ok(name) {
            return Err(blocked(
                "PATH_COMPONENT_REJECTED",
                "检查路径含有不支持的片段。",
            ));
        }
        current.push(name);
        let next = open_object(&current, true)?;
        if next.identity.volume != handles[0].identity.volume {
            return Err(blocked("VOLUME_CHANGED", "路径跨越了未核验的卷。"));
        }
        handles.push(next);
    }
    Ok(handles)
}

fn sid_text(sid: PSID) -> Result<String> {
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        return Err(blocked("ACL_INVALID", "权限信息无法识别。"));
    }
    let mut text = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 {
        return Err(last_error("ACL_UNAVAILABLE"));
    }
    let allocation = LocalAllocation(text.cast());
    let value = unsafe { terminated_text(text, 256)? };
    drop(allocation);
    Ok(value)
}

unsafe fn terminated_text(pointer: *const u16, maximum: usize) -> Result<String> {
    if pointer.is_null() {
        return Err(blocked("SYSTEM_PATH_UNAVAILABLE", "系统返回了空文本。"));
    }
    let mut length = 0;
    while length < maximum && unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    if length == maximum {
        return Err(blocked(
            "SYSTEM_PATH_TOO_LONG",
            "系统文本超过本次检查限制。",
        ));
    }
    Ok(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(pointer, length)
    }))
}

fn current_user_sid() -> Result<String> {
    let mut raw = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
        return Err(last_error("USER_IDENTITY_UNAVAILABLE"));
    }
    let token = Handle::new(raw);
    let mut needed = 0;
    unsafe {
        GetTokenInformation(token.raw(), TokenUser, null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 65536 {
        return Err(blocked(
            "USER_IDENTITY_UNAVAILABLE",
            "无法取得当前用户身份。",
        ));
    }
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
    if unsafe {
        GetTokenInformation(
            token.raw(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(last_error("USER_IDENTITY_UNAVAILABLE"));
    }
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    sid_text(user.User.Sid)
}

fn private_acl(object: &Opened, current_user: &str) -> Result<()> {
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let mut descriptor = null_mut();
    let error = unsafe {
        GetSecurityInfo(
            object.handle.raw(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    let allocation = LocalAllocation(descriptor);
    if error != 0
        || descriptor.is_null()
        || unsafe { IsValidSecurityDescriptor(descriptor) } == 0
        || dacl.is_null()
        || unsafe { IsValidAcl(dacl) } == 0
    {
        return Err(blocked(
            "PRIVATE_ACL_UNVERIFIED",
            "私有权限无法确认；没有修改现有 ACL。",
        ));
    }
    if sid_text(owner)? != current_user {
        return Err(blocked(
            "PRIVATE_OWNER_REJECTED",
            "检查对象不属于当前用户；没有修改所有者或 ACL。",
        ));
    }
    let mut info = ACL_SIZE_INFORMATION::default();
    if unsafe {
        GetAclInformation(
            dacl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        return Err(last_error("ACL_UNAVAILABLE"));
    }
    if info.AceCount > 256 {
        return Err(blocked("ACL_UNSUPPORTED", "ACL 超过本次支持的检查范围。"));
    }
    for index in 0..info.AceCount {
        let mut ace = null_mut();
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            return Err(last_error("ACL_UNAVAILABLE"));
        }
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        // Only ordinary allow/deny ACEs are understood; callback/object and
        // other policy forms fail closed instead of silently granting trust.
        if header.AceType == 1 {
            continue;
        }
        let sid_offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        if header.AceType != 0 || (header.AceSize as usize) < sid_offset + 8 {
            return Err(blocked("ACL_UNSUPPORTED", "ACL 含有本阶段不支持的权限项。"));
        }
        let sid = unsafe { ace.cast::<u8>().add(sid_offset) };
        let count = unsafe { *sid.add(1) } as usize;
        if sid_offset + 8 + count * 4 > header.AceSize as usize {
            return Err(blocked("ACL_INVALID", "ACL 权限项结构无法识别。"));
        }
        let principal = sid_text(sid.cast())?;
        if principal != current_user && principal != "S-1-5-18" && principal != "S-1-5-32-544" {
            return Err(blocked(
                "PRIVATE_ACL_REJECTED",
                "现有 ACL 向其他身份授权；本次只拒绝，不修复权限。",
            ));
        }
    }
    drop(allocation);
    Ok(())
}

fn diagnostic_object_path(object: &Opened) -> String {
    let mut buffer = vec![0u16; 32768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            object.handle.raw(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            VOLUME_NAME_GUID,
        )
    };
    if length == 0 {
        return format!("<unavailable: {}>", std::io::Error::last_os_error());
    }
    if length >= buffer.len() as u32 {
        return "<unavailable: path exceeds diagnostic limit>".into();
    }
    String::from_utf16_lossy(&buffer[..length as usize])
}

fn system_parent_security(object: &Opened, current_user: &str) -> Result<()> {
    crate::windows_parent_security::verify_system_parent(object.handle.raw(), current_user)
        .map(|verified| {
            let _owner = verified.owner_sid;
        })
        .map_err(|failure| {
            let code = if failure.code == "SYSTEM_PARENT_OWNER_REJECTED" {
                "CACHE_PARENT_OWNER_REJECTED"
            } else {
                failure.code
            };
            blocked(
                code,
                &format!(
                    "{}; actualObject={}",
                    failure.message,
                    diagnostic_object_path(object)
                ),
            )
        })
}

fn known_local_app_data() -> Result<PathBuf> {
    let mut pointer = null_mut();
    let result =
        unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, 0, null_mut(), &mut pointer) };
    if result < 0 {
        return Err(blocked(
            "KNOWN_FOLDER_UNAVAILABLE",
            "无法通过系统 KnownFolder 定位本地应用数据目录。",
        ));
    }
    let text = unsafe { terminated_text(pointer, 32768) };
    unsafe {
        CoTaskMemFree(pointer.cast());
    }
    text.map(PathBuf::from)
}

struct Entry {
    name: OsString,
    directory: bool,
    index: u64,
    reparse: bool,
}
fn shallow_entries(directory: &Opened) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut restart = true;
    loop {
        let mut buffer = vec![0u64; ENUM_BUFFER_BYTES / 8];
        let class = if restart {
            FileIdBothDirectoryRestartInfo
        } else {
            FileIdBothDirectoryInfo
        };
        restart = false;
        if unsafe {
            GetFileInformationByHandleEx(
                directory.handle.raw(),
                class,
                buffer.as_mut_ptr().cast(),
                ENUM_BUFFER_BYTES as u32,
            )
        } == 0
        {
            let error = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
            if error == ERROR_NO_MORE_FILES {
                break;
            }
            return Err(last_error("SHALLOW_ENUMERATION_UNAVAILABLE"));
        }
        let mut offset = 0;
        loop {
            let name_offset = offset_of!(FILE_ID_BOTH_DIR_INFO, FileName);
            if offset + size_of::<FILE_ID_BOTH_DIR_INFO>() > ENUM_BUFFER_BYTES {
                return Err(blocked("ENUMERATION_INVALID", "目录摘要无法完整识别。"));
            }
            let entry = unsafe {
                &*buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_ID_BOTH_DIR_INFO>()
            };
            let bytes = entry.FileNameLength as usize;
            if bytes == 0
                || !bytes.is_multiple_of(2)
                || bytes > 510
                || offset + name_offset + bytes > ENUM_BUFFER_BYTES
            {
                return Err(blocked("ENUMERATION_INVALID", "目录名超过本次检查范围。"));
            }
            let units = unsafe {
                std::slice::from_raw_parts(
                    (entry as *const FILE_ID_BOTH_DIR_INFO)
                        .cast::<u8>()
                        .add(name_offset)
                        .cast::<u16>(),
                    bytes / 2,
                )
            };
            let name = OsString::from_wide(units);
            if name != "." && name != ".." {
                if entries.len() == ENTRY_LIMIT {
                    return Err(blocked(
                        "ENUMERATION_LIMIT",
                        "目录项目超过浅层检查上限；未继续遍历。",
                    ));
                }
                entries.push(Entry {
                    name,
                    directory: entry.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
                    index: entry.FileId as u64,
                    reparse: entry.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
                });
            }
            let next = entry.NextEntryOffset as usize;
            if next == 0 {
                break;
            }
            if next < name_offset + bytes
                || !next.is_multiple_of(8)
                || offset + next >= ENUM_BUFFER_BYTES
            {
                return Err(blocked("ENUMERATION_INVALID", "目录摘要偏移无法识别。"));
            }
            offset += next;
        }
    }
    Ok(entries)
}

fn check(code: &str, status: &str, message: &str) -> HostCheck {
    HostCheck {
        code: code.into(),
        status: status.into(),
        message: message.into(),
    }
}

fn inspect_existing(path: &Path, current_user: &str) -> Result<Vec<EvidenceCount>> {
    let chain = open_chain(path)?;
    let root = chain
        .last()
        .ok_or_else(|| blocked("ROOT_UNAVAILABLE", "固定根不可用。"))?;
    private_acl(root, current_user)?;
    let mut evidence = Vec::new();
    let mut unknown = 0;
    let mut known = std::collections::BTreeSet::new();
    for entry in shallow_entries(root)? {
        if entry.reparse {
            return Err(blocked(
                "EVIDENCE_TYPE_REJECTED",
                "顶层存在重解析点；未跟随或读取内容。",
            ));
        }
        let normalized = entry.name.to_str().map(str::to_ascii_lowercase);
        let kind = match normalized.as_deref() {
            Some(".environment.json") => Some(("environmentMarker", false, true)),
            Some("settings.json") => Some(("settings", false, true)),
            Some("migrations") => Some(("migrationEntries", true, true)),
            Some("cleanup") => Some(("cleanupEntries", true, true)),
            Some("workspace") => Some(("workspaceEntries", true, true)),
            Some("runtime") => Some(("runtimeDirectory", true, false)),
            _ => None,
        };
        let Some((kind, is_directory, enumerate)) = kind else {
            unknown += 1;
            continue;
        };
        if !known.insert(kind) {
            return Err(blocked(
                "EVIDENCE_NAME_COLLISION",
                "已知证据名称存在大小写冲突，检查已停止。",
            ));
        }
        if entry.reparse || entry.directory != is_directory {
            return Err(blocked(
                "EVIDENCE_TYPE_REJECTED",
                "已知顶层证据为重解析点或类型不符；未跟随。",
            ));
        }
        let object = open_object(&path.join(&entry.name), is_directory).map_err(|mut error| {
            // A vanished child does not mean that the fixed root is absent.
            error.missing = false;
            error
        })?;
        if object.index != entry.index || object.identity.volume != root.identity.volume {
            return Err(blocked(
                "OBJECT_CHANGED",
                "目录枚举与句柄身份不一致，检查已停止。",
            ));
        }
        private_acl(&object, current_user)?;
        if !is_directory && (object.size < 0 || object.size > METADATA_LIMIT) {
            return Err(blocked(
                "METADATA_LIMIT",
                "控制文件超过本阶段大小限制；未读取内容。",
            ));
        }
        let count = if is_directory && enumerate {
            shallow_entries(&object)?.len() as u32
        } else {
            1
        };
        evidence.push(EvidenceCount {
            kind: kind.into(),
            count,
        });
    }
    let now: FILE_ID_INFO = information(root.handle.raw(), FileIdInfo)?;
    if root.identity
        != (ObjectIdentity {
            volume: now.VolumeSerialNumber,
            file: now.FileId.Identifier,
        })
    {
        return Err(blocked(
            "OBJECT_CHANGED",
            "固定根身份发生变化，检查已停止。",
        ));
    }
    evidence.push(EvidenceCount {
        kind: "unknownTopLevelEntries".into(),
        count: unknown,
    });
    Ok(evidence)
}

pub(super) fn inspect() -> ProbeReport {
    let mut report = ProbeReport {
        data_root: None,
        status: "unavailable".into(),
        checks: vec![],
        evidence: vec![],
    };
    let outcome: Result<()> = (|| {
        let path = known_local_app_data()?.join(APP_IDENTIFIER);
        report.data_root = Some(path.to_string_lossy().into_owned());
        let user = current_user_sid()?;
        report.evidence = inspect_existing(&path, &user)?;
        Ok(())
    })();
    match outcome {
        Ok(()) => {
            report.status = "blocked".into();
            report.checks.push(check(
                "EXISTING_ROOT_METADATA",
                "passed",
                "固定根的有界浅层元数据检查完成；未读取记录内容、令牌、数据库或源码。",
            ));
            report.checks.push(check(
                "RUNTIME_UNAVAILABLE",
                "blocked",
                "旧现场仍被保留；浅层摘要不等于完整恢复检查，本阶段不开放业务运行。",
            ));
        }
        Err(error) if error.missing => {
            report.status = "missing".into();
            report.checks.push(check(
                "FIXED_ROOT_MISSING",
                "passed",
                "固定数据根不存在；没有创建目录、空项目或替代工作区。",
            ));
        }
        Err(error) => {
            report.status = "blocked".into();
            report
                .checks
                .push(check(error.code, "blocked", &error.message));
        }
    }
    report
}

fn new_guid_name() -> Result<String> {
    let mut guid = windows_sys::core::GUID::default();
    if unsafe { CoCreateGuid(&mut guid) } < 0 {
        return Err(blocked("CACHE_ID_UNAVAILABLE", "无法创建浏览器缓存身份。"));
    }
    Ok(format!(
        "local-projects-dev-w1-webview-{:08x}{:04x}{:04x}{}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    ))
}

fn private_directory(path: &Path, sid: &str) -> Result<()> {
    let sddl = format!("O:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
    let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut descriptor = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(last_error("CACHE_ACL_UNAVAILABLE"));
    }
    let allocation = LocalAllocation(descriptor);
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let name = wide(path)?;
    if unsafe { CreateDirectoryW(name.as_ptr(), &security) } == 0 {
        return Err(last_error("CACHE_CREATE_REJECTED"));
    }
    drop(allocation);
    Ok(())
}

/// A browser profile created and checked by this process. The path is private
/// to prevent adopting an arbitrary existing directory. Creation-time checks
/// are not a business-data backend or isolation from another same-user process.
/// No directory locks remain while Chromium performs its normal atomic writes.
pub struct WebviewCache {
    path: PathBuf,
}

impl WebviewCache {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn cache_under(parent: &Path) -> Result<WebviewCache> {
    let user = current_user_sid()?;
    let pins = open_chain(parent)?;
    // LocalAppData is an OS-owned parent for many applications. Its existing
    // DACL need not match a private business-control directory, and is never
    // repaired here. Only the newly created profile gets our protected DACL.
    system_parent_security(
        pins.last()
            .ok_or_else(|| blocked("CACHE_PARENT_UNAVAILABLE", "浏览器缓存父目录不可用。"))?,
        &user,
    )
    .map_err(|mut failure| {
        failure.message = format!("{}; knownFolderPath={}", failure.message, parent.display());
        failure
    })?;
    let path = parent.join(new_guid_name()?);
    // CreateDirectoryW is create-new: an existing name is rejected and is
    // never adopted. This cache is separate from APP_IDENTIFIER's data root.
    private_directory(&path, &user)?;
    let directory = open_object(&path, true)?;
    private_acl(&directory, &user)?;
    if directory.identity.volume != pins[0].identity.volume {
        return Err(blocked(
            "CACHE_VOLUME_CHANGED",
            "浏览器缓存目录的卷身份不一致。",
        ));
    }
    drop(directory);
    drop(pins);
    Ok(WebviewCache { path })
}

pub(super) fn create_webview_cache() -> std::result::Result<WebviewCache, String> {
    (|| cache_under(&known_local_app_data()?))()
        .map_err(|error: ProbeError| format!("{}: {}", error.code, error.message))
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
