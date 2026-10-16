//! Persisted task wire protocol. Finality is derived from the saved status.
use crate::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Interrupted,
}

impl TaskStatus {
    pub fn is_final(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Interrupted)
    }
}

/// `final` is derived from status during serialization, so waiting clients cannot
/// observe a terminal task erroneously described as still pending.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    pub task_id: String,
    pub project_id: Option<String>,
    #[serde(rename = "type")]
    pub task_type: String,
    pub status: TaskStatus,
    pub phase: String,
    pub result: Option<Value>,
    pub error: Option<ApiError>,
}

impl Serialize for TaskRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut wire = serializer.serialize_struct("TaskRecord", 8)?;
        wire.serialize_field("taskId", &self.task_id)?;
        wire.serialize_field("projectId", &self.project_id)?;
        wire.serialize_field("type", &self.task_type)?;
        wire.serialize_field("status", &self.status)?;
        wire.serialize_field("final", &self.status.is_final())?;
        wire.serialize_field("phase", &self.phase)?;
        wire.serialize_field("result", &self.result)?;
        wire.serialize_field("error", &self.error)?;
        wire.end()
    }
}
