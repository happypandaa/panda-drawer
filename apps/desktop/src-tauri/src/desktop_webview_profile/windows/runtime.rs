use platform_core::ApiError;

pub(super) fn checked_version<E: std::fmt::Display>(
    query: Result<String, E>,
) -> Result<String, ApiError> {
    match query {
        Ok(version) if !version.trim().is_empty() => Ok(version),
        result => Err(ApiError::new(
            "WEBVIEW2_RUNTIME_UNAVAILABLE",
            "无法使用 Microsoft Edge WebView2 运行时。请安装当前用户可用的 WebView2 Evergreen 运行时，然后重新启动熊猫小抽屉。\n\n官方下载：https://developer.microsoft.com/microsoft-edge/webview2/",
        )
        .detail("cause", match result { Err(error) => error.to_string(), Ok(_) => "Runtime version is empty".into() })
        .outcome("not_started")),
    }
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
}
