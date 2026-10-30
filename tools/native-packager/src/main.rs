//! Developer-only preparation of fixed, checksum-verified Windows tool resources.
//! This program never discovers an application workspace or installation tree.
#[cfg(all(windows, target_arch = "x86_64"))]
mod windows;

fn main() {
    #[cfg(all(windows, target_arch = "x86_64"))]
    {
        let arguments: Vec<_> = std::env::args_os().skip(1).collect();
        let git = arguments
            .first()
            .is_some_and(|arg| arg.to_string_lossy().starts_with("git-"));
        match windows::run(arguments) {
            Ok(value) => println!("{value}"),
            Err(error) => {
                eprintln!(
                    "{}",
                    serde_json::json!({"code":if git {"WINDOWS_GIT_PACK_FAILED"} else {"WINDOWS_NODE_PACK_FAILED"},"message":error.to_string(),"operationOutcome":if error.to_string().contains("outcome is unknown") {"unknown"} else {"not_started"}})
                );
                std::process::exit(1);
            }
        }
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        eprintln!("Windows x64 tool packaging is unavailable on this host");
        std::process::exit(1);
    }
}
