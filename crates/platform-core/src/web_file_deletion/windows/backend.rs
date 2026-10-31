use super::{io::*, *};
use crate::projects::check_no_symlinks;
use platform_native::windows::files as native;
use std::sync::{Mutex, OnceLock};

static LOCK: OnceLock<Mutex<()>> = OnceLock::new();

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

/// Only the selected original file and a stage containing that exact payload
/// belong to the plan. Directory ownership never recursively adopts contents.
pub(crate) fn owned_scopes(plan: &FileDeletionPlan) -> Result<Vec<PathBuf>> {
    plan.validate()?;
    let mut paths = vec![];
    check_no_symlinks(&plan.original, true)?;
    if !plan
        .expected
        .absent_on_expected_volume(&plan.original)
        .map_err(unavailable)?
    {
        let _parent = open_directory(plan.original.parent().unwrap(), Some(&plan.parent_identity))?;
        let file =
            open_file(&plan.original)?.ok_or_else(|| changed("读取删除范围时原文件发生变化。"))?;
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
                if directory_names(&plan.staging_path)?
                    .iter()
                    .any(|name| name != PAYLOAD)
                {
                    return Err(changed("文件删除暂存目录含未归属内容，已保留。"));
                }
                if let Some(file) = open_file(&plan.staged_payload())? {
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
    require_windows_identity(expected)?;
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
        let parent_path = path.parent().ok_or_else(|| changed("文件父目录缺失。"))?;
        let parent = open_directory(parent_path, None)?;
        let opened = open_file(path)?.ok_or_else(|| changed("受理文件删除时目标已变化。"))?;
        require_identity(&opened, expected)?;
        let plan = FileDeletionPlan {
            original: path.into(),
            expected: expected.clone(),
            parent_identity: PersistedIdentity::from_file(parent.file()).map_err(unavailable)?,
            staging_path: parent_path.join(format!("{PREFIX}{}", Uuid::new_v4())),
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
            native::create_private_directory(&plan.staging_path).map_err(|cause| {
                changed(format!(
                    "未绑定的文件删除暂存目录已存在或无法创建，已保留：{}（{cause}）",
                    plan.staging_path.display()
                ))
            })?;
            parent.sync()?;
            let stage = open_directory(&plan.staging_path, None)?;
            private_stage(&stage, &parent)?;
            plan.staging_identity =
                Some(PersistedIdentity::from_file(stage.file()).map_err(unavailable)?);
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
            parent.sync()?;
            ownership.forget_file_deletion_plan(id, path, expected)?;
            hook(Point::PlanForgotten, &plan)?;
            return Ok(());
        }
        return Err(changed("文件删除暂存目录提前消失，未确认删除完成。"));
    }
    let staging = open_directory(&plan.staging_path, Some(&staging_identity))?;
    private_stage(&staging, &parent)?;

    if plan.state == FileDeletionState::RenameIntent {
        let source = open_file(&plan.original)?;
        let payload = open_file(&plan.staged_payload())?;
        match (source, payload) {
            (Some(source), None) => {
                require_identity(&source, &plan.expected)?;
                verify_directory(parent_path, &parent, &plan.parent_identity)?;
                verify_directory(&plan.staging_path, &staging, &staging_identity)?;
                hook(Point::BeforeRename, &plan)?;
                verify_directory(parent_path, &parent, &plan.parent_identity)?;
                verify_directory(&plan.staging_path, &staging, &staging_identity)?;
                rename_file(&plan.original, &plan.staged_payload(), &plan.expected)?;
                parent.sync()?;
                staging.sync()?;
                hook(Point::Renamed, &plan)?;
                let moved = match open_file(&plan.staged_payload()) {
                    Ok(Some(moved)) => moved,
                    _ => {
                        restore_foreign(&plan)?;
                        return Err(changed("暂存后的文件不是原受管文件，已保留或归还现场。"));
                    }
                };
                if require_identity(&moved, &plan.expected).is_err() {
                    restore_foreign(&plan)?;
                    return Err(changed("捕获到同名替换文件，已保留或归还现场。"));
                }
            }
            (None, Some(payload)) => {
                if require_identity(&payload, &plan.expected).is_err() {
                    restore_foreign(&plan)?;
                    return Err(changed("恢复删除时发现替换文件，已保留或归还现场。"));
                }
            }
            _ => {
                return Err(changed(
                    "文件删除原件与暂存状态不一致，未删除任何替换文件。",
                ))
            }
        }
        require_original_absent(&plan)?;
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
        require_original_absent(&plan)?;
        hook(Point::BeforeUnlink, &plan)?;
        require_original_absent(&plan)?;
        verify_directory(&plan.staging_path, &staging, &staging_identity)?;
        if let Some(payload) = open_file(&plan.staged_payload())? {
            require_identity(&payload, &plan.expected)?;
            drop(payload);
            remove_name(&plan.staged_payload(), &plan.expected, false)?;
            staging.sync()?;
            hook(Point::Unlinked, &plan)?;
        } else {
            plan.expected
                .absent_on_expected_volume(&plan.staged_payload())
                .map_err(unavailable)?;
        }
        require_original_absent(&plan)?;
        plan.state = FileDeletionState::Removed;
        ownership.save_file_deletion_plan(id, &plan)?;
        hook(Point::RemovedPersisted, &plan)?;
    }
    if plan.state != FileDeletionState::Removed {
        return Err(changed("文件删除进度无效。"));
    }
    require_original_absent(&plan)?;
    if open_file(&plan.staged_payload())?.is_some() {
        return Err(changed("已删除文件的暂存位置出现新内容，已保留。"));
    }
    verify_directory(parent_path, &parent, &plan.parent_identity)?;
    verify_directory(&plan.staging_path, &staging, &staging_identity)?;
    // Release our own anti-rename pin before requesting the empty-directory
    // disposition. The retained parent and full-ID native handle bind removal.
    drop(staging);
    remove_name(&plan.staging_path, &staging_identity, true)?;
    parent.sync()?;
    hook(Point::StageRemoved, &plan)?;
    ownership.forget_file_deletion_plan(id, path, expected)?;
    hook(Point::PlanForgotten, &plan)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
