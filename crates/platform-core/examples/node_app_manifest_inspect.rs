//! Read-only experimental declaration preview; no workspace, runtime or credentials.
use platform_core::node_app_manifest::TrialManifest;
use std::{path::PathBuf, process::ExitCode};

fn app_argument() -> Result<PathBuf, &'static str> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--app")) {
        return Err("用法：node_app_manifest_inspect --app 应用目录的绝对路径");
    }
    let path = args
        .next()
        .map(PathBuf::from)
        .ok_or("需要明确指定应用目录的绝对路径。")?;
    if args.next().is_some() || !path.is_absolute() {
        return Err("本实验检查器只接受 --app 和一个绝对目录，不接受其他参数。");
    }
    Ok(path)
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let app_dir = app_argument()?;
    let manifest = TrialManifest::load(&app_dir)?;
    let inspection = manifest.inspect(&app_dir)?;
    println!("{}", serde_json::to_string_pretty(&inspection)?);
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("实验清单只读检查未通过：{error}");
            ExitCode::FAILURE
        }
    }
}
