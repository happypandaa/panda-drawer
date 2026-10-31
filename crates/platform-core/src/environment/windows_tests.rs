use super::*;
use std::{collections::BTreeMap, ffi::OsString, fs};

fn verified(root: PathBuf) -> VerifiedToolchain {
    VerifiedToolchain {
        node: root.join("node.exe"),
        npm: root.join("node_modules/npm/bin/npm-cli.js"),
        root,
        node_version: "24.22.0".into(),
        npm_version: "11.19.1".into(),
    }
}

#[test]
fn native_path_discovers_only_node_exe_and_preserves_order_without_duplicates() {
    let temp = tempfile::Builder::new()
        .prefix("disposable-windows-node 中文 space-")
        .tempdir()
        .unwrap();
    let first = temp.path().join("first 安装");
    let second = temp.path().join("second space");
    let unix = temp.path().join("unix-only");
    for directory in [&first, &second, &unix] {
        fs::create_dir(directory).unwrap();
    }
    fs::write(first.join("node.exe"), b"fixture, never executed").unwrap();
    fs::write(second.join("node.exe"), b"fixture, never executed").unwrap();
    fs::write(unix.join("node"), b"not the Windows executable").unwrap();
    fs::write(unix.join("npm.cmd"), b"not a Node executable").unwrap();
    let path = std::env::join_paths([&first, &unix, &first, &second]).unwrap();
    assert_eq!(
        candidates_from_path(Some(&path)),
        vec![first.join("node.exe"), second.join("node.exe")]
    );
    assert!(candidates_from_path(None).is_empty());
}

#[test]
fn local_node_path_uses_native_separators_and_reported_system_directories() {
    let node = Path::new(r"D:\用户 中文 space;&\Node\node.exe");
    let system = SystemPaths::current().unwrap();
    let path = tool_path(node).unwrap();
    assert_eq!(
        std::env::split_paths(&path).collect::<Vec<_>>(),
        vec![
            node.parent().unwrap().to_path_buf(),
            system.system,
            system.windows
        ]
    );
}

#[test]
fn managed_probe_uses_verified_root_and_only_explicit_native_environment() {
    let verified = verified(PathBuf::from(r"D:\安装 中文 space\development-node"));
    let system = SystemPaths::current().unwrap();
    let path = managed_path(&verified).unwrap();
    assert_eq!(
        std::env::split_paths(&path).collect::<Vec<_>>(),
        vec![
            verified.root.clone(),
            system.system.clone(),
            system.windows.clone()
        ]
    );
    let command = managed_command(&verified, &path).unwrap();
    let command = command.as_std();
    assert_eq!(command.get_program(), verified.node.as_os_str());
    assert_eq!(command.get_current_dir(), Some(verified.root.as_path()));
    let environment: BTreeMap<OsString, OsString> = command
        .get_envs()
        .map(|(key, value)| (key.to_os_string(), value.unwrap().to_os_string()))
        .collect();
    assert_eq!(environment.len(), 17);
    assert_eq!(
        environment.get(OsStr::new("PATH")),
        Some(&OsString::from(path))
    );
    assert_eq!(
        environment.get(OsStr::new("HOME")),
        Some(&verified.root.as_os_str().to_os_string())
    );
    assert_eq!(
        environment.get(OsStr::new("SystemRoot")),
        Some(&system.windows.as_os_str().to_os_string())
    );
    assert_eq!(
        environment.get(OsStr::new("WINDIR")),
        Some(&system.windows.as_os_str().to_os_string())
    );
    assert_eq!(
        environment.get(OsStr::new("ComSpec")),
        Some(&system.system.join("cmd.exe").into_os_string())
    );
    assert_eq!(
        environment.get(OsStr::new("LC_ALL")),
        Some(&OsString::from("C"))
    );
    assert_eq!(
        environment.get(OsStr::new("NPM_CONFIG_USERCONFIG")),
        Some(&verified.root.join(".unused-user-npmrc").into_os_string())
    );
    assert_eq!(
        environment.get(OsStr::new("NPM_CONFIG_GLOBALCONFIG")),
        Some(&verified.root.join(".unused-global-npmrc").into_os_string())
    );
    for key in ["USERPROFILE", "APPDATA", "LOCALAPPDATA", "TEMP", "TMP"] {
        assert_eq!(
            environment.get(OsStr::new(key)),
            Some(&verified.root.as_os_str().to_os_string())
        );
    }
    assert_eq!(
        environment.get(OsStr::new("NODE_DISABLE_COMPILE_CACHE")),
        Some(&OsString::from("1"))
    );
    assert_eq!(
        environment.get(OsStr::new("NPM_CONFIG_CACHE")),
        Some(&verified.root.join(".npm-cache").into_os_string())
    );
    assert_eq!(
        environment.get(OsStr::new("NPM_CONFIG_UPDATE_NOTIFIER")),
        Some(&OsString::from("false"))
    );
    assert_eq!(
        environment.get(OsStr::new("NPM_CONFIG_LOGS_MAX")),
        Some(&OsString::from("0"))
    );
}
