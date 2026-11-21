use super::super::{InstalledResources, ResourceInfo};
use std::path::{Path, PathBuf};
use tauri::Manager;

#[cfg(feature = "production-profile")]
pub(crate) struct ProductionCliGuard {
    _directory: platform_native::windows::files::PinnedDirectory,
    _file: std::fs::File,
}

#[cfg(any(test, feature = "production-profile"))]
fn checked_cli_file(
    path: &Path,
    expected: &str,
) -> Result<std::fs::File, Box<dyn std::error::Error>> {
    use platform_native::windows::files;
    use sha2::{Digest, Sha256};
    use std::{fs::OpenOptions, io::Read, os::windows::fs::OpenOptionsExt};
    use windows::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

    if expected.len() != 64
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("正式 Windows 程序未编译内置 CLI 完整性锚。".into());
    }
    let original = files::open_no_reparse(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let identity = files::identity(&file)?;
    if identity.directory || identity.links != 1 || !identity.same_object(&original.identity) {
        return Err("正式 Windows 内置 CLI 的文件身份发生变化。".into());
    }
    let mut digest = Sha256::new();
    let mut count = 0u64;
    let mut bytes = [0u8; 65536];
    loop {
        let read = file.read(&mut bytes)?;
        if read == 0 {
            break;
        }
        count += read as u64;
        if count > 512 * 1024 * 1024 {
            return Err("正式 Windows 内置 CLI 大小超出限制。".into());
        }
        digest.update(&bytes[..read]);
    }
    if count == 0 || format!("{:x}", digest.finalize()) != expected {
        return Err("正式 Windows 内置 CLI 完整性校验失败。".into());
    }
    Ok(file)
}

#[cfg(feature = "production-profile")]
pub(crate) fn verify_production_cli() -> Result<ProductionCliGuard, Box<dyn std::error::Error>> {
    let executable = std::env::current_exe()?;
    if executable.file_name() != Some(std::ffi::OsStr::new("PandaDrawer.exe")) {
        return Err("请从正式 PandaDrawer 安装目录启动程序。".into());
    }
    let directory = executable.parent().ok_or("正式程序安装目录不存在。")?;
    let pin = platform_native::windows::files::pin_directory(directory)?;
    let file = checked_cli_file(
        &directory.join("local-projects-dev.exe"),
        env!("PANDA_DRAWER_COMPILED_CLI_SHA256"),
    )?;
    Ok(ProductionCliGuard {
        _directory: pin,
        _file: file,
    })
}

fn locations(
    resource_directory: &Path,
    executable: &Path,
) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    Ok((
        resource_directory.join("resources"),
        executable
            .parent()
            .ok_or("Missing executable directory")?
            .join("local-projects-dev.exe"),
    ))
}

fn inspect(
    resource_directory: &Path,
    executable: &Path,
) -> Result<InstalledResources, Box<dyn std::error::Error>> {
    let (resources, cli) = locations(resource_directory, executable)?;
    let cli_path = cli.is_file().then(|| cli.to_string_lossy().into_owned());
    Ok(InstalledResources {
        info: ResourceInfo {
            sdk: resources.join("sdk/sdk.js").is_file()
                && resources.join("sdk/panda.d.ts").is_file(),
            template: resources.join("template/app/package.json").is_file(),
            readme: std::fs::read_to_string(resources.join("README.md")).is_ok(),
            cli: cli_path.is_some(),
        },
        cli_path,
        root: resources,
        cli_executable: cli,
    })
}

pub(super) fn resource_info(
    app: &tauri::App,
) -> Result<InstalledResources, Box<dyn std::error::Error>> {
    inspect(&app.path().resource_dir()?, &std::env::current_exe()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn production_cli_digest_rejects_substitution_and_keeps_the_verified_file_locked() {
        use sha2::{Digest, Sha256};
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("local-projects-dev.exe");
        fs::write(&path, b"disposable verified CLI fixture").unwrap();
        let expected = format!("{:x}", Sha256::digest(b"disposable verified CLI fixture"));
        assert!(checked_cli_file(&path, &"0".repeat(64)).is_err());
        let held = checked_cli_file(&path, &expected).unwrap();
        assert!(fs::write(&path, b"replacement").is_err());
        assert!(fs::remove_file(&path).is_err());
        drop(held);
        fs::write(&path, b"replacement").unwrap();
        assert!(checked_cli_file(&path, &expected).is_err());
    }

    #[test]
    fn windows_paths_select_only_tauri_resources_and_the_copied_sidecar_name() {
        let (resources, cli) = locations(
            Path::new(r"D:\installed 中文\resource-root"),
            Path::new(r"D:\installed 中文\bin\panda-drawer.exe"),
        )
        .unwrap();
        assert_eq!(
            resources,
            Path::new(r"D:\installed 中文\resource-root\resources")
        );
        assert_eq!(
            cli,
            Path::new(r"D:\installed 中文\bin\local-projects-dev.exe")
        );
        assert_eq!(
            locations(Path::new(r"D:\resources"), Path::new(r"D:\"))
                .unwrap_err()
                .to_string(),
            "Missing executable directory"
        );
    }

    #[test]
    fn missing_installed_resources_and_sidecar_never_select_source_or_mac_names() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let resource_directory = root.join("installed-resource-root");
        let executable = root.join("bin").join("panda-drawer.exe");
        fs::create_dir(executable.parent().unwrap()).unwrap();
        fs::write(
            executable.parent().unwrap().join("local-projects-dev"),
            b"old Mac name",
        )
        .unwrap();
        fs::write(
            executable
                .parent()
                .unwrap()
                .join("local-projects-dev-x86_64-pc-windows-msvc.exe"),
            b"source sidecar name",
        )
        .unwrap();
        let source = root.join("source-tree").join("src-tauri").join("resources");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("README.md"), b"preserve source resources").unwrap();
        let report = inspect(&resource_directory, &executable).unwrap();
        assert_eq!(report.root, resource_directory.join("resources"));
        assert_eq!(
            report.cli_executable,
            executable.parent().unwrap().join("local-projects-dev.exe")
        );
        assert!(report.cli_path.is_none());
        assert!(
            !report.info.sdk && !report.info.template && !report.info.readme && !report.info.cli
        );
        assert_eq!(
            fs::read(source.join("README.md")).unwrap(),
            b"preserve source resources"
        );
        assert!(!resource_directory.exists());
    }

    #[test]
    fn actual_installed_resource_flags_and_cli_follow_only_the_selected_layout() {
        let temporary = tempfile::tempdir().unwrap();
        let resource_directory = temporary.path().canonicalize().unwrap();
        let resources = resource_directory.join("resources");
        fs::create_dir_all(resources.join("sdk")).unwrap();
        fs::write(
            resources.join("sdk/panda.d.ts"),
            "declare const panda: unknown;",
        )
        .unwrap();
        fs::create_dir_all(resources.join("template/app")).unwrap();
        fs::write(resources.join("sdk/sdk.js"), b"disposable SDK marker").unwrap();
        fs::write(resources.join("template/app/package.json"), b"{}").unwrap();
        fs::write(resources.join("README.md"), b"disposable resource readme").unwrap();
        let executable = resource_directory.join("panda-drawer.exe");
        let cli = resource_directory.join("local-projects-dev.exe");
        fs::write(&cli, b"disposable sidecar marker; never executed").unwrap();
        let report = inspect(&resource_directory, &executable).unwrap();
        assert!(report.info.sdk && report.info.template && report.info.readme && report.info.cli);
        assert_eq!(report.cli_path.as_deref(), Some(cli.to_str().unwrap()));
        assert_eq!(report.cli_executable, cli);
        fs::write(resources.join("README.md"), [0xff]).unwrap();
        assert!(
            !inspect(&resource_directory, &executable)
                .unwrap()
                .info
                .readme
        );
        assert_eq!(
            fs::read(&report.cli_executable).unwrap(),
            b"disposable sidecar marker; never executed"
        );
    }
}
