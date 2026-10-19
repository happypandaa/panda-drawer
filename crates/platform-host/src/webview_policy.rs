//! Reject WebView2 startup overrides before creating its private cache.
//!
//! Microsoft documents these per-application policy subkeys and environment
//! replacements in CreateCoreWebView2EnvironmentWithOptions:
//! https://learn.microsoft.com/en-us/microsoft-edge/webview2/reference/win32/webview2-idl#createcorewebview2environmentwithoptions
//! Policy values are queried for existence only; their contents are never read
//! into the report or included in errors. The registry is never modified.

use std::{ffi::OsString, ptr};
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS},
    System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
        KEY_QUERY_VALUE, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_SAM_FLAGS,
    },
    UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND},
};

const ENVIRONMENT_OVERRIDES: [&str; 3] = [
    "WEBVIEW2_USER_DATA_FOLDER",
    "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
    "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
];
const POLICY_SUBKEYS: [&str; 3] = [
    "UserDataFolder",
    "BrowserExecutableFolder",
    "AdditionalBrowserArguments",
];
const POLICY_ROOT: &str = r"Software\Policies\Microsoft\Edge\WebView2";

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn check_environment_with(mut read: impl FnMut(&str) -> Option<OsString>) -> Result<(), String> {
    for name in ENVIRONMENT_OVERRIDES {
        if read(name).is_some_and(|value| !value.is_empty()) {
            return Err(format!(
                "检测到 WebView2 启动覆盖环境变量 {name}。为保护现有数据，本次未创建浏览窗口；请使用未设置此变量的启动环境重试。"
            ));
        }
    }
    Ok(())
}

fn policy_targets(executable_name: &str) -> [&str; 3] {
    [crate::APP_IDENTIFIER, executable_name, "*"]
}

fn policy_path(subkey: &str) -> String {
    format!(r"{POLICY_ROOT}\{subkey}")
}

fn check_registry_with(
    executable_name: &str,
    mut value_exists: impl FnMut(&str, &str) -> Result<bool, String>,
) -> Result<(), String> {
    for subkey in POLICY_SUBKEYS {
        for target in policy_targets(executable_name) {
            // An applicable value is refused even if it is empty or has an
            // unexpected type. We do not infer that a malformed override is safe.
            // "*" is deliberately also checked for UserDataFolder, although the
            // current Microsoft documentation disallows that wildcard there.
            if value_exists(subkey, target)? {
                return Err(format!(
                    "检测到适用于本应用的 WebView2 {subkey} 策略。为保护现有数据，本次未创建浏览窗口；请检查本应用的 WebView2 启动策略后重试。"
                ));
            }
        }
    }
    Ok(())
}

fn absent(status: u32) -> bool {
    matches!(status, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND)
}

fn query_result(status: u32) -> Result<bool, String> {
    match status {
        ERROR_SUCCESS | ERROR_MORE_DATA => Ok(true),
        status if absent(status) => Ok(false),
        _ => Err(format!(
            "无法只读核对 WebView2 启动策略（系统错误 {status}）；本次未创建浏览窗口。"
        )),
    }
}

struct PolicyKey(HKEY);

impl Drop for PolicyKey {
    fn drop(&mut self) {
        // SAFETY: This handle is owned by PolicyKey after a successful open.
        unsafe { RegCloseKey(self.0) };
    }
}

fn policy_value_exists(
    hive: HKEY,
    view: REG_SAM_FLAGS,
    subkey: &str,
    target: &str,
) -> Result<bool, String> {
    let path = wide(&policy_path(subkey));
    let mut key = ptr::null_mut();
    // SAFETY: path is NUL terminated; key is valid writable output. Access is
    // limited to query-only rights and an explicit registry view.
    let opened = unsafe { RegOpenKeyExW(hive, path.as_ptr(), 0, KEY_QUERY_VALUE | view, &mut key) };
    if absent(opened) {
        return Ok(false);
    }
    if opened != ERROR_SUCCESS {
        return Err(format!(
            "无法只读打开 WebView2 启动策略（系统错误 {opened}）；本次未创建浏览窗口。"
        ));
    }
    let key = PolicyKey(key);
    let name = wide(target);
    let mut byte_count = 0;
    // SAFETY: The key is open for querying, the value name is NUL terminated,
    // and no data buffer is supplied. This reads only existence/size metadata.
    let queried = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut byte_count,
        )
    };
    query_result(queried)
}

pub(super) fn check() -> Result<(), String> {
    check_environment_with(|name| std::env::var_os(name))?;
    let executable = std::env::current_exe()
        .map_err(|_| "无法核对当前应用名称；本次未创建浏览窗口。".to_owned())?;
    let executable_name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty() && !name.contains('\0'))
        .ok_or_else(|| "无法核对当前应用名称；本次未创建浏览窗口。".to_owned())?;
    // Microsoft checks HKLM and HKCU. Reading both registry views is more
    // conservative than relying on implicit loader/registry redirection.
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for view in [KEY_WOW64_64KEY, KEY_WOW64_32KEY] {
            check_registry_with(executable_name, |subkey, target| {
                policy_value_exists(hive, view, subkey, target)
            })?;
        }
    }
    Ok(())
}

pub(super) fn show_startup_error(message: &str) {
    let safe_message: String = message
        .chars()
        .filter(|character| *character != '\0')
        .take(4096)
        .collect();
    let message = wide(&format!(
        "Windows 只读应用未能启动。\n\n{safe_message}\n\n本次没有启动项目运行时，也没有创建或切换业务工作区。"
    ));
    let title = wide("本地项目 · 无法启动");
    // SAFETY: Both strings are NUL terminated and live for the synchronous call.
    // A null owner is intentional because startup may fail before any window.
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

    #[test]
    fn missing_and_empty_environment_values_do_not_add_an_override() {
        assert_eq!(check_environment_with(|_| None), Ok(()));
        assert_eq!(check_environment_with(|_| Some(OsString::new())), Ok(()));
    }

    #[test]
    fn every_nonempty_environment_override_is_blocked_without_echoing_its_value() {
        for blocked in ENVIRONMENT_OVERRIDES {
            let error = check_environment_with(|name| {
                (name == blocked).then(|| OsString::from("private-synthetic-value"))
            })
            .unwrap_err();
            assert!(error.contains(blocked));
            assert!(!error.contains("private-synthetic-value"));
        }
    }

    #[test]
    fn exact_documented_policy_subkeys_and_applicable_value_names_are_checked() {
        let mut observed = Vec::new();
        assert_eq!(
            check_registry_with("fixture.exe", |subkey, target| {
                observed.push((policy_path(subkey), target.to_owned()));
                Ok(false)
            }),
            Ok(())
        );
        assert_eq!(observed.len(), 9);
        for (index, subkey) in POLICY_SUBKEYS.iter().enumerate() {
            for (offset, target) in policy_targets("fixture.exe").iter().enumerate() {
                assert_eq!(
                    observed[index * 3 + offset],
                    (format!(r"{POLICY_ROOT}\{subkey}"), (*target).to_owned())
                );
            }
        }
    }

    #[test]
    fn any_applicable_policy_value_blocks_including_conservative_wildcard_checks() {
        for blocked_subkey in POLICY_SUBKEYS {
            for blocked_target in policy_targets("fixture.exe") {
                let error = check_registry_with("fixture.exe", |subkey, target| {
                    Ok(subkey == blocked_subkey && target == blocked_target)
                })
                .unwrap_err();
                assert!(error.contains(blocked_subkey));
                assert!(!error.contains("fixture.exe"));
                assert!(!error.contains(crate::APP_IDENTIFIER));
            }
        }
    }

    #[test]
    fn registry_read_failure_never_becomes_absence() {
        assert_eq!(query_result(ERROR_FILE_NOT_FOUND), Ok(false));
        assert_eq!(query_result(ERROR_PATH_NOT_FOUND), Ok(false));
        assert_eq!(query_result(ERROR_SUCCESS), Ok(true));
        assert_eq!(query_result(ERROR_MORE_DATA), Ok(true));
        assert!(query_result(ERROR_ACCESS_DENIED).is_err());
        assert!(query_result(987654).is_err());
        assert_eq!(
            check_registry_with("fixture.exe", |_, _| Err("synthetic read failure".into())),
            Err("synthetic read failure".into())
        );
    }
}
