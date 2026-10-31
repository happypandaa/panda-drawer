//! Read-only startup inventory. Missing/empty is observed; no existing journal
//! or unpublished/terminal container is adopted, replayed or removed here.
use crate::ApiError;
use platform_native::windows::files::{self as native, PinnedDirectory};
use std::{
    fs,
    os::windows::fs::MetadataExt,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, ApiError>;
const DIRECTORY: &str = "node-deletion-tasks";

#[derive(Debug, PartialEq, Eq)]
enum Observation {
    Missing,
    Empty,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Point {
    MissingObserved,
    EmptyObserved,
}

fn unconfirmed(path: &Path, cause: impl std::fmt::Display) -> ApiError {
    ApiError::new(
        "NODE_DELETION_JOURNAL_UNCONFIRMED",
        "Windows 删除事务目录尚未核实，现有材料已保留。",
    )
    .detail("platform", "windows")
    .detail("capability", "node-deletion-recovery")
    .detail("inventory", "unconfirmed")
    .detail("path", path.to_string_lossy().into_owned())
    .detail("cause", cause.to_string())
    .outcome("unknown")
}

fn retained(path: &Path) -> ApiError {
    ApiError::new(
        "PLATFORM_FEATURE_UNAVAILABLE",
        "Windows 删除事务恢复尚未完成，已保留现有删除任务材料。",
    )
    .detail("platform", "windows")
    .detail("capability", "node-deletion-recovery")
    .detail("inventory", "retained")
    .detail("path", path.to_string_lossy().into_owned())
    .outcome("not_started")
}

fn verify_same_directory(path: &Path, pinned: &PinnedDirectory) -> Result<()> {
    let before = native::identity(&pinned.file).map_err(|error| unconfirmed(path, error))?;
    let current = native::open_no_reparse(path).map_err(|error| unconfirmed(path, error))?;
    if !before.directory || !before.same_object(&current.identity) {
        return Err(unconfirmed(path, "目录身份已变化。"));
    }
    Ok(())
}

fn require_empty(path: &Path) -> Result<()> {
    let mut entries = fs::read_dir(path).map_err(|error| unconfirmed(path, error))?;
    match entries.next() {
        None => Ok(()),
        Some(Ok(_)) => Err(retained(path)),
        Some(Err(error)) => Err(unconfirmed(path, error)),
    }
}

fn inspect_with(
    data_root: &Path,
    mut observe: impl FnMut(Point) -> Result<()>,
) -> Result<Observation> {
    // Pin only the actual initialized data directory and its no-reparse
    // ancestors. As in the macOS list operation, an empty journal root does
    // not gain an extra parent-ACL or private-DACL admission requirement.
    let data = native::pin_directory(data_root).map_err(|error| unconfirmed(data_root, error))?;
    // Enumerate the actual handle's path, rather than feeding an original
    // verbatim/mixed-separator spelling back to the standard filesystem APIs.
    let (bound_data, _scope) =
        native::directory_scope(&data.file).map_err(|error| unconfirmed(data_root, error))?;
    let path: PathBuf = bound_data.join(DIRECTORY);
    let journal = match native::pin_directory(&path) {
        Ok(journal) => journal,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            observe(Point::MissingObserved)?;
            // The parent is held: NotFound here describes this fixed leaf,
            // rather than a missing/replaced ancestor or a swallowed I/O error.
            match fs::symlink_metadata(&path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    verify_same_directory(data_root, &data)?;
                    return Ok(Observation::Missing);
                }
                Err(error) => return Err(unconfirmed(&path, error)),
                Ok(_) => return Err(unconfirmed(&path, "删除事务目录在检查期间出现。")),
            }
        }
        Err(error) => return Err(unconfirmed(&path, error)),
    };
    let before = journal
        .file
        .metadata()
        .map_err(|error| unconfirmed(&path, error))?
        .last_write_time();
    require_empty(&path)?;
    observe(Point::EmptyObserved)?;
    require_empty(&path)?;
    let after = journal
        .file
        .metadata()
        .map_err(|error| unconfirmed(&path, error))?
        .last_write_time();
    verify_same_directory(&path, &journal)?;
    verify_same_directory(data_root, &data)?;
    if before != after {
        return Err(unconfirmed(&path, "删除事务目录在检查期间变化。"));
    }
    Ok(Observation::Empty)
}

pub(super) fn require_absent_or_empty(data_root: &Path) -> Result<()> {
    inspect_with(data_root, |_| Ok(())).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-node-deletion-inventory-中文 space-")
            .tempdir()
            .unwrap();
        let root = temporary.path().canonicalize().unwrap();
        (temporary, root)
    }

    #[test]
    fn actual_missing_and_empty_roots_are_observed_without_creating_or_removing_objects() {
        let (_temporary, root) = fixture();
        fs::write(root.join("retained-data"), b"disposable retained bytes").unwrap();
        assert_eq!(
            inspect_with(&root, |_| Ok(())).unwrap(),
            Observation::Missing
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        let journal = root.join(DIRECTORY);
        // Deliberately ordinary/inherited permissions, matching the macOS
        // empty-root list policy rather than adding a Windows private ACL gate.
        fs::create_dir(&journal).unwrap();
        let before = native::open_no_reparse(&journal).unwrap().identity;
        assert_eq!(inspect_with(&root, |_| Ok(())).unwrap(), Observation::Empty);
        assert!(before.same_object(&native::open_no_reparse(&journal).unwrap().identity));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        assert_eq!(fs::read_dir(&journal).unwrap().count(), 0);
        assert_eq!(
            fs::read(root.join("retained-data")).unwrap(),
            b"disposable retained bytes"
        );
    }

    #[test]
    fn every_retained_child_blocks_inventory_without_adopting_or_cleaning_it() {
        for name in [
            ".creating-50bfcf0c-5102-4a27-8446-b3c7312e2bc1",
            "50bfcf0c-5102-4a27-8446-b3c7312e2bc1",
            "unknown-material",
        ] {
            let (_temporary, root) = fixture();
            let journal = root.join(DIRECTORY);
            fs::create_dir(&journal).unwrap();
            let child = journal.join(name);
            fs::create_dir(&child).unwrap();
            if name != "50bfcf0c-5102-4a27-8446-b3c7312e2bc1" {
                fs::write(child.join("task.json"), b"disposable unverified record").unwrap();
            }
            let before = native::open_no_reparse(&child).unwrap().identity;
            let error = require_absent_or_empty(&root).unwrap_err();
            assert_eq!(error.code, "PLATFORM_FEATURE_UNAVAILABLE");
            assert_eq!(error.details["inventory"], "retained");
            assert_eq!(error.details["operationOutcome"], "not_started");
            assert!(before.same_object(&native::open_no_reparse(&child).unwrap().identity));
            assert_eq!(fs::read_dir(&journal).unwrap().count(), 1);
            if name != "50bfcf0c-5102-4a27-8446-b3c7312e2bc1" {
                assert_eq!(
                    fs::read(child.join("task.json")).unwrap(),
                    b"disposable unverified record"
                );
            } else {
                assert_eq!(fs::read_dir(&child).unwrap().count(), 0);
            }
        }
    }

    #[test]
    fn newly_appearing_material_is_retained_and_never_reported_empty() {
        for point in [Point::MissingObserved, Point::EmptyObserved] {
            let (_temporary, root) = fixture();
            let journal = root.join(DIRECTORY);
            if point == Point::EmptyObserved {
                fs::create_dir(&journal).unwrap();
            }
            let error = inspect_with(&root, |observed| {
                assert!(observed == point);
                if point == Point::MissingObserved {
                    fs::create_dir(&journal).unwrap();
                }
                fs::write(journal.join("appeared"), b"disposable concurrent evidence").unwrap();
                Ok(())
            })
            .unwrap_err();
            assert_eq!(
                error.details["operationOutcome"],
                if point == Point::MissingObserved {
                    "unknown"
                } else {
                    "not_started"
                }
            );
            assert_eq!(
                fs::read(journal.join("appeared")).unwrap(),
                b"disposable concurrent evidence"
            );
            assert_eq!(fs::read_dir(&journal).unwrap().count(), 1);
        }
    }

    #[test]
    fn a_real_junction_is_rejected_without_changing_its_target() {
        let (_temporary, root) = fixture();
        let target = root.join("disposable-target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("retained"), b"disposable junction target").unwrap();
        let journal = root.join(DIRECTORY);
        let result = std::process::Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&journal)
            .arg(&target)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "Could not create disposable junction"
        );
        assert_eq!(
            require_absent_or_empty(&root).unwrap_err().code,
            "NODE_DELETION_JOURNAL_UNCONFIRMED"
        );
        assert_eq!(
            fs::read(target.join("retained")).unwrap(),
            b"disposable junction target"
        );
        assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
        fs::remove_dir(&journal).unwrap();
    }

    #[test]
    fn missing_parents_and_non_directory_roots_are_not_accepted_as_empty() {
        let (_temporary, root) = fixture();
        let missing_parent = root.join("missing-parent");
        assert!(require_absent_or_empty(&missing_parent).is_err());
        assert!(!missing_parent.exists());
        let journal = root.join(DIRECTORY);
        fs::write(&journal, b"disposable unexpected file").unwrap();
        assert_eq!(
            require_absent_or_empty(&root).unwrap_err().code,
            "NODE_DELETION_JOURNAL_UNCONFIRMED"
        );
        assert_eq!(fs::read(&journal).unwrap(), b"disposable unexpected file");
    }

    #[test]
    fn raw_verbatim_mixed_separators_inspect_the_actual_bound_directory() {
        use std::os::windows::ffi::OsStrExt;
        let (_temporary, root) = fixture();
        let actual = root.join("nested").join("actual 中文");
        fs::create_dir_all(&actual).unwrap();
        let mut raw = root.as_os_str().to_os_string();
        raw.push(r"\nested/actual 中文");
        let path = PathBuf::from(raw);
        assert!(path
            .as_os_str()
            .encode_wide()
            .any(|unit| unit == u16::from(b'/')));
        assert_eq!(
            inspect_with(&path, |_| Ok(())).unwrap(),
            Observation::Missing
        );
        let journal = actual.join(DIRECTORY);
        fs::create_dir(&journal).unwrap();
        fs::write(journal.join("retained"), b"disposable actual material").unwrap();
        assert_eq!(
            require_absent_or_empty(&path).unwrap_err().details["inventory"],
            "retained"
        );
        assert_eq!(
            fs::read(journal.join("retained")).unwrap(),
            b"disposable actual material"
        );
    }
}
