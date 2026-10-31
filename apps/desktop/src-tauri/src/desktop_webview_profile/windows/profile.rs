use platform_core::ApiError;
use platform_native::windows::files as native;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

pub(super) fn not_initialized() -> ApiError {
    ApiError::new(
        "MANAGER_PROFILE_NOT_INITIALIZED",
        "管理器浏览器目录尚未初始化，未使用共享默认目录。",
    )
    .outcome("not_started")
}

pub(super) struct ManagerProfile {
    directory: PathBuf,
    scope: Mutex<Option<native::PinnedDirectory>>,
    identity: native::FileIdentity,
}

impl ManagerProfile {
    pub(super) fn initialize(data_root: &Path) -> Result<Self, ApiError> {
        // RuntimePaths already initialized the fixed application data root.
        // Never create/adopt a data root or inspect system-parent ownership here.
        let _root = native::pin_directory(data_root)?;
        let directory = data_root.join("webview-manager");
        match native::create_private_directory(&directory) {
            Ok(()) => (),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        let scope = native::pin_directory(&directory)?;
        native::verify_private(&scope.file)?;
        let identity = native::identity(&scope.file)?;
        Ok(Self {
            directory,
            scope: Mutex::new(Some(scope)),
            identity,
        })
    }

    pub(super) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(super) fn verify(&self) -> Result<(), ApiError> {
        let scope = self.scope.lock().unwrap_or_else(|p| p.into_inner());
        let scope = scope.as_ref().ok_or_else(|| {
            ApiError::new(
                "MANAGER_PROFILE_RETIRED",
                "管理器浏览器已经为本次完整清理关闭，未重新创建存储。",
            )
        })?;
        let actual = native::open_no_reparse(&self.directory)?;
        if !self.identity.same_object(&native::identity(&scope.file)?)
            || !self.identity.same_object(&actual.identity)
            || !actual.identity.directory
        {
            return Err(ApiError::new(
                "MANAGER_PROFILE_CHANGED",
                "管理器浏览器目录身份已变化，未继续创建窗口。",
            )
            .outcome("not_started"));
        }
        Ok(())
    }

    pub(super) fn release_after_native_close(&self) -> Result<native::FileIdentity, ApiError> {
        if self.is_released() {
            return Ok(self.identity.clone());
        }
        self.verify()?;
        let scope = self.scope.lock().unwrap_or_else(|p| p.into_inner()).take();
        drop(scope);
        Ok(self.identity.clone())
    }
    pub(super) fn is_released(&self) -> bool {
        self.scope
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_none()
    }
    pub(super) fn original_identity(&self) -> &native::FileIdentity {
        &self.identity
    }

    pub(super) fn verify_actual_directory(&self, directory: &Path) -> Result<(), ApiError> {
        self.verify()?;
        let actual = native::open_no_reparse(directory).map_err(|_| {
            ApiError::new(
                "MANAGER_PROFILE_ACTUAL_UNKNOWN",
                "无法核实 WebView2 实际使用的浏览器目录，管理页面尚未加载。",
            )
            .outcome("not_started")
        })?;
        if !actual.identity.directory || !self.identity.same_object(&actual.identity) {
            return Err(ApiError::new(
                "MANAGER_PROFILE_ACTUAL_MISMATCH",
                "WebView2 实际浏览器目录与平台私有目录不一致。请检查 WebView2 的目录覆盖设置后重新启动。管理页面尚未加载，实际目录已保留。",
            )
            .outcome("not_started"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn profile_is_private_bound_and_existing_preferences_are_preserved() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let profile = ManagerProfile::initialize(&root).unwrap();
        assert_eq!(profile.directory(), root.join("webview-manager"));
        native::verify_private(&native::open_no_reparse(profile.directory()).unwrap().file)
            .unwrap();
        fs::write(
            profile.directory().join("disposable-preferences"),
            b"keep preferences",
        )
        .unwrap();
        profile.verify().unwrap();
        let expected = profile.identity.clone();
        drop(profile);
        let reopened = ManagerProfile::initialize(&root).unwrap();
        assert!(expected.same_object(&reopened.identity));
        assert_eq!(
            fs::read(reopened.directory().join("disposable-preferences")).unwrap(),
            b"keep preferences"
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    }

    #[test]
    fn initialized_scope_prevents_profile_or_ancestor_replacement() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("disposable-data-root");
        fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let profile = ManagerProfile::initialize(&root).unwrap();
        assert!(fs::rename(profile.directory(), root.join("held-profile")).is_err());
        assert!(fs::rename(&root, temporary.path().join("held-data-root")).is_err());
        profile.verify().unwrap();
        drop(profile);
        fs::rename(root.join("webview-manager"), root.join("held-profile")).unwrap();
    }

    #[test]
    fn ordinary_file_and_junction_are_rejected_without_altering_external_data() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let name = root.join("webview-manager");
        fs::write(&name, b"caller file").unwrap();
        assert!(ManagerProfile::initialize(&root).is_err());
        assert_eq!(fs::read(&name).unwrap(), b"caller file");
        fs::remove_file(&name).unwrap();
        let external = root.join("external-original");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("sentinel"), b"preserve external original").unwrap();
        let result = std::process::Command::new(std::env::var_os("ComSpec").unwrap())
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&name)
            .arg(&external)
            .output()
            .unwrap();
        assert!(result.status.success());
        assert!(ManagerProfile::initialize(&root).is_err());
        assert_eq!(
            fs::read(external.join("sentinel")).unwrap(),
            b"preserve external original"
        );
        fs::remove_dir(name).unwrap();
    }
}
