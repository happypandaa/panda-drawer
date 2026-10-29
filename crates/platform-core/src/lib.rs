//! Local development platform: project storage, SQLite workers and loopback runtime.

pub mod app_windows;
pub mod backups;
pub mod build_profile;
pub mod builds;
mod control;
mod data_restore_guard;
mod database;
pub mod environment;
mod error;
mod file_identity;
mod files;
pub mod git_environment;
pub mod git_remote;
mod history_cleanup;
mod identity_upgrade;
pub mod io_lifetime;
mod lifecycle;
pub mod local_cleanup;
mod maintenance;
mod managed_git;
mod managed_toolchain;
mod metadata;
mod native_project_files;
pub mod native_project_io;
pub mod node_app_configuration;
pub mod node_app_install;
pub mod node_app_manifest;
mod node_app_proxy;
pub mod node_apps;
mod node_deletion_journal;
pub mod node_development_build;
pub mod node_development_tasks;
pub mod node_install_tasks;
mod node_process_identity;
mod node_project_ownership;
pub use project_git::node_development_git;
mod durable_writer;
mod node_runtime_log;
pub mod node_runtime_trial;
pub mod object_layout;
#[allow(dead_code)] // Registry foundation; legacy helper ports are a separate step.
mod owned_process;
mod paths;
mod platform_info;
pub mod platform_support;
mod process_diagnostics;
pub mod production_resources;
pub mod project_cleanup;
#[cfg(test)]
mod project_cleanup_tests;
mod project_directory_name;
pub mod project_git;
pub mod project_review;
mod project_tool_boundary;
pub mod projects;
pub mod resources;
mod runtime_notice;
pub mod runtime_policy;
mod service;
pub mod shutdown;
pub mod source_acquisition;
pub mod source_restore_guard;
mod storage;
pub mod tasks;
mod web_file_deletion;
mod web_project_cleanup;
mod web_project_ownership;
pub mod webview_storage;
mod workspace;
pub use tasks::{TaskRecord, TaskStatus};

pub use paths::{Discovery, InstanceLock, RuntimePaths};
pub use platform_preflight as startup_preflight;
pub use service::{BootstrapInfo, HealthInfo, LaunchInfo, RuntimeHandle, RuntimeService};

pub use build_profile::{APP_IDENTIFIER, APP_VERSION, DISPLAY_NAME, ENVIRONMENT};
pub const API_VERSION: u32 = 1;

pub use database::{
    sql_limits, SchemaInfo, SqlCaller, SqlColumn, SqlOptions, SqlRequest, SqlResult, SqlStatement,
    TransactionRequest,
};
pub use error::ApiError;
pub use projects::{CreateProjectInput, ProjectHandle, ProjectInfo, ProjectManifest, ProjectStore};

pub use service::{NodeDevelopmentLease, NodeGitAction};
