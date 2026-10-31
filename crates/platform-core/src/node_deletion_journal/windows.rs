//! Durable Windows deletion plans, with a read-only missing/empty fast path.
use crate::ApiError;
use std::path::Path;

#[path = "windows/backend.rs"]
mod backend;
#[path = "windows/inventory.rs"]
mod inventory;
pub(crate) use backend::{local_cleanup_owned_paths, NodeDeletionJournal};

fn is_empty_root(data_root: &Path) -> Result<bool, ApiError> {
    match inventory::require_absent_or_empty(data_root) {
        Ok(()) => Ok(true),
        Err(issue)
            if issue.code == "PLATFORM_FEATURE_UNAVAILABLE"
                && issue.details["inventory"] == "retained" =>
        {
            Ok(false)
        }
        Err(issue) => Err(issue),
    }
}
