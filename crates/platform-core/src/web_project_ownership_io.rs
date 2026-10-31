//! Prepare only the two selected ownership directories, never their parents.
use crate::{path_checks::check_no_symlinks, ApiError};
use std::path::Path;

pub(crate) fn prepare_directory(directory: &Path) -> Result<(), ApiError> {
    check_no_symlinks(directory, true)?;
    #[cfg(unix)]
    {
        use std::{fs, os::unix::fs::PermissionsExt};
        if !directory.exists() {
            fs::create_dir(directory)?;
        }
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    {
        use platform_native::windows::files as native;
        if !directory.exists() {
            native::create_private_directory(directory)?;
        } else {
            // This is the ownership scope admitted by the caller. Like the
            // original chmod, repair only this directory, never data/system
            // ancestors, user source directories or unrelated siblings.
            native::make_directory_private(directory)?;
        }
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use platform_native::windows::files as native;
    use std::fs;

    #[test]
    fn selected_scope_creation_and_repair_preserve_parent_identity_and_contents() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-web-ownership-acl-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let parent = native::open_no_reparse(&root).unwrap();
        assert!(native::verify_private(&parent.file).is_err());
        let scope = root.join("web-project-ownership");
        prepare_directory(&scope).unwrap();
        native::verify_private(&native::open_no_reparse(&scope).unwrap().file).unwrap();
        let project = scope.join("prj_disposable_acl");
        // A pre-existing ordinary selected directory receives only its own
        // private access, as the original Mac update does with chmod.
        fs::create_dir(&project).unwrap();
        let before = native::open_no_reparse(&project).unwrap().identity;
        fs::write(
            project.join("sentinel"),
            b"preserve selected scope contents",
        )
        .unwrap();
        prepare_directory(&project).unwrap();
        prepare_directory(&project).unwrap();
        let after = native::open_no_reparse(&project).unwrap();
        assert!(before.same_object(&after.identity));
        native::verify_private(&after.file).unwrap();
        assert!(parent
            .identity
            .same_object(&native::open_no_reparse(&root).unwrap().identity));
        assert!(native::verify_private(&parent.file).is_err());
        assert_eq!(
            fs::read(project.join("sentinel")).unwrap(),
            b"preserve selected scope contents"
        );
    }

    #[test]
    fn absent_parent_and_reparse_scope_are_preserved_without_creating_an_ownership_root() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-web-ownership-boundary-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        assert!(prepare_directory(&root.join("missing/child")).is_err());
        assert!(!root.join("missing").exists());
        let external = root.join("disposable-external-original");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("sentinel"), b"preserve external original").unwrap();
        let original = native::open_no_reparse(&external).unwrap();
        let linked = root.join("web-project-ownership");
        let result = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&linked)
            .arg(&external)
            .output()
            .unwrap();
        assert!(result.status.success());
        assert!(prepare_directory(&linked).is_err());
        assert!(original
            .identity
            .same_object(&native::open_no_reparse(&external).unwrap().identity));
        assert!(native::verify_private(&original.file).is_err());
        assert_eq!(
            fs::read(external.join("sentinel")).unwrap(),
            b"preserve external original"
        );
        fs::remove_dir(linked).unwrap();
    }
}
