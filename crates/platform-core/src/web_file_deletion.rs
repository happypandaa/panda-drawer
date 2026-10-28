//! Identity-bound removal of a web project's ordinary files. Public names move
//! into a private directory on their own volume before any unlink takes place.
use crate::{
    file_identity::PersistedIdentity, projects::check_no_symlinks,
    web_project_ownership::WebProjectOwnership, ApiError,
};
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CStr, CString},
    fs::{File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Component, Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, ApiError>;
static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
const PREFIX: &str = ".panda-delete-file-";
const PAYLOAD: &str = "payload";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FileDeletionPlan {
    pub(crate) original: PathBuf,
    pub(crate) expected: PersistedIdentity,
    pub(crate) parent_identity: PersistedIdentity,
    pub(crate) staging_path: PathBuf,
    pub(crate) staging_identity: Option<PersistedIdentity>,
    pub(crate) state: FileDeletionState,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum FileDeletionState {
    Preparing,
    RenameIntent,
    Staged,
    Removing,
    Removed,
}

impl FileDeletionPlan {
    pub(crate) fn validate(&self) -> Result<()> {
        if !self.original.is_absolute()
            || self.original.file_name().is_none()
            || self
                .original
                .components()
                .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
            || self.staging_path.parent() != self.original.parent()
        {
            return Err(changed("文件删除计划路径无效。"));
        }
        let name = self
            .staging_path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(PREFIX))
            .ok_or_else(|| changed("文件删除暂存路径无效。"))?;
        if Uuid::parse_str(name)
            .ok()
            .is_none_or(|id| id.is_nil() || id.to_string() != name)
        {
            return Err(changed("文件删除暂存标识无效。"));
        }
        if self.staging_identity.is_none() && self.state != FileDeletionState::Preparing {
            return Err(changed("文件删除暂存身份缺失。"));
        }
        Ok(())
    }

    /// Owner updates may advance progress, never widen a recorded path/identity.
    pub(crate) fn same_authorization(&self, previous: &Self) -> bool {
        self.original == previous.original
            && self.expected.matches_captured(&previous.expected)
            && self
                .parent_identity
                .matches_captured(&previous.parent_identity)
            && self.staging_path == previous.staging_path
            && match (&previous.staging_identity, &self.staging_identity) {
                (None, _) => true,
                (Some(a), Some(b)) => a.matches_captured(b),
                (Some(_), None) => false,
            }
    }

    pub(crate) fn staged_payload(&self) -> PathBuf {
        self.staging_path.join(PAYLOAD)
    }
}

fn changed(message: impl Into<String>) -> ApiError {
    ApiError::new("WEB_PROJECT_MATERIAL_CHANGED", message)
}
fn unavailable(error: impl std::fmt::Display) -> ApiError {
    ApiError::new("WEB_PROJECT_MATERIAL_UNAVAILABLE", error.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    PlanPersisted,
    StageCreated,
    StageBound,
    RenameIntentPersisted,
    BeforeRename,
    Renamed,
    StagePersisted,
    RemoveIntentPersisted,
    BeforeUnlink,
    Unlinked,
    RemovedPersisted,
    StageRemoved,
    PlanForgotten,
}

pub(crate) fn remove_owned_file(
    ownership: &WebProjectOwnership,
    id: &str,
    path: &Path,
    expected: &PersistedIdentity,
) -> Result<()> {
    remove_with(ownership, id, path, expected, |_, _| Ok(()))
}

#[cfg(test)]
pub(crate) fn fixture_remove_after_capture(
    ownership: &WebProjectOwnership,
    id: &str,
    path: &Path,
    expected: &PersistedIdentity,
    mut replace: impl FnMut() -> Result<()>,
) -> Result<()> {
    remove_with(ownership, id, path, expected, |point, _| {
        if point == Point::BeforeRename {
            replace()?;
        }
        Ok(())
    })
}

/// Read-only uninstall scope. A captured public replacement can remain in a
/// stage after a failed exclusive restoration; the stage directory's identity
/// alone never authorizes deletion of that foreign payload.
pub(crate) fn owned_scopes(plan: &FileDeletionPlan) -> Result<Vec<PathBuf>> {
    plan.validate()?;
    let mut paths = vec![];
    check_no_symlinks(&plan.original, true)?;
    if !plan
        .expected
        .absent_on_expected_volume(&plan.original)
        .map_err(unavailable)?
    {
        let parent = open_directory(plan.original.parent().unwrap(), Some(&plan.parent_identity))?;
        let file = open_file_at(&parent, &name(&plan.original)?)?
            .ok_or_else(|| changed("读取删除范围时原文件发生变化。"))?;
        require_identity(&file, &plan.expected)?;
        paths.push(plan.original.clone());
    }
    match &plan.staging_identity {
        None => {
            if !plan
                .parent_identity
                .absent_on_expected_volume(&plan.staging_path)
                .map_err(unavailable)?
            {
                return Err(changed(format!(
                    "文件删除暂存目录未绑定身份，已保留：{}",
                    plan.staging_path.display()
                )));
            }
        }
        Some(expected) => {
            if !expected
                .absent_on_expected_volume(&plan.staging_path)
                .map_err(unavailable)?
            {
                let stage = open_directory(&plan.staging_path, Some(expected))?;
                let entries = directory_names(&stage)?;
                if entries
                    .iter()
                    .any(|entry| entry.as_bytes() != PAYLOAD.as_bytes())
                {
                    return Err(changed("文件删除暂存目录含未归属内容，已保留。"));
                }
                if let Some(file) = open_file_at(&stage, &CString::new(PAYLOAD).unwrap())? {
                    require_identity(&file, &plan.expected)?;
                } else {
                    plan.expected
                        .absent_on_expected_volume(&plan.staged_payload())
                        .map_err(unavailable)?;
                }
                verify_directory(&plan.staging_path, &stage, expected)?;
                paths.push(plan.staging_path.clone());
            }
        }
    }
    Ok(paths)
}

fn remove_with(
    ownership: &WebProjectOwnership,
    id: &str,
    path: &Path,
    expected: &PersistedIdentity,
    mut hook: impl FnMut(Point, &FileDeletionPlan) -> Result<()>,
) -> Result<()> {
    let _guard = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let previous = ownership
        .file_deletion_plans(id)?
        .into_iter()
        .find(|plan| plan.original == path);
    let mut plan = if let Some(plan) = previous {
        plan.validate()?;
        if !expected.matches_captured(&plan.expected) {
            return Err(changed("文件删除计划与受理身份不符。"));
        }
        plan
    } else {
        check_no_symlinks(path, true)?;
        if expected
            .absent_on_expected_volume(path)
            .map_err(unavailable)?
        {
            return Ok(());
        }
        let parent = path.parent().ok_or_else(|| changed("文件父目录缺失。"))?;
        let parent_file = open_directory(parent, None)?;
        let opened = open_file_at(&parent_file, &name(path)?)?
            .ok_or_else(|| changed("受理文件删除时目标已变化。"))?;
        if !expected.matches_captured(&PersistedIdentity::from_file(&opened).map_err(unavailable)?)
        {
            return Err(changed("文件删除目标身份已变化。"));
        }
        let plan = FileDeletionPlan {
            original: path.into(),
            expected: expected.clone(),
            parent_identity: PersistedIdentity::from_file(&parent_file).map_err(unavailable)?,
            staging_path: parent.join(format!("{PREFIX}{}", Uuid::new_v4())),
            staging_identity: None,
            state: FileDeletionState::Preparing,
        };
        plan.validate()?;
        ownership.save_file_deletion_plan(id, &plan)?;
        hook(Point::PlanPersisted, &plan)?;
        plan
    };

    let parent_path = plan.original.parent().unwrap();
    let parent = open_directory(parent_path, Some(&plan.parent_identity))?;
    if plan.state == FileDeletionState::Preparing {
        if plan.staging_identity.is_none() {
            let stage_name = name(&plan.staging_path)?;
            if unsafe { libc::mkdirat(parent.as_raw_fd(), stage_name.as_ptr(), 0o700) } != 0 {
                let cause = io::Error::last_os_error();
                return Err(changed(format!(
                    "未绑定的文件删除暂存目录已存在或无法创建，已保留：{}（{cause}）",
                    plan.staging_path.display()
                )));
            }
            parent.sync_all()?;
            let stage = open_directory(&plan.staging_path, None)?;
            if stage.metadata()?.permissions().mode() & 0o777 != 0o700
                || stage.metadata()?.dev() != parent.metadata()?.dev()
            {
                return Err(changed("文件删除暂存权限或卷不符。"));
            }
            plan.staging_identity =
                Some(PersistedIdentity::from_file(&stage).map_err(unavailable)?);
            hook(Point::StageCreated, &plan)?;
            verify_directory(
                &plan.staging_path,
                &stage,
                plan.staging_identity.as_ref().unwrap(),
            )?;
            ownership.save_file_deletion_plan(id, &plan)?;
            hook(Point::StageBound, &plan)?;
        }
        plan.state = FileDeletionState::RenameIntent;
        ownership.save_file_deletion_plan(id, &plan)?;
        hook(Point::RenameIntentPersisted, &plan)?;
    }

    let staging_identity = plan.staging_identity.clone().unwrap();
    if staging_identity
        .absent_on_expected_volume(&plan.staging_path)
        .map_err(unavailable)?
    {
        if plan.state == FileDeletionState::Removed
            && plan
                .expected
                .absent_on_expected_volume(&plan.original)
                .map_err(unavailable)?
        {
            parent.sync_all()?;
            ownership.forget_file_deletion_plan(id, path, expected)?;
            hook(Point::PlanForgotten, &plan)?;
            return Ok(());
        }
        return Err(changed("文件删除暂存目录提前消失，未确认删除完成。"));
    }
    let staging = open_directory(&plan.staging_path, Some(&staging_identity))?;
    if staging.metadata()?.permissions().mode() & 0o777 != 0o700
        || staging.metadata()?.dev() != parent.metadata()?.dev()
    {
        return Err(changed("文件删除暂存权限或卷已变化。"));
    }
    let source_name = name(&plan.original)?;
    let payload_name = CString::new(PAYLOAD).unwrap();

    if plan.state == FileDeletionState::RenameIntent {
        let source = open_file_at(&parent, &source_name)?;
        let payload = open_file_at(&staging, &payload_name)?;
        match (source, payload) {
            (Some(source), None) => {
                require_identity(&source, &plan.expected)?;
                verify_directory(parent_path, &parent, &plan.parent_identity)?;
                verify_directory(&plan.staging_path, &staging, &staging_identity)?;
                hook(Point::BeforeRename, &plan)?;
                verify_directory(parent_path, &parent, &plan.parent_identity)?;
                verify_directory(&plan.staging_path, &staging, &staging_identity)?;
                rename_exclusive(&parent, &source_name, &staging, &payload_name)?;
                parent.sync_all()?;
                staging.sync_all()?;
                hook(Point::Renamed, &plan)?;
                let moved = match open_file_at(&staging, &payload_name) {
                    Ok(Some(moved)) => moved,
                    _ => {
                        restore_foreign(&parent, &source_name, &staging, &payload_name)?;
                        return Err(changed("暂存后的文件不是原受管文件，已保留或归还现场。"));
                    }
                };
                if require_identity(&moved, &plan.expected).is_err() {
                    restore_foreign(&parent, &source_name, &staging, &payload_name)?;
                    return Err(changed("捕获到同名替换文件，已保留或归还现场。"));
                }
            }
            (None, Some(payload)) => {
                if require_identity(&payload, &plan.expected).is_err() {
                    restore_foreign(&parent, &source_name, &staging, &payload_name)?;
                    return Err(changed("恢复删除时发现替换文件，已保留或归还现场。"));
                }
            }
            _ => {
                return Err(changed(
                    "文件删除原件与暂存状态不一致，未删除任何替换文件。",
                ))
            }
        }
        require_original_absent(&parent, &source_name, &plan)?;
        plan.state = FileDeletionState::Staged;
        ownership.save_file_deletion_plan(id, &plan)?;
        hook(Point::StagePersisted, &plan)?;
    }

    if plan.state == FileDeletionState::Staged {
        plan.state = FileDeletionState::Removing;
        ownership.save_file_deletion_plan(id, &plan)?;
        hook(Point::RemoveIntentPersisted, &plan)?;
    }
    if plan.state == FileDeletionState::Removing {
        require_original_absent(&parent, &source_name, &plan)?;
        hook(Point::BeforeUnlink, &plan)?;
        require_original_absent(&parent, &source_name, &plan)?;
        verify_directory(&plan.staging_path, &staging, &staging_identity)?;
        if let Some(payload) = open_file_at(&staging, &payload_name)? {
            require_identity(&payload, &plan.expected)?;
            if unsafe { libc::unlinkat(staging.as_raw_fd(), payload_name.as_ptr(), 0) } != 0 {
                return Err(io::Error::last_os_error().into());
            }
            staging.sync_all()?;
            hook(Point::Unlinked, &plan)?;
        } else {
            plan.expected
                .absent_on_expected_volume(&plan.staged_payload())
                .map_err(unavailable)?;
        }
        require_original_absent(&parent, &source_name, &plan)?;
        plan.state = FileDeletionState::Removed;
        ownership.save_file_deletion_plan(id, &plan)?;
        hook(Point::RemovedPersisted, &plan)?;
    }

    if plan.state != FileDeletionState::Removed {
        return Err(changed("文件删除进度无效。"));
    }
    require_original_absent(&parent, &source_name, &plan)?;
    if open_file_at(&staging, &payload_name)?.is_some() {
        return Err(changed("已删除文件的暂存位置出现新内容，已保留。"));
    }
    verify_directory(parent_path, &parent, &plan.parent_identity)?;
    verify_directory(&plan.staging_path, &staging, &staging_identity)?;
    // rmdir refuses nonempty directories, so unrecognized contents survive.
    if unsafe {
        libc::unlinkat(
            parent.as_raw_fd(),
            name(&plan.staging_path)?.as_ptr(),
            libc::AT_REMOVEDIR,
        )
    } != 0
    {
        return Err(io::Error::last_os_error().into());
    }
    parent.sync_all()?;
    hook(Point::StageRemoved, &plan)?;
    ownership.forget_file_deletion_plan(id, path, expected)?;
    hook(Point::PlanForgotten, &plan)?;
    Ok(())
}

fn name(path: &Path) -> Result<CString> {
    CString::new(
        path.file_name()
            .ok_or_else(|| changed("文件名缺失。"))?
            .as_bytes(),
    )
    .map_err(|_| changed("文件名包含无效字符。"))
}
fn open_directory(path: &Path, expected: Option<&PersistedIdentity>) -> Result<File> {
    check_no_symlinks(path, false)?;
    let before = PersistedIdentity::capture(path).map_err(unavailable)?;
    if expected.is_some_and(|expected| !expected.matches_captured(&before)) {
        return Err(changed("文件删除父目录身份已变化。"));
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    verify_directory(path, &file, &before)?;
    Ok(file)
}
fn verify_directory(path: &Path, file: &File, expected: &PersistedIdentity) -> Result<()> {
    if !file.metadata()?.is_dir()
        || !expected.matches_captured(&PersistedIdentity::from_file(file).map_err(unavailable)?)
        || !expected.matches_path(path).map_err(unavailable)?
    {
        return Err(changed("文件删除目录身份或位置已变化。"));
    }
    Ok(())
}
fn open_file_at(parent: &File, name: &CString) -> Result<Option<File>> {
    use std::os::fd::FromRawFd;
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(changed(format!("无法安全打开文件删除目标：{error}")));
    }
    let file = unsafe { File::from_raw_fd(fd) };
    if !file.metadata()?.is_file() {
        return Err(changed("文件删除目标不是普通文件。"));
    }
    Ok(Some(file))
}
fn directory_names(directory: &File) -> Result<Vec<CString>> {
    let duplicate = unsafe { libc::dup(directory.as_raw_fd()) };
    if duplicate < 0 {
        return Err(io::Error::last_os_error().into());
    }
    let opened = unsafe { libc::fdopendir(duplicate) };
    if opened.is_null() {
        unsafe { libc::close(duplicate) };
        return Err(io::Error::last_os_error().into());
    }
    struct Directory(*mut libc::DIR);
    impl Drop for Directory {
        fn drop(&mut self) {
            unsafe { libc::closedir(self.0) };
        }
    }
    let opened = Directory(opened);
    let mut names = vec![];
    loop {
        #[cfg(target_os = "macos")]
        unsafe {
            *libc::__error() = 0;
        }
        #[cfg(target_os = "linux")]
        unsafe {
            *libc::__errno_location() = 0;
        }
        let entry = unsafe { libc::readdir(opened.0) };
        if entry.is_null() {
            let error = io::Error::last_os_error();
            if error.raw_os_error().unwrap_or(0) != 0 {
                return Err(error.into());
            }
            break;
        }
        let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) };
        if name.to_bytes() != b"." && name.to_bytes() != b".." {
            names.push(name.to_owned());
            if names.len() > 1 {
                return Err(changed("文件删除暂存目录包含多余内容，已保留。"));
            }
        }
    }
    Ok(names)
}
fn require_identity(file: &File, expected: &PersistedIdentity) -> Result<()> {
    if !expected.matches_captured(&PersistedIdentity::from_file(file).map_err(unavailable)?) {
        return Err(changed("文件删除目标身份已变化。"));
    }
    Ok(())
}
fn require_original_absent(
    parent: &File,
    original: &CString,
    plan: &FileDeletionPlan,
) -> Result<()> {
    if open_file_at(parent, original)?.is_some()
        || !plan
            .expected
            .absent_on_expected_volume(&plan.original)
            .map_err(unavailable)?
    {
        return Err(changed("文件原位置出现新内容，已保留。"));
    }
    Ok(())
}
fn restore_foreign(
    parent: &File,
    original: &CString,
    staging: &File,
    payload: &CString,
) -> Result<()> {
    // Exclusive rename never overwrites a public file created since capture.
    let _ = rename_exclusive(staging, payload, parent, original);
    parent.sync_all()?;
    staging.sync_all()?;
    Ok(())
}
fn rename_exclusive(from_dir: &File, from: &CString, to_dir: &File, to: &CString) -> Result<()> {
    #[cfg(target_os = "macos")]
    let status = unsafe {
        libc::renameatx_np(
            from_dir.as_raw_fd(),
            from.as_ptr(),
            to_dir.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    #[cfg(target_os = "linux")]
    let status = unsafe {
        libc::renameat2(
            from_dir.as_raw_fd(),
            from.as_ptr(),
            to_dir.as_raw_fd(),
            to.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let status = {
        let _ = (from_dir, from, to_dir, to);
        -1
    };
    if status != 0 {
        return Err(changed(format!(
            "文件删除的安全原子移动未完成：{}",
            io::Error::last_os_error()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    include!("web_file_deletion_tests.rs");
}
