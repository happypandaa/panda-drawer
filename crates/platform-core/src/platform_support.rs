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
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
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
            runtime_available: true,
            error_code: None,
            message: "Windows x64 已开放开发版运行入口；具体功能由各自的平台能力检查准入，此策略不代表主界面或完整产品验收已经通过。",
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
            error_code: Some(if crate::production_resources::COMPILED_RESOURCE_ANCHORS {
                crate::build_profile::PRODUCTION_PROOF_ERROR
            } else {
                crate::build_profile::PRODUCTION_RESOURCE_ERROR
            }),
            message: if crate::production_resources::COMPILED_RESOURCE_ANCHORS {
                crate::build_profile::PRODUCTION_PROOF_MESSAGE
            } else {
                crate::build_profile::PRODUCTION_RESOURCE_MESSAGE
            },
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

/// Only new public workspace-migration admission is closed on Windows. Existing
/// durable records retain their startup recovery and explicit cleanup paths.
pub fn require_workspace_migration() -> Result<(), crate::ApiError> {
    require_workspace_migration_for(HostPlatform::current())
}

fn require_workspace_migration_for(platform: HostPlatform) -> Result<(), crate::ApiError> {
    if platform == HostPlatform::Windows {
        return Err(crate::ApiError::new(
            "PLATFORM_FEATURE_UNAVAILABLE",
            "Windows 当前版本不提供工作区迁移，请继续使用当前项目文件夹。",
        )
        .detail("platform", "windows")
        .detail("capability", "workspace-migration")
        .outcome("not_started"));
    }
    Ok(())
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
    fn new_workspace_migration_is_closed_only_on_windows_without_changing_runtime() {
        let error = require_workspace_migration_for(HostPlatform::Windows).unwrap_err();
        assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
        assert_eq!(error.details["capability"], "workspace-migration");
        assert_eq!(error.details["operationOutcome"], "not_started");
        assert!(require_workspace_migration_for(HostPlatform::MacOs).is_ok());
        assert!(support_for(HostPlatform::Windows).runtime_available);
    }

    #[test]
    fn windows_admits_startup_without_claiming_individual_features_or_acceptance() {
        assert_eq!(require_for(HostPlatform::Windows), Ok(()));
        let support = support_for(HostPlatform::Windows);
        assert!(support.runtime_available);
        assert_eq!(support.error_code, None);
        assert!(support.message.contains("各自的平台能力检查"));
        assert!(support.message.contains("不代表主界面或完整产品验收"));
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
        let expected = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
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
                runtime_support().error_code.unwrap()
            );
            assert!(!runtime_support().runtime_available);
        } else {
            assert_eq!(runtime_support(), support_for(expected));
            assert_eq!(require_runtime(), require_for(expected));
        }
        #[cfg(all(
            target_os = "windows",
            target_arch = "x86_64",
            not(feature = "production-profile")
        ))]
        assert_eq!(require_runtime(), Ok(()));
    }
}
