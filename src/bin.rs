use std::path::PathBuf;
use structopt::clap::AppSettings;
use structopt::StructOpt;
use wascc_host::{HostManifest, WasccHost};

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

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let args = Cli::from_args();
    let cmd = args.command;
    env_logger::init();
    let host = WasccHost::new();

    let manifest = HostManifest::from_yaml(cmd.manifest_path, cmd.expand_env)?;
    info!(
        "waSCC Host Manifest loaded, CWD: {:?}",
        std::env::current_dir()?
    );
    host.apply_manifest(manifest)?;

    std::thread::park();

    Ok(())
}
