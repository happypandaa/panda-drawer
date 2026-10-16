use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub details: Value,
    pub retryable: bool,
}

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: json!({}),
            retryable: false,
        }
    }
    pub fn detail(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.details[key] = value.into();
        self
    }
    pub fn outcome(self, value: &str) -> Self {
        self.detail("operationOutcome", value)
    }
    pub(crate) fn io(error: std::io::Error) -> Self {
        Self::new("FILESYSTEM_ERROR", error.to_string())
    }
}
impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ApiError {}

impl From<std::io::Error> for ApiError {
    fn from(value: std::io::Error) -> Self {
        Self::io(value)
    }
}
impl From<serde_json::Error> for ApiError {
    fn from(value: serde_json::Error) -> Self {
        Self::new("INVALID_JSON", value.to_string())
    }
}
