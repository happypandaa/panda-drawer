use platform_core::ApiError;

#[path = "runtime/installer.rs"]
mod installer;

const POST_INSTALL_ATTEMPTS: usize = 20;

/// Official website origin. The help page is published at
/// `{origin}/wiki/webview2/` (Chinese) and `{origin}/en/wiki/webview2/`.
const WEBSITE_ORIGIN: &str = "https://panda.zhimibuhui.com";
const MICROSOFT_DOWNLOAD: &str = "https://go.microsoft.com/fwlink/?linkid=2124701";

const UNAVAILABLE_MESSAGE: &str = "熊猫小抽屉暂时无法启动：这台电脑缺少微软的 WebView2 组件，或当前 Windows 用户无法使用它。\n\n点「查看解决办法」，按教程安装或修复后，重新打开熊猫小抽屉。若仍失败，请将此提示完整截图发给 AI 或技术支持协助排查。";

/// Tutorial page for the current interface language. Known loader failures
/// jump straight to their section; anything else opens the page top.
pub(super) fn help_url(locale: &str, cause: &str) -> String {
    let prefix = if locale == "en" { "/en" } else { "" };
    let cause = cause.to_ascii_lowercase();
    let anchor = if cause.contains("0x80070005") {
        "#access-denied"
    } else if cause.contains("0x80070002") {
        "#not-installed"
    } else {
        ""
    };
    format!("{WEBSITE_ORIGIN}{prefix}/wiki/webview2/{anchor}")
}

fn cause(error: &ApiError) -> &str {
    error.details["cause"].as_str().unwrap_or("")
}

fn is_missing(error: &ApiError) -> bool {
    let cause = cause(error).to_ascii_lowercase();
    !cause.contains("0x80070005")
        && !cause.contains("0x800401f0")
        && (cause.contains("0x80070002") || cause == "runtime version is empty")
}

pub(super) fn ensure_available() -> Result<String, ApiError> {
    ensure_available_with(
        || checked_version(tauri::webview_version()),
        || tauri::async_runtime::block_on(installer::install()),
        || std::thread::sleep(std::time::Duration::from_millis(500)),
    )
}

fn ensure_available_with(
    mut query: impl FnMut() -> Result<String, ApiError>,
    install: impl FnOnce() -> Result<(), String>,
    mut wait: impl FnMut(),
) -> Result<String, ApiError> {
    let initial = match query() {
        Ok(version) => return Ok(version),
        Err(error) if is_missing(&error) => error,
        Err(error) => return Err(error),
    };
    let installation = install();
    // Always query again, including when another installer supplied the runtime
    // while ours was cancelled/failed. Only actual availability resumes setup.
    // A successful bootstrapper may finish before loader registration is visible.
    for attempt in 0..POST_INSTALL_ATTEMPTS {
        let error = match query() {
            Ok(version) => return Ok(version),
            Err(error) => error,
        };
        if installation.is_ok() && is_missing(&error) && attempt + 1 < POST_INSTALL_ATTEMPTS {
            wait();
            continue;
        }
        let error = error.detail("initialCause", cause(&initial));
        return Err(match installation {
            Ok(()) => error.detail("installationCompleted", true),
            Err(failure) => error.detail("installationFailure", failure),
        });
    }
    unreachable!("The final probe returns its original loader error")
}

/// The page the "查看解决办法" button opens for this failure.
pub(super) fn failure_help_url(error: &ApiError) -> String {
    help_url(crate::ui_locale::current(), cause(error))
}

pub(super) fn manual_install_help() -> String {
    format!(
        "{}\n{}",
        crate::ui_locale::text(
            "也可下载微软的 WebView2 64 位离线安装包（数百 MB），安装完成后重新打开熊猫小抽屉。"
        ),
        MICROSOFT_DOWNLOAD,
    )
}

pub(super) fn checked_version<E: std::fmt::Display>(
    query: Result<String, E>,
) -> Result<String, ApiError> {
    match query {
        Ok(version) if !version.trim().is_empty() => Ok(version),
        result => Err(
            ApiError::new("WEBVIEW2_RUNTIME_UNAVAILABLE", UNAVAILABLE_MESSAGE)
                .detail(
                    "cause",
                    match result {
                        Err(error) => error.to_string(),
                        Ok(_) => "Runtime version is empty".into(),
                    },
                )
                .outcome("not_started"),
        ),
    }
}

pub(super) fn failure_message(error: &ApiError) -> String {
    use crate::ui_locale::text;
    let cause = match cause(error) {
        "" => text("未提供详细信息"),
        cause => cause,
    };
    let mut message = format!(
        "{}\n\n{}: {}\n{}: {}",
        text(&error.message),
        text("错误代码"),
        error.code,
        text("系统返回"),
        cause,
    );
    if let Some(initial) = error.details["initialCause"]
        .as_str()
        .filter(|initial| *initial != cause)
    {
        message.push_str(&format!("\n{}: {}", text("首次检测"), initial));
    }
    if let Some(failure) = error.details["installationFailure"].as_str() {
        message.push_str(&format!("\n\n{}: {}", text("自动安装未完成"), failure));
    } else if error.details["installationCompleted"].as_bool() == Some(true) {
        message.push_str(&format!(
            "\n\n{}",
            text("自动安装已结束，但 WebView2 仍不可用。")
        ));
    }
    message.push_str(&format!(
        "\n\n{}: {}",
        text("解决办法"),
        failure_help_url(error)
    ));
    message.push_str(&format!("\n\n{}", manual_install_help()));
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::VecDeque};

    const MISSING: &str = "WebView2 Runtime not found (0x80070002)";

    #[test]
    fn available_or_non_missing_runtime_never_starts_an_installer() {
        for query in [
            Ok("157.0".to_owned()),
            Err("Access is denied (0x80070005)"),
            Err("COM not initialized (0x800401F0)"),
            Err("Unknown loader failure"),
            Err("0x80070002; 0x80070005"),
            Err("0x80070002; 0x800401F0"),
        ] {
            let expected = query.clone();
            let result = ensure_available_with(
                || checked_version(query.clone()),
                || panic!("An available or inaccessible runtime must not be reinstalled"),
                || panic!("No post-install wait is needed"),
            );
            match expected {
                Ok(version) => assert_eq!(result.unwrap(), version),
                Err(cause) => assert_eq!(result.unwrap_err().details["cause"], cause),
            }
        }
    }

    #[test]
    fn missing_runtime_installs_once_and_resumes_only_after_a_real_successful_query() {
        let mut queries = VecDeque::from([Err(MISSING), Err(MISSING), Ok("157.0".to_owned())]);
        let installs = Cell::new(0);
        let waits = Cell::new(0);
        let version = ensure_available_with(
            || checked_version(queries.pop_front().expect("Only the required probes run")),
            || {
                installs.set(installs.get() + 1);
                Ok(())
            },
            || waits.set(waits.get() + 1),
        )
        .unwrap();
        assert_eq!(version, "157.0");
        assert_eq!(installs.get(), 1);
        assert_eq!(waits.get(), 1);
        assert!(queries.is_empty());
    }

    #[test]
    fn actual_availability_wins_even_when_the_installer_reports_failure() {
        let mut queries = VecDeque::from([Err(MISSING), Ok("157.0".to_owned())]);
        assert_eq!(
            ensure_available_with(
                || checked_version(queries.pop_front().unwrap()),
                || Err("Another installer was running".into()),
                || panic!("The first post-install probe already succeeded"),
            )
            .unwrap(),
            "157.0"
        );
    }

    #[test]
    fn failed_installation_preserves_download_or_installer_and_loader_diagnostics() {
        let mut queries = VecDeque::from([
            Err::<String, _>(MISSING),
            Err("Access is denied (0x80070005)"),
        ]);
        let error = ensure_available_with(
            || checked_version(queries.pop_front().unwrap()),
            || Err("Download failed: network timed out".into()),
            || panic!("A failed installation must not start a retry loop"),
        )
        .unwrap_err();
        assert_eq!(error.details["initialCause"], MISSING);
        assert_eq!(error.details["cause"], "Access is denied (0x80070005)");
        assert_eq!(
            error.details["installationFailure"],
            "Download failed: network timed out"
        );
        assert_eq!(error.details["operationOutcome"], "not_started");
        let message = failure_message(&error);
        assert!(message.contains(MISSING));
        assert!(message.contains("Access is denied (0x80070005)"));
        assert!(message.contains("network timed out"));
        assert!(message.contains("WEBVIEW2_RUNTIME_UNAVAILABLE"));
        assert!(message.contains("/wiki/webview2/#access-denied"));
    }

    #[test]
    fn successful_installer_cannot_resume_startup_without_a_runtime_or_loop_forever() {
        let probes = Cell::new(0);
        let installs = Cell::new(0);
        let waits = Cell::new(0);
        let error = ensure_available_with(
            || {
                probes.set(probes.get() + 1);
                checked_version::<&str>(Ok(String::new()))
            },
            || {
                installs.set(installs.get() + 1);
                Ok(())
            },
            || waits.set(waits.get() + 1),
        )
        .unwrap_err();
        assert_eq!(installs.get(), 1);
        assert_eq!(probes.get(), 1 + POST_INSTALL_ATTEMPTS);
        assert_eq!(waits.get(), POST_INSTALL_ATTEMPTS - 1);
        assert_eq!(error.details["installationCompleted"], true);
        assert_eq!(error.details["initialCause"], "Runtime version is empty");
    }

    #[test]
    fn installer_configuration_requires_the_automatic_webview2_hook_for_current_user() {
        let config: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tauri.windows.conf.json"
        )))
        .unwrap();
        assert_eq!(
            config["bundle"]["windows"]["webviewInstallMode"]["type"],
            "skip"
        );
        // The custom hook owns automatic setup and continues host installation
        // on failure; enabling the stock downloader would abort before the hook.
        assert_eq!(
            config["bundle"]["windows"]["nsis"]["installerHooks"],
            "windows-webview2.nsh"
        );
        assert_eq!(
            config["bundle"]["windows"]["nsis"]["installMode"],
            "currentUser"
        );
        let hook = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/windows-webview2.nsh"));
        assert!(hook.contains("https://go.microsoft.com/fwlink/p/?LinkId=2124703"));
        assert!(hook.contains(MICROSOFT_DOWNLOAD));
        assert!(!hook.contains("developer.microsoft.com"));
    }

    #[test]
    fn missing_or_unobservable_runtime_requires_native_install_guidance() {
        for query in [
            Err("simulated loader: runtime not found"),
            Ok(String::new()),
            Ok("  ".into()),
        ] {
            let error = checked_version(query).unwrap_err();
            assert_eq!(error.code, "WEBVIEW2_RUNTIME_UNAVAILABLE");
            assert_eq!(error.details["operationOutcome"], "not_started");
            assert!(error.message.contains("WebView2"));
            assert!(error.message.contains("查看解决办法"));
            assert!(!error.details["cause"].as_str().unwrap().is_empty());
        }
        assert_eq!(
            checked_version::<&str>(Ok("156.0.4314.50".into())).unwrap(),
            "156.0.4314.50"
        );
    }

    #[test]
    fn native_guidance_preserves_the_loader_failure_for_user_diagnosis() {
        for cause in [
            "WebView2 Runtime not found (0x80070002)",
            "Access is denied. (0x80070005)",
        ] {
            let error = checked_version::<&str>(Err(cause)).unwrap_err();
            let message = failure_message(&error);
            assert!(message.contains(cause));
            assert!(message.contains("WEBVIEW2_RUNTIME_UNAVAILABLE"));
            assert!(message.contains("/wiki/webview2/#"));
            assert!(message.contains(MICROSOFT_DOWNLOAD));
        }
    }

    #[test]
    fn help_url_follows_language_and_known_loader_failures() {
        for (locale, cause, expected) in [
            (
                "zh-CN",
                "WebView2 Runtime not found (0x80070002)",
                "https://panda.zhimibuhui.com/wiki/webview2/#not-installed",
            ),
            (
                "en",
                "Access is denied. (0x80070005)",
                "https://panda.zhimibuhui.com/en/wiki/webview2/#access-denied",
            ),
            (
                "en",
                "Runtime version is empty",
                "https://panda.zhimibuhui.com/en/wiki/webview2/",
            ),
            (
                "zh-CN",
                "0X80070002",
                "https://panda.zhimibuhui.com/wiki/webview2/#not-installed",
            ),
            (
                "en",
                "0X80070005",
                "https://panda.zhimibuhui.com/en/wiki/webview2/#access-denied",
            ),
            (
                "zh-CN",
                "0x80070002; 0x80070005",
                "https://panda.zhimibuhui.com/wiki/webview2/#access-denied",
            ),
            ("en", "", "https://panda.zhimibuhui.com/en/wiki/webview2/"),
            (
                "other",
                "unknown",
                "https://panda.zhimibuhui.com/wiki/webview2/",
            ),
        ] {
            assert_eq!(
                help_url(locale, cause),
                expected,
                "locale={locale}, cause={cause}"
            );
        }
        assert!(WEBSITE_ORIGIN.starts_with("https://"));
    }
}
