//! Original Unix Node record and intent lifecycle.
use super::*;
pub(super) fn resume_cleanup_receipts(_slot: &Path, _boot: std::time::SystemTime) -> Result<()> {
    Ok(())
}
use std::os::unix::fs::MetadataExt;
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::Component,
};
pub(super) fn ordinary_path(path: &Path, directory: bool) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(fail("应用路径必须为规范的绝对路径。"));
    }
    for current in platform_native::filesystem::path_components(path) {
        if platform_native::filesystem::is_reparse(&fs::symlink_metadata(&current)?) {
            return Err(fail("应用路径不能经过符号链接。"));
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(fail("应用文件类型不符。"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if !directory && metadata.nlink() != 1 {
            return Err(fail("应用记录不能是硬链接。"));
        }
    }
    Ok(())
}

pub(super) fn private_dir(path: &Path) -> Result<()> {
    ordinary_path(path.parent().ok_or_else(|| fail("应用目录无效。"))?, true)?;
    if !path.try_exists()? {
        fs::create_dir(path)?;
    }
    ordinary_path(path, true)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub(super) fn read_small(path: &Path) -> Result<Vec<u8>> {
    ordinary_path(path, false)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut bytes = Vec::new();
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > 256 * 1024 {
            return Err(fail("应用记录必须为大小受限且不共享硬链接的普通文件。"));
        }
    }
    file.take(256 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 256 * 1024 {
        return Err(fail("应用记录超出大小限制。"));
    }
    Ok(bytes)
}

#[derive(Clone, Copy)]
pub(super) enum IntentCleanupPoint {
    BeforeCapture,
    Captured,
}

/// This open file reserves the identity of precisely the intent we created.
/// It has no automatic cleanup: uncertainty and host crashes stay fail-closed.
pub(super) struct StartIntentOwnership {
    pub(super) file: fs::File,
    pub(super) contents: Vec<u8>,
}

impl StartIntentOwnership {
    /// Validate the original open marker without replacing, retiring or
    /// adopting it. Migration may exempt a live new-workspace run only while
    /// its retained owner still names this exact file and exact contents.
    pub(super) fn require_current(&self, path: &Path) -> Result<()> {
        ordinary_path(path, false)?;
        let owned = self.file.metadata()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let matches = |current: &fs::Metadata| {
                owned.is_file()
                    && current.is_file()
                    && !current.file_type().is_symlink()
                    && owned.dev() == current.dev()
                    && owned.ino() == current.ino()
                    && owned.nlink() == 1
                    && current.nlink() == 1
            };
            if !matches(&fs::symlink_metadata(path)?)
                || read_small(path)? != self.contents
                || !matches(&fs::symlink_metadata(path)?)
            {
                return Err(run_marker::unknown());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        Err(run_marker::unknown())
    }

    #[cfg(unix)]
    pub(super) fn open_existing(path: &Path) -> Result<Self> {
        ordinary_path(path, false)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = options.open(path)?;
        let metadata = file.metadata()?;
        #[cfg(unix)]
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.len() > 256 * 1024
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(run_marker::unknown());
        }
        let mut contents = Vec::new();
        (&mut file)
            .take(256 * 1024 + 1)
            .read_to_end(&mut contents)?;
        if contents.len() > 256 * 1024 {
            return Err(run_marker::unknown());
        }
        let current = fs::symlink_metadata(path)?;
        if current.dev() != metadata.dev()
            || current.ino() != metadata.ino()
            || current.nlink() != 1
        {
            return Err(run_marker::unknown());
        }
        Ok(Self { file, contents })
    }

    #[cfg(not(unix))]
    pub(super) fn open_existing(_path: &Path) -> Result<Self> {
        Err(run_marker::unknown())
    }

    #[cfg(unix)]
    pub(super) fn publish(&mut self, path: &Path, value: &impl Serialize) -> Result<()> {
        use std::io::{Seek, SeekFrom};
        let changed = || fail("运行标记的归属或内容变化，已保留记录；请核对后再启动。");
        let owned = self.file.metadata()?;
        let before = fs::symlink_metadata(path)?;
        if before.dev() != owned.dev()
            || before.ino() != owned.ino()
            || before.nlink() != 1
            || read_small(path)? != self.contents
        {
            return Err(changed());
        }
        let contents = serde_json::to_vec_pretty(value)?;
        // Keep the original inode and creation time. A crash during this second
        // phase leaves an invalid/incomplete present marker, hence unconfirmed.
        self.file.seek(SeekFrom::Start(0))?;
        self.file.set_len(0)?;
        self.file.write_all(&contents)?;
        self.file.sync_all()?;
        self.contents = contents;
        let after = fs::symlink_metadata(path)?;
        if after.dev() != owned.dev()
            || after.ino() != owned.ino()
            || after.nlink() != 1
            || read_small(path)? != self.contents
        {
            return Err(changed());
        }
        Ok(())
    }

    #[cfg(not(unix))]
    pub(super) fn publish(&mut self, _path: &Path, _value: &impl Serialize) -> Result<()> {
        Err(run_marker::unknown())
    }

    pub(super) fn create(path: &Path, value: &impl Serialize) -> Result<Self> {
        ordinary_path(path.parent().ok_or_else(|| fail("记录目录无效。"))?, true)?;
        let contents = serde_json::to_vec_pretty(value)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(path)?;
        file.write_all(&contents)?;
        file.sync_all()?;
        platform_native::filesystem::sync_directory(path.parent().unwrap())?;
        Ok(Self { file, contents })
    }

    pub(super) fn remove_if_owned(&self, path: &Path) -> Result<()> {
        self.remove_if_owned_with(path, |_| ())
    }

    pub(super) fn remove_if_owned_with(
        &self,
        path: &Path,
        mut observe: impl FnMut(IntentCleanupPoint),
    ) -> Result<()> {
        let changed = || fail("启动标记归属或内容发生变化，已保留记录，不能解除启动保护。");
        ordinary_path(path, false).map_err(|_| changed())?;
        let owned = self.file.metadata()?;
        let check = |current: &fs::Metadata| -> Result<()> {
            if !owned.is_file() || !current.is_file() || current.file_type().is_symlink() {
                return Err(changed());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if owned.dev() != current.dev()
                    || owned.ino() != current.ino()
                    || owned.nlink() != 1
                    || current.nlink() != 1
                {
                    return Err(changed());
                }
                Ok(())
            }
            #[cfg(not(unix))]
            Err(changed())
        };
        check(&fs::symlink_metadata(path)?)?;
        if read_small(path).map_err(|_| changed())? != self.contents {
            return Err(changed());
        }
        check(&fs::symlink_metadata(path)?)?;
        observe(IntentCleanupPoint::BeforeCapture);
        let parent = path.parent().ok_or_else(changed)?;
        let cleanup = parent.join(format!("{START_INTENT_CLEANUP_PREFIX}{}", Uuid::new_v4()));
        let mut directory = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directory.mode(0o700);
        }
        // Persist the fail-closed gate before removing anything from active.
        // A crash or any subsequent error leaves this evidence for recovery.
        directory.create(&cleanup)?;
        platform_native::filesystem::sync_directory(&cleanup)?;
        platform_native::filesystem::sync_directory(parent)?;
        let captured = cleanup.join("captured-intent");
        move_node_record_exclusive(path, &captured).map_err(|_| changed())?;
        observe(IntentCleanupPoint::Captured);
        let captured_is_owned = (|| -> Result<()> {
            ordinary_path(&captured, false).map_err(|_| changed())?;
            check(&fs::symlink_metadata(&captured)?)?;
            if read_small(&captured).map_err(|_| changed())? != self.contents {
                return Err(changed());
            }
            check(&fs::symlink_metadata(&captured)?)
        })();
        if captured_is_owned.is_err() {
            // Restore only if active is still absent. A newer active marker is
            // never overwritten; either way the captured foreign object and/or
            // its restored active path survive, with the cleanup gate retained.
            let _ = move_node_record_exclusive(&captured, path);
            return Err(changed());
        }
        // Unlink only the verified object inside our exclusive private slot.
        // Never unlink the mutable public active path after a check.
        fs::remove_file(&captured)?;
        platform_native::filesystem::sync_directory(&cleanup)?;
        match fs::symlink_metadata(path) {
            Ok(_) => return Err(changed()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        fs::remove_dir(&cleanup)?;
        platform_native::filesystem::sync_directory(parent)?;
        Ok(())
    }
}

/// The same dirfd + exclusive rename primitive used by restoration transactions.
/// Paths may be replaced at the active name; displaced objects are never overwritten.
pub(super) fn move_node_record_exclusive(from: &Path, to: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::{
            ffi::CString,
            os::{
                fd::AsRawFd,
                unix::{
                    ffi::OsStrExt,
                    fs::{MetadataExt, OpenOptionsExt},
                },
            },
        };
        let open_parent = |path: &Path| -> Result<fs::File> {
            let parent = path.parent().ok_or_else(|| fail("应用记录目录无效。"))?;
            ordinary_path(parent, true)?;
            Ok(OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(parent)?)
        };
        let left = open_parent(from)?;
        let right = open_parent(to)?;
        if left.metadata()?.dev() != right.metadata()?.dev() {
            return Err(fail("应用记录提交必须留在原有数据卷。"));
        }
        let name = |path: &Path| -> Result<CString> {
            CString::new(
                path.file_name()
                    .ok_or_else(|| fail("应用记录名称无效。"))?
                    .as_bytes(),
            )
            .map_err(|_| fail("应用记录名称无效。"))
        };
        let from = name(from)?;
        let to = name(to)?;
        if unsafe {
            libc::renameatx_np(
                left.as_raw_fd(),
                from.as_ptr(),
                right.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        } != 0
        {
            return Err(fail("应用记录原子提交未完成，现有记录均保留。"));
        }
        left.sync_all()?;
        right.sync_all()?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (from, to);
        Err(fail("应用记录提交目前仅验证 macOS。"))
    }
}

pub(super) fn write_new_json(path: &Path, value: &impl Serialize) -> Result<()> {
    ordinary_path(path.parent().ok_or_else(|| fail("记录目录无效。"))?, true)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    platform_native::filesystem::sync_directory(path.parent().unwrap())?;
    Ok(())
}
