//! Local development platform: project storage, SQLite workers and loopback runtime.

pub mod app_windows;
pub mod backups;
pub mod builds;
mod control;
mod database;
pub mod environment;
mod error;
mod file_identity;
mod files;
pub mod git_environment;
pub mod git_remote;
mod history_cleanup;
mod identity_upgrade;
mod lifecycle;
mod maintenance;
mod managed_git;
mod managed_toolchain;
mod metadata;
pub mod node_app_configuration;
pub mod node_app_install;
pub mod node_app_manifest;
mod node_app_proxy;
pub mod node_apps;
pub mod node_install_tasks;
pub mod node_runtime_trial;
pub mod object_layout;
mod paths;
mod platform_info;
pub mod platform_support;
mod project_directory_name;
pub mod project_git;
pub mod projects;
pub mod resources;
mod runtime_notice;
pub mod runtime_policy;
mod service;
pub mod source_acquisition;
pub mod source_restore_guard;
mod storage;
pub mod tasks;
mod workspace;
pub use tasks::{TaskRecord, TaskStatus};

pub use paths::{Discovery, InstanceLock, RuntimePaths};
pub use platform_preflight as startup_preflight;
pub use service::{BootstrapInfo, HealthInfo, LaunchInfo, RuntimeHandle, RuntimeService};

pub const APP_IDENTIFIER: &str = "dev.localprojects.platform.m0";
pub const DISPLAY_NAME: &str = "熊猫应用小抽屉 · 开发版";
pub const ENVIRONMENT: &str = "development";
pub const API_VERSION: u32 = 1;

pub use database::{
    sql_limits, SchemaInfo, SqlCaller, SqlColumn, SqlOptions, SqlRequest, SqlResult, SqlStatement,
    TransactionRequest,
};
pub use error::ApiError;
pub use projects::{CreateProjectInput, ProjectHandle, ProjectInfo, ProjectManifest, ProjectStore};
