//! Native file copying for disposable build inputs. No project script runs here.
use crate::{path_checks::check_no_symlinks, ApiError};
use std::{fs::Metadata, path::Path};

fn changed() -> ApiError {
    ApiError::new(
        "BUILD_INPUT_CHANGED",
        "A source file changed while being copied",
    )
}

pub(crate) fn copy_file(from: &Path, to: &Path, metadata: &Metadata) -> Result<(), ApiError> {
    #[cfg(unix)]
    {
        use std::{
            fs, io,
            os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        };
        fn same_file(before: &Metadata, after: &Metadata) -> bool {
            before.dev() == after.dev()
                && before.ino() == after.ino()
                && before.len() == after.len()
                && before.mtime() == after.mtime()
                && before.mtime_nsec() == after.mtime_nsec()
                && before.mode() == after.mode()
        }
        check_no_symlinks(from, false)?;
        let mut input = platform_native::filesystem::open_read(
            from,
            platform_native::filesystem::ReadOptions::NoLinks,
        )?;
        if !same_file(metadata, &input.metadata()?) {
            return Err(changed());
        }
        let mode = if metadata.mode() & 0o111 == 0 {
            0o600
        } else {
            0o700
        };
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(to)?;
        io::copy(&mut input, &mut output)?;
        output.set_permissions(fs::Permissions::from_mode(mode))?;
        output.sync_all()?;
        if !same_file(metadata, &input.metadata()?)
            || !same_file(metadata, &fs::symlink_metadata(from)?)
        {
            return Err(changed());
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use platform_native::tool_files::Facts;
        let _ = metadata;
        check_no_symlinks(from, false)?;
        let before = Facts::capture(from)?;
        if !before.is_file() {
            return Err(changed());
        }
        let mut input = platform_native::filesystem::open_read(
            from,
            platform_native::filesystem::ReadOptions::NoLinks,
        )?;
        if !before.same_modified_permissions(&Facts::from_file(&input)?) {
            return Err(changed());
        }
        let mut output = platform_native::windows::files::create_private_file(to)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
        if !before.same_modified_permissions(&Facts::from_file(&input)?)
            || !before.same_modified_permissions(&Facts::capture(from)?)
        {
            return Err(changed());
        }
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use platform_native::windows::files::{open_no_reparse, verify_private};
    use std::fs;

    #[test]
    fn read_only_and_hardlinked_source_bytes_copy_to_independent_private_files() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-build-copy-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let source = root.join("source.mjs");
        let target = root.join("candidate.mjs");
        fs::write(&source, b"disposable source; never executed").unwrap();
        fs::hard_link(&source, root.join("retained-source-link")).unwrap();
        let original_permissions = fs::metadata(&source).unwrap().permissions();
        let mut read_only = original_permissions.clone();
        read_only.set_readonly(true);
        fs::set_permissions(&source, read_only).unwrap();
        let result = copy_file(&source, &target, &fs::symlink_metadata(&source).unwrap());
        fs::set_permissions(&source, original_permissions).unwrap();
        result.unwrap();
        let before = open_no_reparse(&source).unwrap();
        let after = open_no_reparse(&target).unwrap();
        assert!(!before.identity.same_object(&after.identity));
        assert_eq!(before.identity.links, 2);
        assert_eq!(after.identity.links, 1);
        verify_private(&after.file).unwrap();
        assert!(!fs::metadata(&target).unwrap().permissions().readonly());
        assert_eq!(fs::read(&target).unwrap(), fs::read(&source).unwrap());
        assert!(copy_file(&source, &target, &fs::symlink_metadata(&source).unwrap()).is_err());
        assert_eq!(
            fs::read(&target).unwrap(),
            b"disposable source; never executed"
        );
    }

    #[test]
    fn junction_source_is_rejected_without_creating_a_candidate() {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-build-copy-junction-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let outside = root.join("retained-source");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("input.txt"), b"retained disposable bytes").unwrap();
        let link = root.join("junction");
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(status.status.success());
        let source = link.join("input.txt");
        let target = root.join("candidate.txt");
        assert!(copy_file(&source, &target, &fs::symlink_metadata(&source).unwrap()).is_err());
        assert!(!target.exists());
        assert_eq!(
            fs::read(outside.join("input.txt")).unwrap(),
            b"retained disposable bytes"
        );
    }
}
