//! Fixed per-instance loopback addresses. Assignment is private platform state.
use crate::{projects::check_no_symlinks, ApiError};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use uuid::Uuid;
static ALLOCATION: OnceLock<Mutex<()>> = OnceLock::new();
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FixedAppAddress {
    pub port: u16,
}
impl FixedAppAddress {
    pub fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    pub(crate) fn validate(&self) -> Result<(), ApiError> {
        if self.port < 1024 {
            return Err(invalid("应用固定端口无效。"));
        }
        Ok(())
    }
    pub async fn bind(&self) -> Result<tokio::net::TcpListener, ApiError> {
        self.validate()?;
        let listener = platform_native::app_listener::bind_loopback(self.port).map_err(|e| {
            let code = if matches!(
                e.kind(),
                std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied
            ) {
                "APP_PORT_IN_USE"
            } else {
                "APP_ADDRESS_BIND_FAILED"
            };
            ApiError::new(
                code,
                format!("应用的固定地址 {} 暂时无法使用。请关闭占用端口 {} 的程序后重试；平台不会更换地址。", self.origin(), self.port),
            )
            .detail("origin", self.origin())
            .detail("port", self.port)
            .detail("reason", e.to_string())
        })?;
        tokio::net::TcpListener::from_std(listener).map_err(|e| {
            ApiError::new("APP_ADDRESS_BIND_FAILED", e.to_string()).detail("origin", self.origin())
        })
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AppAddressKind {
    Web,
    Node,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AddressReservation {
    version: u32,
    runtime: AppAddressKind,
    id: String,
    address: FixedAppAddress,
}
fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::new("APP_ADDRESS_INVALID", message)
}
fn private_directory(data: &Path, kind: AppAddressKind, id: &str) -> Result<PathBuf, ApiError> {
    let uuid = match kind {
        AppAddressKind::Web => id
            .strip_prefix("prj_")
            .and_then(|s| Uuid::parse_str(s).ok())
            .filter(|u| format!("prj_{}", u.simple()) == id),
        AppAddressKind::Node => Uuid::parse_str(id).ok().filter(|u| u.to_string() == id),
    };
    if uuid.is_none() {
        return Err(invalid("应用地址实例标识无效。"));
    }
    Ok(data
        .join(match kind {
            AppAddressKind::Web => "web-apps",
            AppAddressKind::Node => "node-apps",
        })
        .join(id))
}
fn read_bytes(path: &Path, limit: u64) -> Result<Vec<u8>, ApiError> {
    check_no_symlinks(path, false)?;
    let facts = platform_native::tool_files::Facts::capture(path)?;
    if !facts.is_file() || facts.links() != 1 || facts.len() > limit {
        return Err(invalid("应用地址记录无效。"));
    }
    let file = platform_native::filesystem::open_read(
        path,
        platform_native::filesystem::ReadOptions::NoLinksNonblocking,
    )?;
    let opened = platform_native::tool_files::Facts::from_file(&file)?;
    if !opened.same_object(&facts) {
        return Err(invalid("应用地址记录已变化。"));
    }
    let mut bytes = vec![];
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit
        || !facts.same_modified_file(&platform_native::tool_files::Facts::capture(path)?)
    {
        return Err(invalid("应用地址记录已变化。"));
    }
    Ok(bytes)
}
fn read_reservation(
    path: &Path,
    kind: AppAddressKind,
    id: &str,
) -> Result<AddressReservation, ApiError> {
    let record: AddressReservation = serde_json::from_slice(&read_bytes(path, 4096)?)
        .map_err(|_| invalid("应用地址记录格式无效。"))?;
    if record.version != 1 || record.runtime != kind || record.id != id {
        return Err(invalid("应用地址记录与实例不符。"));
    }
    record.address.validate()?;
    Ok(record)
}
fn registered_address(
    path: &Path,
    kind: AppAddressKind,
    id: &str,
) -> Result<FixedAppAddress, ApiError> {
    let value: serde_json::Value = serde_json::from_slice(&read_bytes(path, 1024 * 1024)?)
        .map_err(|_| invalid("应用地址登记无效。"))?;
    let valid_version = match kind {
        AppAddressKind::Web => value["schemaVersion"] == 3,
        AppAddressKind::Node => value["version"] == 4,
    };
    if !valid_version || value["id"] != id {
        return Err(invalid("应用地址登记版本或实例标识无效。"));
    }
    let address: FixedAppAddress = serde_json::from_value(value["address"].clone())
        .map_err(|_| invalid("应用登记缺少固定地址，未分配替代地址。"))?;
    address.validate()?;
    Ok(address)
}
pub(crate) fn read_app_address(
    data: &Path,
    kind: AppAddressKind,
    id: &str,
) -> Result<FixedAppAddress, ApiError> {
    let directory = private_directory(data, kind, id)?;
    Ok(read_reservation(&directory.join("address.json"), kind, id)?.address)
}
pub(crate) fn require_app_address(
    data: &Path,
    kind: AppAddressKind,
    id: &str,
    address: FixedAppAddress,
) -> Result<(), ApiError> {
    address.validate()?;
    if read_app_address(data, kind, id)? != address {
        return Err(invalid("固定地址与实例登记不符，未分配替代地址。"));
    }
    Ok(())
}
/// The held candidate listener prevents another process from taking the selected
/// port until its exact reservation is durably published. Closed and pending
/// instances still reserve their ports through the same per-instance record.
pub(crate) fn reserve_app_address(
    data: &Path,
    kind: AppAddressKind,
    id: &str,
) -> Result<FixedAppAddress, ApiError> {
    let _process = ALLOCATION
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| invalid("应用地址分配锁不可用。"))?;
    check_no_symlinks(data, false)?;
    let lock_path = data.join("app-address.lock");
    check_no_symlinks(&lock_path, true)?;
    let lock = platform_native::filesystem::open_private_file(&lock_path, true, false)?;
    if !lock.metadata()?.is_file() || crate::project_file_io::link_count(&lock_path)? != 1 {
        return Err(invalid("应用地址分配锁无效。"));
    }
    lock.lock_exclusive()?;
    let directory = private_directory(data, kind, id)?;
    check_no_symlinks(&directory, false)?;
    let target = directory.join("address.json");
    if target.try_exists()? {
        return Ok(read_reservation(&target, kind, id)?.address);
    }
    if directory.join("registration.json").try_exists()? {
        return Err(invalid("已登记实例缺少固定地址记录，未分配替代地址。"));
    }
    let mut used = HashSet::new();
    for candidate_kind in [AppAddressKind::Web, AppAddressKind::Node] {
        let parent = data.join(match candidate_kind {
            AppAddressKind::Web => "web-apps",
            AppAddressKind::Node => "node-apps",
        });
        check_no_symlinks(&parent, true)?;
        if !parent.try_exists()? {
            continue;
        }
        for entry in fs::read_dir(parent)? {
            let entry = entry?;
            let candidate_id = entry.file_name().to_string_lossy().into_owned();
            if private_directory(data, candidate_kind, &candidate_id).is_err() {
                continue;
            }
            let path = entry.path().join("address.json");
            let registration = entry.path().join("registration.json");
            check_no_symlinks(&path, true)?;
            check_no_symlinks(&registration, true)?;
            let reserved = if path.try_exists()? {
                Some(read_reservation(&path, candidate_kind, &candidate_id)?.address)
            } else {
                None
            };
            // An outdated or damaged registration is reported by its own
            // listing; it must not stop other instances from getting an
            // address. Its reserved port, if any, stays in use below.
            let registered = if registration.try_exists()? {
                registered_address(&registration, candidate_kind, &candidate_id).ok()
            } else {
                None
            };
            if reserved
                .zip(registered)
                .is_some_and(|(left, right)| left != right)
            {
                return Err(invalid("应用固定地址预留与登记不符。"));
            }
            if let Some(address) = registered.or(reserved) {
                if !used.insert(address.port) {
                    return Err(invalid("多个实例声明了同一固定端口，未重新分配地址。"));
                }
            }
        }
    }
    for _ in 0..1024 {
        let candidate = platform_native::app_listener::bind_loopback(0)
            .map_err(|e| ApiError::new("APP_ADDRESS_ALLOCATION_FAILED", e.to_string()))?;
        let address = FixedAppAddress {
            port: candidate.local_addr()?.port(),
        };
        if used.contains(&address.port) {
            continue;
        }
        address.validate()?;
        crate::builds::atomic_json(
            &target,
            &AddressReservation {
                version: 1,
                runtime: kind,
                id: id.into(),
                address,
            },
        )?;
        return Ok(address);
    }
    Err(ApiError::new(
        "APP_ADDRESS_ALLOCATION_FAILED",
        "无法取得未被其他实例预留的本机端口。",
    ))
}
