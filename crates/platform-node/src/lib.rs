//! Shared native Node application management, independent of project-v1 recovery.
pub use platform_project_format::ApiError;
pub mod app_windows;
pub mod node_app_configuration;
pub mod node_app_manifest;
mod node_app_proxy;
pub mod node_apps;
pub mod node_runtime_trial;
mod paths;
#[cfg(windows)]
pub mod windows_fs;
