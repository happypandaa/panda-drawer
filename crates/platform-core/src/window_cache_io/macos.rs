//! Existing Mac descriptor-relative window preference publication and retirement.
use super::{invalid, private_directory};
use crate::app_windows::{read_snapshot, same_snapshot, Snapshot, MAX_CACHE_BYTES};
use crate::ApiError;
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

pub(crate) fn publish_cache(
    owner: &Path,
    previous: Option<&Snapshot>,
    bytes: &[u8],
    before_publish: &mut impl FnMut(),
) -> Result<Snapshot, ApiError> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::OpenOptionsExt,
        },
    };
    let path = owner.join("window-size.json");
    if !same_snapshot(read_snapshot(&path)?.as_ref(), previous) {
        return Err(ApiError::new(
            "WINDOW_CACHE_CHANGED",
            "本项目窗口尺寸缓存已被外部修改，已停止写入。",
        ));
    }
    private_directory(owner)?;
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(owner)?;
    let name = format!(".window-size-write-{}.json", uuid::Uuid::new_v4());
    let temporary = owner.join(&name);
    let from = CString::new(name).unwrap();
    let to = CString::new("window-size.json").unwrap();
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            from.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut pending = unsafe { fs::File::from_raw_fd(descriptor) };
    pending.write_all(bytes)?;
    pending.sync_all()?;
    let identity = crate::file_identity::PersistedIdentity::from_file(&pending)
        .map_err(|_| invalid("窗口尺寸暂存身份无法核验。"))?;
    before_publish();
    if unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            from.as_ptr(),
            directory.as_raw_fd(),
            to.as_ptr(),
            if previous.is_some() {
                libc::RENAME_SWAP
            } else {
                libc::RENAME_EXCL
            },
        )
    } != 0
    {
        return Err(ApiError::new(
            "WINDOW_CACHE_PUBLISH_FAILED",
            "窗口尺寸缓存未发布，暂存仍在本项目控制目录。",
        )
        .detail("conflictPath", temporary.to_string_lossy().into_owned()));
    }
    directory.sync_all()?;
    let current = read_snapshot(&path)?.ok_or_else(|| invalid("窗口尺寸缓存发布结果缺失。"))?;
    if current.bytes != bytes || !identity.same_runtime_object(&current.identity) {
        return Err(ApiError::new(
            "WINDOW_CACHE_CHANGED",
            "窗口尺寸缓存发布时发生外部变化，现场保留在本项目控制目录。",
        ));
    }
    if let Some(previous) = previous {
        let displaced = read_snapshot(&temporary)?;
        if !same_snapshot(displaced.as_ref(), Some(previous)) {
            return Err(ApiError::new(
                "WINDOW_CACHE_CHANGED",
                "外部窗口尺寸文件已换出保留，本项目停止继续写入。",
            )
            .detail("conflictPath", temporary.to_string_lossy().into_owned()));
        }
        retire_file(
            owner,
            temporary.file_name().unwrap(),
            &previous.identity,
            Some(&previous.bytes),
        )?;
    }
    Ok(current)
}

pub(crate) fn retire_file(
    owner: &Path,
    name: &std::ffi::OsStr,
    expected: &crate::file_identity::PersistedIdentity,
    bytes: Option<&[u8]>,
) -> Result<(), ApiError> {
    use std::{
        ffi::CString,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{ffi::OsStrExt, fs::OpenOptionsExt},
        },
    };
    private_directory(owner)?;
    let directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(owner)?;
    let from = CString::new(name.as_bytes()).map_err(|_| invalid("窗口尺寸文件名无效。"))?;
    let to = CString::new(format!(
        ".window-size-retiring-{}.json",
        uuid::Uuid::new_v4()
    ))
    .unwrap();
    if unsafe {
        libc::renameatx_np(
            directory.as_raw_fd(),
            from.as_ptr(),
            directory.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    let result = (|| {
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                to.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        if descriptor < 0 {
            return Err(invalid("窗口尺寸移除身份无法核验。"));
        }
        let file = unsafe { fs::File::from_raw_fd(descriptor) };
        let observed = crate::file_identity::PersistedIdentity::from_file(&file)
            .map_err(|_| invalid("窗口尺寸移除身份无法核验。"))?;
        if !expected.same_runtime_object(&observed) {
            return Err(invalid("窗口尺寸文件身份已变化，未删除。"));
        }
        if let Some(expected_bytes) = bytes {
            let mut current = Vec::new();
            file.take(MAX_CACHE_BYTES + 1).read_to_end(&mut current)?;
            if current != expected_bytes {
                return Err(invalid("窗口尺寸文件内容已变化，未删除。"));
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        unsafe {
            libc::renameatx_np(
                directory.as_raw_fd(),
                to.as_ptr(),
                directory.as_raw_fd(),
                from.as_ptr(),
                libc::RENAME_EXCL,
            );
        }
        return Err(error);
    }
    if unsafe { libc::unlinkat(directory.as_raw_fd(), to.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    directory.sync_all()?;
    Ok(())
}
