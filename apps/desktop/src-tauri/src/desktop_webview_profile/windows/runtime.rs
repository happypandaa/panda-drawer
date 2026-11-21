use platform_core::ApiError;

pub(super) fn checked_version<E: std::fmt::Display>(
    query: Result<String, E>,
) -> Result<String, ApiError> {
    match query {
        Ok(version) if !version.trim().is_empty() => Ok(version),
        result => Err(ApiError::new(
            "WEBVIEW2_RUNTIME_UNAVAILABLE",
            "熊猫小抽屉暂时无法启动：Microsoft Edge WebView2 运行时未安装，或当前 Windows 用户无法使用它。\n\n请从微软官方网站安装或修复 WebView2 Evergreen 运行时，然后重新打开熊猫小抽屉。若仍失败，请将此提示完整截图发给 AI 或技术支持协助排查。\n\n官方下载：https://developer.microsoft.com/microsoft-edge/webview2/",
        )
        .detail("cause", match result { Err(error) => error.to_string(), Ok(_) => "Runtime version is empty".into() })
        .outcome("not_started")),
    }
}

pub(super) fn failure_message(error: &ApiError) -> String {
    use crate::ui_locale::text;
    format!(
        "{}\n\n{}: {}\n{}: {}",
        text(&error.message),
        text("错误代码"),
        error.code,
        text("系统返回"),
        error.details["cause"]
            .as_str()
            .unwrap_or_else(|| text("未提供详细信息")),
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
            assert!(error.message.contains("WebView2 Evergreen"));
            assert!(error
                .message
                .contains("https://developer.microsoft.com/microsoft-edge/webview2/"));
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
            assert!(message.contains("WebView2 Evergreen"));
            assert!(message.contains("https://developer.microsoft.com/microsoft-edge/webview2/"));
        }
    }
}
