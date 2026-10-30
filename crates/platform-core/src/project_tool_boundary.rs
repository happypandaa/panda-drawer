//! Global tools must outlive every independently removable project.
use crate::{web_project_ownership::WebProjectOwnership, ApiError};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

#[derive(Clone)]
pub(crate) struct ProjectToolBoundary {
    data: PathBuf,
    workspace: PathBuf,
}
impl ProjectToolBoundary {
    pub(crate) fn new(data: PathBuf, workspace: PathBuf) -> Self {
        Self { data, workspace }
    }
    pub(crate) fn require_path(&self, path: &Path) -> Result<(), ApiError> {
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(ApiError::new(
                "GLOBAL_TOOL_PATH_INVALID",
                "全局工具必须使用不含上级跳转的绝对路径。",
            ));
        }
        // Check the entered path too: a project-owned symlink to an external
        // tool still disappears with the project and must not enter settings.
        self.require_unowned(path)?;
        self.require_unowned(&fs::canonicalize(path)?)
    }
    pub(crate) fn require_dependency_path(&self, path: &Path) -> Result<(), ApiError> {
        self.require_path(&normalized(path)?)?;
        self.require_path(&fs::canonicalize(path)?)
    }
    fn require_unowned(&self, path: &Path) -> Result<(), ApiError> {
        if within(path, &self.data)
            || ["projects", "project-sources", "backups", ".platform"]
                .iter()
                .any(|name| within(path, &self.workspace.join(name)))
        {
            return Err(project_owned());
        }
        // Resolve each entered ancestor independently. This catches a link
        // inside a claimed source that points outside it, while still allowing
        // package-manager links whose entered and resolved paths are external.
        for ancestor in path.ancestors().skip(1) {
            let ancestor = fs::canonicalize(ancestor)?;
            crate::node_apps::development::require_export_location_unclaimed(&self.data, &ancestor)
                .map_err(|error| {
                    if error.code == "NODE_SOURCE_ALREADY_OWNED" {
                        project_owned()
                    } else {
                        error
                    }
                })?;
        }
        crate::node_project_ownership::NodeProjectOwnership::new(
            self.data.clone(),
            self.workspace.clone(),
        )
        .require_global_tool_unowned(path)?;
        WebProjectOwnership::new(self.data.clone(), self.workspace.clone())
            .require_global_tool_unowned(path)
    }
}
fn normalized(path: &Path) -> Result<PathBuf, ApiError> {
    if !path.is_absolute() {
        return Err(ApiError::new(
            "GLOBAL_TOOL_PATH_INVALID",
            "全局工具依赖路径无效。",
        ));
    }
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                clean.pop();
            }
            Component::CurDir => (),
            _ => clean.push(component.as_os_str()),
        }
    }
    Ok(clean)
}
fn within(candidate: &Path, root: &Path) -> bool {
    candidate.starts_with(root)
        || crate::file_identity::PersistedIdentity::capture(root)
            .ok()
            .is_some_and(|identity| {
                candidate
                    .ancestors()
                    .any(|ancestor| identity.matches_path(ancestor).unwrap_or(false))
            })
}
pub(crate) fn project_owned() -> ApiError {
    ApiError::new(
        "GLOBAL_TOOL_PROJECT_OWNED",
        "全局工具不能来自某个项目或平台受管材料，请选择独立安装的工具。",
    )
}
/// Reverse admission reads only current settings and neighboring tool paths;
/// it never executes a selected tool or walks project contents.
pub(crate) fn require_source_excludes_global_tools(
    data: &Path,
    source: &Path,
) -> Result<(), ApiError> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct NodeSettings {
        schema_version: u32,
        node_path: Option<PathBuf>,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct GitSettings {
        schema_version: u32,
        git_path: Option<PathBuf>,
    }
    fn settings<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, ApiError> {
        crate::projects::check_no_symlinks(path, true)?;
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() && metadata.len() <= 1024 * 1024 => {
                Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
            }
            Ok(_) => Err(ApiError::new(
                "GLOBAL_TOOL_SETTINGS_INVALID",
                "全局工具配置无法核验。",
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    fn resolved(path: &Path) -> Result<Option<PathBuf>, ApiError> {
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(ApiError::new(
                "GLOBAL_TOOL_SETTINGS_INVALID",
                "全局工具配置路径无效。",
            ));
        }
        match fs::canonicalize(path) {
            Ok(path) => Ok(Some(path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
    let mut paths = vec![];
    if let Some(settings) = settings::<NodeSettings>(&data.join("toolchain.json"))? {
        if settings.schema_version != 1 {
            return Err(ApiError::new(
                "GLOBAL_TOOL_SETTINGS_INVALID",
                "Node 工具配置版本无效。",
            ));
        }
        if let Some(node) = settings.node_path {
            let actual = resolved(&node)?;
            if let Some(actual) = actual.as_ref() {
                if let Some((entered, npm)) =
                    crate::environment::npm_entry_candidates(&node, actual)
                        .into_iter()
                        .filter_map(|path| {
                            fs::canonicalize(&path).ok().map(|actual| (path, actual))
                        })
                        .find(|(_, path)| path.is_file())
                {
                    paths.push(normalized(&entered)?);
                    paths.push(npm);
                }
                paths.push(actual.clone());
            }
            paths.push(node);
        }
    }
    if let Some(settings) = settings::<GitSettings>(&data.join("git-toolchain.json"))? {
        if settings.schema_version != 1 {
            return Err(ApiError::new(
                "GLOBAL_TOOL_SETTINGS_INVALID",
                "Git 工具配置版本无效。",
            ));
        }
        if let Some(git) = settings.git_path {
            if let Some(actual) = resolved(&git)? {
                paths.push(actual);
            }
            paths.push(git);
        }
    }
    if paths.iter().any(|path| within(path, source)) {
        return Err(project_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    fn fixture() -> (tempfile::TempDir, ProjectToolBoundary) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let data = root.join("data");
        let workspace = root.join("workspace");
        fs::create_dir(&data).unwrap();
        fs::create_dir(&workspace).unwrap();
        (temp, ProjectToolBoundary::new(data, workspace))
    }
    fn file(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "disposable tool path, never executed").unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn fixed_project_material_and_both_symlink_directions_cannot_be_global_tools() {
        let (temp, boundary) = fixture();
        let external = temp.path().canonicalize().unwrap().join("external-tool");
        file(&external);
        boundary.require_path(&external).unwrap();
        for root in [
            boundary.data.join("node-apps"),
            boundary.workspace.join("projects"),
            boundary.workspace.join("project-sources"),
            boundary.workspace.join(".platform"),
            boundary.workspace.join("backups"),
        ] {
            let owned = root.join("disposable/tool");
            file(&owned);
            assert_eq!(
                boundary.require_path(&owned).unwrap_err().code,
                "GLOBAL_TOOL_PROJECT_OWNED"
            );
        }
        let owned = boundary.workspace.join("projects/disposable/tool");
        let outside_alias = external.with_extension("outside-alias");
        symlink(&owned, &outside_alias).unwrap();
        assert_eq!(
            boundary.require_path(&outside_alias).unwrap_err().code,
            "GLOBAL_TOOL_PROJECT_OWNED"
        );
        let inside_alias = owned.with_extension("inside-alias");
        symlink(&external, &inside_alias).unwrap();
        assert_eq!(
            boundary.require_path(&inside_alias).unwrap_err().code,
            "GLOBAL_TOOL_PROJECT_OWNED"
        );
    }
    #[cfg(unix)]
    #[test]
    fn outside_generated_and_original_claimed_sources_cannot_supply_global_tools() {
        let (temp, boundary) = fixture();
        for generated in [true, false] {
            let id = uuid::Uuid::new_v4().to_string();
            let source = temp
                .path()
                .canonicalize()
                .unwrap()
                .join(format!("disposable-source-{generated}"));
            fs::create_dir(&source).unwrap();
            let owner = crate::node_project_ownership::NodeProjectOwnership::new(
                boundary.data.clone(),
                boundary.workspace.clone(),
            );
            owner.initialize(&id).unwrap();
            if generated {
                owner.record_generated_source(&id, &source).unwrap();
            }
            crate::node_apps::development::claim_source_after_verified_migration(
                &boundary.data,
                &boundary.workspace,
                &id,
                &source,
            )
            .unwrap();
            let tool = source.join("nested/tool");
            file(&tool);
            assert_eq!(
                boundary.require_path(&tool).unwrap_err().code,
                "GLOBAL_TOOL_PROJECT_OWNED"
            );
            let outside = temp
                .path()
                .canonicalize()
                .unwrap()
                .join(format!("external-tool-{generated}"));
            file(&outside);
            let inside_alias = source.join("outside-tool-alias");
            symlink(&outside, &inside_alias).unwrap();
            assert_eq!(
                boundary.require_path(&inside_alias).unwrap_err().code,
                "GLOBAL_TOOL_PROJECT_OWNED"
            );
            let outside_alias = outside.with_extension("source-tool-alias");
            symlink(&tool, &outside_alias).unwrap();
            assert_eq!(
                boundary.require_path(&outside_alias).unwrap_err().code,
                "GLOBAL_TOOL_PROJECT_OWNED"
            );
        }
    }
    #[test]
    fn retained_web_workspace_copies_cannot_supply_global_tools() {
        let (temp, boundary) = fixture();
        let old = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("previous-workspace");
        let id = format!("prj_{}", uuid::Uuid::new_v4().simple());
        let old_root = old.join("projects/disposable-project");
        let new_root = boundary.workspace.join("projects/disposable-project");
        file(&old_root.join("tool"));
        file(&new_root.join("tool"));
        let owner = WebProjectOwnership::new(boundary.data.clone(), old.clone());
        owner.initialize(&id).unwrap();
        owner.record_project_root(&id, &old_root).unwrap();
        WebProjectOwnership::rebind_after_migration(&boundary.data, &old, &boundary.workspace)
            .unwrap();
        assert_eq!(
            boundary
                .require_path(&old_root.join("tool"))
                .unwrap_err()
                .code,
            "GLOBAL_TOOL_PROJECT_OWNED"
        );
    }
    #[test]
    fn retained_node_task_copies_cannot_supply_global_tools() {
        let (temp, boundary) = fixture();
        let old = temp
            .path()
            .canonicalize()
            .unwrap()
            .join("previous-node-workspace");
        let id = uuid::Uuid::new_v4().to_string();
        let task = format!("task_{}", uuid::Uuid::new_v4().simple());
        let old_task = old.join(".platform/tasks").join(&task);
        let new_task = boundary.workspace.join(".platform/tasks").join(&task);
        file(&old_task.join("tool"));
        file(&new_task.join("tool"));
        let owner = crate::node_project_ownership::NodeProjectOwnership::new(
            boundary.data.clone(),
            old.clone(),
        );
        owner.initialize(&id).unwrap();
        owner.record_task(&id, &task).unwrap();
        crate::node_project_ownership::NodeProjectOwnership::rebind_after_migration(
            &boundary.data,
            &old,
            &boundary.workspace,
        )
        .unwrap();
        assert_eq!(
            boundary
                .require_path(&old_task.join("tool"))
                .unwrap_err()
                .code,
            "GLOBAL_TOOL_PROJECT_OWNED"
        );
    }
    #[cfg(unix)]
    #[test]
    fn source_admission_checks_configured_raw_resolved_and_npm_paths_without_probing() {
        let (temp, boundary) = fixture();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("disposable-source");
        let outside = root.join("external-tools/bin/node");
        file(&outside);
        let inside = source.join("tool");
        file(&inside);
        let outside_alias = root.join("tool-alias");
        symlink(&inside, &outside_alias).unwrap();
        let inside_alias = source.join("outside-alias");
        symlink(&outside, &inside_alias).unwrap();
        for (name, key, selected) in [
            ("toolchain.json", "nodePath", &inside_alias),
            ("toolchain.json", "nodePath", &outside_alias),
            ("git-toolchain.json", "gitPath", &outside_alias),
        ] {
            let settings = boundary.data.join(name);
            let bytes =
                serde_json::to_vec(&serde_json::json!({"schemaVersion":1,(key):selected})).unwrap();
            fs::write(&settings, &bytes).unwrap();
            assert_eq!(
                require_source_excludes_global_tools(&boundary.data, &source)
                    .unwrap_err()
                    .code,
                "GLOBAL_TOOL_PROJECT_OWNED"
            );
            assert_eq!(fs::read(&settings).unwrap(), bytes);
            fs::remove_file(settings).unwrap();
        }
        // Node is outside; its adjacent npm entry resolves into the source.
        symlink(&inside, outside.parent().unwrap().join("npm")).unwrap();
        fs::write(
            boundary.data.join("toolchain.json"),
            serde_json::to_vec(&serde_json::json!({"schemaVersion":1,"nodePath":outside})).unwrap(),
        )
        .unwrap();
        assert_eq!(
            require_source_excludes_global_tools(&boundary.data, &source)
                .unwrap_err()
                .code,
            "GLOBAL_TOOL_PROJECT_OWNED"
        );
        let id = uuid::Uuid::new_v4().to_string();
        crate::node_project_ownership::NodeProjectOwnership::new(
            boundary.data.clone(),
            boundary.workspace.clone(),
        )
        .initialize(&id)
        .unwrap();
        assert_eq!(
            crate::node_apps::development::claim_source_after_verified_migration(
                &boundary.data,
                &boundary.workspace,
                &id,
                &source
            )
            .unwrap_err()
            .code,
            "GLOBAL_TOOL_PROJECT_OWNED"
        );
        // The entered npm link is inside a future source while its target is
        // external. Claiming that source must also be refused.
        fs::remove_file(outside.parent().unwrap().join("npm")).unwrap();
        let npm_source = root.join("external-tools/lib");
        let npm_entry = npm_source.join("node_modules/npm/bin/npm-cli.js");
        fs::create_dir_all(npm_entry.parent().unwrap()).unwrap();
        let external_npm = root.join("external-npm.js");
        file(&external_npm);
        symlink(external_npm, npm_entry).unwrap();
        assert_eq!(
            require_source_excludes_global_tools(&boundary.data, &npm_source)
                .unwrap_err()
                .code,
            "GLOBAL_TOOL_PROJECT_OWNED"
        );
    }
}
