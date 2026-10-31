//! Native facts and isolated Windows Git command preparation.
use super::*;
use platform_native::windows::files as native;
use std::{
    borrow::Cow,
    ffi::{OsStr, OsString},
};

pub(super) fn valid_context_root(root: &Path) -> bool {
    root.is_absolute()
        && root
            .to_str()
            .is_some_and(|value| !value.chars().any(char::is_control))
}
pub(super) fn supported_version(value: &str) -> bool {
    ["2.54.0", "2.56.0"].iter().any(|base| {
        value == *base
            || value.strip_prefix(base).is_some_and(|suffix| {
                suffix.strip_prefix(".windows.").is_some_and(|revision| {
                    !revision.is_empty() && revision.bytes().all(|byte| byte.is_ascii_digit())
                }) || suffix
                    .strip_prefix(" (Apple Git-")
                    .and_then(|value| value.strip_suffix(')'))
                    .is_some_and(|value| {
                        !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
                    })
            })
    })
}
pub(super) fn command_path(path: &Path) -> Result<Cow<'_, str>, ApiError> {
    crate::node_tool_layout::command_argument(path)?
        .into_os_string()
        .into_string()
        .map(Cow::Owned)
        .map_err(|_| unsafe_path())
}
pub(super) fn source_entry_matches(
    actual: Option<&(String, String)>,
    expected: &(String, String),
) -> bool {
    // Git retains its 100755 tree/index bits. Windows has no Unix executable
    // bit, just as this command's explicit core.filemode=false states.
    actual.is_some_and(|actual| {
        actual.1 == expected.1
            && matches!(actual.0.as_str(), "100644" | "100755")
            && matches!(expected.0.as_str(), "100644" | "100755")
    })
}
pub(super) fn prepare_git_command(command: Command) -> Result<Command, ApiError> {
    let original = command.as_std();
    let argument = crate::node_tool_layout::command_argument;
    let system = platform_native::windows::system_paths::SystemPaths::current()?;
    let mut prepared = Command::new(argument(Path::new(original.get_program()))?);
    prepared.env_clear();
    let mut file_argument = false;
    for value in original.get_args() {
        let converted = if file_argument {
            file_argument = false;
            argument(Path::new(value))?.into_os_string()
        } else if let Some(value) = value.to_str() {
            if let Some((prefix, path)) = ["--git-dir=", "--work-tree=", "core.hooksPath="]
                .iter()
                .find_map(|prefix| value.strip_prefix(prefix).map(|path| (*prefix, path)))
            {
                let mut converted = OsString::from(prefix);
                converted.push(argument(Path::new(path))?);
                converted
            } else {
                file_argument = value == "--file";
                value.into()
            }
        } else {
            value.to_owned()
        };
        prepared.arg(converted);
    }
    for (key, value) in original.get_envs() {
        if let Some(value) = value {
            let path = key.to_str().is_some_and(|key| {
                matches!(
                    key,
                    "HOME"
                        | "XDG_CONFIG_HOME"
                        | "GIT_EXEC_PATH"
                        | "GIT_INDEX_FILE"
                        | "GIT_OBJECT_DIRECTORY"
                        | "GIT_CEILING_DIRECTORIES"
                )
            });
            if path {
                prepared.env(key, argument(Path::new(value))?);
            } else {
                prepared.env(key, value);
            }
        }
    }
    if let Some(directory) = original.get_current_dir() {
        prepared.current_dir(argument(directory)?);
    }
    if let Some(home) = original
        .get_envs()
        .find_map(|(key, value)| (key == OsStr::new("HOME")).then_some(value).flatten())
    {
        let home = argument(Path::new(home))?;
        for key in ["USERPROFILE", "APPDATA", "LOCALAPPDATA", "TEMP", "TMP"] {
            prepared.env(key, &home);
        }
    }
    prepared
        .env("SystemRoot", system.windows)
        .env("SSH_ASKPASS_REQUIRE", "never")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::owned_process::prepare_command(&mut prepared);
    Ok(prepared)
}
pub(super) fn private_dir(path: &Path) -> Result<(), ApiError> {
    crate::project_file_io::ensure_directory(path)
}
pub(super) fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>, ApiError> {
    let opened = native::open_no_reparse(path)?;
    if opened.identity.directory
        || opened.identity.links != 1
        || opened.file.metadata()?.len() > limit
    {
        return Err(unsafe_path());
    }
    let mut file = native::open_read_file(path)?;
    let before = platform_native::directory::stamp(&file)?;
    if !opened.identity.same_object(&native::identity(&file)?) {
        return Err(stale());
    }
    let length = file.metadata()?.len();
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    let named = native::open_no_reparse(path)?;
    if bytes.len() as u64 != length
        || before != platform_native::directory::stamp(&file)?
        || before != platform_native::directory::stamp(&named.file)?
        || native::identity(&file)?.links != 1
        || named.identity.links != 1
    {
        return Err(stale());
    }
    Ok(bytes)
}
pub(super) fn identity(path: &Path) -> Result<String, ApiError> {
    let opened = native::open_no_reparse(path)?;
    Ok(format!(
        "{}:{:02x?}",
        opened.identity.volume_serial, opened.identity.file_id
    ))
}
pub(super) fn rename_new(from: &Path, to: &Path) -> Result<(), ApiError> {
    let expected = native::open_no_reparse(from)?.identity;
    if expected.directory {
        native::rename_directory_noreplace_expected(from, to, &expected)?;
    } else {
        native::rename_file_noreplace_expected(from, to, &expected)?;
    }
    Ok(())
}
fn label(path: &Path) -> Result<String, ApiError> {
    path.components()
        .map(|part| match part {
            Component::Normal(value) => value.to_str().ok_or_else(unsafe_path),
            _ => Err(unsafe_path()),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
}
pub(super) fn marker(
    path: &Path,
    scope: &str,
    markers: &mut Vec<GitMarker>,
) -> Result<(), ApiError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(unsafe_path()),
    };
    let ordinary = !platform_native::filesystem::is_reparse(&metadata);
    let kind = if ordinary && metadata.is_dir() {
        "directory"
    } else if ordinary && metadata.is_file() && native::open_no_reparse(path)?.identity.links == 1 {
        "file"
    } else {
        "unsafe"
    };
    let path = if ordinary {
        fs::canonicalize(path)?
    } else {
        path.to_owned()
    };
    markers.push(GitMarker {
        path,
        scope: scope.into(),
        kind: kind.into(),
    });
    Ok(())
}
pub(super) fn scan_source(
    root: &Path,
    manifest: &ProjectManifest,
    roots: &[String],
) -> Result<Scan, ApiError> {
    fn visit(
        root: &Path,
        relative: &Path,
        manifest: &ProjectManifest,
        roots: &[String],
        scan: &mut Scan,
        bytes: &mut u64,
        count: &mut usize,
    ) -> Result<(), ApiError> {
        let directory = native::pin_directory(&root.join(relative))?;
        let mut entries = fs::read_dir(root.join(relative))?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            *count += 1;
            if *count > MAX_FILES * 2 {
                return Err(limit());
            }
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(unsafe_path)?;
            if name.chars().any(char::is_control) || name.contains('\\') {
                return Err(unsafe_path());
            }
            let path = relative.join(name);
            let label = label(&path)?;
            if name.eq_ignore_ascii_case(".git") {
                if !relative.as_os_str().is_empty() {
                    marker(&entry.path(), "source", &mut scan.markers)?;
                }
                continue;
            }
            if relative.as_os_str().is_empty() && name.eq_ignore_ascii_case(version_data::DIRECTORY)
            {
                if name != version_data::DIRECTORY {
                    return Err(unsafe_path());
                }
                scan.excluded.push(GitExcludedPath {
                    path: label,
                    reason: "数据版本快照单独核验，源码取回不修改此目录".into(),
                });
                continue;
            }
            if let Some(reason) = protected(&label, manifest) {
                scan.excluded.push(GitExcludedPath {
                    path: label,
                    reason: reason.into(),
                });
                continue;
            }
            if !included(&label, roots) && !is_ancestor(&label, roots) {
                scan.unclassified.push(label);
                continue;
            }
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.is_dir() && !platform_native::filesystem::is_reparse(&metadata) {
                visit(root, &path, manifest, roots, scan, bytes, count)?;
            } else if metadata.is_file()
                && !platform_native::filesystem::is_reparse(&metadata)
                && native::open_no_reparse(&entry.path())?.identity.links == 1
            {
                if metadata.len() > MAX_BLOB || scan.files.len() >= MAX_FILES {
                    return Err(limit());
                }
                *bytes = bytes.checked_add(metadata.len()).ok_or_else(limit)?;
                if *bytes > MAX_BYTES {
                    return Err(limit());
                }
                let contents = read_file(&entry.path(), MAX_BLOB)?;
                scan.files.insert(
                    label,
                    SourceFile {
                        mode: "100644".into(),
                        digest: sha256(&contents),
                        size: contents.len() as u64,
                    },
                );
            } else {
                scan.issues.push(unsafe_path().detail("path", label));
            }
        }
        let current = native::open_no_reparse(&root.join(relative))?;
        if !native::identity(&directory.file)?.same_object(&current.identity) {
            return Err(stale());
        }
        Ok(())
    }
    let mut scan = Scan {
        files: BTreeMap::new(),
        excluded: vec![],
        unclassified: vec![],
        markers: vec![],
        issues: vec![],
    };
    visit(
        root,
        Path::new(""),
        manifest,
        roots,
        &mut scan,
        &mut 0,
        &mut 0,
    )?;
    Ok(scan)
}
pub(super) fn inspect_repository_files(
    root: &Path,
    issues: &mut Vec<ApiError>,
) -> Result<(), ApiError> {
    fn visit(
        root: &Path,
        path: &Path,
        issues: &mut Vec<ApiError>,
        count: &mut usize,
        depth: usize,
    ) -> Result<(), ApiError> {
        if depth > 64 {
            return Err(limit());
        }
        let directory = native::pin_directory(path)?;
        for entry in fs::read_dir(path)? {
            let path = entry?.path();
            *count += 1;
            if *count > 100_000 {
                return Err(limit());
            }
            let metadata = fs::symlink_metadata(&path)?;
            if platform_native::filesystem::is_reparse(&metadata) {
                return Err(unsafe_path());
            }
            if metadata.is_dir() {
                visit(root, &path, issues, count, depth + 1)?;
            } else if !metadata.is_file() || native::open_no_reparse(&path)?.identity.links != 1 {
                return Err(unsafe_path());
            }
            let relative = path.strip_prefix(root).map_err(|_| unsafe_path())?;
            let label = label(relative)?;
            if label.ends_with(".lock") {
                issues.push(error(
                    "PROJECT_GIT_LOCKED",
                    "仓库存在其他 Git 操作锁；不会删除锁文件或继续写入。",
                ));
            }
            if matches!(
                label.as_str(),
                "commondir"
                    | "config.worktree"
                    | "objects/info/alternates"
                    | "objects/info/http-alternates"
                    | "info/grafts"
                    | "info/sparse-checkout"
                    | "MERGE_HEAD"
                    | "CHERRY_PICK_HEAD"
                    | "REVERT_HEAD"
                    | "BISECT_LOG"
            ) || label.starts_with("sharedindex.")
                || relative.starts_with("worktrees")
                || relative.starts_with("rebase-merge")
                || relative.starts_with("rebase-apply")
                || relative.starts_with("sequencer")
            {
                issues.push(error("PROJECT_GIT_REPOSITORY_UNSUPPORTED","仓库包含本步未支持的共享对象、工作树、稀疏索引或未完成 Git 操作；保持现场不变。"));
            }
        }
        let current = native::open_no_reparse(path)?;
        if !native::identity(&directory.file)?.same_object(&current.identity) {
            return Err(stale());
        }
        Ok(())
    }
    visit(root, root, issues, &mut 0, 0)
}
pub(super) fn git_command(
    tools: &GitToolchain,
    private: &Path,
    project: Option<&ProjectHandle>,
) -> Command {
    let mut command = Command::new(&tools.git.path);
    command
        .env_clear()
        .current_dir(project.map_or(private, |project| project.root.as_path()))
        .env("PATH", &tools.path)
        .env("LC_ALL", "C")
        .env("HOME", private.join("home"))
        .env("XDG_CONFIG_HOME", private.join("home"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "NUL")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .env("GIT_CONFIG_COUNT", "0")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env(
            "GIT_CEILING_DIRECTORIES",
            private.parent().unwrap_or(private),
        )
        .env("GIT_DISCOVERY_ACROSS_FILESYSTEM", "0")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_PROTOCOL_FROM_USER", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "NUL")
        .env("GIT_SSH", "NUL")
        .env("GIT_SSH_COMMAND", "NUL")
        .env("SSH_ASKPASS", "NUL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::owned_process::prepare_command(&mut command);
    if let Some(path) = &tools.exec_path {
        command.env("GIT_EXEC_PATH", path);
    }
    command.arg("--no-pager");
    for setting in [
        "protocol.allow=never",
        "credential.helper=",
        "credential.interactive=false",
        "core.askPass=NUL",
        "core.fsmonitor=false",
        "core.untrackedCache=false",
        "core.attributesFile=NUL",
        "core.excludesFile=NUL",
        "core.autocrlf=false",
        "core.longpaths=true",
        "core.filemode=false",
        "commit.gpgSign=false",
        "tag.gpgSign=false",
        "gpg.program=NUL",
        "gc.auto=0",
        "maintenance.auto=false",
        "core.fsync=all",
        "core.fsyncMethod=fsync",
    ] {
        command.arg("-c").arg(setting);
    }
    command.arg("-c").arg(format!(
        "core.hooksPath={}",
        private.join("empty-hooks").display()
    ));
    if let Some(project) = project {
        command
            .arg(format!("--git-dir={}", project.root.join(".git").display()))
            .arg(format!("--work-tree={}", project.root.display()));
    }
    command
}
