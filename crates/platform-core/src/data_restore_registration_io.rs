//! Native registration namespace operations. The Mac exchange stays in its
//! original caller; Windows journals only the exact already-owned file pair.
use super::*;

#[cfg(windows)]
#[path = "data_restore_registration_io/windows.rs"]
mod windows;

pub(super) fn identity_key_allowed(key: &str) -> bool {
    #[cfg(unix)]
    {
        ["device", "inode", "volumeUuid"].contains(&key)
    }
    #[cfg(windows)]
    {
        ["device", "inode", "volumeUuid", "windowsFileId"].contains(&key)
    }
}
pub(super) fn same_volume(left: &FileIdentity, right: &FileIdentity) -> bool {
    #[cfg(unix)]
    {
        left.volume_uuid == right.volume_uuid
    }
    #[cfg(windows)]
    {
        left.device == right.device
            && left.windows_file_id.is_some()
            && right.windows_file_id.is_some()
            && left.volume_uuid.is_none()
            && right.volume_uuid.is_none()
    }
}
pub(super) fn sync_directory(path: &Path) -> Result<(), ApiError> {
    platform_native::filesystem::sync_directory(path)?;
    Ok(())
}

/// The caller has already validated its original retained recovery permit.
/// An interrupted Windows rename can temporarily leave the ordinary live name
/// absent; only that exact bound state can load its original maintenance control.
pub(super) fn recovery_control(
    store: &ProjectStore,
    id: &str,
    operation: &Path,
    operation_identity: &FileIdentity,
    plan: &DataRestoreRegistrationPlan,
) -> Result<Arc<crate::control::ProjectControl>, ApiError> {
    #[cfg(unix)]
    {
        let _ = (operation, operation_identity, plan);
        store.control(id)
    }
    #[cfg(windows)]
    {
        let original = match store.control(id) {
            Ok(control) => return Ok(control),
            Err(error) if error.code == "PROJECT_REMOVED" => error,
            Err(error) => return Err(error),
        };
        let Some(direction) = windows::pending_direction(operation)? else {
            return Err(original);
        };
        if windows::inspect_pending(&binding(
            plan,
            &store.workspace,
            id,
            operation,
            operation_identity,
            direction,
        ))? != Some(windows::State::OldHeld)
        {
            return Err(original);
        }
        let mut controls = store.controls.lock().unwrap();
        if let Some(control) = controls.get(id) {
            return Ok(control.clone());
        }
        let control = crate::control::ProjectControl::load(&store.workspace, id)?;
        controls.insert(id.into(), control.clone());
        Ok(control)
    }
}

#[cfg(windows)]
fn binding(
    plan: &DataRestoreRegistrationPlan,
    workspace: &Path,
    id: &str,
    operation: &Path,
    operation_identity: &FileIdentity,
    direction: DataRestoreDirection,
) -> windows::Binding {
    windows::Binding {
        live: registration_path(workspace, id),
        operation: operation.into(),
        operation_identity: operation_identity.clone(),
        direction,
        old: plan.before.proof.clone(),
        next: if direction == DataRestoreDirection::Continue {
            plan.apply.proof.clone()
        } else {
            plan.undo.proof.clone()
        },
    }
}
#[cfg(windows)]
pub(super) fn pending_state(
    plan: &DataRestoreRegistrationPlan,
    workspace: &Path,
    id: &str,
    operation: &Path,
) -> Result<Option<DataRestoreEpochState>, ApiError> {
    let Some(direction) = windows::pending_direction(operation)? else {
        return Ok(None);
    };
    let operation_identity = FileIdentity::capture(operation).map_err(|_| invalid())?;
    let binding = binding(
        plan,
        workspace,
        id,
        operation,
        &operation_identity,
        direction,
    );
    let state = windows::inspect_pending(&binding)?.ok_or_else(changed)?;
    let (other, image) = if direction == DataRestoreDirection::Continue {
        (UNDO_SLOT, &plan.undo)
    } else {
        (APPLY_SLOT, &plan.apply)
    };
    image.proof.verify(&operation.join(other), MAX_RECORD)?;
    Ok(Some(if !state.committed() {
        DataRestoreEpochState::Before
    } else if direction == DataRestoreDirection::Continue {
        DataRestoreEpochState::Applied
    } else {
        DataRestoreEpochState::Undone
    }))
}
#[cfg(windows)]
pub(super) fn stabilize(access: &DataRestoreRecoveryAccess) -> Result<(), ApiError> {
    let Some(direction) = windows::pending_direction(&access.operation)? else {
        return Ok(());
    };
    windows::stabilize_pending(&binding(
        &access.plan,
        &access.workspace,
        &access.id,
        &access.operation,
        &access.operation_identity,
        direction,
    ))?;
    Ok(())
}
#[cfg(windows)]
pub(super) fn exchange(
    access: &DataRestoreRecoveryAccess,
    direction: DataRestoreDirection,
) -> Result<(), ApiError> {
    let _capacity = crate::data_restore_guard::reserve_registration_exchange(
        &access.workspace,
        &access.id,
        &access.operation,
        windows::MAX_EXCHANGE_JOURNAL
            .checked_add(access.plan.before.proof.bytes)
            .ok_or_else(invalid)?,
        2,
    )?;
    windows::exchange(&binding(
        &access.plan,
        &access.workspace,
        &access.id,
        &access.operation,
        &access.operation_identity,
        direction,
    ))
}
#[derive(Default)]
pub(crate) struct ExchangeMetadata {
    pub(crate) entries: Vec<(String, u64)>,
}
pub(super) fn exchange_metadata(
    plan: &DataRestoreRegistrationPlan,
    workspace: &Path,
    id: &str,
    operation: &Path,
    operation_identity: &FileIdentity,
) -> Result<ExchangeMetadata, ApiError> {
    #[cfg(unix)]
    {
        let _ = (plan, workspace, id, operation, operation_identity);
        Ok(ExchangeMetadata::default())
    }
    #[cfg(windows)]
    {
        let entries = windows::metadata(&|direction| {
            binding(
                plan,
                workspace,
                id,
                operation,
                operation_identity,
                direction,
            )
        })?;
        Ok(ExchangeMetadata { entries })
    }
}
