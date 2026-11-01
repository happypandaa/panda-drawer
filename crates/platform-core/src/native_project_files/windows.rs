//! The selected final name is an external delivery, never project material.
//! Private original write handles flush every payload byte before publication.
use crate::{
    file_identity::PersistedIdentity,
    projects::check_no_symlinks,
    web_project_ownership::{windows_delivery, WebProjectOwnership},
    ApiError, ProjectHandle,
};
use platform_native::windows::files as native;
use std::{
    fs::File,
    io::{self, Read, Write},
    path::Path,
};
const LIMIT: u64 = 256 * 1024 * 1024;
type Result<T> = std::result::Result<T, ApiError>;
fn invalid(error: impl std::fmt::Display) -> ApiError {
    ApiError::new("DOWNLOAD_FAILED", error.to_string())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Point {
    Bound,
    Copied,
    BeforePublish,
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
fn destination_identity(path: &Path) -> Result<Option<PersistedIdentity>> {
    let opened = match native::open_no_reparse(path) {
        Ok(opened) => opened,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if opened.identity.directory || !opened.file.metadata()?.is_file() {
        return Err(invalid("Save destination must be a regular file"));
    }
    PersistedIdentity::from_file(&opened.file)
        .map(Some)
        .map_err(invalid)
}
fn same(a: Option<&PersistedIdentity>, b: Option<&PersistedIdentity>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.matches_captured(b),
        _ => false,
    }
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
    let before = source.metadata()?;
    if !before.is_file() || before.len() > LIMIT {
        return Err(invalid(
            "Download exceeds the internal 256 MiB save limit or is not a regular file",
        ));
    }
    let source_identity = native::identity(&source)?;
    let parent_path = destination
        .parent()
        .ok_or_else(|| invalid("Save destination needs a parent directory"))?;
    let ownership = WebProjectOwnership::new(data.into(), workspace.into());
    let task = format!("task_{}", uuid::Uuid::new_v4().simple());
    let stage = parent_path.join(format!(".export-{task}.partial"));
    let (parent, parent_identity, expected, mut output, stage_identity) =
        crate::node_apps::source_ownership::with_source_ownership_admission(|| {
            project.validate_epoch(Some(&project.document_epoch))?;
            validate(destination)?;
            check_no_symlinks(destination, true)?;
            let parent = native::pin_directory(parent_path)?;
            let parent_identity = PersistedIdentity::from_file(&parent.file).map_err(invalid)?;
            let expected = destination_identity(destination)?;
            ownership.record_task(&project.manifest.id, &task)?;
            if let Err(error) = ownership.reserve_export_stage(&project.manifest.id, &task, &stage)
            {
                ownership.forget_native_copy_task(&project.manifest.id, &task)?;
                return Err(error);
            }
            let output = match native::create_private_file_for_publication(&stage) {
                Ok(file) => file,
                Err(error) => {
                    ownership.forget_export_stage(&project.manifest.id, &task)?;
                    ownership.forget_native_copy_task(&project.manifest.id, &task)?;
                    return Err(error.into());
                }
            };
            let stage_identity = PersistedIdentity::from_file(&output).map_err(invalid)?;
            ownership.capture_export_stage(&project.manifest.id, &task, &stage, &stage_identity)?;
            Ok((parent, parent_identity, expected, output, stage_identity))
        })?;
    checkpoint(Point::Bound, &stage)?;
    let size = io::copy(&mut (&mut source).take(LIMIT + 1), &mut output)?;
    if size > LIMIT {
        return Err(invalid("Download exceeds the internal 256 MiB save limit"));
    }
    let after = source.metadata()?;
    if size != before.len()
        || before.len() != after.len()
        || !source_identity.same_object(&native::identity(&source)?)
        || before.modified().ok() != after.modified().ok()
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
        || !same(
            expected.as_ref(),
            destination_identity(destination)?.as_ref(),
        )
    {
        return Err(invalid(
            "Save destination or temporary file changed while copying; choose it again",
        ));
    }
    if let Some(selected) = &expected {
        ownership.prepare_export_stage_overwrite(
            &project.manifest.id,
            &task,
            &stage,
            &stage_identity,
            destination,
            selected,
        )?;
    }
    drop(output);
    // All external moves are expected-ID, no-replace operations. The immutable
    // decision and original owner index distinguish delivery from an abort.
    let result = windows_delivery::publish(
        &ownership,
        &project.manifest.id,
        windows_delivery::Plan {
            task,
            stage,
            destination: destination.into(),
            parent_identity,
            payload: stage_identity,
            selected: expected,
        },
    );
    drop(parent);
    result
}

#[cfg(test)]
#[path = "windows/tests.rs"]
mod tests;
