//! Shared declaration-only source inspection; native callers provide bounded I/O.
use super::SourceInspection;
use crate::{
    panda_manifest::{PandaManifest, PandaRuntime, MANIFEST_FILE},
    ApiError,
};
use std::{fs, path::Path};

pub(super) fn inspect(
    path: &Path,
    read_public: impl Fn(&Path) -> Result<Vec<u8>, ApiError>,
) -> Result<SourceInspection, ApiError> {
    let mut result = SourceInspection {
        kind: "needs-adaptation".into(),
        name: None,
        next_step: "review-and-adapt-source".into(),
        execution_state: "not-executed".into(),
        requires_build: false,
        issues: Vec::new(),
    };
    let manifest_path = path.join(MANIFEST_FILE);
    match fs::symlink_metadata(&manifest_path) {
        Ok(_) => {
            let declaration = read_public(&manifest_path).and_then(|bytes| {
                PandaManifest::parse(&bytes)
                    .map_err(|error| ApiError::new("SOURCE_DECLARATION_INVALID", error.to_string()))
            });
            match declaration {
                Ok(manifest) => match manifest.inspect_local(path) {
                    Ok(inspection) => {
                        result.name = Some(manifest.app.name.clone());
                        match manifest.runtime() {
                            PandaRuntime::Node => {
                                result.kind = "node-service-source".into();
                                result.next_step =
                                    "prepare-node-runtime-before-registration".into();
                            }
                            PandaRuntime::Web => {
                                result.kind = "web-source".into();
                                result.requires_build = inspection["needsBuild"] == true;
                                result.next_step = if result.requires_build {
                                    "confirm-web-build-before-registration"
                                } else {
                                    "register-web-source"
                                }
                                .into();
                                if inspection["entryMissing"] == true && !result.requires_build {
                                    result.next_step = "review-and-adapt-source".into();
                                    result.issues.push(ApiError::new(
                                        "SOURCE_WEB_ENTRY_MISSING",
                                        "web.entry：入口文件不存在，且未声明构建方式。",
                                    ));
                                }
                            }
                        }
                    }
                    Err(error) => result.issues.push(ApiError::new(
                        "SOURCE_DECLARATION_INVALID",
                        error.to_string(),
                    )),
                },
                Err(error) => result.issues.push(error),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Dependency metadata is public; never inspect .env or business data.
            for name in ["package.json", "package-lock.json"] {
                let file = path.join(name);
                if fs::symlink_metadata(&file).is_ok()
                    && read_public(&file)
                        .and_then(|bytes| {
                            serde_json::from_slice::<serde_json::Value>(&bytes)
                                .map_err(ApiError::from)
                        })
                        .is_err()
                {
                    result.issues.push(ApiError::new(
                        "SOURCE_DECLARATION_INVALID",
                        "npm 声明文件无效或超限。",
                    ));
                }
            }
            result.issues.push(ApiError::new(
                "SOURCE_DECLARATION_MISSING",
                "尚无有效 panda.json；请让 AI 检查并准备项目声明。",
            ));
        }
        Err(error) => return Err(error.into()),
    }
    Ok(result)
}
