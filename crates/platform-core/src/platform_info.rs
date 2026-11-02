use serde_json::{json, Value};
use std::path::PathBuf;

#[cfg(not(windows))]
pub(crate) fn installation_root() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    executable
        .ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .map(|p| p.to_path_buf())
}

#[cfg(windows)]
pub(crate) fn installation_root() -> Option<PathBuf> {
    // Windows has selected members, not a .app ancestor or a whole exe parent.
    None
}

pub(crate) fn info() -> Value {
    json!({
        "name": crate::DISPLAY_NAME,
        "identifier": crate::APP_IDENTIFIER,
        "version": crate::APP_VERSION,
        "architecture": std::env::consts::ARCH,
        "installationPath": installation_root(),
        "executablePath": std::env::current_exe().ok(),
        "distribution": crate::build_profile::DISTRIBUTION,
        "build": crate::build_profile::info(),
        "updates": {"available":false,"reason": if crate::build_profile::PRODUCTION_PROFILE { "正式版资源签名、公证和升级流程尚未完成" } else { "正式发行源尚未配置；内部开发包由项目交付" }}
    })
}

pub(crate) fn with_resources(resources: Option<&crate::resources::ProjectResources>) -> Value {
    let mut value = info();
    value["manifestSchemaPath"] = resources
        .map(|resources| resources.manifest_schema_path())
        .filter(|path| crate::path_checks::check_no_symlinks(path, false).is_ok() && path.is_file())
        .map_or(Value::Null, |path| json!(path));
    value["skillPath"] = resources
        .map(|resources| resources.skill_path())
        .filter(|path| crate::path_checks::check_no_symlinks(path, false).is_ok() && path.is_file())
        .map_or(Value::Null, |path| json!(path));
    if let Some(scope) = resources.and_then(|resources| resources.installation_scope()) {
        scope.add_platform_info(&mut value);
    }
    value
}
