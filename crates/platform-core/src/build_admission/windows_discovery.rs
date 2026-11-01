use super::*;
use platform_native::windows::files::{self as native, FileIdentity};
#[derive(Debug)]
struct Repository {
    git_dir: PathBuf,
    // Native repository identity is retained by discovery, never represented
    // with the truncated device/inode pair used by old Unix-only transactions.
    _identity: FileIdentity,
}

struct Marker {
    path: PathBuf,
    identity: Option<FileIdentity>,
    permitted: bool,
}

struct DirectoryObservation {
    path: PathBuf,
    identity: FileIdentity,
}

fn unavailable() -> ApiError {
    ApiError::new(
        "BUILD_GIT_CHECK_UNAVAILABLE",
        "无法安全确认项目 Git 索引没有冲突；未执行 Git 修复或改用另一份工具，请检查仓库及 Git 工具设置。",
    )
}

fn changed() -> ApiError {
    ApiError::new(
        "BUILD_GIT_STATE_CHANGED",
        "构建期间 Git 仓库、索引或配置发生变化；未发布候选产物，请结束 Git 操作后重新构建。",
    )
}

fn observation_error(error: impl std::fmt::Display) -> ApiError {
    unavailable().detail("cause", error.to_string())
}

fn marker(path: &Path, permitted: bool, markers: &mut Vec<Marker>) -> Result<(), ApiError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(observation_error(error)),
    };
    let identity = if metadata.is_dir() && !platform_native::filesystem::is_reparse(&metadata) {
        let opened = native::open_no_reparse(path).map_err(observation_error)?;
        Some(opened.identity)
    } else {
        None
    };
    // Windows spelling can differ between the fixed '.git' probe and read_dir
    // ('.GIT'). Deduplicate their actual object, without following a gitfile/link.
    if let Some(identity) = &identity {
        if markers.iter().any(|item| {
            item.identity
                .as_ref()
                .is_some_and(|previous| previous.same_object(identity))
        }) {
            return Ok(());
        }
    } else if markers.iter().any(|item| item.path == path) {
        return Ok(());
    }
    markers.push(Marker {
        path: path.to_owned(),
        identity,
        permitted,
    });
    Ok(())
}

fn protected(relative: &Path, manifest: &ProjectManifest) -> Result<bool, ApiError> {
    // The Mac rule consumes project-relative '/' names. Native Windows paths
    // must be converted component by component before those same exclusions.
    let names = relative
        .components()
        .map(|part| match part {
            Component::Normal(name) => name
                .to_str()
                .map(str::to_ascii_lowercase)
                .ok_or_else(unavailable),
            _ => Err(unavailable()),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let folded = names.join("/");
    let path = Path::new(&folded);
    Ok(path.starts_with("data")
        || path.starts_with(".platform")
        || path.starts_with(manifest.output_dir.to_ascii_lowercase())
        || names.iter().any(|name| {
            matches!(
                name.as_str(),
                ".git"
                    | "node_modules"
                    | ".cache"
                    | ".vite"
                    | ".hg"
                    | ".svn"
                    | ".ds_store"
                    | "credentials"
                    | "configuration"
                    | ".ssh"
                    | ".npmrc"
                    | ".netrc"
                    | "id_rsa"
                    | "id_ed25519"
                    | "id_ecdsa"
                    | "id_dsa"
            ) || name.starts_with(".env")
                || [".pem", ".key", ".p12", ".pfx"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
        }))
}

fn discover(root: &Path, manifest: &ProjectManifest) -> Result<Option<Repository>, ApiError> {
    discover_with(root, manifest, |directory| {
        #[cfg(test)]
        TEST_SCAN_OBSERVER.with(|observer| {
            if let Some(observer) = observer.borrow_mut().as_mut() {
                observer(directory);
            }
        });
        #[cfg(not(test))]
        let _ = directory;
        Ok(())
    })
}

#[cfg(test)]
type ScanObserver = Box<dyn FnMut(&Path)>;
#[cfg(test)]
thread_local! {
    static TEST_SCAN_OBSERVER: std::cell::RefCell<Option<ScanObserver>> = const { std::cell::RefCell::new(None) };
}

fn discover_with(
    root: &Path,
    manifest: &ProjectManifest,
    mut after_directory: impl FnMut(&Path) -> Result<(), ApiError>,
) -> Result<Option<Repository>, ApiError> {
    let project = native::pin_directory(root).map_err(observation_error)?;
    let (root, _project_scope) =
        native::directory_scope(&project.file).map_err(observation_error)?;
    let source = checked_relative(&root, &manifest.source_dir, false).map_err(observation_error)?;
    let mut fixed = vec![(root.join(".git"), true)];
    fixed.extend(
        root.ancestors()
            .skip(1)
            .map(|parent| (parent.join(".git"), false)),
    );
    for ancestor in source.ancestors().skip(1).take_while(|path| *path != root) {
        if !ancestor.starts_with(&root) {
            return Err(unavailable());
        }
        fixed.push((ancestor.join(".git"), false));
    }
    let mut markers = Vec::new();
    for (path, permitted) in &fixed {
        marker(path, *permitted, &mut markers)?;
    }
    let mut pending = vec![source.clone()];
    let mut observed = Vec::new();
    while let Some(directory) = pending.pop() {
        let pinned = native::pin_directory(&directory).map_err(observation_error)?;
        let identity = native::identity(&pinned.file).map_err(observation_error)?;
        for entry in fs::read_dir(&directory).map_err(observation_error)? {
            let entry = entry.map_err(observation_error)?;
            let path = entry.path();
            if entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(".git")
            {
                marker(
                    &path,
                    directory == root || directory == source,
                    &mut markers,
                )?;
                continue;
            }
            if protected(
                path.strip_prefix(&root).map_err(observation_error)?,
                manifest,
            )? {
                continue;
            }
            let metadata = fs::symlink_metadata(&path).map_err(observation_error)?;
            // As with the Mac marker scan's symlink leaves, never follow a
            // source link into a different tree. Build input admission retains
            // its own separate rules for copying project source files.
            if metadata.is_dir() && !platform_native::filesystem::is_reparse(&metadata) {
                pending.push(path);
            }
        }
        after_directory(&directory)?;
        observed.push(DirectoryObservation {
            path: directory,
            identity,
        });
    }
    for directory in observed {
        let actual = native::pin_directory(&directory.path).map_err(observation_error)?;
        if !directory
            .identity
            .same_object(&native::identity(&actual.file).map_err(observation_error)?)
        {
            return Err(changed());
        }
    }
    for (path, permitted) in fixed {
        marker(&path, permitted, &mut markers)?;
    }
    if markers.is_empty() {
        return Ok(None);
    }
    if markers.len() != 1 || !markers[0].permitted || markers[0].identity.is_none() {
        return Err(ApiError::new(
            "BUILD_GIT_CHECK_UNAVAILABLE",
            "发现祖先、嵌套、多个仓库或非普通 .git 目录；本步无法安全核对冲突，未迁移或修改仓库。",
        ));
    }
    let repository = markers.remove(0);
    Ok(Some(Repository {
        git_dir: repository.path,
        _identity: repository.identity.unwrap(),
    }))
}

pub(super) fn discover_project(
    project: &ProjectHandle,
) -> Result<Option<super::Repository>, ApiError> {
    discover(&project.root, &project.manifest).map(|found| {
        found.map(|repo| super::Repository {
            git_dir: repo.git_dir,
            identity: format!(
                "{}:{:02x?}",
                repo._identity.volume_serial, repo._identity.file_id
            ),
        })
    })
}
#[cfg(test)]
impl super::BuildGitGuard {
    // Existing synchronous fixtures contain no valid HEAD/config. This helper
    // exercises discovery only; actual repository admission uses the async API.
    fn capture_at(root: &Path, manifest: &ProjectManifest) -> Result<Self, ApiError> {
        if let Some(repository) = discover(root, manifest)? {
            return Err(unavailable()
                .detail("platform", "windows")
                .detail("capability", "build-git-index-check")
                .detail("repository", repository.git_dir.display().to_string())
                .outcome("not_started"));
        }
        Ok(Self {
            repository: None,
            tools: None,
            digest: None,
        })
    }
    fn recheck_at(&self, root: &Path, manifest: &ProjectManifest) -> Result<(), ApiError> {
        if discover(root, manifest)?.is_some() {
            return Err(changed());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "windows_discovery_tests.rs"]
mod tests;
