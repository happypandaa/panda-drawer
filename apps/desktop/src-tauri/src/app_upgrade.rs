//! Refuse old development material without deleting it or adopting author folders.
//! The current workspace is already selected and validated by RuntimePaths.
use platform_core::{ApiError, RuntimePaths};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MESSAGE: &str = "发现旧数据，请先用旧版清理本机数据";
const JSON_LIMIT: u64 = 64 * 1024;

pub(super) fn check(paths: &RuntimePaths) -> Result<(), ApiError> {
    check_at(paths.data_root(), paths.workspace_root())
}

fn plain_directory(path: &Path) -> Result<bool, ApiError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            Ok(metadata.is_dir() && !platform_native::filesystem::is_reparse(&metadata))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn json(path: &Path) -> Result<Option<Value>, ApiError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file()
        || platform_native::filesystem::is_reparse(&metadata)
        || metadata.len() > JSON_LIMIT
    {
        return Ok(None);
    }
    let file = platform_native::filesystem::open_read(
        path,
        platform_native::filesystem::ReadOptions::NoLinksNonblocking,
    )?;
    let mut bytes = Vec::new();
    file.take(JSON_LIMIT + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > JSON_LIMIT {
        return Ok(None);
    }
    Ok(serde_json::from_slice(&bytes).ok())
}

fn current_roots(data: &Path) -> Result<BTreeSet<PathBuf>, ApiError> {
    let mut roots = BTreeSet::new();
    let apps = data.join("apps");
    if !plain_directory(&apps)? {
        return Ok(roots);
    }
    for entry in fs::read_dir(apps)? {
        let directory = entry?.path();
        if !plain_directory(&directory)? {
            continue;
        }
        for name in ["registration.json", "reservation.json"] {
            let Some(record) = json(&directory.join(name))? else {
                continue;
            };
            for key in ["projectRoot", "sourcePath"] {
                if let Some(path) = record.get(key).and_then(Value::as_str) {
                    roots.insert(PathBuf::from(path));
                }
            }
        }
    }
    Ok(roots)
}

fn check_at(data: &Path, workspace: &Path) -> Result<(), ApiError> {
    let mut old = BTreeSet::new();
    // These names belong exclusively to the retired application stores. Empty
    // containers are harmless; settings and current apps are deliberately ignored.
    for name in [
        "web-apps",
        "node-apps",
        "web-project-ownership",
        "node-project-ownership",
        "node-source-owners",
        "node-deletion-tasks",
    ] {
        let directory = data.join(name);
        match fs::symlink_metadata(&directory) {
            Ok(metadata) if platform_native::filesystem::is_reparse(&metadata) => {
                old.insert(directory);
            }
            Ok(metadata) if metadata.is_dir() => {
                if fs::read_dir(&directory)?.next().transpose()?.is_some() {
                    old.insert(directory);
                }
            }
            Ok(_) => {
                old.insert(directory);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let ledger = data.join("webview-stores");
    if plain_directory(&ledger)? {
        for entry in fs::read_dir(ledger)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("web-")
                || name.starts_with("node-")
                || name.starts_with("clear-web-")
                || name.starts_with("clear-node-")
            {
                old.insert(entry.path());
            }
        }
    }
    let current = current_roots(data)?;
    // Inspect only direct children of the platform's two fixed source containers.
    // Never walk registered external originals, arbitrary workspace folders, data,
    // attachments, or links. New registered/reserved roots may be temporarily invalid.
    for container in [
        workspace.join("projects"),
        workspace.join("project-sources"),
    ] {
        if !plain_directory(&container)? {
            continue;
        }
        for entry in fs::read_dir(container)? {
            let directory = entry?.path();
            if current.contains(&directory) || !plain_directory(&directory)? {
                continue;
            }
            if json(&directory.join("panda.json"))?.is_some_and(|manifest| {
                manifest.get("manifestVersion").and_then(Value::as_u64) == Some(1)
            }) {
                old.insert(directory);
            }
        }
    }
    if old.is_empty() {
        Ok(())
    } else {
        Err(ApiError::new("OLD_APP_DATA_FOUND", MESSAGE).detail("paths", serde_json::json!(old)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_registration_is_reported_and_never_modified() {
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().join("data");
        let workspace = temporary.path().join("selected-workspace");
        let registration = data.join("node-apps/fixture/registration.json");
        fs::create_dir_all(registration.parent().unwrap()).unwrap();
        fs::write(&registration, b"old evidence").unwrap();
        let error = check_at(&data, &workspace).unwrap_err();
        assert_eq!(error.code, "OLD_APP_DATA_FOUND");
        assert_eq!(error.message, MESSAGE);
        assert_eq!(fs::read(registration).unwrap(), b"old evidence");
    }

    #[test]
    fn current_apps_reservations_settings_and_empty_old_containers_are_allowed() {
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().join("data");
        let workspace = temporary.path().join("selected-workspace");
        for root in [
            data.join("web-apps"),
            data.join("node-apps"),
            data.join("webview-stores/app-current"),
            data.join("app-ownership/current"),
        ] {
            fs::create_dir_all(root).unwrap();
        }
        fs::write(data.join("settings.json"), b"preserved settings").unwrap();
        for (id, record) in [
            ("registered", "registration.json"),
            ("reserved", "reservation.json"),
        ] {
            let source = workspace.join("projects").join(id);
            fs::create_dir_all(&source).unwrap();
            fs::write(source.join("panda.json"), br#"{"manifestVersion":1}"#).unwrap();
            let slot = data.join("apps").join(id);
            fs::create_dir_all(&slot).unwrap();
            fs::write(
                slot.join(record),
                serde_json::to_vec(&serde_json::json!({"projectRoot":source})).unwrap(),
            )
            .unwrap();
        }
        check_at(&data, &workspace).unwrap();
        assert_eq!(
            fs::read(data.join("settings.json")).unwrap(),
            b"preserved settings"
        );
    }

    #[test]
    fn finds_old_managed_manifest_and_ledger_but_does_not_search_external_originals() {
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().join("data");
        let workspace = temporary.path().join("migrated-workspace");
        let external = temporary.path().join("external-original");
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("panda.json"), br#"{"manifestVersion":1}"#).unwrap();
        check_at(&data, &workspace).unwrap();
        let source = workspace.join("project-sources/old-application");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("panda.json"), br#"{"manifestVersion":1}"#).unwrap();
        assert_eq!(
            check_at(&data, &workspace).unwrap_err().code,
            "OLD_APP_DATA_FOUND"
        );
        fs::remove_file(source.join("panda.json")).unwrap();
        let ledger = data.join("webview-stores/clear-web-prj_old.json");
        fs::create_dir_all(ledger.parent().unwrap()).unwrap();
        fs::write(&ledger, b"native recovery receipt").unwrap();
        assert_eq!(
            check_at(&data, &workspace).unwrap_err().code,
            "OLD_APP_DATA_FOUND"
        );
        assert_eq!(fs::read(ledger).unwrap(), b"native recovery receipt");
    }

    #[cfg(unix)]
    #[test]
    fn managed_container_links_never_search_their_external_target() {
        let temporary = tempfile::tempdir().unwrap();
        let data = temporary.path().join("data");
        let workspace = temporary.path().join("workspace");
        let external = temporary.path().join("external-original");
        fs::create_dir_all(external.join("application")).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        fs::write(
            external.join("application/panda.json"),
            br#"{"manifestVersion":1}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink(&external, workspace.join("projects")).unwrap();
        check_at(&data, &workspace).unwrap();
        assert!(external.join("application/panda.json").exists());
    }
}
