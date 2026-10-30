//! Disposable copies for trusted builds, not a sandbox for arbitrary scripts.
//! Only sourceDir is copied. Data, published outputs, Git metadata and existing
//! node_modules are never mounted as candidate inputs. Scripts can still access
//! absolute paths allowed to the platform process; the existing publication
//! guard remains responsible for detecting writes to the live output.
use super::{
    atomic_json, check_no_symlinks, checked_relative, collect_files, digest_project_inputs,
    ensure_dir, ensure_unchanged, excluded, ApiError, ProjectHandle, ProjectManifest,
};
use serde_json::{json, Value};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, OpenOptions},
    io,
    path::{Component, Path, PathBuf},
};
use tempfile::TempDir;
use tokio::process::Command;

pub(super) struct BuildCandidate {
    directory: Option<TempDir>,
    source: PathBuf,
    record: PathBuf,
    relative: String,
}
impl BuildCandidate {
    pub(super) fn prepare(
        project: &ProjectHandle,
        task_root: &Path,
        baseline: &str,
    ) -> Result<Self, ApiError> {
        check_no_symlinks(task_root, false)?;
        let directory = tempfile::Builder::new()
            .prefix("candidate-v1-")
            .tempdir_in(task_root)?;
        let relative = directory
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let source = checked_relative(directory.path(), &project.manifest.source_dir, true)?;
        let candidate = Self {
            directory: Some(directory),
            source,
            record: task_root.join("candidate-v1.json"),
            relative,
        };
        candidate.record("retained")?;
        let original = checked_relative(&project.root, &project.manifest.source_dir, false)?;
        copy_source(&original, candidate.source())?;
        ensure_unchanged(project, baseline)?;
        candidate.ensure_unchanged(&project.manifest, baseline)?;
        validate_local_dependencies(candidate.source())?;
        Ok(candidate)
    }
    pub(super) fn source(&self) -> &Path {
        &self.source
    }
    pub(super) fn ensure_unchanged(
        &self,
        manifest: &ProjectManifest,
        baseline: &str,
    ) -> Result<(), ApiError> {
        if digest_project_inputs(self.directory.as_ref().unwrap().path(), manifest)? != baseline {
            return Err(ApiError::new(
                "BUILD_INPUT_CHANGED",
                "Candidate inputs changed during copy, installation or build; the previous webpage remains active",
            ));
        }
        Ok(())
    }
    pub(super) fn validate_installed_links(&self) -> Result<(), ApiError> {
        fn visit(root: &Path, directory: &Path) -> Result<(), ApiError> {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let path = entry.path();
                let kind = entry.file_type()?;
                if kind.is_symlink() {
                    // npm's workspace and .bin links are expected, but an installed
                    // link must stay inside this candidate. Source links were rejected
                    // before installation; no source link is followed while copying.
                    let target = fs::canonicalize(&path).map_err(|_| {
                        unsupported("An installed dependency link cannot be resolved")
                    })?;
                    if !target.starts_with(root) {
                        return Err(unsupported(
                            "An installed dependency link points outside the build candidate",
                        ));
                    }
                } else if kind.is_dir() {
                    visit(root, &path)?;
                } else if !kind.is_file() {
                    return Err(unsupported("Installed dependencies contain a special file"));
                }
            }
            Ok(())
        }
        let root = fs::canonicalize(self.directory.as_ref().unwrap().path())?;
        visit(&root, &self.source.join("node_modules"))
    }
    fn record(&self, state: &str) -> Result<(), ApiError> {
        atomic_json(
            &self.record,
            &json!({"schemaVersion":1,"kind":"disposable-build-candidate","path":self.relative,"state":state}),
        )
    }
}
impl Drop for BuildCandidate {
    fn drop(&mut self) {
        // Normal command completion stops its process group before this guard is
        // released. On process crash the marker and directory deliberately remain:
        // startup marks the task interrupted and never deletes that evidence.
        if let Some(directory) = self.directory.take() {
            if directory.close().is_ok() {
                let _ = self.record("removed");
            }
        }
    }
}

fn same_file(before: &fs::Metadata, after: &fs::Metadata) -> bool {
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.mode() == after.mode()
}
fn copy_source(source: &Path, target: &Path) -> Result<(), ApiError> {
    check_no_symlinks(source, false)?;
    ensure_dir(target)?;
    let mut entries = fs::read_dir(source)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            ApiError::new("BUILD_INPUT_INVALID", "Non UTF-8 filenames are unsupported")
        })?;
        if excluded(name) {
            continue;
        }
        let from = entry.path();
        let to = target.join(name);
        let metadata = fs::symlink_metadata(&from)?;
        if metadata.is_dir() {
            copy_source(&from, &to)?;
        } else if metadata.is_file() {
            check_no_symlinks(&from, false)?;
            let mut input = platform_native::filesystem::open_read(
                &from,
                platform_native::filesystem::ReadOptions::NoLinks,
            )?;
            if !same_file(&metadata, &input.metadata()?) {
                return Err(copy_changed());
            }
            let mode = if metadata.mode() & 0o111 == 0 {
                0o600
            } else {
                0o700
            };
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(mode)
                .open(&to)?;
            io::copy(&mut input, &mut output)?;
            output.set_permissions(fs::Permissions::from_mode(mode))?;
            output.sync_all()?;
            if !same_file(&metadata, &input.metadata()?)
                || !same_file(&metadata, &fs::symlink_metadata(&from)?)
            {
                return Err(copy_changed());
            }
        } else {
            return Err(ApiError::new(
                "BUILD_INPUT_UNSAFE",
                "Source copies allow only ordinary files and directories, without symbolic links",
            ));
        }
    }
    Ok(())
}
fn copy_changed() -> ApiError {
    ApiError::new(
        "BUILD_INPUT_CHANGED",
        "A source file changed while being copied",
    )
}
fn unsupported(message: &str) -> ApiError {
    ApiError::new("BUILD_LOCAL_DEPENDENCY_UNSUPPORTED", message)
}

/// Interpret only relative filesystem paths, never URLs or shell expansion.
/// Nested packages may use ../siblings if the result remains in sourceDir.
fn local_path(base: &Path, value: &str, allow_install_path: bool) -> Result<PathBuf, ApiError> {
    if value.is_empty()
        || value.contains(['\\', '%', '?', '#', '\0', ':'])
        || value.starts_with('~')
        || Path::new(value).is_absolute()
    {
        return Err(unsupported(
            "Local dependency paths must be relative to sourceDir",
        ));
    }
    let mut path = base.to_path_buf();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(name) => {
                if !allow_install_path && name.to_str().is_some_and(excluded) {
                    return Err(unsupported(
                        "Local dependencies cannot use excluded source directories",
                    ));
                }
                path.push(name);
            }
            Component::CurDir => (),
            Component::ParentDir if path.pop() => (),
            _ => return Err(unsupported("Local dependency paths cannot leave sourceDir")),
        }
    }
    Ok(path)
}
fn local_spec(value: &str) -> Option<&str> {
    value.strip_prefix("file:").or_else(|| {
        (value.starts_with(['.', '/']) || value.starts_with("~/") || value.contains('\\'))
            .then_some(value)
    })
}
fn validate_target(root: &Path, base: &Path, spec: &str) -> Result<(), ApiError> {
    let target = local_path(base, spec, false)?;
    let target = root.join(target);
    check_no_symlinks(&target, false).map_err(|_| {
        unsupported(
            "Local dependencies must exist as ordinary files or directories inside sourceDir",
        )
    })?;
    let metadata = fs::symlink_metadata(target)?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(unsupported(
            "Local dependencies must be ordinary files or directories",
        ));
    }
    Ok(())
}
fn validate_package(root: &Path, base: &Path, package: &Value) -> Result<(), ApiError> {
    for section in ["dependencies", "devDependencies", "optionalDependencies"] {
        if let Some(dependencies) = package[section].as_object() {
            for spec in dependencies.values().filter_map(Value::as_str) {
                if let Some(path) = local_spec(spec) {
                    validate_target(root, base, path)?;
                }
            }
        }
    }
    if let Some(workspaces) = package.get("workspaces") {
        let patterns = workspaces
            .as_array()
            .or_else(|| workspaces.get("packages").and_then(Value::as_array))
            .ok_or_else(|| unsupported("Workspaces must be an array of in-source path patterns"))?;
        for pattern in patterns {
            let pattern = pattern
                .as_str()
                .ok_or_else(|| unsupported("Invalid workspace path"))?;
            // Support ordinary in-source paths, * and **, without expansion that
            // could conceal a parent segment (for example {safe,../external}).
            if !pattern
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"@_./*-".contains(&b))
                || pattern.split('/').any(|part| part == "..")
            {
                return Err(unsupported("Workspace patterns must stay inside sourceDir"));
            }
            local_path(base, pattern, false)?;
        }
    }
    Ok(())
}
fn validate_lock_paths(root: &Path, base: &Path, lock: &Value) -> Result<(), ApiError> {
    if let Some(packages) = lock["packages"].as_object() {
        for (name, package) in packages {
            let location = if name.is_empty() {
                base.to_path_buf()
            } else {
                local_path(base, name, true)?
            };
            validate_package(root, &location, package)?;
            if let Some(resolved) = package["resolved"].as_str() {
                if package["link"].as_bool() == Some(true) {
                    validate_target(
                        root,
                        base,
                        resolved.strip_prefix("file:").unwrap_or(resolved),
                    )?;
                } else if let Some(path) = local_spec(resolved) {
                    validate_target(root, base, path)?;
                }
            }
        }
    }
    // lockfileVersion 2 also carries a recursive legacy dependency graph. Validate
    // every nested file: version/resolved entry, not just the root package record.
    fn nested(root: &Path, base: &Path, value: &Value) -> Result<(), ApiError> {
        if let Some(object) = value.as_object() {
            for (key, value) in object {
                if matches!(key.as_str(), "resolved" | "version") {
                    if let Some(path) = value.as_str().and_then(local_spec) {
                        validate_target(root, base, path)?;
                    }
                }
                nested(root, base, value)?;
            }
        } else if let Some(array) = value.as_array() {
            for value in array {
                nested(root, base, value)?;
            }
        }
        Ok(())
    }
    nested(root, base, &lock["dependencies"])
}
fn validate_local_dependencies(root: &Path) -> Result<(), ApiError> {
    if root.join("npm-shrinkwrap.json").exists() {
        return Err(unsupported(
            "A root npm-shrinkwrap.json would override the validated package-lock.json",
        ));
    }
    let mut files = vec![];
    collect_files(root, Path::new(""), true, &mut files)?;
    for file in files {
        let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !matches!(
            name,
            "package.json" | "package-lock.json" | "npm-shrinkwrap.json"
        ) {
            continue;
        }
        let value: Value = serde_json::from_slice(&fs::read(root.join(&file))?)?;
        let base = file.parent().unwrap();
        if name == "package.json" {
            validate_package(root, base, &value)?;
        } else {
            validate_lock_paths(root, base, &value)?;
        }
    }
    Ok(())
}

pub(super) fn isolate_git_environment(command: &mut Command, cwd: &Path) -> Result<(), ApiError> {
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|key| key.starts_with("GIT_")) {
            command.env_remove(key);
        }
    }
    let parent = cwd
        .parent()
        .ok_or_else(|| ApiError::new("INVALID_PATH", "Candidate has no parent"))?;
    command
        .env("GIT_CEILING_DIRECTORIES", parent)
        .env_remove("NODE_PATH");
    Ok(())
}

#[cfg(test)]
#[path = "build_candidate_tests.rs"]
mod tests;
