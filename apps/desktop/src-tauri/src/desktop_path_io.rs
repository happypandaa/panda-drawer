//! Native directory identity for a transient, read-only editor confirmation.
use platform_native::{directory::ObjectIdentity, tool_files::Facts};
use std::{io, path::Path};

pub(super) type DirectoryIdentity = ObjectIdentity;

pub(super) fn directory_identity(path: &Path) -> io::Result<Option<DirectoryIdentity>> {
    let facts = Facts::capture(path)?;
    if !facts.is_dir() || facts.is_reparse() {
        return Ok(None);
    }
    // This confirmation binds only the object. Editing its contents must not
    // revoke it by adding timestamp or permission comparisons.
    Ok(Some(facts.stamp().identity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn content_changes_keep_the_binding_but_replacement_revokes_it() {
        let temp = tempfile::Builder::new()
            .prefix("panda-disposable-editor-中文 space-")
            .tempdir()
            .unwrap();
        let root = temp.path().join("project");
        fs::create_dir(&root).unwrap();
        let before = directory_identity(&root).unwrap().unwrap();
        fs::write(root.join("source.txt"), b"changed contents").unwrap();
        assert_eq!(directory_identity(&root).unwrap(), Some(before.clone()));
        fs::rename(&root, temp.path().join("retained-original")).unwrap();
        fs::create_dir(&root).unwrap();
        assert_ne!(directory_identity(&root).unwrap(), Some(before));
        assert!(temp.path().join("retained-original/source.txt").is_file());
    }

    #[test]
    fn files_are_not_directories_and_missing_paths_are_not_created() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("source.txt");
        fs::write(&file, b"retain").unwrap();
        assert_eq!(directory_identity(&file).unwrap(), None);
        let missing = temp.path().join("missing");
        assert!(directory_identity(&missing).is_err());
        assert!(!missing.exists());
        assert_eq!(fs::read(file).unwrap(), b"retain");
    }
}
