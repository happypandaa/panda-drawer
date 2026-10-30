//! Native release schema and installed paths for the fixed development Git.
//! Selecting a path does not execute Git or admit repository/import operations.
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[cfg(windows)]
pub(super) const RELEASE: &str =
    include_str!("../../../../scripts/development-git-release.windows-x64.json");
#[cfg(not(windows))]
pub(super) const RELEASE: &str = include_str!("../../../../scripts/development-git-release.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Release {
    pub schema_version: u32,
    pub git_version: String,
    pub platform: String,
    pub arch: String,
    pub source: serde_json::Value,
    #[cfg(not(windows))]
    pub build: serde_json::Value,
    #[cfg(not(windows))]
    pub signature: serde_json::Value,
    #[cfg(windows)]
    pub distribution: String,
    #[cfg(windows)]
    pub license: License,
    #[cfg(windows)]
    pub layout: Layout,
    pub tree: Tree,
    pub executables: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Tree {
    pub rule: String,
    pub file_count: usize,
    pub total_bytes: u64,
    pub sha256: String,
}

#[cfg(windows)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct License {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[cfg(windows)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Layout {
    pub git: String,
    pub exec_path: String,
    pub templates_path: String,
}

pub(super) fn metadata_supported(release: &Release) -> bool {
    #[cfg(not(windows))]
    {
        release.build.is_object() && release.signature.is_object()
    }
    #[cfg(windows)]
    {
        release.distribution == "MinGit"
            && release.license.path == "LICENSE.txt"
            && release.license.size > 0
            && release.license.size <= 16 * 1024 * 1024
            && release.license.sha256.len() == 64
            && release
                .license
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && release.layout.git == git_relative()
            && release.layout.exec_path == exec_relative()
            && release.layout.templates_path == templates_relative()
    }
}

pub(super) fn platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        std::env::consts::OS
    }
}

pub(super) fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        "x64"
    } else {
        std::env::consts::ARCH
    }
}

pub(super) fn required_executables() -> &'static [&'static str] {
    if cfg!(windows) {
        &["cmd/git.exe", "ucrt64/bin/git.exe"]
    } else {
        &["bin/git"]
    }
}

fn git_relative() -> &'static str {
    if cfg!(windows) {
        "cmd/git.exe"
    } else {
        "bin/git"
    }
}
fn exec_relative() -> &'static str {
    if cfg!(windows) {
        "ucrt64/libexec/git-core"
    } else {
        "libexec/git-core"
    }
}
fn templates_relative() -> &'static str {
    if cfg!(windows) {
        "ucrt64/share/git-core/templates"
    } else {
        "share/git-core/templates"
    }
}

pub(super) fn required_directories() -> [&'static str; 2] {
    [exec_relative(), templates_relative()]
}

pub(super) fn git(root: &Path) -> PathBuf {
    root.join(git_relative())
}
pub(super) fn exec_path(root: &Path) -> PathBuf {
    root.join(exec_relative())
}
pub(super) fn template_path(root: &Path) -> PathBuf {
    root.join(templates_relative())
}
