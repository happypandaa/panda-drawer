//! Windows copy inventories use the actual read-only bit, never a Unix mode.
use super::*;
use anyhow::{bail, Context};
use platform_native::{
    tool_files::Facts,
    windows::{files as native, removal},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Component,
};

pub(super) fn require_supported_filesystem(info: &Value) -> Result<()> {
    if !info["type"]
        .as_str()
        .is_some_and(|name| name.eq_ignore_ascii_case("NTFS"))
        || info["local"] != true
        || info["internal"] != true
    {
        bail!("迁移目标必须是本机内置 NTFS 磁盘，不能使用外置盘、网络盘或未能确认的磁盘")
    }
    Ok(())
}

fn leaf(root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|p| !matches!(p, Component::Normal(_)))
    {
        bail!("迁移清单包含越界或空路径")
    }
    Ok(root.join(relative))
}

fn hash(file: &mut File) -> Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0; 128 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn set_read_only(path: &Path, read_only: bool) -> Result<()> {
    let mut permissions = fs::symlink_metadata(path)?.permissions();
    permissions.set_readonly(read_only);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

pub(super) fn collect(root: &Path, excluded: &BTreeSet<PathBuf>) -> Result<Vec<Entry>> {
    fn walk(
        root: &Path,
        relative: &Path,
        excluded: &BTreeSet<PathBuf>,
        out: &mut Vec<Entry>,
    ) -> Result<()> {
        let directory = root.join(relative);
        let pin = native::pin_directory(&directory)?;
        let mut children = fs::read_dir(&directory)?.collect::<std::io::Result<Vec<_>>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let rel = relative.join(child.file_name());
            if excluded.iter().any(|p| rel == *p || rel.starts_with(p)) {
                continue;
            }
            let path = rel
                .to_str()
                .context("工作区包含无法表示为 UTF-8 的文件名")?
                .to_owned();
            let full = directory.join(child.file_name());
            let observed = removal::inspect(&full)?;
            let metadata = observed.file.metadata()?;
            let reparse = platform_native::filesystem::is_reparse(&metadata);
            let entry = if reparse {
                Entry {
                    path,
                    kind: "symlink".into(),
                    bytes: 0,
                    digest: None,
                    target: Some(fs::read_link(&full)?),
                    read_only: metadata.permissions().readonly(),
                }
            } else if observed.identity.directory {
                Entry {
                    path,
                    kind: "directory".into(),
                    bytes: 0,
                    digest: None,
                    target: None,
                    read_only: metadata.permissions().readonly(),
                }
            } else if metadata.is_file() {
                let mut file = native::open_read_file_allowing_hard_links(&full)?;
                let before = Facts::from_file(&file)?;
                if !observed.identity.same_object(&native::identity(&file)?) {
                    bail!("迁移源文件在读取时已替换")
                }
                let digest = hash(&mut file)?;
                if !before.unchanged(&Facts::from_file(&file)?)
                    || !before.same_object(&Facts::capture(&full)?)
                {
                    bail!("迁移源文件在读取时发生变化")
                }
                Entry {
                    path,
                    kind: "file".into(),
                    bytes: before.len(),
                    digest: Some(digest),
                    target: None,
                    read_only: metadata.permissions().readonly(),
                }
            } else {
                bail!("工作区包含不支持的特殊文件：{}", full.display())
            };
            let recurse = entry.kind == "directory";
            out.push(entry);
            if recurse {
                walk(root, &rel, excluded, out)?;
            }
        }
        if !native::identity(&pin.file)?.same_object(&native::open_no_reparse(&directory)?.identity)
        {
            bail!("迁移目录在检查时发生变化")
        }
        Ok(())
    }
    let root = platform_native::windows::separator_path(root);
    let mut entries = vec![];
    walk(&root, Path::new(""), excluded, &mut entries)?;
    Ok(entries)
}

pub(super) fn copy_entries(source: &Path, target: &Path, entries: &[Entry]) -> Result<()> {
    let source = platform_native::windows::separator_path(source);
    let target = platform_native::windows::separator_path(target);
    let _source = native::pin_directory(&source)?;
    let _parent = native::pin_directory(target.parent().context("迁移副本缺少父目录")?)?;
    native::create_private_directory(&target)?;
    for entry in entries {
        let from = leaf(&source, Path::new(&entry.path))?;
        let to = leaf(&target, Path::new(&entry.path))?;
        let _from_parent = native::pin_directory(from.parent().unwrap())?;
        let _to_parent = native::pin_directory(to.parent().unwrap())?;
        match entry.kind.as_str() {
            "directory" => native::create_private_directory(&to)?,
            "symlink" => platform_native::windows::reparse_copy::copy(
                &from,
                &to,
                entry.target.as_deref().context("链接目标缺失")?,
            )?,
            "file" => {
                let mut input = native::open_read_file_allowing_hard_links(&from)?;
                let before = Facts::from_file(&input)?;
                if before.len() != entry.bytes
                    || input.metadata()?.permissions().readonly() != entry.read_only
                {
                    bail!("迁移源文件类型或大小发生变化")
                }
                let mut output = native::create_private_file(&to)?;
                std::io::copy(&mut input, &mut output)?;
                output.sync_all()?;
                if !before.unchanged(&Facts::from_file(&input)?)
                    || !before.same_object(&Facts::capture(&from)?)
                {
                    bail!("复制期间源文件发生变化")
                }
                drop(output);
                let mut copied = native::open_read_file_allowing_hard_links(&to)?;
                if entry.digest.as_deref() != Some(hash(&mut copied)?.as_str()) {
                    bail!("迁移副本内容摘要不一致")
                }
                drop(copied);
                set_read_only(&to, entry.read_only)?;
            }
            _ => bail!("未知迁移文件类型"),
        }
    }
    for entry in entries
        .iter()
        .rev()
        .filter(|entry| entry.kind == "directory")
    {
        set_read_only(&leaf(&target, Path::new(&entry.path))?, entry.read_only)?;
    }
    platform_native::filesystem::sync_directory(&target)?;
    Ok(())
}

pub(super) fn append_phase_log(path: &Path, phase: &str) -> Result<(), crate::ApiError> {
    let parent = path
        .parent()
        .ok_or_else(|| crate::ApiError::new("WORKSPACE_MIGRATION_FAILED", "迁移日志缺少父目录"))?;
    let _parent = native::pin_directory(parent)?;
    let mut file = native::open_private_file(path, true, false)?;
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::End(0))?;
    writeln!(file, "{phase}")?;
    file.sync_all()?;
    Ok(())
}
