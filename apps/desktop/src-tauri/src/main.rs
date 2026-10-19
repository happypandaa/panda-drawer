#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
mod macos_host;
#[cfg(target_os = "windows")]
mod windows_host;

fn main() {
    #[cfg(target_os = "macos")]
    macos_host::run();
    #[cfg(target_os = "windows")]
    windows_host::run();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        eprintln!("PLATFORM_UNSUPPORTED: 当前系统未纳入平台支持范围。");
        std::process::exit(1);
    }
}
