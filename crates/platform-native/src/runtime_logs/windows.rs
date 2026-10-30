use crate::windows::files;
use std::{fs::File, io, path::Path};

pub fn private_current(control: &Path, limit: u64) -> io::Result<File> {
    // Both handles pin the directory scope throughout validation and rotation.
    // No ACL policy is imposed on the control directory's system ancestors.
    let _control = files::pin_directory(control)?;
    let logs = control.join("logs");
    match files::create_private_directory(&logs) {
        Ok(()) => (),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
        Err(error) => return Err(error),
    }
    let directory = files::pin_directory(&logs)?;
    files::verify_private(&directory.file)?;
    let current = logs.join("current.log");
    let previous = logs.join("previous.log");
    let check = |path: &Path| -> io::Result<bool> {
        let file = match files::open_private_file(path, false, false) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        super::validate_file(&file, &metadata)?;
        if metadata.len() > limit {
            return Err(io::Error::other("unsafe log file"));
        }
        Ok(true)
    };
    let has_current = check(&current)?;
    check(&previous)?;
    if has_current {
        // Windows replaces the previous file in one native rename. An occupied
        // destination returns its actual I/O error and is never called success.
        files::rename_file(&current, &previous, true)?;
    }
    let file = files::create_private_file(&current)?;
    super::validate_file(&file, &file.metadata()?)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write, os::windows::fs::OpenOptionsExt};

    fn root() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("panda-disposable-logs-中文 space-")
            .tempdir()
            .unwrap()
    }

    #[test]
    fn rotation_retains_only_two_private_files() {
        let temporary = root();
        let control = temporary.path().canonicalize().unwrap();
        for bytes in [b"first\n".as_slice(), b"second\n", b"third\n"] {
            let mut file = private_current(&control, 4096).unwrap();
            file.write_all(bytes).unwrap();
            file.sync_all().unwrap();
            super::super::validate_file(&file, &file.metadata().unwrap()).unwrap();
        }
        let logs = control.join("logs");
        assert_eq!(fs::read_dir(&logs).unwrap().count(), 2);
        assert_eq!(fs::read(logs.join("current.log")).unwrap(), b"third\n");
        assert_eq!(fs::read(logs.join("previous.log")).unwrap(), b"second\n");
        for path in [&logs, &logs.join("current.log"), &logs.join("previous.log")] {
            files::verify_private(&files::open_no_reparse(path).unwrap().file).unwrap();
        }
    }

    #[test]
    fn refuses_inherited_acl_and_hardlinks_without_modifying_foreign_bytes() {
        for kind in ["directory-acl", "file-acl", "hardlink", "oversized"] {
            let temporary = root();
            let control = temporary.path().canonicalize().unwrap();
            let logs = control.join("logs");
            if kind == "directory-acl" {
                fs::create_dir(&logs).unwrap();
            } else {
                files::create_private_directory(&logs).unwrap();
                let current = logs.join("current.log");
                if kind == "file-acl" {
                    fs::write(&current, b"foreign disposable bytes").unwrap();
                } else {
                    files::create_private_file(&current)
                        .unwrap()
                        .write_all(b"private disposable bytes")
                        .unwrap();
                    if kind == "hardlink" {
                        fs::hard_link(&current, control.join("foreign-hardlink")).unwrap();
                    }
                }
            }
            let original = fs::read(logs.join("current.log")).ok();
            let limit = if kind == "oversized" { 4 } else { 4096 };
            assert!(private_current(&control, limit).is_err(), "{kind}");
            assert_eq!(fs::read(logs.join("current.log")).ok(), original, "{kind}");
            assert!(!logs.join("previous.log").exists());
        }
    }

    #[test]
    fn occupied_rotation_preserves_both_logs_and_retries_after_release() {
        let temporary = root();
        let control = temporary.path().canonicalize().unwrap();
        private_current(&control, 4096)
            .unwrap()
            .write_all(b"first\n")
            .unwrap();
        private_current(&control, 4096)
            .unwrap()
            .write_all(b"second\n")
            .unwrap();
        let previous = control.join("logs/previous.log");
        let blocker = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&previous)
            .unwrap();
        assert!(private_current(&control, 4096).is_err());
        assert_eq!(
            fs::read(control.join("logs/current.log")).unwrap(),
            b"second\n"
        );
        drop(blocker);
        assert_eq!(fs::read(&previous).unwrap(), b"first\n");
        drop(private_current(&control, 4096).unwrap());
        assert_eq!(fs::read(&previous).unwrap(), b"second\n");
    }

    #[test]
    fn rejects_a_junction_for_the_logs_directory() {
        let temporary = root();
        let control = temporary.path().canonicalize().unwrap();
        let foreign = control.join("disposable-foreign");
        fs::create_dir(&foreign).unwrap();
        fs::write(foreign.join("retained"), b"disposable retained bytes").unwrap();
        let logs = control.join("logs");
        let output = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&logs)
            .arg(&foreign)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(private_current(&control, 4096).is_err());
        fs::remove_dir(&logs).unwrap();
        assert_eq!(
            fs::read(foreign.join("retained")).unwrap(),
            b"disposable retained bytes"
        );
        assert_eq!(fs::read_dir(&foreign).unwrap().count(), 1);
    }
}
