//! In-process declaration of the desktop's selected installation members.
//! This is neither a configurable path selector nor a release-signature gate.
use crate::ApiError;
use serde_json::Value;
use std::{path::Path, sync::Arc};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::Stamp;

pub(crate) struct InstallationScope {
    #[cfg(windows)]
    native: windows::Scope,
}

impl InstallationScope {
    pub(crate) fn for_desktop(resources: &Path, cli: &Path) -> Option<Arc<Self>> {
        #[cfg(windows)]
        {
            std::env::current_exe().ok().map(|manager| {
                Arc::new(Self {
                    native: windows::Scope::capture(&manager, cli, resources),
                })
            })
        }
        #[cfg(not(windows))]
        {
            let _ = (resources, cli);
            None
        }
    }

    pub(crate) fn require_destination(&self, destination: &Path) -> Result<(), ApiError> {
        #[cfg(windows)]
        {
            self.native.require_destination(destination)
        }
        #[cfg(not(windows))]
        {
            let _ = destination;
            Ok(())
        }
    }

    pub(crate) fn add_platform_info(&self, info: &mut Value) {
        #[cfg(windows)]
        self.native.add_platform_info(info);
        #[cfg(not(windows))]
        let _ = info;
    }

    #[cfg(windows)]
    pub(crate) fn members(&self) -> &[windows::Member; 3] {
        &self.native.members
    }

    #[cfg(all(test, windows))]
    pub(crate) fn for_test(manager: &Path, cli: &Path, resources: &Path) -> Arc<Self> {
        Arc::new(Self {
            native: windows::Scope::capture(manager, cli, resources),
        })
    }
}

pub(crate) fn require_destination(
    scope: Option<&InstallationScope>,
    destination: &Path,
) -> Result<(), ApiError> {
    match scope {
        Some(scope) => scope.require_destination(destination),
        None => Ok(()),
    }
}
