//! Local development platform: project storage, SQLite workers and loopback runtime.

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
mod paths;
mod platform_info;
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
pub use service::{BootstrapInfo, HealthInfo, LaunchInfo, RuntimeHandle, RuntimeService};

pub const APP_IDENTIFIER: &str = "dev.localprojects.platform.m0";
pub const DISPLAY_NAME: &str = "Local Projects Dev";
pub const ENVIRONMENT: &str = "development";
pub const API_VERSION: u32 = 1;

pub use database::{
    sql_limits, SchemaInfo, SqlCaller, SqlColumn, SqlOptions, SqlRequest, SqlResult, SqlStatement,
    TransactionRequest,
};
pub use error::ApiError;
pub use projects::{CreateProjectInput, ProjectHandle, ProjectInfo, ProjectManifest, ProjectStore};
