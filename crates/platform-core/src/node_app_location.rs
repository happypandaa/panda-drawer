//! The directory action resolves an existing instance, never an author source
//! supplied by the frontend. The instance mutex stays held through OS dispatch.
use super::*;
use crate::file_identity::PersistedIdentity;
use tokio::sync::MutexGuard;

pub struct NodeAppLocationLease<'a> {
    apps: &'a NodeApps,
    id: String,
    location: PathBuf,
    identity: PersistedIdentity,
    registration_digest: String,
    inner: MutexGuard<'a, Inner>,
}
impl NodeAppLocationLease<'_> {
    pub fn location(&self) -> &Path {
        &self.location
    }
    pub fn validate(&self) -> Result<()> {
        if self.inner.shutting_down {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "平台正在退出，请稍后重试",
            ));
        }
        self.apps.require_no_pending_delete(&self.id)?;
        ordinary_path(&self.location, true)?;
        let record = self.apps.registration(&self.id)?;
        let digest = format!(
            "{:x}",
            Sha256::digest(read_small(&self.location.join("registration.json"))?)
        );
        if self.apps.root.join(record.id) != self.location
            || digest != self.registration_digest
            || !self.identity.matches_path(&self.location).unwrap_or(false)
        {
            return Err(ApiError::new(
                "NODE_APP_LOCATION_CHANGED",
                "应用目录已变化，请刷新后重试",
            ));
        }
        Ok(())
    }
}
impl NodeApps {
    pub async fn local_directory_lease(&self, id: &str) -> Result<NodeAppLocationLease<'_>> {
        let inner = self.inner.lock().await;
        if inner.shutting_down {
            return Err(ApiError::new(
                "SERVICE_STOPPING",
                "平台正在退出，请稍后重试",
            ));
        }
        self.require_no_pending_delete(id)?;
        let record = self.registration(id)?;
        let location = self.root.join(record.id);
        ordinary_path(&location, true)?;
        let registration_digest = format!(
            "{:x}",
            Sha256::digest(read_small(&location.join("registration.json"))?)
        );
        let identity = PersistedIdentity::capture(&location).map_err(|_| {
            ApiError::new(
                "NODE_APP_LOCATION_CHANGED",
                "无法核对应用目录，请刷新后重试",
            )
        })?;
        let lease = NodeAppLocationLease {
            apps: self,
            id: id.into(),
            location,
            identity,
            registration_digest,
            inner,
        };
        lease.validate()?;
        Ok(lease)
    }
}

#[cfg(test)]
#[path = "node_app_location_tests.rs"]
mod tests;
