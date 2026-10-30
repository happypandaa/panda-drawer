//! Shared platform boundary for the existing component-by-component precheck.
//! Sensitive operations still need their actual handle-relative I/O boundary.
use crate::ApiError;
use std::path::Path;

pub fn check_no_symlinks(path: &Path, allow_missing: bool) -> Result<(), ApiError> {
    if platform_native::filesystem::find_reparse_component(path, allow_missing)?.is_some() {
        return Err(ApiError::new(
            "UNSAFE_PATH",
            "Symbolic links are not permitted in project-controlled paths",
        ));
    }
    Ok(())
}
