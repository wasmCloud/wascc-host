// An example demonstrating loading a manifest
// For this to run, you'll need the `manifest` feature enabled. You can do that as follows from the
// root wascc-host directory:
// cargo run --example kvcounter_manfifest --features "manifest"

use wascc_host::{Host, HostManifest};

fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "wascc_host=info"),
    )
    .format_module_path(false)
    .try_init();
    let host = Host::new();
    host.apply_manifest(HostManifest::from_path(
        "./examples/sample_manifest.yaml",
        true,
    )?)?;

    println!("**> curl localhost:8081/counter1 to test");

    std::thread::park();
    Ok(())
}
