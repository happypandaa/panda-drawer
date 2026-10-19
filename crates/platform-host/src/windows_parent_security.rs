//! Existing system-parent admission only. App-owned roots and files retain
//! their stricter current-owner/private-DACL checks in their existing modules.
use std::{
    ffi::c_void,
    mem::{offset_of, size_of},
    ptr::null_mut,
};
use windows_sys::Win32::{
    Foundation::{LocalFree, HANDLE},
    Security::{
        AclSizeInformation,
        Authorization::{ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT},
        GetAce, GetAclInformation, GetLengthSid, IsValidAcl, IsValidSecurityDescriptor, IsValidSid,
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
        OWNER_SECURITY_INFORMATION, PSID,
    },
    Storage::FileSystem::{
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_TRAVERSE, READ_CONTROL,
        SYNCHRONIZE,
    },
};

const SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";
const INHERIT_ONLY: u8 = 0x08;
const READ_ONLY: u32 = FILE_LIST_DIRECTORY
    | FILE_READ_EA
    | FILE_TRAVERSE
    | FILE_READ_ATTRIBUTES
    | READ_CONTROL
    | SYNCHRONIZE
    | 0x8000_0000
    | 0x2000_0000;

#[derive(Debug)]
pub(super) struct ParentSecurityError {
    pub code: &'static str,
    pub message: String,
}
#[derive(Debug)]
pub(super) struct VerifiedSystemParent {
    pub owner_sid: String,
}
type Result<T> = std::result::Result<T, ParentSecurityError>;
fn fail(code: &'static str, message: impl Into<String>) -> ParentSecurityError {
    ParentSecurityError {
        code,
        message: message.into(),
    }
}
fn trusted(sid: &str, current: &str) -> bool {
    sid == current || sid == SYSTEM || sid == ADMINISTRATORS
}
fn owner_rule(owner: &str, current: &str) -> Result<()> {
    if trusted(owner, current) {
        Ok(())
    } else {
        Err(fail(
            "SYSTEM_PARENT_OWNER_REJECTED",
            format!(
                "系统父目录所有者不受信；未修改权限。 expectedSid={current}; observedSid={owner}"
            ),
        ))
    }
}
fn ace_rule(kind: u8, flags: u8, mask: u32, sid: &str, current: &str) -> Result<()> {
    // Unknown ACEs are rejected even if inherit-only. Native parsing validates
    // the complete SID before this pure access-mask decision is reached.
    if !matches!(kind, 0 | 1) || flags & !0x1f != 0 {
        return Err(fail(
            "SYSTEM_PARENT_ACL_UNSUPPORTED",
            "系统父目录含未支持的 ACE 类型或标志。",
        ));
    }
    if flags & INHERIT_ONLY != 0 || kind == 1 || trusted(sid, current) {
        return Ok(());
    }
    if mask & !READ_ONLY != 0 {
        return Err(fail(
            "SYSTEM_PARENT_ACL_REJECTED",
            format!(
            "系统父目录向其他身份授予变更或未识别权限；未修改 ACL。 sid={sid}; mask=0x{mask:08X}"),
        ));
    }
    Ok(())
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
fn sid_text(sid: PSID) -> Result<String> {
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        return Err(fail(
            "SYSTEM_PARENT_ACL_INVALID",
            "系统父目录 SID 无法识别。",
        ));
    }
    let mut pointer = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut pointer) } == 0 {
        return Err(fail(
            "SYSTEM_PARENT_ACL_UNVERIFIED",
            "系统父目录 SID 无法读取。",
        ));
    }
    let allocation = Allocation(pointer.cast());
    let mut length = 0usize;
    while length < 256 && unsafe { *pointer.add(length) } != 0 {
        length += 1;
    }
    if length == 256 {
        return Err(fail("SYSTEM_PARENT_ACL_INVALID", "SID 超过检查上限。"));
    }
    let value = String::from_utf16(unsafe { std::slice::from_raw_parts(pointer, length) })
        .map_err(|_| fail("SYSTEM_PARENT_ACL_INVALID", "SID 文本无法识别。"))?;
    drop(allocation);
    Ok(value)
}

pub(super) fn verify_system_parent(raw: HANDLE, current: &str) -> Result<VerifiedSystemParent> {
    let mut owner = null_mut();
    let mut dacl = null_mut();
    let mut descriptor = null_mut();
    let status = unsafe {
        GetSecurityInfo(
            raw,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    let allocation = Allocation(descriptor);
    if status != 0
        || descriptor.is_null()
        || unsafe { IsValidSecurityDescriptor(descriptor) } == 0
        || dacl.is_null()
        || unsafe { IsValidAcl(dacl) } == 0
    {
        return Err(fail(
            "SYSTEM_PARENT_ACL_UNVERIFIED",
            "系统父目录的所有者或非空 DACL 无法确认；未修复权限。",
        ));
    }
    let owner_sid = sid_text(owner)?;
    owner_rule(&owner_sid, current)?;
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
        return Err(fail(
            "SYSTEM_PARENT_ACL_UNVERIFIED",
            "系统父目录 ACL 大小无法确认。",
        ));
    }
    let base = dacl as usize;
    let limit = base
        .checked_add(info.AclBytesInUse as usize)
        .ok_or_else(|| fail("SYSTEM_PARENT_ACL_INVALID", "ACL 长度溢出。"))?;
    if info.AceCount > 256
        || info.AclBytesInUse < size_of::<windows_sys::Win32::Security::ACL>() as u32
        || info.AclBytesInUse > unsafe { (*dacl).AclSize } as u32
    {
        return Err(fail(
            "SYSTEM_PARENT_ACL_INVALID",
            "系统父目录 ACL 超过检查范围。",
        ));
    }
    for index in 0..info.AceCount {
        let mut ace = null_mut();
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            return Err(fail("SYSTEM_PARENT_ACL_INVALID", "ACE 无法读取。"));
        }
        let address = ace as usize;
        if address < base
            || address
                .checked_add(size_of::<ACE_HEADER>())
                .is_none_or(|end| end > limit)
        {
            return Err(fail("SYSTEM_PARENT_ACL_INVALID", "ACE 头部越界。"));
        }
        let header = unsafe { std::ptr::read_unaligned(ace.cast::<ACE_HEADER>()) };
        let size = header.AceSize as usize;
        let sid_offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        if !matches!(header.AceType, 0 | 1) {
            return Err(fail(
                "SYSTEM_PARENT_ACL_UNSUPPORTED",
                "系统父目录含未支持的 ACE 类型。",
            ));
        }
        if size < sid_offset + 8 || address.checked_add(size).is_none_or(|end| end > limit) {
            return Err(fail(
                "SYSTEM_PARENT_ACL_INVALID",
                "ACE 长度或 SID 头部无效。",
            ));
        }
        let sid = unsafe { ace.cast::<u8>().add(sid_offset) };
        let count = unsafe { *sid.add(1) } as usize;
        let sid_length = 8 + count * 4;
        if count > 15 || sid_offset + sid_length != size {
            return Err(fail("SYSTEM_PARENT_ACL_INVALID", "ACE SID 长度无效。"));
        }
        // IsValidSid/GetLengthSid run only after every claimed byte is bounded.
        if unsafe { IsValidSid(sid.cast()) } == 0
            || unsafe { GetLengthSid(sid.cast()) } as usize != sid_length
        {
            return Err(fail("SYSTEM_PARENT_ACL_INVALID", "ACE SID 无效。"));
        }
        let mask = unsafe {
            std::ptr::read_unaligned(
                ace.cast::<u8>()
                    .add(offset_of!(ACCESS_ALLOWED_ACE, Mask))
                    .cast::<u32>(),
            )
        };
        ace_rule(
            header.AceType,
            header.AceFlags,
            mask,
            &sid_text(sid.cast())?,
            current,
        )?;
    }
    drop(allocation);
    Ok(VerifiedSystemParent { owner_sid })
}

#[cfg(test)]
#[path = "windows_parent_security_tests.rs"]
mod tests;
