use serde_json::{json, Value};
use std::path::PathBuf;

pub(crate) fn installation_root() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    executable
        .ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .map(|p| p.to_path_buf())
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
