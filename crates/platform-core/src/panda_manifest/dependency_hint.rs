//! Conservative dependency advice. It never loads project code or prepares files.
use crate::files;
use platform_native::tool_files::Facts;
use serde_json::Value;
use std::{fs, io::Read, path::Path};

const MAX_PACKAGE_BYTES: u64 = 64 * 1024;
const MAX_DIRECTORIES: usize = 64;
const MISSING_WARNING: &str = "项目声明了依赖组件，但没有找到已安装的组件。打开前可能需要先在这个项目文件夹中安装依赖，准备好后再重新打开。";

enum PublicPackage {
    Missing,
    Known(Value),
    Unknown,
}

fn read_package(directory: &Path) -> PublicPackage {
    let path = directory.join("package.json");
    let before = match Facts::capture(&path) {
        Ok(facts) => facts,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return PublicPackage::Missing;
        }
        Err(_) => return PublicPackage::Unknown,
    };
    if !before.is_file()
        || before.is_reparse()
        || before.links() != 1
        || before.len() > MAX_PACKAGE_BYTES
    {
        return PublicPackage::Unknown;
    }
    let Some(root) = files::root(directory).ok() else {
        return PublicPackage::Unknown;
    };
    let Some(file) = files::read(&root, "package.json").ok() else {
        return PublicPackage::Unknown;
    };
    let Some(opened) = Facts::from_file(&file).ok() else {
        return PublicPackage::Unknown;
    };
    if !before.unchanged(&opened) || opened.links() != 1 {
        return PublicPackage::Unknown;
    }
    let mut bytes = Vec::new();
    if (&file)
        .take(MAX_PACKAGE_BYTES + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() as u64 != opened.len()
    {
        return PublicPackage::Unknown;
    }
    let Some(after) = Facts::capture(&path).ok() else {
        return PublicPackage::Unknown;
    };
    let Some(held) = Facts::from_file(&file).ok() else {
        return PublicPackage::Unknown;
    };
    if !opened.unchanged(&after) || !opened.unchanged(&held) || after.links() != 1 {
        return PublicPackage::Unknown;
    }
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(value) if value.is_object() => PublicPackage::Known(value),
        _ => PublicPackage::Unknown,
    }
}

// Any existing entry, including a dangling link or special file, is uncertainty.
// Permission errors are not evidence that components are missing.
fn definitely_absent(path: &Path) -> bool {
    matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}

fn required_dependencies(package: &Value) -> Option<bool> {
    if package.get("workspaces").is_some() {
        return None;
    }
    let optional = match package.get("optionalDependencies") {
        None => None,
        Some(Value::Object(values)) => Some(values),
        _ => return None,
    };
    let dependencies = match package.get("dependencies") {
        None => return Some(false),
        Some(Value::Object(values)) => values,
        _ => return None,
    };
    let mut required = false;
    for (name, version) in dependencies {
        if optional.is_some_and(|values| values.contains_key(name)) {
            continue;
        }
        let version = version.as_str().filter(|value| !value.is_empty())?;
        if version.starts_with("workspace:") {
            return None;
        }
        required = true;
    }
    Some(required)
}

pub(super) fn warnings(app_dir: &Path, backend_entry: &Path) -> Vec<String> {
    let Some(root) = app_dir.canonicalize().ok() else {
        return Vec::new();
    };
    let Some(entry) = backend_entry.canonicalize().ok() else {
        return Vec::new();
    };
    let Some(start) = entry.parent().filter(|parent| parent.starts_with(&root)) else {
        return Vec::new();
    };
    let directories: Vec<_> = start.ancestors().collect();
    if directories.len() > MAX_DIRECTORIES {
        return Vec::new();
    }
    let mut required = false;
    for directory in directories
        .iter()
        .copied()
        .take_while(|path| path.starts_with(&root))
    {
        if !definitely_absent(&directory.join("node_modules"))
            || !definitely_absent(&directory.join("pnpm-workspace.yaml"))
        {
            return Vec::new();
        }
        match read_package(directory) {
            PublicPackage::Missing => (),
            PublicPackage::Unknown => return Vec::new(),
            PublicPackage::Known(package) => match required_dependencies(&package) {
                Some(value) => required |= value,
                None => return Vec::new(),
            },
        }
    }
    if !required {
        return Vec::new();
    }
    // Node can resolve modules from ancestors outside the selected project.
    // A shared workspace or any existing module directory makes this hint unsafe.
    for directory in root.ancestors().skip(1) {
        if !definitely_absent(&directory.join("node_modules"))
            || !definitely_absent(&directory.join("pnpm-workspace.yaml"))
        {
            return Vec::new();
        }
        match read_package(directory) {
            PublicPackage::Missing => (),
            PublicPackage::Unknown => return Vec::new(),
            PublicPackage::Known(package) if package.get("workspaces").is_some() => {
                return Vec::new();
            }
            PublicPackage::Known(_) => (),
        }
    }
    vec![MISSING_WARNING.into()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(
        package: Option<&str>,
    ) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let temporary = tempfile::Builder::new()
            .prefix("panda-disposable-dependency-hint-")
            .tempdir()
            .unwrap();
        let source = temporary.path().join("app");
        let server = source.join("server");
        fs::create_dir_all(&server).unwrap();
        let entry = server.join("main.mjs");
        fs::write(&entry, b"export {};\n").unwrap();
        if let Some(package) = package {
            fs::write(source.join("package.json"), package).unwrap();
        }
        (temporary, source, entry)
    }

    #[test]
    fn missing_components_warn_without_writing_and_warning_clears_after_preparation() {
        let (_temporary, source, entry) = fixture(Some(r#"{"dependencies":{"example":"1.0.0"}}"#));
        let original = fs::read(source.join("package.json")).unwrap();
        let names = || {
            let mut names = fs::read_dir(&source)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect::<Vec<_>>();
            names.sort();
            names
        };
        let entries = names();
        assert_eq!(warnings(&source, &entry), vec![MISSING_WARNING]);
        assert_eq!(fs::read(source.join("package.json")).unwrap(), original);
        assert_eq!(names(), entries);
        fs::create_dir(source.join("node_modules")).unwrap();
        assert!(warnings(&source, &entry).is_empty());
    }

    #[test]
    fn absent_empty_development_and_optional_dependencies_do_not_warn() {
        for package in [
            None,
            Some("{}"),
            Some(r#"{"dependencies":{}}"#),
            Some(r#"{"devDependencies":{"example":"1.0.0"}}"#),
            Some(r#"{"optionalDependencies":{"example":"1.0.0"}}"#),
            Some(
                r#"{"dependencies":{"example":"1.0.0"},"optionalDependencies":{"example":"2.0.0"}}"#,
            ),
        ] {
            let (_temporary, source, entry) = fixture(package);
            assert!(warnings(&source, &entry).is_empty());
        }
    }

    #[test]
    fn backend_packages_and_intermediate_module_directories_follow_entry_lookup() {
        let (_temporary, source, entry) = fixture(None);
        fs::write(
            source.join("server/package.json"),
            r#"{"dependencies":{"example":"1.0.0"}}"#,
        )
        .unwrap();
        assert_eq!(warnings(&source, &entry), vec![MISSING_WARNING]);
        fs::create_dir(source.join("server/node_modules")).unwrap();
        assert!(warnings(&source, &entry).is_empty());
    }

    #[test]
    fn shared_modules_and_workspace_declarations_are_unknown() {
        for marker in ["node_modules", "pnpm-workspace.yaml", "package.json"] {
            let (temporary, source, entry) =
                fixture(Some(r#"{"dependencies":{"example":"1.0.0"}}"#));
            let marker = temporary.path().join(marker);
            if marker.file_name().unwrap() == "node_modules" {
                fs::create_dir(marker).unwrap();
            } else {
                fs::write(marker, r#"{"workspaces":["app"]}"#).unwrap();
            }
            assert!(warnings(&source, &entry).is_empty());
        }
        for package in [
            r#"{"workspaces":["packages/*"],"dependencies":{"example":"1.0.0"}}"#,
            r#"{"dependencies":{"example":"workspace:*"}}"#,
        ] {
            let (_temporary, source, entry) = fixture(Some(package));
            assert!(warnings(&source, &entry).is_empty());
        }
    }

    #[test]
    fn invalid_and_special_public_metadata_are_unknown() {
        for package in [
            "invalid json",
            "[]",
            r#"{"dependencies":[]}"#,
            r#"{"dependencies":{"example":null}}"#,
        ] {
            let (_temporary, source, entry) = fixture(Some(package));
            assert!(warnings(&source, &entry).is_empty());
        }
        let (_temporary, source, entry) = fixture(Some(r#"{"dependencies":{"example":"1.0.0"}}"#));
        fs::write(
            source.join("node_modules"),
            b"not an installation directory",
        )
        .unwrap();
        assert!(warnings(&source, &entry).is_empty());
        let (_temporary, source, entry) = fixture(None);
        fs::write(
            source.join("package.json"),
            vec![b'x'; MAX_PACKAGE_BYTES as usize + 1],
        )
        .unwrap();
        assert!(warnings(&source, &entry).is_empty());
    }

    #[test]
    fn shared_public_manifest_is_unknown_and_original_bytes_are_preserved() {
        let (temporary, source, entry) = fixture(Some(r#"{"dependencies":{"example":"1.0.0"}}"#));
        let manifest = source.join("package.json");
        let retained = temporary.path().join("retained-public-manifest");
        fs::hard_link(&manifest, &retained).unwrap();
        let original = fs::read(&retained).unwrap();
        assert!(warnings(&source, &entry).is_empty());
        assert_eq!(fs::read(&retained).unwrap(), original);
        assert_eq!(fs::read(&manifest).unwrap(), original);
    }
}
