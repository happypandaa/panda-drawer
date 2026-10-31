use super::*;

pub(super) fn scan_source(
    root: &Path,
    manifest: &ProjectManifest,
    roots: &[String],
) -> Result<Scan, ApiError> {
    let mut scan = Scan {
        files: BTreeMap::new(),
        excluded: vec![],
        unclassified: vec![],
        markers: vec![],
        issues: vec![],
    };
    let mut bytes = 0;
    let mut count = 0;
    fn visit(
        root: &Path,
        relative: &Path,
        manifest: &ProjectManifest,
        roots: &[String],
        scan: &mut Scan,
        bytes: &mut u64,
        count: &mut usize,
    ) -> Result<(), ApiError> {
        check_no_symlinks(&root.join(relative), false)?;
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
            let label = path.to_str().ok_or_else(unsafe_path)?.to_owned();
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
            if metadata.is_dir() {
                visit(root, &path, manifest, roots, scan, bytes, count)?;
            } else if metadata.is_file() && metadata.nlink() == 1 {
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
                        mode: if metadata.mode() & 0o111 != 0 {
                            "100755"
                        } else {
                            "100644"
                        }
                        .into(),
                        digest: sha256(&contents),
                        size: contents.len() as u64,
                    },
                );
            } else {
                scan.issues.push(unsafe_path().detail("path", label));
            }
        }
        Ok(())
    }
    visit(
        root,
        Path::new(""),
        manifest,
        roots,
        &mut scan,
        &mut bytes,
        &mut count,
    )?;
    Ok(scan)
}
pub(super) fn marker(
    path: &Path,
    scope: &str,
    markers: &mut Vec<GitMarker>,
) -> Result<(), ApiError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => markers.push(GitMarker {
            path: path.to_path_buf(),
            scope: scope.into(),
            kind: if metadata.is_dir() {
                "directory"
            } else if metadata.is_file() && metadata.nlink() == 1 {
                "file"
            } else {
                "unsafe"
            }
            .into(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err(unsafe_path()),
    }
    Ok(())
}
pub(super) fn inspect_repository_files(
    root: &Path,
    issues: &mut Vec<ApiError>,
) -> Result<(), ApiError> {
    check_no_symlinks(root, false)?;
    let mut count = 0usize;
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
        check_no_symlinks(path, false)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            *count += 1;
            if *count > 100_000 {
                return Err(limit());
            }
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)?;
            if meta.is_dir() {
                visit(root, &path, issues, count, depth + 1)?;
            } else if !meta.is_file() || meta.nlink() != 1 {
                return Err(unsafe_path());
            }
            let relative = path.strip_prefix(root).map_err(|_| unsafe_path())?;
            let label = relative.to_str().ok_or_else(unsafe_path)?;
            if label.ends_with(".lock") {
                issues.push(error(
                    "PROJECT_GIT_LOCKED",
                    "仓库存在其他 Git 操作锁；不会删除锁文件或继续写入。",
                ));
            }
            if matches!(
                label,
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
        Ok(())
    }
    visit(root, root, issues, &mut count, 0)
}
pub(super) fn private_dir(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    fs::create_dir_all(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
pub(super) fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>, ApiError> {
    check_no_symlinks(path, false)?;
    let before = fs::symlink_metadata(path)?;
    if !before.is_file() || before.nlink() != 1 || before.len() > limit {
        return Err(unsafe_path());
    }
    let mut file = platform_native::filesystem::open_read(
        path,
        platform_native::filesystem::ReadOptions::NoLinksNonblocking,
    )?;
    if !same_file(&before, &file.metadata()?) {
        return Err(stale());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != before.len()
        || !same_file(&before, &file.metadata()?)
        || !same_file(&before, &fs::symlink_metadata(path)?)
    {
        return Err(stale());
    }
    Ok(bytes)
}
pub(super) fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.mode() == b.mode()
        && a.nlink() == b.nlink()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}
pub(super) fn identity(path: &Path) -> Result<String, ApiError> {
    check_no_symlinks(path, false)?;
    let m = fs::symlink_metadata(path)?;
    Ok(format!("{}:{}", m.dev(), m.ino()))
}
pub(super) fn rename_new(from: &Path, to: &Path) -> Result<(), ApiError> {
    check_no_symlinks(from, false)?;
    check_no_symlinks(to, true)?;
    #[cfg(target_os = "macos")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        let from = CString::new(from.as_os_str().as_bytes()).map_err(|_| unsafe_path())?;
        let to = CString::new(to.as_os_str().as_bytes()).map_err(|_| unsafe_path())?;
        if unsafe {
            libc::renameatx_np(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        } != 0
        {
            return Err(error(
                "PROJECT_GIT_INITIALIZE_CONFLICT",
                "Git 根已出现或不能安全发布；保留现有目录。",
            ));
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (from, to);
        Err(error(
            "UNSUPPORTED_PLATFORM",
            "此 Git 初始化事务目前仅验证 macOS。",
        ))
    }
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
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
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
        .env("GIT_ASKPASS", "/usr/bin/false")
        .env("GIT_SSH", "/usr/bin/false")
        .env("GIT_SSH_COMMAND", "/usr/bin/false")
        .env("SSH_ASKPASS", "/usr/bin/false")
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
        "core.askPass=/usr/bin/false",
        "core.fsmonitor=false",
        "core.untrackedCache=false",
        "core.attributesFile=/dev/null",
        "core.excludesFile=/dev/null",
        "core.autocrlf=false",
        "commit.gpgSign=false",
        "tag.gpgSign=false",
        "gpg.program=/usr/bin/false",
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
pub(super) fn supported_version(value: &str) -> bool {
    ["2.54.0", "2.56.0"].into_iter().any(|version| {
        value == version
            || value
                .strip_prefix(version)
                .and_then(|rest| rest.strip_prefix(" (Apple Git-")?.strip_suffix(')'))
                .is_some_and(|number| {
                    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
                })
    })
}

pub(super) fn valid_context_root(root: &Path) -> bool {
    !root
        .to_str()
        .is_none_or(|path| path.contains(':') || path.chars().any(char::is_control))
}
pub(super) fn prepare_git_command(command: Command) -> Result<Command, ApiError> {
    Ok(command)
}
pub(super) fn command_path(path: &Path) -> Result<std::borrow::Cow<'_, str>, ApiError> {
    path.to_str()
        .map(std::borrow::Cow::Borrowed)
        .ok_or_else(unsafe_path)
}
pub(super) fn source_entry_matches(a: Option<&(String, String)>, b: &(String, String)) -> bool {
    a == Some(b)
}
