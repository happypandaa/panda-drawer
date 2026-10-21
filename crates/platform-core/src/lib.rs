//! Local development platform: project storage, SQLite workers and loopback runtime.

pub use platform_node::app_windows;
pub mod backups;
pub mod builds;
mod control;
mod database;
pub mod environment;
mod error;
mod file_identity;
mod files;
mod history_cleanup;
mod identity_upgrade;
mod lifecycle;
mod maintenance;
mod metadata;
pub use platform_node::node_app_configuration;
pub use platform_node::node_app_manifest;
pub use platform_node::node_apps;
pub use platform_node::node_runtime_trial;
mod paths;
mod platform_info;
pub use platform_host::platform_support;
pub mod projects;
pub mod resources;
mod runtime_notice;
pub mod runtime_policy;
mod service;
mod storage;
pub mod tasks;
mod workspace;
pub use tasks::{TaskRecord, TaskStatus};

pub use paths::{Discovery, InstanceLock, RuntimePaths};
pub use platform_preflight as startup_preflight;
pub use service::{BootstrapInfo, HealthInfo, LaunchInfo, RuntimeHandle, RuntimeService};

pub use platform_host::{API_VERSION, APP_IDENTIFIER, DISPLAY_NAME, ENVIRONMENT};

pub use database::{
    sql_limits, SchemaInfo, SqlCaller, SqlColumn, SqlOptions, SqlRequest, SqlResult, SqlStatement,
    TransactionRequest,
};
pub use error::ApiError;
pub use projects::{CreateProjectInput, ProjectHandle, ProjectInfo, ProjectManifest, ProjectStore};
