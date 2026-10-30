use super::super::{InstalledResources, ResourceInfo};
use std::path::PathBuf;
use tauri::Manager;

pub(super) fn resource_info(
    app: &tauri::App,
) -> Result<InstalledResources, Box<dyn std::error::Error>> {
    let mut resources = app.path().resource_dir()?.join("resources");
    if cfg!(debug_assertions) && !resources.join("README.md").is_file() {
        resources = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
    }
    let installed_cli = std::env::current_exe()?
        .parent()
        .ok_or("Missing executable directory")?
        .join("local-projects-dev");
    let cli = if installed_cli.is_file() {
        installed_cli
    } else if cfg!(debug_assertions) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries/local-projects-dev-aarch64-apple-darwin")
    } else {
        installed_cli
    };
    let cli_path = cli.is_file().then(|| cli.to_string_lossy().into_owned());
    Ok(InstalledResources {
        info: ResourceInfo {
            sdk: resources
                .join("sdk/releases/local-projects-sdk-0.1.0.tgz")
                .is_file(),
            template: resources.join("template/app/package.json").is_file(),
            readme: std::fs::read_to_string(resources.join("README.md")).is_ok(),
            cli: cli_path.is_some(),
        },
        cli_path,
        root: resources,
        cli_executable: cli,
    })
}
