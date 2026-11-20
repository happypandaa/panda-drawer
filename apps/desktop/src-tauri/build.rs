fn main() {
    println!("cargo:rerun-if-env-changed=PANDA_DRAWER_UPDATE_PUBLIC_KEY");
    let test_profile = std::env::var_os("CARGO_FEATURE_LOCAL_UPDATE_TEST").is_some();
    let production = std::env::var_os("CARGO_FEATURE_PRODUCTION_PROFILE").is_some();
    let key = if production {
        const KEY_FILE: &str = "../../../build-profiles/production-updater.pub";
        println!("cargo:rerun-if-changed={KEY_FILE}");
        std::fs::read_to_string(KEY_FILE)
            .expect("the production update public key must be present")
            .trim_end()
            .to_owned()
    } else if test_profile {
        std::env::var("PANDA_DRAWER_UPDATE_PUBLIC_KEY")
            .expect("the local-update test profile requires its public verification key")
    } else {
        String::new()
    };
    if production || test_profile {
        assert!(
            (40..=512).contains(&key.len())
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"+/=".contains(&byte)),
            "invalid public update verification key format"
        );
    }
    println!("cargo:rustc-env=PANDA_DRAWER_UPDATE_PUBLIC_KEY={key}");
    tauri_build::build();
}
