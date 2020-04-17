// An example demonstrating loading a manifest
// For this to run, you'll need both the `manifest` and `gantry`
// features enabled, you can do that as follows from the root wascc-host directory:
// cargo run --example kvcounter_manfifest --all-features

use wascc_host::{HostManifest, WasccHost};

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let host = WasccHost::new();
    host.apply_manifest(HostManifest::from_yaml(
        "./examples/sample_manifest.yaml",
        false,
    )?)?;

    std::thread::park();
    Ok(())
}
