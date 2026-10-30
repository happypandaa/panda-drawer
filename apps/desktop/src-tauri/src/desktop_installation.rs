//! Platform installation layout. Resource validation remains with core startup.
#[cfg(not(windows))]
#[path = "desktop_installation/macos.rs"]
mod native;
#[cfg(windows)]
#[path = "desktop_installation/windows.rs"]
mod native;

pub(super) fn resource_info(
    app: &tauri::App,
) -> Result<super::InstalledResources, Box<dyn std::error::Error>> {
    native::resource_info(app)
}
