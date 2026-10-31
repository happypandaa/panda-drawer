use super::*;
use platform_native::windows::{files as native, removal};
use std::{fs::File, ops::Deref, path::Component};

pub(super) struct Directory(native::PinnedDirectory);
impl Deref for Directory {
    type Target = File;
    fn deref(&self) -> &File {
        &self.0.file
    }
}
impl Directory {
    pub(super) fn sync_all(&self) -> Result<()> {
        // Windows has no directory fsync. The selected directory is held and
        // observed; every newly written protocol file is flushed separately.
        native::sync_directory(&self.0.file).map_err(Into::into)
    }
}

pub(super) fn normalized(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path.components().any(|part| {
            !matches!(
                part,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
    {
        return Err(error("删除路径必须是规范的绝对路径。"));
    }
    Ok(())
}

pub(super) fn ordinary(path: &Path, directory: bool) -> Result<()> {
    normalized(path)?;
    let opened = native::open_no_reparse(path)?;
    if opened.identity.directory != directory || (!directory && !opened.file.metadata()?.is_file())
    {
        return Err(error("删除路径不是预期的普通对象。"));
    }
    Ok(())
}

pub(super) fn absent(path: &Path) -> Result<bool> {
    normalized(path)?;
    crate::path_checks::check_no_symlinks(path, true)?;
    match fs::symlink_metadata(path) {
        Err(issue) if issue.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(issue) => Err(issue.into()),
        Ok(_) => Ok(false),
    }
}

pub(super) fn absent_with_identity(
    path: &Path,
    expected: Option<&PersistedIdentity>,
) -> Result<bool> {
    match expected {
        Some(expected) => expected.absent_on_expected_volume(path).map_err(fs_error),
        None => absent(path),
    }
}

pub(super) fn optional_identity(path: &Path) -> Result<Option<PersistedIdentity>> {
    if absent(path)? {
        return Ok(None);
    }
    ordinary(path, true)?;
    PersistedIdentity::capture(path).map(Some).map_err(fs_error)
}

pub(super) fn private_directory(path: &Path, exclusive: bool) -> Result<()> {
    normalized(path)?;
    match native::create_private_directory(path) {
        Ok(()) => (),
        Err(issue) if !exclusive && issue.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(issue) => return Err(issue.into()),
    }
    check_private_directory(path)
}

pub(super) fn check_private_directory(path: &Path) -> Result<()> {
    let directory = directory_file(path)?;
    native::verify_private(&directory).map_err(Into::into)
}

pub(super) fn directory_file(path: &Path) -> Result<Directory> {
    normalized(path)?;
    Ok(Directory(native::pin_directory(path)?))
}

pub(super) fn preflight_targets(
    _data: &PersistedIdentity,
    targets: &[DirectoryTarget],
    mut mounted: impl FnMut(&Path) -> Result<bool>,
) -> Result<()> {
    for target in targets {
        let Some(identity) = &target.identity else {
            continue;
        };
        let parent = target
            .parent_identity
            .as_ref()
            .ok_or_else(|| error("删除父目录身份缺失。"))?;
        if identity.device != parent.device
            || identity.volume_uuid != parent.volume_uuid
            || mounted(&target.original)?
        {
            return Err(ApiError::new(
                "NODE_DELETION_TARGET_VOLUME_UNSUPPORTED",
                format!(
                    "平台私有目录位于独立卷或挂载点，本次未受理删除、未移动任何目录：{}",
                    target.original.display()
                ),
            ));
        }
    }
    Ok(())
}

pub(super) fn known_mount_point(path: &Path) -> Result<bool> {
    // Windows mount/junction roots are reparse points. Observe the leaf itself;
    // never follow it to acquire a different volume or deletion authority.
    Ok(platform_native::filesystem::is_reparse(
        &removal::inspect(path)?.file.metadata()?,
    ))
}

pub(super) fn native_identity(
    expected: &PersistedIdentity,
    directory: bool,
) -> Result<native::FileIdentity> {
    let file_id = expected
        .windows_file_id
        .ok_or_else(|| error("删除任务缺少完整 Windows 文件身份。"))?;
    if expected.volume_uuid.is_some() {
        return Err(error("删除任务包含其他平台的卷身份。"));
    }
    Ok(native::FileIdentity {
        volume_serial: expected.device,
        file_id,
        directory,
        links: 1,
    })
}

pub(super) fn rename_directory(
    from: &Path,
    to: &Path,
    expected: &PersistedIdentity,
    source_parent: &PersistedIdentity,
    staging_parent: &PersistedIdentity,
) -> Result<()> {
    let left = directory_file(from.parent().ok_or_else(|| error("删除目标父目录缺失。"))?)?;
    let right = directory_file(to.parent().ok_or_else(|| error("暂存父目录缺失。"))?)?;
    let left_id = PersistedIdentity::from_file(&left).map_err(fs_error)?;
    let right_id = PersistedIdentity::from_file(&right).map_err(fs_error)?;
    if !source_parent.matches_captured(&left_id)
        || !staging_parent.matches_captured(&right_id)
        || expected.device != left_id.device
        || left_id.device != right_id.device
        || known_mount_point(from)?
    {
        return Err(error("删除目标身份变化、跨卷或是挂载点，未移动目录。"));
    }
    native::rename_directory_noreplace_expected(from, to, &native_identity(expected, true)?)
        .map_err(|issue| {
            let unknown = native::publication_outcome_unknown(&issue);
            let issue = fs_error(issue);
            if unknown {
                issue.outcome("unknown")
            } else {
                issue
            }
        })?;
    left.sync_all()?;
    right.sync_all()
}

pub(super) fn remove_empty_directory(path: &Path, expected: &PersistedIdentity) -> Result<()> {
    let _parent = directory_file(path.parent().ok_or_else(|| error("删除容器父级缺失。"))?)?;
    let opened = native::open_no_reparse(path)?;
    if !opened.identity.directory
        || !expected
            .matches_open_runtime(&opened.file)
            .map_err(fs_error)?
        || fs::read_dir(path)?.next().is_some()
    {
        return Err(error("收尾容器身份变化或仍有内容，未删除。"));
    }
    // Do not recurse here. An item arriving after the empty observation makes
    // native disposition fail, and is never adopted as task-owned content.
    removal::remove(path, &opened.identity)?;
    drop(opened);
    if !absent_with_identity(path, Some(expected))? {
        return Err(error("容器删除请求尚未产生名字缺失回执。").outcome("unknown"));
    }
    Ok(())
}
