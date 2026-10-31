//! Windows source snapshots and exclusive materialization. Existing directories
//! are observed as ordinary objects; their DACL is not an additional admission.
use super::*;
use platform_native::{tool_files::Facts, windows::files as native};

pub(super) fn new_dir(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    native::create_private_directory(path)?;
    Ok(())
}

pub(super) fn private_dir(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, true)?;
    let mut missing = Vec::new();
    let mut current = path;
    while !current.try_exists()? {
        missing.push(current);
        current = current
            .parent()
            .ok_or_else(|| invalid("源码目录没有既有父目录。"))?;
    }
    for directory in missing.into_iter().rev() {
        native::create_private_directory(directory)?;
    }
    if !Facts::capture(path)?.is_dir() {
        return Err(invalid("预期为普通目录。"));
    }
    Ok(())
}

pub(super) fn ordinary_file(path: &Path) -> Result<(), ApiError> {
    check_no_symlinks(path, false)?;
    let facts = Facts::capture(path)?;
    if !facts.is_file() || facts.links() != 1 {
        return Err(invalid("仅支持普通且无链接的文件。"));
    }
    Ok(())
}

pub(super) fn read_small(path: &Path) -> Result<Vec<u8>, ApiError> {
    ordinary_file(path)?;
    let before = Facts::capture(path)?;
    let file = native::open_read_file(path)?;
    let opened = Facts::from_file(&file)?;
    if !opened.is_file() || opened.links() != 1 || !before.same_object(&opened) {
        return Err(invalid("声明文件在读取时发生变化。"));
    }
    if opened.len() > MAX_METADATA {
        return Err(limit());
    }
    let mut bytes = Vec::new();
    file.take(MAX_METADATA + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_METADATA {
        return Err(limit());
    }
    let after = Facts::capture(path)?;
    if !before.same_modified_file(&after) || after.len() != bytes.len() as u64 {
        return Err(invalid("声明文件在读取时发生变化。"));
    }
    Ok(bytes)
}

pub(super) fn create_source_file(path: &Path) -> Result<fs::File, ApiError> {
    check_no_symlinks(path, true)?;
    native::create_private_file(path).map_err(|issue| {
        if issue.kind() == std::io::ErrorKind::AlreadyExists {
            // The filesystem is the final authority if its case table differs
            // from the lexical comparison. Do not replace, merge or rename an
            // existing materialized entry to make the tree fit this machine.
            policy_native::needs_adaptation(&path.to_string_lossy())
                .detail("cause", issue.to_string())
        } else {
            ApiError::from(issue)
        }
    })
}

pub(super) fn verify_created_file(path: &Path, file: &fs::File) -> Result<(), ApiError> {
    let owned = native::identity(file)?;
    let observed = native::open_no_reparse(path)?;
    if owned.directory
        || observed.identity.directory
        || observed.identity.links != 1
        || !owned.same_object(&observed.identity)
    {
        return Err(invalid("物化源码文件的身份已经变化，请保留现场。"));
    }
    Ok(())
}

fn relative_label(root: &Path, path: &Path) -> Result<String, ApiError> {
    path.strip_prefix(root)
        .map_err(|_| invalid("源码路径越界。"))?
        .iter()
        .map(|part| part.to_str().ok_or_else(|| invalid("源码路径不是 UTF-8。")))
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("/"))
}

pub(super) fn directory_digest(root: &Path) -> Result<String, ApiError> {
    fn collect(
        root: &Path,
        path: &Path,
        files: &mut BTreeMap<String, PathBuf>,
        depth: usize,
    ) -> Result<(), ApiError> {
        if depth > 32 {
            return Err(limit());
        }
        check_no_symlinks(path, false)?;
        let _scope = native::pin_directory(path)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if path == root
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(".git"))
            {
                continue;
            }
            let full = entry.path();
            let facts = Facts::capture(&full)?;
            if facts.is_dir() {
                collect(root, &full, files, depth + 1)?;
            } else {
                ordinary_file(&full)?;
                let relative = relative_label(root, &full)?;
                validate_tree_path(&relative)?;
                if facts.len() > MAX_BLOB || files.len() >= MAX_FILES {
                    return Err(limit());
                }
                files.insert(relative, full);
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    collect(root, root, &mut files, 0)?;
    let mut tree = Sha256::new();
    let mut total = 0u64;
    for (relative, path) in files {
        let before = Facts::capture(&path)?;
        let mut file = native::open_read_file(&path)?;
        let opened = Facts::from_file(&file)?;
        if !opened.is_file() || opened.links() != 1 || !before.same_object(&opened) {
            return Err(invalid("源码在校验时改变。"));
        }
        let mut digest = Sha256::new();
        let mut buffer = vec![0u8; 65536];
        let mut size = 0u64;
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            size += count as u64;
            total += count as u64;
            if size > MAX_BLOB || total > MAX_TREE {
                return Err(limit());
            }
            digest.update(&buffer[..count]);
        }
        let after = Facts::capture(&path)?;
        if !before.same_modified_permissions(&after) || after.len() != size {
            return Err(invalid("源码在校验时改变。"));
        }
        tree.update(relative.as_bytes());
        tree.update([0]);
        tree.update(size.to_le_bytes());
        // The Windows host has no Unix execute permission bit. Git's original
        // commit/index keeps its mode; this same host digest is used on re-read.
        tree.update([0]);
        tree.update(digest.finalize());
    }
    Ok(format!("{:x}", tree.finalize()))
}

pub(super) fn check_disk(root: &Path) -> Result<(), ApiError> {
    fn scan(path: &Path, bytes: &mut u64, count: &mut usize) -> Result<(), ApiError> {
        let _scope = native::pin_directory(path)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let facts = match Facts::capture(&entry.path()) {
                Ok(facts) => facts,
                Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => continue,
                Err(problem) => return Err(problem.into()),
            };
            *count += 1;
            if *count > 60_000 {
                return Err(limit());
            }
            if facts.is_dir() {
                scan(&entry.path(), bytes, count)?;
            } else if facts.is_file() && facts.links() == 1 {
                *bytes = bytes.checked_add(facts.len()).ok_or_else(limit)?;
                if *bytes > MAX_DISK {
                    return Err(limit());
                }
            } else {
                return Err(invalid("获取目录包含不支持的链接或特殊文件。"));
            }
        }
        Ok(())
    }
    check_no_symlinks(root, false)?;
    scan(root, &mut 0, &mut 0)?;
    if fs2::available_space(root)? < MIN_FREE {
        return Err(error(
            "SOURCE_SPACE_INSUFFICIENT",
            "磁盘可用空间不足，源码获取已停止。",
        ));
    }
    Ok(())
}

pub(super) fn publish_candidate(source: &Path, target: &Path) -> Result<(), ApiError> {
    let scope = native::open_no_reparse(source)?;
    native::rename_directory_noreplace_expected(source, target, &scope.identity).map_err(|issue| {
        if native::publication_outcome_unknown(&issue) {
            error(
                "SOURCE_PUBLICATION_UNCONFIRMED",
                "源码候选移动结果尚未核实，已保留归属与现场。",
            )
            .detail("candidateOwnershipConfirmed", false)
            .detail("sourcePublicationPending", true)
            .detail("reason", issue.to_string())
            .outcome("unknown")
        } else {
            ApiError::from(issue)
        }
    })
}
