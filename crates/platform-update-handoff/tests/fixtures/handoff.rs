// Test-only process entry point, never bundled into the desktop app.
// The fixture imports the production pure coordinator in normal binary mode;
// its own unit tests are checked/run in platform-desktop, not in this binary.
#[cfg(all(target_os = "macos", not(test)))]
#[path = "handoff_macos.rs"]
mod macos;
#[cfg(all(target_os = "macos", not(test)))]
fn main() {
    macos::run();
}
#[cfg(any(not(target_os = "macos"), test))]
fn main() {
    panic!("macOS fixture executable only");
}
