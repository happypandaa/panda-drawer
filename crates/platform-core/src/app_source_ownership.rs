//! Source admission is identity checked and never grants ownership from a path.
use crate::{ApiError, RuntimePaths};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
static ADMISSION: OnceLock<Mutex<()>> = OnceLock::new();
type Result<T> = std::result::Result<T, ApiError>;
const WORKSPACE_SOURCE_CONTAINERS: [&str; 2] = ["projects", "project-sources"];

pub(crate) fn is_workspace_app_folder(workspace: &Path, source: &Path) -> bool {
    source.parent().is_some_and(|parent| {
        WORKSPACE_SOURCE_CONTAINERS
            .iter()
            .any(|name| parent == workspace.join(name))
    })
}

pub(crate) fn owned_material_paths(paths: &RuntimePaths) -> Result<BTreeSet<PathBuf>> {
    let data = paths.data_root();
    let workspace = paths.workspace_root();
    let imports =
        crate::node_project_ownership::NodeProjectOwnership::new(data.into(), workspace.into());
    let apps =
        crate::web_project_ownership::WebProjectOwnership::new(data.into(), workspace.into());
    Ok(imports
        .all_owned_scopes()?
        .into_iter()
        .chain(apps.all_owned_scopes()?)
        .flat_map(|(_, paths)| paths)
        .collect())
}

pub(crate) fn workspace_sources_to_keep(
    workspace: &Path,
    owned: &BTreeSet<PathBuf>,
    preserved: &[PathBuf],
) -> Result<BTreeSet<PathBuf>> {
    let mut retained = BTreeSet::new();
    for name in WORKSPACE_SOURCE_CONTAINERS {
        let root = workspace.join(name);
        crate::projects::check_no_symlinks(&root, true)?;
        if !root.try_exists()? {
            continue;
        }
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if !kind.is_dir() && !kind.is_symlink() {
                continue;
            }
            let folder = entry.path();
            if preserved.contains(&folder) || !owned.iter().any(|path| folder.starts_with(path)) {
                retained.insert(folder);
            }
        }
    }
    Ok(retained)
}
pub(crate) fn with_source_ownership_admission<T>(work: impl FnOnce() -> Result<T>) -> Result<T> {
    let _lock = ADMISSION
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| ApiError::new("APP_SOURCE_BUSY", "源码登记锁不可用。"))?;
    work()
}
pub(crate) fn require_new_web_project_location(data: &Path, source: &Path) -> Result<()> {
    crate::projects::check_no_symlinks(source, true)?;
    crate::project_tool_boundary::require_source_excludes_global_tools(data, source)?;
    crate::web_project_ownership::WebProjectOwnership::require_source_unoccupied(data, source)
}
pub(crate) fn require_delivery_location_unclaimed(data: &Path, source: &Path) -> Result<()> {
    // A delivery parent may contain an unrelated application's sibling. Only
    // an actual owned ancestor excludes this location; reverse overlap is a
    // source-admission rule and would incorrectly reject common ancestors.
    crate::web_project_ownership::WebProjectOwnership::new(data.into(), Default::default())
        .require_global_tool_unowned(source)
}

#[cfg(test)]
pub(crate) fn claim_source_after_verified_migration(
    data: &Path,
    workspace: &Path,
    id: &str,
    source: &Path,
) -> Result<()> {
    with_source_ownership_admission(|| {
        require_new_web_project_location(data, source)?;
        let owner =
            crate::web_project_ownership::WebProjectOwnership::new(data.into(), workspace.into());
        owner.initialize(id)?;
        owner.record_user_root(id, source)
    })
}
