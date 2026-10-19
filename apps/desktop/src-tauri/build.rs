fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let resources = std::path::Path::new("resources/windows/embedded-project-resources.json");
        if !resources.is_file() {
            panic!("Windows project resources are missing. Run node scripts/prepare-windows-resources.mjs from the repository before compiling the desktop.");
        }
        println!("cargo:rerun-if-changed={}", resources.display());
    }
    tauri_build::build()
}
