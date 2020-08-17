// An example demonstrating loading a manifest
// For this to run, you'll need both the `manifest` and `gantry`
// features enabled, you can do that as follows from the root wascc-host directory:
// cargo run --example kvcounter_manfifest --all-features

use wascc_host::{HostManifest, WasccHost};

fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "wascc_host=info"),
    )
    .format_module_path(false)
    .try_init();
    let host = WasccHost::new();
    host.apply_manifest(HostManifest::from_path(
        "./examples/sample_manifest.yaml",
        true,
    )?)?;

    std::thread::park();
    Ok(())
}
