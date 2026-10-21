use crate::ApiError;
use std::{
    fs,
    path::{Path, PathBuf},
};
pub fn check_no_symlinks(path: &Path, allow_missing: bool) -> Result<(), ApiError> {
    let mut checked = PathBuf::new();
    for component in path.components() {
        checked.push(component.as_os_str());
        match fs::symlink_metadata(&checked) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(ApiError::new(
                    "UNSAFE_PATH",
                    "Symbolic links are not permitted in project-controlled paths",
                ))
            }
            Ok(_) => (),
            Err(error) if allow_missing && error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
