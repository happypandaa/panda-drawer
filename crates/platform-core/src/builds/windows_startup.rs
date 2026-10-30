//! Temporary, read-only admission for unfinished Windows task/publication
//! recovery. No retained task, provenance or publication is adopted or changed.
use crate::ApiError;
use platform_native::windows::files::{self as native, PinnedDirectory};
use std::{
    fs,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, ApiError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    Tasks,
    Publications,
}
impl Scope {
    fn components(self) -> &'static [&'static str] {
        match self {
            Self::Tasks => &[".platform", "tasks"],
            Self::Publications => &[".platform", "project-state", "builds"],
        }
    }
    fn capability(self) -> &'static str {
        match self {
            Self::Tasks => "task-startup-recovery",
            Self::Publications => "build-publication-recovery",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Observation {
    Missing,
    Empty,
}

struct Bound {
    path: PathBuf,
    directory: PinnedDirectory,
}
fn unknown(scope: Scope, path: &Path, cause: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        "BUILD_STARTUP_INVENTORY_UNCONFIRMED",
        "Windows 启动恢复目录尚未核实，现有材料已保留。",
    )
    .detail("platform", "windows")
    .detail("capability", scope.capability())
    .detail("inventory", "unconfirmed")
    .detail("path", path.to_string_lossy().into_owned())
    .detail("cause", cause.to_string())
    .outcome("unknown")
}
fn retained(scope: Scope, path: &Path) -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 任务和构建发布的启动恢复尚未完成，现有材料已保留。",
    )
    .detail("platform", "windows")
    .detail("capability", scope.capability())
    .detail("inventory", "retained")
    .detail("path", path.to_string_lossy().into_owned())
    .outcome("not_started")
}
fn verify_bound(scope: Scope, bound: &[Bound]) -> Result<()> {
    for current in bound {
        let expected = native::identity(&current.directory.file)
            .map_err(|error| unknown(scope, &current.path, error))?;
        let actual = native::open_no_reparse(&current.path)
            .map_err(|error| unknown(scope, &current.path, error))?;
        if !expected.directory || !expected.same_object(&actual.identity) {
            return Err(unknown(scope, &current.path, "目录身份已变化。"));
        }
    }
    Ok(())
}
fn require_empty(scope: Scope, path: &Path) -> Result<()> {
    match fs::read_dir(path)
        .map_err(|error| unknown(scope, path, error))?
        .next()
    {
        None => Ok(()),
        Some(Ok(_)) => Err(retained(scope, path)),
        Some(Err(error)) => Err(unknown(scope, path, error)),
    }
}

fn inspect_with(
    workspace: &Path,
    scope: Scope,
    mut observe: impl FnMut(Observation, &Path) -> Result<()>,
) -> Result<Observation> {
    let selected =
        native::pin_directory(workspace).map_err(|error| unknown(scope, workspace, error))?;
    let (path, directory) = native::directory_scope(&selected.file)
        .map_err(|error| unknown(scope, workspace, error))?;
    let mut bound = vec![Bound { path, directory }];
    for name in scope.components() {
        let path = bound.last().unwrap().path.join(name);
        let directory = match native::pin_directory(&path) {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                observe(Observation::Missing, &path)?;
                // All existing ancestors remain held; absence names only the
                // first missing fixed component, never an arbitrary I/O error.
                match fs::symlink_metadata(&path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        verify_bound(scope, &bound)?;
                        return Ok(Observation::Missing);
                    }
                    Err(error) => return Err(unknown(scope, &path, error)),
                    Ok(_) => return Err(unknown(scope, &path, "目录在检查期间出现。")),
                }
            }
            Err(error) => return Err(unknown(scope, &path, error)),
        };
        bound.push(Bound { path, directory });
    }
    let leaf = bound.last().unwrap();
    let before = leaf
        .directory
        .file
        .metadata()
        .map_err(|error| unknown(scope, &leaf.path, error))?
        .last_write_time();
    require_empty(scope, &leaf.path)?;
    observe(Observation::Empty, &leaf.path)?;
    require_empty(scope, &leaf.path)?;
    let after = leaf
        .directory
        .file
        .metadata()
        .map_err(|error| unknown(scope, &leaf.path, error))?
        .last_write_time();
    verify_bound(scope, &bound)?;
    if before != after {
        return Err(unknown(scope, &leaf.path, "目录在检查期间变化。"));
    }
    Ok(Observation::Empty)
}

pub(super) fn require_supported_inventory(workspace: &Path) -> Result<()> {
    for scope in [Scope::Tasks, Scope::Publications] {
        inspect_with(workspace, scope, |_, _| Ok(()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-build-startup-inventory-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        (temporary, root)
    }
    fn leaf(root: &Path, scope: Scope) -> PathBuf {
        scope
            .components()
            .iter()
            .fold(root.to_path_buf(), |path, part| path.join(part))
    }
    #[test]
    fn genuinely_missing_or_empty_fixed_scopes_preserve_unrelated_siblings() {
        let (_temporary, root) = fixture();
        fs::write(root.join("retained"), b"disposable unrelated bytes").unwrap();
        require_supported_inventory(&root).unwrap();
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        for scope in [Scope::Tasks, Scope::Publications] {
            let path = leaf(&root, scope);
            fs::create_dir_all(&path).unwrap();
            assert_eq!(
                inspect_with(&root, scope, |_, _| Ok(())).unwrap(),
                Observation::Empty
            );
        }
        fs::write(
            root.join(".platform/project-state/retained-sibling.json"),
            b"unrelated state",
        )
        .unwrap();
        require_supported_inventory(&root).unwrap();
        assert_eq!(
            fs::read(root.join("retained")).unwrap(),
            b"disposable unrelated bytes"
        );
        assert_eq!(
            fs::read(root.join(".platform/project-state/retained-sibling.json")).unwrap(),
            b"unrelated state"
        );
    }
    #[test]
    fn retained_final_nonfinal_unknown_and_publication_material_all_stay_untouched() {
        for scope in [Scope::Tasks, Scope::Publications] {
            for name in [
                "task_final",
                "task_nonfinal",
                "unknown",
                "prj_disposable.publication.json",
            ] {
                let (_temporary, root) = fixture();
                let path = leaf(&root, scope);
                fs::create_dir_all(&path).unwrap();
                let candidate = path.join(name);
                fs::write(&candidate, b"disposable retained startup evidence").unwrap();
                let error = inspect_with(&root, scope, |_, _| Ok(())).unwrap_err();
                assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
                assert_eq!(error.details["operationOutcome"], "not_started");
                assert_eq!(error.details["capability"], scope.capability());
                assert_eq!(
                    fs::read(&candidate).unwrap(),
                    b"disposable retained startup evidence"
                );
                assert_eq!(fs::read_dir(&path).unwrap().count(), 1);
            }
        }
    }
    #[test]
    fn newly_appearing_roots_or_children_cannot_be_reported_missing_or_empty() {
        for observation in [Observation::Missing, Observation::Empty] {
            let (_temporary, root) = fixture();
            if observation == Observation::Empty {
                fs::create_dir_all(leaf(&root, Scope::Tasks)).unwrap();
            }
            let mut retained_path = None;
            let error = inspect_with(&root, Scope::Tasks, |actual, path| {
                assert_eq!(actual, observation);
                if observation == Observation::Missing {
                    fs::create_dir(path).unwrap();
                }
                let retained = path.join("appeared");
                fs::write(&retained, b"disposable new evidence").unwrap();
                retained_path = Some(retained);
                Ok(())
            })
            .unwrap_err();
            assert!(matches!(
                error.code.as_str(),
                "BUILD_STARTUP_INVENTORY_UNCONFIRMED" | "PLATFORM_FEATURE_UNAVAILABLE"
            ));
            assert_eq!(
                fs::read(retained_path.unwrap()).unwrap(),
                b"disposable new evidence"
            );
        }
    }
    #[test]
    fn a_real_parent_junction_is_rejected_without_inspecting_or_changing_its_target() {
        let (_temporary, root) = fixture();
        let target = root.join("disposable-foreign-platform");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("retained"), b"disposable junction bytes").unwrap();
        let link = root.join(".platform");
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(status.status.success());
        assert_eq!(
            require_supported_inventory(&root).unwrap_err().code,
            "BUILD_STARTUP_INVENTORY_UNCONFIRMED"
        );
        assert_eq!(
            fs::read(target.join("retained")).unwrap(),
            b"disposable junction bytes"
        );
        assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
        fs::remove_dir(link).unwrap();
    }
    #[test]
    fn raw_mixed_separators_resolve_to_actual_retained_task_material() {
        let (_temporary, root) = fixture();
        let actual = root.join("nested/workspace");
        fs::create_dir_all(leaf(&actual, Scope::Tasks)).unwrap();
        fs::write(
            leaf(&actual, Scope::Tasks).join("retained"),
            b"actual retained task",
        )
        .unwrap();
        let mut raw = root.as_os_str().to_os_string();
        raw.push(r"\nested/workspace");
        let error = require_supported_inventory(&PathBuf::from(raw)).unwrap_err();
        assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
        assert_eq!(
            fs::read(leaf(&actual, Scope::Tasks).join("retained")).unwrap(),
            b"actual retained task"
        );
    }
}
