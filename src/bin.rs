use std::path::PathBuf;
use structopt::clap::AppSettings;
use structopt::StructOpt;
use wascc_host::{Host, HostManifest};

#[macro_use]
extern crate log;

#[derive(Debug, StructOpt, Clone)]
#[structopt(
    global_settings(&[AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
    name = "wascc-host",
    about = "A general-purpose waSCC runtime host")]
struct Cli {
    #[structopt(flatten)]
    command: CliCommand,
}

#[derive(Debug, Clone, StructOpt)]
struct CliCommand {
    /// Path to the host manifest
    #[structopt(short = "m", long = "manifest", parse(from_os_str))]
    manifest_path: PathBuf,
    /// Whether to expand environment variables in the host manifest
    #[structopt(short = "e", long = "expand-env")]
    expand_env: bool,
}

#[cfg(feature = "manifest")]
fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Cli::from_args();
    let cmd = args.command;
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "wascc_host=info"),
    )
    .format_module_path(false)
    .try_init();

    let host = Host::new();

    let manifest = HostManifest::from_path(cmd.manifest_path, cmd.expand_env)?;
    host.apply_manifest(manifest)?;
    info!("Processed and applied host manifest");

    std::thread::park();

    Ok(())
}

#[cfg(not(feature = "manifest"))]
fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!(
        "The general-purpose waSCC host application needs to be built with the `manifest` feature."
    );
    Ok(())
}
