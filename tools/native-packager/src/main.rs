//! Developer-only preparation of fixed, checksum-verified Windows Node resources.
//! This program never discovers an application workspace or installation tree.
#[cfg(all(windows, target_arch = "x86_64"))]
mod windows;

fn main() {
    #[cfg(all(windows, target_arch = "x86_64"))]
    match windows::run(std::env::args_os().skip(1).collect()) {
        Ok(value) => println!("{value}"),
        Err(error) => {
            eprintln!(
                "{}",
                serde_json::json!({"code":"WINDOWS_NODE_PACK_FAILED","message":error.to_string(),"operationOutcome":if error.to_string().contains("outcome is unknown") {"unknown"} else {"not_started"}})
            );
            std::process::exit(1);
        }
    }
    #[cfg(not(all(windows, target_arch = "x86_64")))]
    {
        eprintln!("Windows x64 Node packaging is unavailable on this host");
        std::process::exit(1);
    }
}
