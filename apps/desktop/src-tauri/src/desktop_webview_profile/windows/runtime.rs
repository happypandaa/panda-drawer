use platform_core::ApiError;

/// Official website origin. The help page is published at
/// `{origin}/wiki/webview2/` (Chinese) and `{origin}/en/wiki/webview2/`.
const WEBSITE_ORIGIN: &str = "https://panda.zhimibuhui.com";

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

/// The page the "查看解决办法" button opens for this failure.
pub(super) fn failure_help_url(error: &ApiError) -> String {
    help_url(crate::ui_locale::current(), cause(error))
}

pub(super) fn checked_version<E: std::fmt::Display>(
    query: Result<String, E>,
) -> Result<String, ApiError> {
    match query {
        Ok(version) if !version.trim().is_empty() => Ok(version),
        result => Err(ApiError::new(
            "WEBVIEW2_RUNTIME_UNAVAILABLE",
            UNAVAILABLE_MESSAGE,
        )
        .detail("cause", match result { Err(error) => error.to_string(), Ok(_) => "Runtime version is empty".into() })
        .outcome("not_started")),
    }
}

pub(super) fn failure_message(error: &ApiError) -> String {
    use crate::ui_locale::text;
    let cause = match cause(error) {
        "" => text("未提供详细信息"),
        cause => cause,
    };
    format!(
        "{}\n\n{}: {}\n{}: {}\n\n{}: {}",
        text(&error.message),
        text("错误代码"),
        error.code,
        text("系统返回"),
        cause,
        text("解决办法"),
        failure_help_url(error),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ("other", "unknown", "https://panda.zhimibuhui.com/wiki/webview2/"),
        ] {
            assert_eq!(help_url(locale, cause), expected, "locale={locale}, cause={cause}");
        }
        assert!(WEBSITE_ORIGIN.starts_with("https://"));
    }
}
