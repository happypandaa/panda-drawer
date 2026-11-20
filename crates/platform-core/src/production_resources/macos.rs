//! Admission for the running executable and its own signed macOS bundle.

use super::{compiled, protocol};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

const DESKTOP: &str = "local-projects-desktop-dev";
const CLI: &str = "local-projects-dev";
const LAYOUT_ERROR: &str = "请从完整的正式版 .app 安装包运行桌面程序或其内置 CLI。";
static ADMISSION: OnceLock<Result<(), &'static str>> = OnceLock::new();

pub(crate) fn admitted() -> bool {
    matches!(ADMISSION.get(), Some(Ok(())))
}

pub(crate) fn verify_current_installation() -> Result<(), &'static str> {
    *ADMISSION.get_or_init(verify)
}

fn app_root(executable: &Path) -> Option<(&Path, &str)> {
    let name = executable.file_name()?.to_str()?;
    if !matches!(name, DESKTOP | CLI) {
        return None;
    }
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let app = contents.parent()?;
    if !macos.ends_with("MacOS") || !contents.ends_with("Contents") || app.extension()? != "app" {
        return None;
    }
    Some((app, name))
}

fn developer_id(team: &str, identifier: &str) -> String {
    // Both values come from the compiled policy/profile or their validated bindings.
    format!(
        r#"anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = "{team}" and identifier "{identifier}""#
    )
}

fn valid_code(path: &Path, requirement: &str, nested: bool) -> bool {
    let mut command = Command::new("/usr/bin/codesign");
    command
        .env_clear()
        .args(["--verify", "--strict"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if nested {
        command.arg("--deep");
    }
    command
        .arg(format!("-R={requirement}"))
        .arg(path)
        .status()
        .is_ok_and(|status| status.success())
}

fn hardened_code_hash(path: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/codesign")
        .env_clear()
        .args(["--display", "--verbose=4"])
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let detail = std::str::from_utf8(&output.stderr).ok()?;
    let flags = detail
        .lines()
        .find(|line| line.starts_with("CodeDirectory "))?
        .split_ascii_whitespace()
        .find_map(|word| word.strip_prefix("flags=0x"))?
        .split_once('(')?
        .0;
    if u32::from_str_radix(flags, 16).ok()? & 0x10000 == 0 {
        return None;
    }
    let hash = detail
        .lines()
        .find_map(|line| line.strip_prefix("CDHash="))?;
    (hash.len() == 40 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())).then(|| hash.to_owned())
}

fn verify() -> Result<(), &'static str> {
    let executable = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|_| LAYOUT_ERROR)?;
    let (app, name) = app_root(&executable).ok_or(LAYOUT_ERROR)?;
    let manifest = compiled().map_err(|_| "正式版程序内的工具资源清单或签名策略校验失败。")?;
    let desktop = developer_id(&manifest.expected_team_id, &manifest.profile.identifier);
    let cli = developer_id(
        &manifest.expected_team_id,
        &format!("{}.cli", manifest.profile.identifier),
    );
    // Check the running process as well as the code on disk. A process started
    // from a different binary cannot borrow an authentic app at the same path.
    let pid = std::process::id().to_string();
    let pid = Path::new(&pid);
    let hash = hardened_code_hash(pid).ok_or("当前程序的 Hardened Runtime 身份校验失败。")?;
    if !valid_code(pid, if name == CLI { &cli } else { &desktop }, false) {
        return Err("当前程序未通过正式版 Developer ID 签名校验。");
    }
    // Apple's notarized requirement uses system ticket lookup, including a
    // registered stapled ticket. Do not force an online check on startup.
    let app_requirement = format!(
        r#"{desktop} and info[CFBundleShortVersionString] = "{}" and info[CFBundleExecutable] = "{DESKTOP}" and notarized"#,
        manifest.profile.version
    );
    if !valid_code(app, &app_requirement, true) {
        return Err("正式安装包的签名、版本、内置代码或 Apple 公证票据校验失败。");
    }
    let macos = app.join("Contents/MacOS");
    let cli_path = macos.join(CLI);
    if cli_path.canonicalize().as_ref().ok() != Some(&cli_path)
        || !valid_code(&cli_path, &cli, false)
    {
        return Err("内置 CLI 未通过正式版签名校验。");
    }
    let companion = if name == CLI { app } else { &cli_path };
    if hardened_code_hash(companion).is_none() {
        return Err("正式版桌面程序或内置 CLI 未启用 Hardened Runtime。");
    }
    let running_file = if name == CLI { &cli_path } else { app };
    if hardened_code_hash(running_file).as_deref() != Some(&hash) {
        return Err("当前进程的签名与安装包内的程序不一致。");
    }
    // The notarized first-party app seals the tool manifest inside its code.
    // Its digests cover whole post-signature files, including their code
    // signatures, instead of trusting an editable manifest beside the tools.
    let resources = app.join("Contents/Resources/resources");
    protocol::verify_resources(&resources, &manifest)
        .map_err(|_| "内置工具的文件、权限或摘要与此正式版程序绑定的资源清单不符。")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_direct_app_executables_have_an_installation() {
        let root = Path::new("/Applications/熊猫小抽屉.app");
        for name in [DESKTOP, CLI] {
            let exe = root.join("Contents/MacOS").join(name);
            assert_eq!(app_root(&exe), Some((root, name)));
        }
        for path in [
            "/tmp/local-projects-dev",
            "/tmp/fake/Contents/MacOS/local-projects-dev",
            "/tmp/Test.app/Contents/Resources/MacOS/local-projects-dev",
            "/tmp/Test.app/Contents/MacOS/helper",
        ] {
            assert!(app_root(Path::new(path)).is_none());
        }
    }
}
