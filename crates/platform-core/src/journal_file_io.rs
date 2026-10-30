//! Bounded read-only journal snapshots. Callers retain their own byte/entry
//! budgets and error policy; this adapter creates or repairs no journal.
use crate::{path_checks::check_no_symlinks, ApiError};
use platform_native::tool_files::Facts;
use std::{io::Read, path::Path};

pub(crate) fn read_snapshot(
    path: &Path,
    maximum: u64,
    remaining: u64,
    charge: impl FnOnce(u64) -> Result<(), ApiError>,
    invalid: fn() -> ApiError,
    limit: fn() -> ApiError,
) -> Result<Vec<u8>, ApiError> {
    check_no_symlinks(path, false).map_err(|_| invalid())?;
    let mut file = platform_native::filesystem::open_read(
        path,
        platform_native::filesystem::ReadOptions::NoLinksNonblocking,
    )
    .map_err(|_| invalid())?;
    let before = Facts::from_file(&file).map_err(|_| invalid())?;
    if !before.is_file() || before.links() != 1 {
        return Err(invalid());
    }
    if before.len() > maximum || before.len() > remaining {
        return Err(limit());
    }
    charge(before.len())?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(before.len())
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    let after = Facts::from_file(&file).map_err(|_| invalid())?;
    check_no_symlinks(path, false).map_err(|_| invalid())?;
    let current = Facts::capture(path).map_err(|_| invalid())?;
    if bytes.len() as u64 != before.len() || !before.unchanged(&after) || !after.unchanged(&current)
    {
        return Err(invalid());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    fn invalid() -> ApiError {
        ApiError::new("DISPOSABLE_SNAPSHOT_INVALID", "Disposable journal changed")
    }
    fn limit() -> ApiError {
        ApiError::new(
            "DISPOSABLE_SNAPSHOT_LIMIT",
            "Disposable journal exceeds budget",
        )
    }
    #[test]
    fn bounds_are_checked_before_charging_and_missing_reads_never_create_a_journal() {
        let temp = tempfile::Builder::new()
            .prefix("panda-disposable-journal-budget-中文 space-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("journal.json");
        assert!(read_snapshot(
            &path,
            64,
            64,
            |_| panic!("missing files cannot charge a budget"),
            invalid,
            limit
        )
        .is_err());
        assert!(!path.exists());
        fs::write(&path, b"disposable journal").unwrap();
        let mut charged = 0;
        assert_eq!(
            read_snapshot(
                &path,
                64,
                64,
                |bytes| {
                    charged += bytes;
                    Ok(())
                },
                invalid,
                limit
            )
            .unwrap(),
            b"disposable journal"
        );
        assert_eq!(charged, 18);
        for (maximum, remaining) in [(17, 64), (64, 17)] {
            assert_eq!(
                read_snapshot(
                    &path,
                    maximum,
                    remaining,
                    |_| panic!("oversize files cannot charge a budget"),
                    invalid,
                    limit
                )
                .unwrap_err()
                .code,
                "DISPOSABLE_SNAPSHOT_LIMIT"
            );
        }
        assert_eq!(fs::read(&path).unwrap(), b"disposable journal");
    }

    #[test]
    fn replacing_a_name_during_a_snapshot_preserves_both_disposable_files_and_fails() {
        let temp = tempfile::Builder::new()
            .prefix("panda-disposable-journal-race-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("journal.json");
        let retained = root.join("retained-original.json");
        fs::write(&path, b"original disposable journal").unwrap();
        let error = read_snapshot(
            &path,
            64,
            64,
            |_| {
                fs::rename(&path, &retained).unwrap();
                fs::write(&path, b"replacement disposable journal").unwrap();
                Ok(())
            },
            invalid,
            limit,
        )
        .unwrap_err();
        assert_eq!(error.code, "DISPOSABLE_SNAPSHOT_INVALID");
        assert_eq!(fs::read(retained).unwrap(), b"original disposable journal");
        assert_eq!(fs::read(path).unwrap(), b"replacement disposable journal");
    }

    #[test]
    fn hardlinked_journals_and_windows_directory_junctions_are_not_admitted() {
        let temp = tempfile::Builder::new()
            .prefix("panda-disposable-journal-boundary-")
            .tempdir()
            .unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root.join("journal.json");
        fs::write(&path, b"retained disposable bytes").unwrap();
        fs::hard_link(&path, root.join("second-name")).unwrap();
        assert_eq!(
            read_snapshot(
                &path,
                64,
                64,
                |_| panic!("hardlinked files cannot charge a budget"),
                invalid,
                limit
            )
            .unwrap_err()
            .code,
            "DISPOSABLE_SNAPSHOT_INVALID"
        );
        fs::remove_file(root.join("second-name")).unwrap();
        #[cfg(windows)]
        {
            let junction = root.join("disposable-junction");
            assert!(std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&junction)
                .arg(&root)
                .output()
                .unwrap()
                .status
                .success());
            assert_eq!(
                read_snapshot(
                    &junction.join("journal.json"),
                    64,
                    64,
                    |_| panic!("junctions cannot charge a budget"),
                    invalid,
                    limit
                )
                .unwrap_err()
                .code,
                "DISPOSABLE_SNAPSHOT_INVALID"
            );
            fs::remove_dir(junction).unwrap();
        }
        assert_eq!(fs::read(path).unwrap(), b"retained disposable bytes");
    }
}
