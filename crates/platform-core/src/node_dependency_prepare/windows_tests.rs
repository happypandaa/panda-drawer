use super::*;
use crate::{environment::ExecutableInfo, owned_process::OwnedCommand};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    os::windows::ffi::{OsStrExt, OsStringExt},
};

fn tools() -> Toolchain {
    Toolchain {
        node: ExecutableInfo {
            path: r"\\?\D:\工具 中文 space;&\node.exe".into(),
            version: "fixture-never-executed".into(),
        },
        npm: ExecutableInfo {
            path: r"\\?\D:\工具 中文 space;&\node_modules\npm\bin\npm-cli.js".into(),
            version: "fixture-never-executed".into(),
        },
        path: "explicit-owned-toolchain-path".into(),
    }
}

fn prepared(tools: &Toolchain, cwd: &Path, operation: &Path) -> Result<Command> {
    let home = operation.join("npm-home");
    command(
        tools,
        cwd,
        operation,
        home.clone(),
        operation.join("npm-cache"),
        home.join("user.npmrc"),
        home.join("global.npmrc"),
    )
}

#[test]
fn npm_arguments_preserve_script_disabled_policy_and_exact_conventional_paths() {
    let tools = tools();
    let cwd = Path::new(r"\\?\D:\项目 中文 space;&\backend");
    let operation = Path::new(r"\\?\D:\operation 中文 space;&");
    let command = prepared(&tools, cwd, operation).unwrap();
    let request = crate::owned_process::windows::request(OwnedCommand::cleared(command)).unwrap();
    let argument = crate::node_tool_layout::command_argument;
    assert_eq!(request.program, argument(&tools.node.path).unwrap());
    assert_eq!(request.current_directory, argument(cwd).unwrap());
    let mut expected = vec![
        argument(&tools.npm.path).unwrap().into_os_string(),
        "--prefix".into(),
        argument(cwd).unwrap().into_os_string(),
        "--global=false".into(),
        "--logs-dir".into(),
        argument(&operation.join("npm-disabled-output"))
            .unwrap()
            .into_os_string(),
    ];
    expected.extend(
        [
            "ci",
            "--omit=dev",
            "--ignore-scripts",
            "--logs-max=0",
            "--no-audit",
            "--no-fund",
            "--fetch-retries=2",
            "--fetch-timeout=60000",
        ]
        .map(OsString::from),
    );
    assert_eq!(request.arguments, expected);
    assert_eq!(request.program.extension().unwrap(), "exe");
    assert_eq!(
        Path::new(&request.arguments[0]).file_name().unwrap(),
        "npm-cli.js"
    );
}

#[test]
fn cleared_native_request_contains_only_operation_local_home_temp_and_explicit_system_values() {
    let tools = tools();
    let operation = Path::new(r"\\?\D:\operation 中文 space;&");
    let cwd = Path::new(r"\\?\D:\project\backend");
    let command = prepared(&tools, cwd, operation).unwrap();
    let request = crate::owned_process::windows::request(OwnedCommand::cleared(command)).unwrap();
    let environment = request.environment.into_iter().collect::<BTreeMap<_, _>>();
    let argument = crate::node_tool_layout::command_argument;
    let home = argument(&operation.join("npm-home")).unwrap();
    let system = SystemPaths::current().unwrap();
    assert_eq!(environment.len(), 17);
    assert_eq!(environment[OsStr::new("PATH")], OsStr::new(&tools.path));
    assert_eq!(
        environment[OsStr::new("SystemRoot")],
        system.windows.as_os_str()
    );
    assert_eq!(
        environment[OsStr::new("WINDIR")],
        system.windows.as_os_str()
    );
    assert_eq!(
        environment[OsStr::new("ComSpec")],
        system.system.join("cmd.exe").as_os_str()
    );
    for key in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "TEMP",
        "TMP",
    ] {
        assert_eq!(environment[OsStr::new(key)], home.as_os_str());
    }
    for (key, relative) in [
        ("NPM_CONFIG_USERCONFIG", "npm-home/user.npmrc"),
        ("NPM_CONFIG_GLOBALCONFIG", "npm-home/global.npmrc"),
        ("NPM_CONFIG_CACHE", "npm-cache"),
    ] {
        assert_eq!(
            environment[OsStr::new(key)],
            argument(&operation.join(relative)).unwrap().as_os_str()
        );
    }
    assert_eq!(
        environment[OsStr::new("GIT_CEILING_DIRECTORIES")],
        argument(operation).unwrap().as_os_str()
    );
    assert_eq!(environment[OsStr::new("NPM_CONFIG_LOGS_MAX")], "0");
    assert_eq!(
        environment[OsStr::new("NPM_CONFIG_UPDATE_NOTIFIER")],
        "false"
    );
    assert_eq!(environment[OsStr::new("NO_COLOR")], "1");
    for key in [
        "NODE_OPTIONS",
        "NODE_PATH",
        "npm_config_script_shell",
        "HTTP_PROXY",
        "HTTPS_PROXY",
    ] {
        assert!(!environment.contains_key(OsStr::new(key)));
    }
}

#[test]
fn native_names_keep_required_verbatim_namespace_and_unpaired_utf16() {
    let mut tools = tools();
    let mut raw: Vec<u16> = r"\\?\D:\工具\".encode_utf16().collect();
    raw.push(0xd800);
    raw.extend(r"\node.exe".encode_utf16());
    tools.node.path = PathBuf::from(OsString::from_wide(&raw));
    let cwd = Path::new(r"\\?\D:\项目\literal.");
    let operation = Path::new(r"\\?\D:\operation\literal.");
    let command = prepared(&tools, cwd, operation).unwrap();
    let request = crate::owned_process::windows::request(OwnedCommand::cleared(command)).unwrap();
    assert!(request
        .program
        .as_os_str()
        .encode_wide()
        .any(|unit| unit == 0xd800));
    assert_eq!(request.current_directory, cwd);
    assert_eq!(request.arguments[2], cwd.as_os_str());
    assert_eq!(
        request.arguments[5],
        crate::node_tool_layout::command_argument(&operation.join("npm-disabled-output"))
            .unwrap()
            .as_os_str()
    );
}

#[test]
fn invalid_native_program_or_script_path_is_rejected_without_a_shell_fallback() {
    let invalid = PathBuf::from(OsString::from_wide(&[
        b'D' as u16,
        b':' as u16,
        b'\\' as u16,
        0,
        b'x' as u16,
    ]));
    for script in [false, true] {
        let mut tools = tools();
        if script {
            tools.npm.path = invalid.clone();
        } else {
            tools.node.path = invalid.clone();
        }
        assert!(prepared(&tools, Path::new(r"D:\project"), Path::new(r"D:\operation")).is_err());
    }
}
