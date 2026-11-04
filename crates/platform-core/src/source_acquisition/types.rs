//! Shared source-acquisition wire data; these types grant no ownership.
use crate::{environment::ExecutableInfo, ApiError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcquireSourceInput {
    pub url: String,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub alternate_url: Option<String>,
    #[serde(default)]
    pub download_idle_timeout: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceInspection {
    pub kind: String,
    pub name: Option<String>,
    pub next_step: String,
    pub execution_state: String,
    #[serde(default)]
    pub requires_build: bool,
    pub issues: Vec<ApiError>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCandidate {
    pub source_id: String,
    pub task_id: String,
    pub name: String,
    pub status: String,
    pub phase: String,
    pub url: String,
    pub branch: Option<String>,
    pub provider: String,
    pub created_at: u64,
    pub completed_at: Option<u64>,
    pub source_path: Option<PathBuf>,
    pub evidence_path: PathBuf,
    pub shallow: bool,
    pub git: Option<ExecutableInfo>,
    pub commit: Option<String>,
    pub file_count: Option<usize>,
    pub total_bytes: Option<u64>,
    pub content_digest: Option<String>,
    pub inspection: Option<SourceInspection>,
    pub error: Option<ApiError>,
}
