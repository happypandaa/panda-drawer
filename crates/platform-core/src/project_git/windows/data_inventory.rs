//! Read-only admission while Windows Git-write recovery is unavailable. An
//! observed empty scope is not a fabricated recovery result for retained work.
use crate::ApiError;
use platform_native::windows::files::{self as native, PinnedDirectory};
use std::{
    fs,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, ApiError>;
const COMPONENTS: &[&str] = &[".platform", "project-state", "git"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observation {
    Missing,
    Empty,
}

struct Bound {
    path: PathBuf,
    directory: PinnedDirectory,
}

fn unconfirmed(path: &Path, cause: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        "PROJECT_GIT_DATA_INVENTORY_UNCONFIRMED",
        "Windows Git 数据保存恢复目录尚未核实，现有材料已保留。",
    )
    .detail("platform", "windows")
    .detail("capability", "project-git-data-recovery")
    .detail("inventory", "unconfirmed")
    .detail("path", path.to_string_lossy().into_owned())
    .detail("cause", cause.to_string())
    .outcome("unknown")
}

fn retained(path: &Path) -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows Git 数据保存恢复尚未完成，已保留现有记录，未开始新操作。",
    )
    .detail("platform", "windows")
    .detail("capability", "project-git-data-recovery")
    .detail("inventory", "retained")
    .detail("path", path.to_string_lossy().into_owned())
    .outcome("not_started")
}

fn verify(bound: &[Bound]) -> Result<()> {
    for item in bound {
        let expected = native::identity(&item.directory.file)
            .map_err(|error| unconfirmed(&item.path, error))?;
        let actual =
            native::open_no_reparse(&item.path).map_err(|error| unconfirmed(&item.path, error))?;
        if !expected.directory || !expected.same_object(&actual.identity) {
            return Err(unconfirmed(&item.path, "目录身份已变化。"));
        }
    }
    Ok(())
}

fn require_empty(path: &Path) -> Result<()> {
    match fs::read_dir(path)
        .map_err(|error| unconfirmed(path, error))?
        .next()
    {
        None => Ok(()),
        Some(Ok(_)) => Err(retained(path)),
        Some(Err(error)) => Err(unconfirmed(path, error)),
    }
}

fn observe_empty(
    leaf: &Bound,
    observe: &mut impl FnMut(Observation, &Path) -> Result<()>,
) -> Result<()> {
    let before = leaf
        .directory
        .file
        .metadata()
        .map_err(|error| unconfirmed(&leaf.path, error))?
        .last_write_time();
    require_empty(&leaf.path)?;
    observe(Observation::Empty, &leaf.path)?;
    require_empty(&leaf.path)?;
    let after = leaf
        .directory
        .file
        .metadata()
        .map_err(|error| unconfirmed(&leaf.path, error))?
        .last_write_time();
    if before != after {
        return Err(unconfirmed(&leaf.path, "目录在检查期间变化。"));
    }
    Ok(())
}

fn inspect_with(
    workspace: &Path,
    project: Option<&str>,
    mut observe: impl FnMut(Observation, &Path) -> Result<()>,
) -> Result<()> {
    if project.is_some_and(|id| !crate::projects::valid_id(id)) {
        return Err(ApiError::new("INVALID_PROJECT_ID", "Invalid project ID"));
    }
    // Match the existing no-reparse observation policy, without requiring a
    // private DACL on any parent or reading unrelated project/state contents.
    let selected =
        native::pin_directory(workspace).map_err(|error| unconfirmed(workspace, error))?;
    let (path, directory) =
        native::directory_scope(&selected.file).map_err(|error| unconfirmed(workspace, error))?;
    let mut bound = vec![Bound { path, directory }];
    for name in COMPONENTS.iter().copied().chain(project) {
        let path = bound.last().unwrap().path.join(name);
        let directory = match native::pin_directory(&path) {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                observe(Observation::Missing, &path)?;
                match fs::symlink_metadata(&path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        verify(&bound)?;
                        return Ok(());
                    }
                    Err(error) => return Err(unconfirmed(&path, error)),
                    Ok(_) => return Err(unconfirmed(&path, "目录在检查期间出现。")),
                }
            }
            Err(error) => return Err(unconfirmed(&path, error)),
        };
        bound.push(Bound { path, directory });
    }
    let leaf = bound.last().unwrap();
    if project.is_some() {
        observe_empty(leaf, &mut observe)?;
    } else {
        let before = leaf
            .directory
            .file
            .metadata()
            .map_err(|error| unconfirmed(&leaf.path, error))?
            .last_write_time();
        let names = project_names(&leaf.path)?;
        let mut children = Vec::new();
        for name in &names {
            let path = leaf.path.join(name);
            let directory =
                native::pin_directory(&path).map_err(|error| unconfirmed(&path, error))?;
            let child = Bound { path, directory };
            observe_empty(&child, &mut observe)?;
            children.push(child);
        }
        // An empty workspace scope still needs the same second observation as
        // an empty project scope. Existing empty project directories are legal.
        if names.is_empty() {
            observe(Observation::Empty, &leaf.path)?;
        }
        if project_names(&leaf.path)? != names
            || leaf
                .directory
                .file
                .metadata()
                .map_err(|error| unconfirmed(&leaf.path, error))?
                .last_write_time()
                != before
        {
            return Err(unconfirmed(&leaf.path, "项目目录列表在检查期间变化。"));
        }
        for child in &children {
            require_empty(&child.path)?;
        }
        verify(&children)?;
    }
    verify(&bound)
}

fn project_names(path: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(path).map_err(|error| unconfirmed(path, error))? {
        if names.len() >= 1000 {
            return Err(retained(path));
        }
        let entry = entry.map_err(|error| unconfirmed(path, error))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| retained(&entry.path()))?;
        if !crate::projects::valid_id(&name) {
            return Err(retained(&entry.path()));
        }
        names.push(name);
    }
    names.sort();
    Ok(names)
}

pub(super) fn check_project(workspace: &Path, id: &str) -> Result<()> {
    inspect_with(workspace, Some(id), |_, _| Ok(()))
}

pub(super) fn check_workspace(workspace: &Path) -> Result<()> {
    inspect_with(workspace, None, |_, _| Ok(()))
}

#[cfg(test)]
#[path = "data_inventory_tests.rs"]
mod tests;
