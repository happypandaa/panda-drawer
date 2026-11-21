//! A complete Windows production installation uses immutable tool bytes from
//! the compiled manifest. Authenticode publisher identity is a separate proof.
use super::{compiled, protocol};
use platform_native::{filesystem as native_fs, windows::files};
use std::{
    fs::{File, OpenOptions},
    os::windows::fs::OpenOptionsExt,
    path::Path,
    sync::OnceLock,
};

const DESKTOP: &str = "PandaDrawer.exe";
const CLI: &str = "local-projects-dev.exe";
const LAYOUT_ERROR: &str = "请从完整的 Windows 正式安装包运行桌面程序或其内置 CLI。";
const RESOURCE_ERROR: &str = "Windows 正式版内置 Node/Git 资源与编译锚不一致，请使用安装包修复。";

struct Admission {
    _installation: files::PinnedDirectory,
    _desktop: File,
    _cli: File,
    _resources: protocol::VerifiedResources,
}
static ADMISSION: OnceLock<Result<Admission, &'static str>> = OnceLock::new();

pub(crate) fn admitted() -> bool {
    matches!(ADMISSION.get(), Some(Ok(_)))
}

pub(crate) fn verify_current_installation() -> Result<(), &'static str> {
    ADMISSION
        .get_or_init(verify)
        .as_ref()
        .map(|_| ())
        .map_err(|message| *message)
}

fn pin_image(path: &Path) -> Result<File, &'static str> {
    let checked = files::open_no_reparse(path).map_err(|_| LAYOUT_ERROR)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x0020_0000)
        .open(path)
        .map_err(|_| LAYOUT_ERROR)?;
    let id = files::identity(&file).map_err(|_| LAYOUT_ERROR)?;
    if id.directory
        || id.links != 1
        || !id.same_object(&checked.identity)
        || native_fs::is_reparse(&file.metadata().map_err(|_| LAYOUT_ERROR)?)
    {
        return Err(LAYOUT_ERROR);
    }
    Ok(file)
}

fn verify() -> Result<Admission, &'static str> {
    let actual = std::env::current_exe().map_err(|_| LAYOUT_ERROR)?;
    if !matches!(
        actual.file_name().and_then(|name| name.to_str()),
        Some(DESKTOP | CLI)
    ) {
        return Err(LAYOUT_ERROR);
    }
    let root = actual.parent().ok_or(LAYOUT_ERROR)?;
    let installation = files::pin_directory(root).map_err(|_| LAYOUT_ERROR)?;
    let desktop = pin_image(&root.join(DESKTOP))?;
    let cli = pin_image(&root.join(CLI))?;
    let current = if actual.file_name() == Some(std::ffi::OsStr::new(DESKTOP)) {
        &desktop
    } else {
        &cli
    };
    let current_id = files::identity(current).map_err(|_| LAYOUT_ERROR)?;
    if !current_id.same_object(
        &files::open_no_reparse(&actual)
            .map_err(|_| LAYOUT_ERROR)?
            .identity,
    ) {
        return Err(LAYOUT_ERROR);
    }
    let manifest = compiled().map_err(|_| RESOURCE_ERROR)?;
    let resources = protocol::pin_verified_resources(&root.join("resources"), &manifest)
        .map_err(|_| RESOURCE_ERROR)?;
    // Keep the verified objects read-locked until this process really exits.
    // Desktop separately checks the compiled first-party CLI SHA-256 before
    // workspace access; the CLI cannot embed its own hash without a cycle.
    Ok(Admission {
        _installation: installation,
        _desktop: desktop,
        _cli: cli,
        _resources: resources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_test_binary_cannot_borrow_a_production_tool_manifest_as_an_installation() {
        assert!(verify().is_err());
        assert!(!admitted());
    }
}
