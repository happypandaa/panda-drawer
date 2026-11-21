//! Platform installation layout. Resource validation remains with core startup.
#[cfg(not(windows))]
#[path = "desktop_installation/macos.rs"]
mod native;
#[cfg(windows)]
#[path = "desktop_installation/windows.rs"]
mod native;

#[cfg(all(windows, feature = "production-profile"))]
pub(super) use native::verify_production_cli;

pub(super) fn resource_info(
    app: &tauri::App,
) -> Result<super::InstalledResources, Box<dyn std::error::Error>> {
    native::resource_info(app)
}
