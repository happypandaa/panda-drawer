//! Read-only verification of the separately bundled development Node/npm tree.
//! The source-controlled release is the trust anchor; package metadata never supplies
//! replacement hashes. This verifier does not execute tools, install dependencies,
//! change permissions, or inspect the independently selected application runtime.

use crate::ApiError;
use platform_native::tool_files::{self as native_tools, Facts as Metadata};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

const RELEASE: &str = crate::node_tool_layout::DEVELOPMENT_RELEASE;
const METADATA_FILE: &str = "development-node.json";
const MAX_FILE_BYTES: u64 = 160 * 1024 * 1024;
const MAX_TREE_BYTES: u64 = 180 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 64 * 1024;
const MAX_FILES: usize = 10_000;
const MAX_ENTRIES: usize = 20_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Release {
    schema_version: u32,
    node_version: String,
    npm_version: String,
    platform: String,
    arch: String,
    archive_name: String,
    archive_sha256: String,
    tree: Tree,
    executables: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Tree {
    rule: String,
    file_count: usize,
    total_bytes: u64,
    sha256: String,
}

pub(crate) struct VerifiedToolchain {
    pub root: PathBuf,
    pub node: PathBuf,
    pub npm: PathBuf,
    pub node_version: String,
    pub npm_version: String,
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new("MANAGED_NODE_INVALID", message)
}

fn io_error(path: &Path, error: std::io::Error) -> ApiError {
    invalid(format!(
        "无法校验受管开发工具 {}：{}",
        path.display(),
        error
    ))
}

fn digest_valid(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn relative_valid(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
        && value
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn release(bytes: &str) -> Result<Release, ApiError> {
    let release: Release =
        serde_json::from_str(bytes).map_err(|_| invalid("程序内的受管开发工具发行声明无效。"))?;
    if release.schema_version != 1
        || release.node_version.is_empty()
        || release.npm_version.is_empty()
        || release.archive_name.is_empty()
        || !digest_valid(&release.archive_sha256)
        || release.tree.rule != "sorted-utf8-path-nul-size-nul-sha256-lf-v1"
        || release.tree.file_count == 0
        || release.tree.file_count > MAX_FILES
        || release.tree.total_bytes > MAX_TREE_BYTES
        || !digest_valid(&release.tree.sha256)
        || !release.executables.iter().all(|path| relative_valid(path))
        || release.executables.iter().collect::<HashSet<_>>().len() != release.executables.len()
        || !crate::node_tool_layout::required_executables()
            .iter()
            .all(|required| release.executables.iter().any(|path| path == required))
    {
        return Err(invalid("程序内的受管开发工具发行声明不受支持。"));
    }
    let platform = crate::node_tool_layout::platform();
    let arch = crate::node_tool_layout::arch();
    if release.platform != platform || release.arch != arch {
        return Err(ApiError::new(
            "MANAGED_NODE_PLATFORM_UNSUPPORTED",
            "此受管开发工具尚未验证支持当前系统与处理器。",
        ));
    }
    Ok(release)
}

fn same_identity(left: &Metadata, right: &Metadata) -> bool {
    left.same_object(right)
}
fn unchanged(left: &Metadata, right: &Metadata) -> bool {
    left.unchanged(right)
}

fn directory_chain(root: &Path) -> Result<Vec<(PathBuf, Metadata)>, ApiError> {
    if !native_tools::absolute_path_valid(root) {
        return Err(invalid(
            "受管开发工具必须位于不含越界或控制字符的绝对路径。",
        ));
    }
    let mut chain = Vec::new();
    for current in platform_native::filesystem::path_components(root) {
        let info = Metadata::capture(&current).map_err(|error| io_error(&current, error))?;
        if !info.is_dir() || info.is_reparse() {
            return Err(invalid(format!(
                "受管开发工具路径包含链接或非目录：{}",
                current.display()
            )));
        }
        chain.push((current.clone(), info));
    }
    Ok(chain)
}

fn open_ordinary(path: &Path, before: &Metadata, executable: bool) -> Result<File, ApiError> {
    if !before.is_file()
        || before.is_reparse()
        || before.links() != 1
        || before.len() > MAX_FILE_BYTES
        || !before.permissions_valid(executable)
    {
        return Err(invalid(format!(
            "受管开发工具需要权限正确、无链接的普通文件：{}",
            path.display()
        )));
    }
    let file = native_tools::open_read(path).map_err(|error| io_error(path, error))?;
    let opened = Metadata::from_file(&file).map_err(|error| io_error(path, error))?;
    if !unchanged(before, &opened) || !opened.is_file() || opened.links() != 1 {
        return Err(invalid("受管开发工具文件在打开时发生变化。"));
    }
    Ok(file)
}

struct Inspection<'a> {
    executables: HashSet<&'a str>,
    files: Vec<(String, u64, String)>,
    directories: Vec<(PathBuf, Metadata)>,
    metadata: Option<Vec<u8>>,
    entry_count: usize,
    total_bytes: u64,
}

impl Inspection<'_> {
    fn visit(&mut self, root: &Path, relative: &Path) -> Result<(), ApiError> {
        if relative.components().count() > 64 {
            return Err(invalid("受管开发工具目录深度超过允许范围。"));
        }
        let directory = root.join(relative);
        let before = Metadata::capture(&directory).map_err(|error| io_error(&directory, error))?;
        if !before.is_dir() || before.is_reparse() {
            return Err(invalid("受管开发工具中存在链接或非普通目录。"));
        }
        if !before.permissions_valid(true) {
            return Err(invalid("受管开发工具目录权限必须为 0555 或 0755。"));
        }
        let mut found = false;
        for entry in fs::read_dir(&directory).map_err(|error| io_error(&directory, error))? {
            found = true;
            self.entry_count += 1;
            if self.entry_count > MAX_ENTRIES {
                return Err(invalid("受管开发工具文件树超过允许数量。"));
            }
            let entry = entry.map_err(|error| io_error(&directory, error))?;
            let child = relative.join(entry.file_name());
            let name = native_tools::relative_name(&child)
                .filter(|path| relative_valid(path))
                .ok_or_else(|| invalid("受管开发工具包含不受支持的文件名。"))?;
            let path = root.join(&child);
            let info = Metadata::capture(&path).map_err(|error| io_error(&path, error))?;
            if info.is_reparse() {
                return Err(invalid(format!(
                    "受管开发工具包含符号链接：{}",
                    path.display()
                )));
            }
            if info.is_dir() {
                self.visit(root, &child)?;
                continue;
            }
            if self.files.len() >= MAX_FILES {
                return Err(invalid("受管开发工具文件超过允许数量。"));
            }
            self.total_bytes = self
                .total_bytes
                .checked_add(info.len())
                .filter(|total| *total <= MAX_TREE_BYTES)
                .ok_or_else(|| invalid("受管开发工具超过允许大小。"))?;
            let is_metadata = name.as_ref() == METADATA_FILE;
            if is_metadata && info.len() > MAX_METADATA_BYTES {
                return Err(invalid("受管开发工具发行声明超过允许大小。"));
            }
            let mut file = open_ordinary(&path, &info, self.executables.contains(name.as_ref()))?;
            let mut hasher = Sha256::new();
            let mut size = 0u64;
            let mut metadata_bytes = Vec::new();
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let count = file
                    .read(&mut buffer)
                    .map_err(|error| io_error(&path, error))?;
                if count == 0 {
                    break;
                }
                size += count as u64;
                if size > info.len() {
                    return Err(invalid("受管开发工具文件在读取时增长。"));
                }
                hasher.update(&buffer[..count]);
                if is_metadata {
                    metadata_bytes.extend_from_slice(&buffer[..count]);
                }
            }
            let after = Metadata::from_file(&file).map_err(|error| io_error(&path, error))?;
            let path_after = Metadata::capture(&path).map_err(|error| io_error(&path, error))?;
            if size != info.len() || !unchanged(&info, &after) || !unchanged(&info, &path_after) {
                return Err(invalid("受管开发工具文件在读取时发生变化。"));
            }
            if is_metadata {
                self.metadata = Some(metadata_bytes);
            } else {
                self.files
                    .push((name.into_owned(), size, format!("{:x}", hasher.finalize())));
            }
        }
        if !found {
            return Err(invalid("受管开发工具中存在未声明的空目录。"));
        }
        self.directories.push((directory, before));
        Ok(())
    }
}

fn verify_sync(root: PathBuf, release_bytes: &str) -> Result<VerifiedToolchain, ApiError> {
    let release = release(release_bytes)?;
    let chain = directory_chain(&root)?;
    let mut inspection = Inspection {
        executables: release.executables.iter().map(String::as_str).collect(),
        files: Vec::new(),
        directories: Vec::new(),
        metadata: None,
        entry_count: 0,
        total_bytes: 0,
    };
    inspection.visit(&root, Path::new(""))?;
    for (path, before) in &inspection.directories {
        let after = Metadata::capture(path).map_err(|error| io_error(path, error))?;
        if !after.is_dir() || !unchanged(before, &after) {
            return Err(invalid("受管开发工具目录在校验时发生变化。"));
        }
    }
    for (path, before) in chain {
        let after = Metadata::capture(&path).map_err(|error| io_error(&path, error))?;
        if !after.is_dir() || after.is_reparse() || !same_identity(&before, &after) {
            return Err(invalid("受管开发工具父目录在校验时发生变化。"));
        }
    }
    if inspection.metadata.as_deref() != Some(release_bytes.as_bytes()) {
        return Err(invalid("受管开发工具发行声明与程序内的固定声明不一致。"));
    }
    inspection
        .files
        .sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut hasher = Sha256::new();
    let mut total = 0u64;
    for (path, size, digest) in &inspection.files {
        hasher.update(format!("{path}\0{size}\0{digest}\n").as_bytes());
        total += size;
    }
    if inspection.files.len() != release.tree.file_count
        || total != release.tree.total_bytes
        || format!("{:x}", hasher.finalize()) != release.tree.sha256
    {
        return Err(invalid(
            "受管开发 Node/npm 文件树与固定发行版本不一致，请修复应用安装。",
        ));
    }
    Ok(VerifiedToolchain {
        node: crate::node_tool_layout::node(&root),
        npm: crate::node_tool_layout::npm(&root),
        root,
        node_version: release.node_version,
        npm_version: release.npm_version,
    })
}

pub(crate) async fn verify(root: PathBuf) -> Result<VerifiedToolchain, ApiError> {
    if crate::build_profile::PRODUCTION_PROFILE {
        return tokio::task::spawn_blocking(move || {
            crate::production_resources::verify_tool_payload(
                &root,
                crate::production_resources::PayloadKind::DevelopmentNode,
            )?;
            let source = release(RELEASE)?;
            Ok(VerifiedToolchain {
                node: crate::node_tool_layout::node(&root),
                npm: crate::node_tool_layout::npm(&root),
                root,
                node_version: source.node_version,
                npm_version: source.npm_version,
            })
        })
        .await
        .map_err(|error| invalid(format!("正式工具校验未完成：{error}")))?;
    }
    tokio::task::spawn_blocking(move || verify_sync(root, RELEASE))
        .await
        .map_err(|error| invalid(format!("受管开发工具校验未完成：{error}")))?
}

#[cfg(all(test, windows))]
mod windows_tests {
    include!("managed_tool_files_windows_tests.rs");
}

#[cfg(all(test, windows))]
#[path = "managed_toolchain_windows_tests.rs"]
mod windows_payload_tests;

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
    use tempfile::{Builder, TempDir};

    fn sample() -> (TempDir, PathBuf, String) {
        let temp = Builder::new()
            .prefix("disposable-managed-toolchain-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap().join("development-node");
        let entries = [
            ("bin/node", "synthetic node, never executed"),
            ("bin/npm", "synthetic npm shim, never executed"),
            ("bin/npx", "synthetic npx shim, never executed"),
            (
                "lib/node_modules/npm/bin/npm-cli.js",
                "synthetic npm JS, never executed",
            ),
            (
                "lib/node_modules/npm/package.json",
                "{\"version\":\"11.19.1\"}",
            ),
        ];
        let mut hash = Sha256::new();
        let mut total = 0;
        for (name, contents) in entries {
            let file = root.join(name);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(&file, contents).unwrap();
            fs::set_permissions(
                &file,
                fs::Permissions::from_mode(if name.starts_with("bin/") {
                    0o755
                } else {
                    0o644
                }),
            )
            .unwrap();
            total += contents.len();
            hash.update(format!(
                "{}\0{}\0{:x}\n",
                name,
                contents.len(),
                Sha256::digest(contents.as_bytes())
            ));
        }
        let platform = if cfg!(target_os = "macos") {
            "darwin"
        } else {
            std::env::consts::OS
        };
        let arch = if cfg!(target_arch = "aarch64") {
            "arm64"
        } else {
            std::env::consts::ARCH
        };
        let release = serde_json::to_string(&json!({
            "schemaVersion":1,"nodeVersion":"24.22.0","npmVersion":"11.19.1",
            "platform":platform,"arch":arch,"archiveName":"synthetic-never-executed.tar.gz",
            "archiveSha256":"0".repeat(64),
            "tree":{"rule":"sorted-utf8-path-nul-size-nul-sha256-lf-v1","fileCount":entries.len(),"totalBytes":total,"sha256":format!("{:x}", hash.finalize())},
            "executables":["bin/node","bin/npm","bin/npx"]
        })).unwrap();
        fs::write(root.join(METADATA_FILE), &release).unwrap();
        fs::set_permissions(root.join(METADATA_FILE), fs::Permissions::from_mode(0o644)).unwrap();
        (temp, root, release)
    }

    #[test]
    fn anchored_tree_accepts_normal_and_read_only_permissions_without_changes() {
        let (_temp, root, release) = sample();
        let verified = verify_sync(root.clone(), &release).unwrap();
        assert_eq!(
            verified.npm,
            root.join("lib/node_modules/npm/bin/npm-cli.js")
        );
        fs::set_permissions(root.join("bin/node"), fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(root.join(METADATA_FILE), fs::Permissions::from_mode(0o444)).unwrap();
        verify_sync(root.clone(), &release).unwrap();
        assert_eq!(
            fs::metadata(root.join("bin/node")).unwrap().mode() & 0o777,
            0o555
        );
        assert_eq!(
            fs::read_to_string(root.join(METADATA_FILE)).unwrap(),
            release
        );
    }

    #[test]
    fn package_metadata_cannot_replace_the_source_trust_anchor() {
        let (_temp, root, release) = sample();
        fs::write(
            root.join(METADATA_FILE),
            release.replace("24.22.0", "24.99.0"),
        )
        .unwrap();
        assert!(verify_sync(root, &release).is_err());
    }

    #[test]
    fn changed_missing_extra_and_empty_directory_entries_fail_closed() {
        for change in ["changed", "missing", "extra", "empty-directory"] {
            let (_temp, root, release) = sample();
            match change {
                "changed" => fs::write(root.join("bin/node"), "altered node").unwrap(),
                "missing" => fs::remove_file(root.join("bin/npm")).unwrap(),
                "extra" => fs::write(root.join("extra.js"), "unexpected file").unwrap(),
                _ => fs::create_dir(root.join("empty")).unwrap(),
            }
            assert!(verify_sync(root, &release).is_err(), "accepted {change}");
        }
    }

    #[test]
    fn symbolic_links_hard_links_and_special_files_are_rejected() {
        for kind in ["symlink", "hardlink", "fifo"] {
            let (_temp, root, release) = sample();
            let target = root.join("bin/npm");
            fs::remove_file(&target).unwrap();
            match kind {
                "symlink" => symlink("node", &target).unwrap(),
                "hardlink" => fs::hard_link(root.join("bin/node"), &target).unwrap(),
                _ => {
                    let name = std::ffi::CString::new(target.to_str().unwrap()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o644) }, 0);
                }
            }
            assert!(verify_sync(root, &release).is_err(), "accepted {kind}");
        }
    }

    #[test]
    fn input_parent_link_and_unexpected_permissions_are_rejected() {
        let (_temp, root, release) = sample();
        let link = root.parent().unwrap().join("linked");
        symlink(root.parent().unwrap(), &link).unwrap();
        assert!(verify_sync(link.join("development-node"), &release).is_err());
        fs::set_permissions(root.join("bin/node"), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(verify_sync(root, &release).is_err());
    }

    #[test]
    fn package_directory_permissions_are_checked_including_the_root() {
        let (_temp, root, release) = sample();
        for directory in [&root, &root.join("lib/node_modules")] {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o555)).unwrap();
            verify_sync(root.clone(), &release).unwrap();
            fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).unwrap();
            for mode in [0o700, 0o775, 0o777, 0o1755] {
                fs::set_permissions(directory, fs::Permissions::from_mode(mode)).unwrap();
                assert!(
                    verify_sync(root.clone(), &release).is_err(),
                    "accepted directory mode {mode:o}"
                );
                fs::set_permissions(directory, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
    }

    #[test]
    fn source_release_is_well_formed_for_the_current_supported_platform() {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            release(RELEASE).unwrap();
        } else {
            assert_eq!(
                release(RELEASE).err().unwrap().code,
                "MANAGED_NODE_PLATFORM_UNSUPPORTED"
            );
        }
    }
}
