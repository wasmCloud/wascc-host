use std::path::PathBuf;
use std::time::Duration;
use structopt::clap::AppSettings;
use structopt::StructOpt;
use wascc_host::{Host, HostBuilder, HostManifest};

#[macro_use]
extern crate log;

#[derive(Debug, StructOpt, Clone)]
#[structopt(
global_settings(& [AppSettings::ColoredHelp, AppSettings::VersionlessSubcommands]),
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
    manifest_path: Option<PathBuf>,
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

    let host = HostBuilder::new().build();

    if let Some(ref mp) = cmd.manifest_path {
        let manifest = HostManifest::from_path(mp, cmd.expand_env)?;
        host.apply_manifest(manifest)?;
        info!("Processed and applied host manifest");
    } else {
        info!("Starting without manifest");
        #[cfg(not(feature = "lattice"))]
        {
            error!("Started without manifest and without lattice. This host cannot launch actors or providers. Shutting down.");
            return Err("Started without manifest or lattice - unusable host".into());
        }
    }

    let (term_s, term_r) = std::sync::mpsc::channel();

    ctrlc::set_handler(move || {
        term_s.send(()).unwrap();
    })
    .expect("Error setting Ctrl-C handler");

    term_r.recv().expect("Failed awaiting termination signal");

    info!("Shutting down host");
    host.shutdown()?;

    Ok(())
}

#[cfg(not(feature = "manifest"))]
fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!(
        "The general-purpose waSCC host application needs to be built with the `manifest` feature."
    );
    Ok(())
}
