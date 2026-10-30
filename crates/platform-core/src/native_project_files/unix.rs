//! Native delivery uses an identity-bound, project-owned external copy stage.
//! The user-selected final file remains an external delivery, not project data.
use crate::{
    file_identity::PersistedIdentity, projects::check_no_symlinks,
    web_project_ownership::WebProjectOwnership, ApiError, ProjectHandle,
};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::MetadataExt,
    },
    path::Path,
};

const LIMIT: u64 = 256 * 1024 * 1024;
type Result<T> = std::result::Result<T, ApiError>;
fn invalid(message: impl std::fmt::Display) -> ApiError {
    ApiError::new("DOWNLOAD_FAILED", message.to_string())
}

#[derive(Clone, Copy)]
enum Point {
    Bound,
    Copied,
    BeforePublish,
    BeforeRename,
    Renamed,
    Published,
}
pub(crate) fn publish_file(
    data: &Path,
    workspace: &Path,
    project: &ProjectHandle,
    source: File,
    destination: &Path,
    validate: impl Fn(&Path) -> Result<()>,
) -> Result<()> {
    publish(
        data,
        workspace,
        project,
        source,
        destination,
        validate,
        |_, _| Ok(()),
    )
}

fn publish(
    data: &Path,
    workspace: &Path,
    project: &ProjectHandle,
    mut source: File,
    destination: &Path,
    validate: impl Fn(&Path) -> Result<()>,
    mut checkpoint: impl FnMut(Point, &Path) -> Result<()>,
) -> Result<()> {
    let source_before = source.metadata()?;
    if !source_before.is_file() || source_before.len() > LIMIT {
        return Err(invalid(
            "Download exceeds the internal 256 MiB save limit or is not a regular file",
        ));
    }
    let parent_path = destination
        .parent()
        .ok_or_else(|| invalid("Save destination needs a parent directory"))?;
    let ownership = WebProjectOwnership::new(data.into(), workspace.into());
    let task = format!("task_{}", uuid::Uuid::new_v4().simple());
    let stage = parent_path.join(format!(".export-{task}.partial"));
    let (parent, parent_identity, expected, mut output, stage_identity) =
        crate::node_apps::development::with_source_ownership_admission(|| {
            project.validate_epoch(Some(&project.document_epoch))?;
            validate(destination)?;
            check_no_symlinks(destination, true)?;
            let parent = crate::files::root(parent_path)?;
            let parent_identity = PersistedIdentity::from_file(&parent).map_err(invalid)?;
            let expected = destination_identity(destination)?;
            ownership.record_task(&project.manifest.id, &task)?;
            if let Err(error) = ownership.reserve_export_stage(&project.manifest.id, &task, &stage)
            {
                ownership.forget_native_copy_task(&project.manifest.id, &task)?;
                return Err(error);
            }
            let stage_name = c_name(&stage)?;
            let descriptor = unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    stage_name.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            if descriptor < 0 {
                let error = io::Error::last_os_error();
                ownership.forget_export_stage(&project.manifest.id, &task)?;
                ownership.forget_native_copy_task(&project.manifest.id, &task)?;
                return Err(error.into());
            }
            let output = unsafe { File::from_raw_fd(descriptor) };
            let stage_identity = PersistedIdentity::from_file(&output).map_err(invalid)?;
            // Persist this exact inode before the first payload byte. A failed worker
            // never unlinks an unverified public pathname or forgets its bound stage.
            ownership.capture_export_stage(&project.manifest.id, &task, &stage, &stage_identity)?;
            parent.sync_all()?;
            Ok((parent, parent_identity, expected, output, stage_identity))
        })?;
    checkpoint(Point::Bound, &stage)?;
    let size = io::copy(&mut (&mut source).take(LIMIT + 1), &mut output)?;
    if size > LIMIT {
        return Err(invalid("Download exceeds the internal 256 MiB save limit"));
    }
    let after = source.metadata()?;
    if size != source_before.len()
        || source_before.len() != after.len()
        || source_before.dev() != after.dev()
        || source_before.ino() != after.ino()
        || source_before.modified().ok() != after.modified().ok()
    {
        return Err(invalid("Temporary download changed while copying"));
    }
    output.flush()?;
    output.sync_all()?;
    checkpoint(Point::Copied, &stage)?;
    checkpoint(Point::BeforePublish, &stage)?;
    project.validate_epoch(Some(&project.document_epoch))?;
    validate(destination)?;
    check_no_symlinks(destination, true)?;
    if !parent_identity.matches_path(parent_path).map_err(invalid)?
        || !stage_identity.matches_path(&stage).map_err(invalid)?
        || !same_destination(
            expected.as_ref(),
            destination_identity(destination)?.as_ref(),
        )
    {
        return Err(invalid(
            "Save destination or temporary file changed while copying; choose it again",
        ));
    }
    if let Some(selected) = &expected {
        // Only the old object explicitly selected for replacement may become
        // our stage after an atomic exchange. Never adopt a post-swap object.
        ownership.prepare_export_stage_overwrite(
            &project.manifest.id,
            &task,
            &stage,
            &stage_identity,
            destination,
            selected,
        )?;
    }
    checkpoint(Point::BeforeRename, &stage)?;
    publish_at(
        &parent,
        &stage,
        destination,
        if expected.is_some() {
            Rename::Swap
        } else {
            Rename::Exclusive
        },
    )?;
    parent.sync_all()?;
    checkpoint(Point::Renamed, &stage)?;
    let postcheck = (|| {
        if !parent_identity.matches_path(parent_path).map_err(invalid)?
            || !identity_at(&parent, destination)?
                .is_some_and(|observed| observed.matches(&stage_identity))
            || match &expected {
                Some(selected) => !identity_at(&parent, &stage)?
                    .is_some_and(|observed| observed.matches(selected)),
                None => identity_at(&parent, &stage)?.is_some(),
            }
        {
            return Err(invalid("Save publication identity changed during rename"));
        }
        Ok(())
    })();
    if let Err(error) = postcheck {
        // Exchange preserves both objects, including a foreign replacement in
        // the last check/rename gap. Restoration also uses only non-destructive
        // renames; a second race keeps both names for explicit rechecking.
        let restored = restore_publication(&parent, &stage, destination, expected.is_some());
        return Err(error
            .detail("stagePath", stage.to_string_lossy().into_owned())
            .detail(
                "destinationPath",
                destination.to_string_lossy().into_owned(),
            )
            .detail("restorationConfirmed", restored.is_ok()));
    }
    if let Some(selected) = &expected {
        ownership.verify_export_stage_overwrite(
            &project.manifest.id,
            &task,
            &stage,
            &stage_identity,
            destination,
            selected,
        )?;
    }
    checkpoint(Point::Published, &stage)?;
    if let Some(selected) = &expected {
        // After a verified exchange the private stage contains only the old
        // user-approved object. Reuse the crash-safe, identity-bound remover.
        crate::web_file_deletion::remove_owned_file(
            &ownership,
            &project.manifest.id,
            &stage,
            selected,
        )?;
    }
    ownership.forget_export_stage(&project.manifest.id, &task)?;
    ownership.forget_native_copy_task(&project.manifest.id, &task)
}
fn same_destination(
    expected: Option<&PersistedIdentity>,
    actual: Option<&PersistedIdentity>,
) -> bool {
    match (expected, actual) {
        (None, None) => true,
        (Some(expected), Some(actual)) => expected.matches_captured(actual),
        _ => false,
    }
}
fn destination_identity(path: &Path) -> Result<Option<PersistedIdentity>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            PersistedIdentity::capture(path).map(Some).map_err(invalid)
        }
        Ok(_) => Err(invalid("Save destination must be a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn c_name(path: &Path) -> Result<CString> {
    CString::new(
        path.file_name()
            .ok_or_else(|| invalid("Save destination needs a file name"))?
            .as_encoded_bytes(),
    )
    .map_err(invalid)
}
#[derive(Clone, Copy)]
enum Rename {
    Swap,
    Exclusive,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct EntryIdentity {
    device: u64,
    inode: u64,
    kind: libc::mode_t,
}
impl EntryIdentity {
    fn matches(self, expected: &PersistedIdentity) -> bool {
        self.kind == libc::S_IFREG && expected.legacy_pair() == (self.device, self.inode)
    }
}
fn identity_at(parent: &File, path: &Path) -> Result<Option<EntryIdentity>> {
    let name = c_name(path)?;
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::NotFound {
            Ok(None)
        } else {
            Err(error.into())
        };
    }
    let metadata = unsafe { metadata.assume_init() };
    Ok(Some(EntryIdentity {
        device: metadata.st_dev as u64,
        inode: metadata.st_ino,
        kind: metadata.st_mode & libc::S_IFMT,
    }))
}
fn restore_publication(
    parent: &File,
    stage: &Path,
    destination: &Path,
    swapped: bool,
) -> Result<()> {
    let final_before = identity_at(parent, destination)?.ok_or_else(|| {
        invalid("Published name disappeared; retained copy stage needs rechecking")
    })?;
    let stage_before = identity_at(parent, stage)?;
    if swapped {
        if stage_before.is_none() {
            return Err(invalid(
                "Exchanged stage disappeared; retained names need rechecking",
            ));
        }
        publish_at(parent, stage, destination, Rename::Swap)?;
    } else {
        if stage_before.is_some() {
            return Err(invalid(
                "Stage name is occupied; foreign files were preserved",
            ));
        }
        publish_at(parent, destination, stage, Rename::Exclusive)?;
    }
    parent.sync_all()?;
    if identity_at(parent, stage)? != Some(final_before)
        || identity_at(parent, destination)? != stage_before
    {
        return Err(invalid(
            "Publication restoration changed; both names were preserved",
        ));
    }
    Ok(())
}
fn publish_at(parent: &File, stage: &Path, destination: &Path, rename: Rename) -> Result<()> {
    let from = c_name(stage)?;
    let to = c_name(destination)?;
    let result = {
        #[cfg(target_os = "macos")]
        {
            unsafe {
                libc::renameatx_np(
                    parent.as_raw_fd(),
                    from.as_ptr(),
                    parent.as_raw_fd(),
                    to.as_ptr(),
                    match rename {
                        Rename::Swap => libc::RENAME_SWAP,
                        Rename::Exclusive => libc::RENAME_EXCL,
                    },
                )
            }
        }
        #[cfg(target_os = "linux")]
        {
            unsafe {
                libc::renameat2(
                    parent.as_raw_fd(),
                    from.as_ptr(),
                    parent.as_raw_fd(),
                    to.as_ptr(),
                    match rename {
                        Rename::Swap => libc::RENAME_EXCHANGE,
                        Rename::Exclusive => libc::RENAME_NOREPLACE,
                    },
                )
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            -1
        }
    };
    if result != 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(test)]
#[path = "../native_project_files_tests.rs"]
mod tests;
