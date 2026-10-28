//! Exclusive admission for externally placed, platform-managed export files.
use crate::{file_identity::PersistedIdentity, projects::check_no_symlinks, ApiError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
type Result<T> = std::result::Result<T, ApiError>;
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExportClaim {
    pub(crate) version: u32,
    pub(crate) project_id: String,
    pub(crate) task_id: String,
    pub(crate) path: PathBuf,
    pub(crate) parent_identity: PersistedIdentity,
    pub(crate) ancestors: Vec<PersistedIdentity>,
}
fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new("EXPORT_PATH_OWNED", message)
}
pub(crate) fn directory(data: &Path, key: &str) -> Result<PathBuf> {
    if key.len() != 64
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(invalid("导出路径归属键无效。"));
    }
    Ok(data.join("web-export-owners").join(key))
}
pub(crate) fn key(path: &Path) -> Result<(String, PersistedIdentity, Vec<PersistedIdentity>)> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("导出目标没有父目录。"))?;
    check_no_symlinks(parent, false)?;
    let identity =
        PersistedIdentity::capture(parent).map_err(|error| invalid(error.to_string()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("导出文件名无效。"))?;
    let key = key_from(&identity, name)?;
    let ancestors = parent
        .ancestors()
        .map(|path| PersistedIdentity::capture(path).map_err(|error| invalid(error.to_string())))
        .collect::<Result<Vec<_>>>()?;
    Ok((key, identity, ancestors))
}
pub(crate) fn key_from(identity: &PersistedIdentity, name: &str) -> Result<String> {
    let folded = fold_name(name)?;
    let volume = identity.volume_uuid.as_ref().map_or_else(
        || serde_json::json!({"device":identity.device}),
        |volume| serde_json::json!({"volume":volume}),
    );
    let key = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(
            &serde_json::json!({"volume":volume,"inode":identity.inode,"name":folded})
        )?)
    );
    Ok(key)
}
pub(crate) fn require_source_unoccupied(data: &Path, source: &Path) -> Result<()> {
    let root = data.join("web-export-owners");
    check_no_symlinks(&root, true)?;
    if !root.exists() {
        return Ok(());
    }
    check_no_symlinks(source, true)?;
    let source_identity = match fs::symlink_metadata(source) {
        Ok(_) => {
            Some(PersistedIdentity::capture(source).map_err(|error| invalid(error.to_string()))?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let key = entry.file_name().to_string_lossy().into_owned();
        let directory = directory(data, &key)?;
        check_no_symlinks(&directory, false)?;
        let path = directory.join("owner.json");
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024 {
            return Err(invalid("导出归属记录无法核验。"));
        }
        let claim: ExportClaim = serde_json::from_slice(&fs::read(path)?)?;
        let filename = claim
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("导出归属文件名无效。"))?;
        if claim.version != 1
            || !claim.project_id.starts_with("prj_")
            || !crate::projects::valid_id(&claim.project_id)
            || !claim.task_id.starts_with("task_")
            || !crate::projects::valid_id(&claim.task_id)
            || !claim.path.is_absolute()
            || claim.path.components().any(|part| {
                matches!(
                    part,
                    std::path::Component::CurDir | std::path::Component::ParentDir
                )
            })
            || claim.path.as_os_str().as_encoded_bytes().contains(&0)
            || !claim
                .ancestors
                .first()
                .is_some_and(|ancestor| ancestor.matches_captured(&claim.parent_identity))
            || key_from(&claim.parent_identity, filename)? != key
        {
            return Err(invalid("导出归属记录无效。"));
        }
        if claim.path.starts_with(source)
            || source_identity.as_ref().is_some_and(|source_identity| {
                claim
                    .ancestors
                    .iter()
                    .any(|ancestor| ancestor.matches_captured(source_identity))
            })
        {
            return Err(invalid(
                "此文件夹包含另一个项目独占的导出文件，请使用独立项目目录。",
            ));
        }
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn fold_name(name: &str) -> Result<String> {
    use std::{
        ffi::{c_char, c_void, CStr},
        ptr,
    };
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFStringCreateWithBytes(
            allocator: *const c_void,
            bytes: *const u8,
            length: isize,
            encoding: u32,
            external: u8,
        ) -> *const c_void;
        fn CFStringCreateMutableCopy(
            allocator: *const c_void,
            max: isize,
            string: *const c_void,
        ) -> *mut c_void;
        fn CFStringFold(string: *mut c_void, flags: usize, locale: *const c_void);
        fn CFStringNormalize(string: *mut c_void, form: isize);
        fn CFStringGetLength(string: *const c_void) -> isize;
        fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
        fn CFStringGetCString(
            string: *const c_void,
            buffer: *mut c_char,
            size: isize,
            encoding: u32,
        ) -> u8;
        fn CFRelease(value: *const c_void);
    }
    struct Object(*const c_void);
    impl Drop for Object {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0) }
        }
    }
    const UTF8: u32 = 0x08000100;
    let original = unsafe {
        CFStringCreateWithBytes(ptr::null(), name.as_ptr(), name.len() as isize, UTF8, 0)
    };
    if original.is_null() {
        return Err(invalid("导出文件名无法规范化。"));
    }
    let original = Object(original);
    let folded = unsafe { CFStringCreateMutableCopy(ptr::null(), 0, original.0) };
    if folded.is_null() {
        return Err(invalid("导出文件名无法规范化。"));
    }
    let folded = Object(folded);
    unsafe {
        CFStringFold(folded.0 as *mut c_void, 1, ptr::null());
        CFStringNormalize(folded.0 as *mut c_void, 0);
    }
    let length = unsafe { CFStringGetMaximumSizeForEncoding(CFStringGetLength(folded.0), UTF8) };
    if !(0..=1024 * 1024).contains(&length) {
        return Err(invalid("导出文件名过大。"));
    }
    let mut bytes = vec![0u8; length as usize + 1];
    if unsafe {
        CFStringGetCString(
            folded.0,
            bytes.as_mut_ptr() as *mut c_char,
            bytes.len() as isize,
            UTF8,
        )
    } == 0
    {
        return Err(invalid("导出文件名无法转换。"));
    }
    unsafe { CStr::from_ptr(bytes.as_ptr() as *const c_char) }
        .to_str()
        .map(str::to_owned)
        .map_err(|_| invalid("导出文件名编码无效。"))
}
#[cfg(not(target_os = "macos"))]
fn fold_name(name: &str) -> Result<String> {
    use unicode_normalization::UnicodeNormalization;
    Ok(name.nfd().flat_map(char::to_lowercase).collect())
}
