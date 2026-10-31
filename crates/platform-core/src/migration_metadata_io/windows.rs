//! Windows copies and historical snapshots carry the real readonly bit. Unix
//! permission bits are never invented or used to authorize source deletion.
use super::blocked;
use crate::ApiError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CopyEntry {
    pub(crate) path: PathBuf,
    pub(crate) kind: String,
    pub(crate) bytes: u64,
    pub(crate) digest: Option<String>,
    pub(crate) target: Option<PathBuf>,
    pub(crate) read_only: bool,
}

fn relative(path: &Path) -> Result<(), ApiError> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(blocked("迁移文件清单含有越界或空路径。"));
    }
    Ok(())
}

pub(crate) fn entries(value: Value) -> Result<BTreeMap<PathBuf, CopyEntry>, ApiError> {
    let list: Vec<CopyEntry> =
        serde_json::from_value(value).map_err(|_| blocked("迁移文件清单无法解析。"))?;
    let mut result = BTreeMap::new();
    for entry in list {
        relative(&entry.path)?;
        let valid = match entry.kind.as_str() {
            "file" => {
                entry.target.is_none()
                    && entry.digest.as_ref().is_some_and(|digest| {
                        digest.len() == 64
                            && digest
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    })
            }
            "directory" => entry.bytes == 0 && entry.digest.is_none() && entry.target.is_none(),
            "symlink" => entry.bytes == 0 && entry.digest.is_none() && entry.target.is_some(),
            _ => false,
        };
        if !valid || result.insert(entry.path.clone(), entry).is_some() {
            return Err(blocked("迁移文件清单含重复或无效项目。"));
        }
    }
    Ok(result)
}
