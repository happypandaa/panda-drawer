//! Platform names and paths for the two independently pinned Node payloads.
//! This selects installation facts; it never executes a tool or grants write access.
use std::path::{Path, PathBuf};

#[cfg(windows)]
pub(crate) const RUNTIME_RELEASE: &str =
    include_str!("../../../scripts/node-runtime-release.windows-x64.json");
#[cfg(not(windows))]
pub(crate) const RUNTIME_RELEASE: &str = include_str!("../../../scripts/node-runtime-release.json");
#[cfg(windows)]
pub(crate) const DEVELOPMENT_RELEASE: &str =
    include_str!("../../../scripts/development-node-release.windows-x64.json");
#[cfg(not(windows))]
pub(crate) const DEVELOPMENT_RELEASE: &str =
    include_str!("../../../scripts/development-node-release.json");

pub(crate) fn platform() -> &'static str {
    if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        std::env::consts::OS
    }
}

pub(crate) fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        "x64"
    } else {
        std::env::consts::ARCH
    }
}

pub(crate) fn required_executables() -> &'static [&'static str] {
    if cfg!(windows) {
        &["node.exe"]
    } else {
        &["bin/node", "bin/npm", "bin/npx"]
    }
}

pub(crate) fn node(root: &Path) -> PathBuf {
    root.join(if cfg!(windows) {
        "node.exe"
    } else {
        "bin/node"
    })
}

pub(crate) fn npm(root: &Path) -> PathBuf {
    root.join(if cfg!(windows) {
        "node_modules/npm/bin/npm-cli.js"
    } else {
        "lib/node_modules/npm/bin/npm-cli.js"
    })
}

pub(crate) fn npm_entry_candidates(candidate: &Path, node_path: &Path) -> [PathBuf; 4] {
    #[cfg(windows)]
    {
        let candidate_root = candidate.parent().unwrap();
        let resolved_root = node_path.parent().unwrap();
        [
            candidate_root.join("node_modules/npm/bin/npm-cli.js"),
            resolved_root.join("node_modules/npm/bin/npm-cli.js"),
            candidate_root.join("npm-cli.js"),
            resolved_root.join("npm-cli.js"),
        ]
    }
    #[cfg(not(windows))]
    {
        [
            candidate
                .parent()
                .unwrap()
                .join("../lib/node_modules/npm/bin/npm-cli.js"),
            node_path
                .parent()
                .unwrap()
                .join("../lib/node_modules/npm/bin/npm-cli.js"),
            candidate.parent().unwrap().join("npm"),
            node_path.parent().unwrap().join("npm"),
        ]
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn windows_layout_selects_the_independent_fixed_payloads() {
        let root = Path::new(r"C:\安装 中文 space\development-node");
        assert_eq!(node(root), root.join("node.exe"));
        assert_eq!(npm(root), root.join("node_modules/npm/bin/npm-cli.js"));
        let runtime: serde_json::Value = serde_json::from_str(RUNTIME_RELEASE).unwrap();
        let development: serde_json::Value = serde_json::from_str(DEVELOPMENT_RELEASE).unwrap();
        assert_eq!(runtime["platform"], platform());
        assert_eq!(runtime["arch"], arch());
        assert_eq!(runtime["binary"]["path"], "node.exe");
        assert_eq!(development["platform"], platform());
        assert_eq!(development["arch"], arch());
        assert_eq!(required_executables(), &["node.exe"]);
        let resolved = Path::new(r"D:\moved 中文\node.exe");
        let entries = npm_entry_candidates(&node(root), resolved);
        assert_eq!(entries[0], npm(root));
        assert_eq!(entries[1], npm(resolved.parent().unwrap()));
        assert!(entries
            .iter()
            .all(|entry| entry.extension().is_some_and(|ext| ext == "js")));
    }
}
