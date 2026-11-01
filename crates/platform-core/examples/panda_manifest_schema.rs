//! Deterministic build-time export; never loads a workspace or user application.
fn main() {
    println!(
        "{}",
        serde_json::to_string_pretty(&platform_core::panda_manifest::export_schema())
            .expect("manifest schema serializes")
    );
}
