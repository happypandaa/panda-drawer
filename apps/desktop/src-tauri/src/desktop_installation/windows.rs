use super::super::{InstalledResources, ResourceInfo};
use std::path::{Path, PathBuf};
use tauri::Manager;

fn locations(
    resource_directory: &Path,
    executable: &Path,
) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    Ok((
        resource_directory.join("resources"),
        executable
            .parent()
            .ok_or("Missing executable directory")?
            .join("local-projects-dev.exe"),
    ))
}

fn inspect(
    resource_directory: &Path,
    executable: &Path,
) -> Result<InstalledResources, Box<dyn std::error::Error>> {
    let (resources, cli) = locations(resource_directory, executable)?;
    let cli_path = cli.is_file().then(|| cli.to_string_lossy().into_owned());
    Ok(InstalledResources {
        info: ResourceInfo {
            sdk: resources.join("sdk/sdk.js").is_file()
                && resources.join("sdk/panda.d.ts").is_file(),
            template: resources.join("template/app/package.json").is_file(),
            readme: std::fs::read_to_string(resources.join("README.md")).is_ok(),
            cli: cli_path.is_some(),
        },
        cli_path,
        root: resources,
        cli_executable: cli,
    })
}

pub(super) fn resource_info(
    app: &tauri::App,
) -> Result<InstalledResources, Box<dyn std::error::Error>> {
    inspect(&app.path().resource_dir()?, &std::env::current_exe()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn windows_paths_select_only_tauri_resources_and_the_copied_sidecar_name() {
        let (resources, cli) = locations(
            Path::new(r"D:\installed 中文\resource-root"),
            Path::new(r"D:\installed 中文\bin\panda-drawer.exe"),
        )
        .unwrap();
        assert_eq!(
            resources,
            Path::new(r"D:\installed 中文\resource-root\resources")
        );
        assert_eq!(
            cli,
            Path::new(r"D:\installed 中文\bin\local-projects-dev.exe")
        );
        assert_eq!(
            locations(Path::new(r"D:\resources"), Path::new(r"D:\"))
                .unwrap_err()
                .to_string(),
            "Missing executable directory"
        );
    }

    #[test]
    fn missing_installed_resources_and_sidecar_never_select_source_or_mac_names() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let resource_directory = root.join("installed-resource-root");
        let executable = root.join("bin").join("panda-drawer.exe");
        fs::create_dir(executable.parent().unwrap()).unwrap();
        fs::write(
            executable.parent().unwrap().join("local-projects-dev"),
            b"old Mac name",
        )
        .unwrap();
        fs::write(
            executable
                .parent()
                .unwrap()
                .join("local-projects-dev-x86_64-pc-windows-msvc.exe"),
            b"source sidecar name",
        )
        .unwrap();
        let source = root.join("source-tree").join("src-tauri").join("resources");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("README.md"), b"preserve source resources").unwrap();
        let report = inspect(&resource_directory, &executable).unwrap();
        assert_eq!(report.root, resource_directory.join("resources"));
        assert_eq!(
            report.cli_executable,
            executable.parent().unwrap().join("local-projects-dev.exe")
        );
        assert!(report.cli_path.is_none());
        assert!(
            !report.info.sdk && !report.info.template && !report.info.readme && !report.info.cli
        );
        assert_eq!(
            fs::read(source.join("README.md")).unwrap(),
            b"preserve source resources"
        );
        assert!(!resource_directory.exists());
    }

    #[test]
    fn actual_installed_resource_flags_and_cli_follow_only_the_selected_layout() {
        let temporary = tempfile::tempdir().unwrap();
        let resource_directory = temporary.path().canonicalize().unwrap();
        let resources = resource_directory.join("resources");
        fs::create_dir_all(resources.join("sdk")).unwrap();
        fs::write(
            resources.join("sdk/panda.d.ts"),
            "declare const panda: unknown;",
        )
        .unwrap();
        fs::create_dir_all(resources.join("template/app")).unwrap();
        fs::write(resources.join("sdk/sdk.js"), b"disposable SDK marker").unwrap();
        fs::write(resources.join("template/app/package.json"), b"{}").unwrap();
        fs::write(resources.join("README.md"), b"disposable resource readme").unwrap();
        let executable = resource_directory.join("panda-drawer.exe");
        let cli = resource_directory.join("local-projects-dev.exe");
        fs::write(&cli, b"disposable sidecar marker; never executed").unwrap();
        let report = inspect(&resource_directory, &executable).unwrap();
        assert!(report.info.sdk && report.info.template && report.info.readme && report.info.cli);
        assert_eq!(report.cli_path.as_deref(), Some(cli.to_str().unwrap()));
        assert_eq!(report.cli_executable, cli);
        fs::write(resources.join("README.md"), [0xff]).unwrap();
        assert!(
            !inspect(&resource_directory, &executable)
                .unwrap()
                .info
                .readme
        );
        assert_eq!(
            fs::read(&report.cli_executable).unwrap(),
            b"disposable sidecar marker; never executed"
        );
    }
}
