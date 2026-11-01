//! One-shot disposable proof: strict declaration -> private Node -> health/ping -> stop.
//! No platform RuntimePaths, project registration, desktop service, or account input.
use platform_core::{
    node_runtime_trial::{NodeRuntimeTrial, RuntimeLaunchConfig, EXPECTED_NODE_VERSION},
    panda_manifest::{PandaManifest, PandaRuntime},
};
use serde::Deserialize;
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Deserialize)]
struct Release {
    version: String,
    platform: String,
    arch: String,
    binary: Binary,
}
#[derive(Deserialize)]
struct Binary {
    sha256: String,
}

fn arguments() -> Result<BTreeMap<String, PathBuf>, &'static str> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 6 {
        return Err("Usage: node_runtime_trial --node ABSOLUTE_NODE --app ABSOLUTE_APP --trial-root ABSOLUTE_EMPTY_NODE_RUNTIME_TRIAL_DIRECTORY");
    }
    let mut paths = BTreeMap::new();
    for pair in args.as_chunks::<2>().0 {
        if !["--node", "--app", "--trial-root"].contains(&pair[0].as_str())
            || !PathBuf::from(&pair[1]).is_absolute()
            || paths
                .insert(pair[0].clone(), PathBuf::from(&pair[1]))
                .is_some()
        {
            return Err("Exactly --node, --app and --trial-root require explicit absolute paths");
        }
    }
    Ok(paths)
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let paths = arguments()?;
    let release: Release =
        serde_json::from_str(include_str!("../../../scripts/node-runtime-release.json"))?;
    if release.version != EXPECTED_NODE_VERSION
        || release.platform != "darwin"
        || release.arch != "arm64"
        || !cfg!(all(target_os = "macos", target_arch = "aarch64"))
    {
        return Err(
            "Pinned trial release does not match this supported macOS arm64 experiment".into(),
        );
    }
    let app = &paths["--app"];
    let manifest = PandaManifest::load(app)?;
    if manifest.runtime != PandaRuntime::Node {
        return Err("A webpage declaration does not start a Node service".into());
    }
    let business_env = manifest.business_defaults()?;
    let (entry_path, working_directory) = manifest.resolve_backend(app)?;
    let mut runtime = NodeRuntimeTrial::launch(RuntimeLaunchConfig {
        node_path: paths["--node"].clone(),
        expected_node_sha256: release.binary.sha256,
        entry_path,
        working_directory,
        trial_root: paths["--trial-root"].clone(),
        business_env,
        control_directory: paths["--trial-root"].clone(),
        redaction_values: Vec::new(),
    })
    .await?;
    let probe = async {
        runtime.ping().await?;
        runtime.verify_health().await?;
        println!(
            "{}",
            json!({"stage":"verified","runtime":"node","status":runtime.status()?})
        );
        Ok::<(), platform_core::ApiError>(())
    }
    .await;
    let stopped = runtime.stop().await?;
    println!("{}", json!({"stage":"stopped","status":stopped}));
    probe?;
    if stopped.state != "stopped" || !stopped.stopping_observed {
        return Err("Trial did not complete the normal stopping and exit protocol".into());
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("Disposable Node trial failed: {error}");
        std::process::exit(1);
    }
}
