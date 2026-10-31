//! Finalize a copy whose newly created output handles were each sync_all'd by
//! copy_tree. Windows must not reopen those files read-only and ask for a
//! write-only FlushFileBuffers operation a second time.
use crate::ApiError;
use std::path::Path;

pub(super) fn finish(root: &Path) -> Result<(), ApiError> {
    #[cfg(not(windows))]
    {
        // Preserve the original Mac second file flush and directory fsyncs.
        super::sync_tree(root)
    }
    #[cfg(windows)]
    {
        // Every file was flushed through its original writable creation
        // handle. Directory fsync has no Windows equivalent; the facade
        // still observes that this is an ordinary, no-reparse directory.
        platform_native::filesystem::sync_directory(root)?;
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use platform_native::windows::files as native;
    use std::fs;

    #[test]
    fn actual_copy_flushes_independent_writable_private_files_from_readonly_sources() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-build-copy-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir_all(source.join("assets")).unwrap();
        let original = root.join("external-original");
        fs::write(&original, b"preserve original bytes").unwrap();
        let source_file = source.join("assets/图😀.js");
        fs::hard_link(&original, &source_file).unwrap();
        native::set_read_only(&original, true).unwrap();
        let original_id = native::open_no_reparse(&original).unwrap().identity;
        super::super::copy_tree(&source, &target).unwrap();
        let copied = target.join("assets/图😀.js");
        assert_eq!(fs::read(&copied).unwrap(), b"preserve original bytes");
        let copied_file = native::open_read_file(&copied).unwrap();
        native::verify_private(&copied_file).unwrap();
        assert!(!copied_file.metadata().unwrap().permissions().readonly());
        assert!(!original_id.same_object(&native::identity(&copied_file).unwrap()));
        assert!(fs::metadata(&original).unwrap().permissions().readonly());
        assert!(original_id.same_object(&native::open_no_reparse(&original).unwrap().identity));
        native::set_read_only(&original, false).unwrap();
    }

    #[test]
    fn empty_source_produces_a_real_private_target_without_touching_its_parent() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-build-empty-copy-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let parent_id = native::open_no_reparse(&root).unwrap().identity;
        let source = root.join("source");
        let target = root.join("target");
        fs::create_dir(&source).unwrap();
        super::super::copy_tree(&source, &target).unwrap();
        let actual = native::open_no_reparse(&target).unwrap();
        assert!(actual.identity.directory);
        native::verify_private(&actual.file).unwrap();
        assert!(fs::read_dir(&target).unwrap().next().is_none());
        assert!(parent_id.same_object(&native::open_no_reparse(&root).unwrap().identity));
    }
}
