use super::*;
use platform_native::windows::{files as native, removal};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::windows::fs::OpenOptionsExt,
    time::Duration,
};

pub(super) struct Directory(pub(super) native::PinnedDirectory);
impl Directory {
    pub(super) fn file(&self) -> &File {
        &self.0.file
    }
    pub(super) fn sync(&self) -> Result<()> {
        native::sync_directory(self.file()).map_err(Into::into)
    }
}

pub(super) fn open_directory(
    path: &Path,
    expected: Option<&PersistedIdentity>,
) -> Result<Directory> {
    let directory = Directory(native::pin_directory(path).map_err(unavailable)?);
    let before = PersistedIdentity::from_file(directory.file()).map_err(unavailable)?;
    if expected.is_some_and(|expected| !expected.matches_captured(&before)) {
        return Err(changed("文件删除父目录身份已变化。"));
    }
    verify_directory(path, &directory, &before)?;
    Ok(directory)
}

pub(super) fn verify_directory(
    path: &Path,
    directory: &Directory,
    expected: &PersistedIdentity,
) -> Result<()> {
    if !directory.file().metadata()?.is_dir()
        || !expected
            .matches_captured(&PersistedIdentity::from_file(directory.file()).map_err(unavailable)?)
        || !expected.matches_path(path).map_err(unavailable)?
    {
        return Err(changed("文件删除目录身份或位置已变化。"));
    }
    Ok(())
}

pub(super) fn private_stage(stage: &Directory, parent: &Directory) -> Result<()> {
    native::verify_private(stage.file()).map_err(|_| changed("文件删除暂存权限或卷已变化。"))?;
    if native::identity(stage.file())?.volume_serial
        != native::identity(parent.file())?.volume_serial
    {
        return Err(changed("文件删除暂存权限或卷已变化。"));
    }
    Ok(())
}

pub(super) fn open_file(path: &Path) -> Result<Option<File>> {
    crate::projects::check_no_symlinks(path, true)?;
    // This is the original ordinary readable-file rule: no private DACL,
    // READ_CONTROL or single-link admission is added to external partials.
    let mut waits = [20, 40, 80, 160, 320, 640, 1000].into_iter();
    let file = loop {
        match OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2 | 0x4)
            .custom_flags(0x0020_0000) // FILE_FLAG_OPEN_REPARSE_POINT
            .open(path)
        {
            Ok(file) => break file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) if retryable(&error) => {
                if let Some(ms) = waits.next() {
                    std::thread::sleep(Duration::from_millis(ms));
                } else {
                    return Err(occupied(path, error));
                }
            }
            Err(error) => return Err(unavailable(error)),
        }
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() || platform_native::filesystem::is_reparse(&metadata) {
        return Err(changed("文件删除目标不是普通文件。"));
    }
    Ok(Some(file))
}

pub(super) fn require_identity(file: &File, expected: &PersistedIdentity) -> Result<()> {
    if !expected.matches_captured(&PersistedIdentity::from_file(file).map_err(unavailable)?) {
        return Err(changed("文件删除目标身份已变化。"));
    }
    Ok(())
}

pub(super) fn require_original_absent(plan: &FileDeletionPlan) -> Result<()> {
    if open_file(&plan.original)?.is_some()
        || !plan
            .expected
            .absent_on_expected_volume(&plan.original)
            .map_err(unavailable)?
    {
        return Err(changed("文件原位置出现新内容，已保留。"));
    }
    Ok(())
}

pub(super) fn directory_names(path: &Path) -> Result<Vec<std::ffi::OsString>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(path)? {
        names.push(entry?.file_name());
        if names.len() > 1 {
            return Err(changed("文件删除暂存目录包含多余内容，已保留。"));
        }
    }
    Ok(names)
}

fn native_identity(expected: &PersistedIdentity, directory: bool) -> Result<native::FileIdentity> {
    require_windows_identity(expected)?;
    Ok(native::FileIdentity {
        volume_serial: expected.device,
        file_id: expected.windows_file_id.unwrap(),
        // Neither selected ordinary files nor directories have a link-count
        // admission here. The allowing-hard-links rename ignores this field.
        links: 0,
        directory,
    })
}

fn retryable(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(5 | 32 | 33))
}
fn occupied(path: &Path, cause: impl std::fmt::Display) -> ApiError {
    unavailable(cause)
        .detail("remainingPath", path.to_string_lossy().into_owned())
        .detail("reason", "file_in_use")
        .detail("hint", "有文件正在被使用，关闭相关程序后重试。")
}

pub(super) fn rename_file(
    source: &Path,
    destination: &Path,
    expected: &PersistedIdentity,
) -> Result<()> {
    let identity = native_identity(expected, false)?;
    let mut waits = [20, 40, 80, 160, 320, 640, 1000].into_iter();
    loop {
        match native::rename_file_noreplace_expected_allowing_hard_links(
            source,
            destination,
            &identity,
        ) {
            Ok(()) => return Ok(()),
            Err(error) if native::publication_outcome_unknown(&error) => {
                return Err(unavailable(error).outcome("unknown"));
            }
            Err(error) if retryable(&error) => {
                if let Some(ms) = waits.next() {
                    std::thread::sleep(Duration::from_millis(ms));
                } else {
                    return Err(occupied(source, error));
                }
            }
            Err(error) => return Err(changed(format!("文件删除的安全原子移动未完成：{error}"))),
        }
    }
}

pub(super) fn restore_foreign(plan: &FileDeletionPlan) -> Result<()> {
    if let Some(file) = open_file(&plan.staged_payload())? {
        let identity = PersistedIdentity::from_file(&file).map_err(unavailable)?;
        drop(file);
        // The original policy only attempts an exclusive restoration. A new
        // public occupant is never overwritten and no foreign file is unlinked.
        let _ = rename_file(&plan.staged_payload(), &plan.original, &identity);
    }
    Ok(())
}

pub(super) fn remove_name(
    path: &Path,
    expected: &PersistedIdentity,
    directory: bool,
) -> Result<()> {
    let identity = native_identity(expected, directory)?;
    let _parent = open_directory(
        path.parent().ok_or_else(|| changed("文件父目录缺失。"))?,
        None,
    )?;
    let mut waits = [20, 40, 80, 160, 320, 640, 1000].into_iter();
    loop {
        // This native API ignores readonly for this disposition alone. It does
        // not temporarily alter an external hard link's shared attributes.
        match removal::remove(path, &identity) {
            Ok(()) => break,
            Err(error) if retryable(&error) => {
                if let Some(ms) = waits.next() {
                    std::thread::sleep(Duration::from_millis(ms));
                } else {
                    return Err(occupied(path, error));
                }
            }
            Err(error) => return Err(unavailable(error)),
        }
    }
    // Successful SetDisposition is not an absence receipt. In particular do
    // not send a second deletion request after a replacement has appeared.
    for ms in [0, 20, 40, 80, 160, 320, 640, 1000] {
        if ms != 0 {
            std::thread::sleep(Duration::from_millis(ms));
        }
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if expected
                    .absent_on_expected_volume(path)
                    .map_err(unavailable)?
                {
                    return Ok(());
                }
                return Err(changed("删除请求后原位置出现新内容，已保留。"));
            }
            Err(error) if retryable(&error) => (),
            Err(error) => return Err(unavailable(error)),
            Ok(_) => match removal::inspect(path) {
                Ok(opened) if !identity.same_object(&opened.identity) => {
                    return Err(changed("删除请求后原位置出现替换对象，已保留。"));
                }
                Ok(_) => (),
                Err(error) if retryable(&error) => (),
                Err(error) if error.kind() == io::ErrorKind::NotFound => (),
                Err(error) => return Err(unavailable(error)),
            },
        }
    }
    Err(occupied(path, "删除请求尚未形成已确认的路径缺失。").outcome("unknown"))
}
