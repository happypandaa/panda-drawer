//! Runtime admission policy for the current porting stage.
//!
//! This module has no filesystem or process side effects. Its descriptions are
//! policy information, not evidence that a workspace or recovery record has
//! been inspected. Runtime admission must happen before those operations.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostPlatform {
    MacOs,
    Windows,
    Unsupported,
}

impl HostPlatform {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Unsupported
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MacOs => "macos",
            Self::Windows => "windows",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Pure data suitable for a caller's existing UI/CLI/API response format.
/// `runtime_available` describes this policy, not full product acceptance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeSupport {
    pub platform: HostPlatform,
    pub runtime_available: bool,
    pub error_code: Option<&'static str>,
    pub message: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeSupportError {
    pub platform: HostPlatform,
    pub code: &'static str,
    pub message: &'static str,
}

impl fmt::Display for RuntimeSupportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for RuntimeSupportError {}

/// Describe an explicit host for diagnostics and policy regression checks.
/// This function cannot admit the running host or change its policy.
pub const fn support_for(platform: HostPlatform) -> RuntimeSupport {
    match platform {
        HostPlatform::MacOs => RuntimeSupport {
            platform,
            runtime_available: true,
            error_code: None,
            message: "macOS 保留现有运行路径；此策略不代表完整产品验收已经通过。",
        },
        HostPlatform::Windows => RuntimeSupport {
            platform,
            runtime_available: false,
            error_code: Some("PLATFORM_STAGE_UNAVAILABLE"),
            message: "此 Windows 阶段尚未开放：安全文件访问与已有维护、迁移、恢复和清理现场的只读识别检查尚待完成，暂不允许启动平台运行时。此判断未读取或检查任何工作区现场。",
        },
        HostPlatform::Unsupported => RuntimeSupport {
            platform,
            runtime_available: false,
            error_code: Some("PLATFORM_UNSUPPORTED"),
            message: "当前系统未纳入平台支持范围，暂不允许启动平台运行时；此判断未读取或检查任何工作区现场。",
        },
    }
}

pub const fn runtime_support() -> RuntimeSupport {
    if crate::build_profile::PRODUCTION_PROFILE {
        return RuntimeSupport {
            platform: HostPlatform::current(),
            runtime_available: false,
            error_code: Some(crate::build_profile::PRODUCTION_RESOURCE_ERROR),
            message: crate::build_profile::PRODUCTION_RESOURCE_MESSAGE,
        };
    }
    support_for(HostPlatform::current())
}

/// Check the compiled target. There is deliberately no caller-selected host,
/// feature flag, environment variable or command-line override for admission.
pub fn require_runtime() -> Result<(), RuntimeSupportError> {
    let support = runtime_support();
    match support.error_code {
        Some(code) => Err(RuntimeSupportError {
            platform: support.platform,
            code,
            message: support.message,
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
fn require_for(platform: HostPlatform) -> Result<(), RuntimeSupportError> {
    let support = support_for(platform);
    match support.error_code {
        Some(code) => Err(RuntimeSupportError {
            platform,
            code,
            message: support.message,
        }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_refuses_runtime_with_an_actionable_unverified_stage_error() {
        let error = require_for(HostPlatform::Windows).unwrap_err();
        assert_eq!(error.code, "PLATFORM_STAGE_UNAVAILABLE");
        assert_eq!(error.platform, HostPlatform::Windows);
        for expected in [
            "此 Windows 阶段尚未开放",
            "安全文件访问",
            "只读识别检查尚待完成",
            "未读取或检查任何工作区现场",
        ] {
            assert!(error.message.contains(expected), "{expected}");
        }
        assert!(error
            .to_string()
            .starts_with("PLATFORM_STAGE_UNAVAILABLE: "));
    }

    #[test]
    fn macos_keeps_its_existing_runtime_policy() {
        assert_eq!(require_for(HostPlatform::MacOs), Ok(()));
        let support = support_for(HostPlatform::MacOs);
        assert!(support.runtime_available);
        assert_eq!(support.error_code, None);
        assert!(support.message.contains("不代表完整产品验收"));
    }

    #[test]
    fn other_hosts_are_not_treated_as_macos() {
        let error = require_for(HostPlatform::Unsupported).unwrap_err();
        assert_eq!(error.code, "PLATFORM_UNSUPPORTED");
        assert!(error.message.contains("未纳入平台支持范围"));
        assert!(!support_for(HostPlatform::Unsupported).runtime_available);
    }

    #[test]
    fn diagnostics_and_admission_agree_for_every_declared_host() {
        for platform in [
            HostPlatform::MacOs,
            HostPlatform::Windows,
            HostPlatform::Unsupported,
        ] {
            let support = support_for(platform);
            assert_eq!(support.platform, platform);
            assert_eq!(require_for(platform).is_ok(), support.runtime_available);
            assert_eq!(support.error_code.is_none(), support.runtime_available);
            if let Err(error) = require_for(platform) {
                assert_eq!(Some(error.code), support.error_code);
                assert_eq!(error.message, support.message);
            }
        }
    }

    #[test]
    fn running_target_uses_its_compile_time_policy_without_an_override() {
        let expected = if cfg!(target_os = "windows") {
            HostPlatform::Windows
        } else if cfg!(target_os = "macos") {
            HostPlatform::MacOs
        } else {
            HostPlatform::Unsupported
        };
        assert_eq!(HostPlatform::current(), expected);
        if crate::build_profile::PRODUCTION_PROFILE {
            assert_eq!(
                require_runtime().unwrap_err().code,
                crate::build_profile::PRODUCTION_RESOURCE_ERROR
            );
            assert!(!runtime_support().runtime_available);
        } else {
            assert_eq!(runtime_support(), support_for(expected));
            assert_eq!(require_runtime(), require_for(expected));
        }
        #[cfg(all(target_os = "windows", not(feature = "production-profile")))]
        assert_eq!(
            require_runtime().unwrap_err().code,
            "PLATFORM_STAGE_UNAVAILABLE"
        );
    }
}
