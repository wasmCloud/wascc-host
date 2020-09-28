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

    let mut builder = HostBuilder::new();
    #[cfg(feature = "lattice")]
    let builder = builder.with_gantryclient(get_gantry_client());

    let host = builder.build();

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

#[cfg(feature = "lattice")]
fn get_gantry_client() -> gantryclient::Client {
    let host = get_env(LATTICE_HOST_KEY, DEFAULT_LATTICE_HOST);
    let creds = get_credsfile();
    let timeout = get_timeout();

    match gantryclient::Client::try_new(&host, creds.map(|c| c.into()), timeout) {
        Ok(c) => {
            info!("Gantry client created.");
            c
        }
        Err(e) => {
            error!("Failed to create Gantry client. Check if NATS is running and client is configured properly.");
            error!("Terminating host (no lattice connectivity)");
            std::process::exit(1);
        }
    }
}

fn get_credsfile() -> Option<String> {
    std::env::var(LATTICE_CREDSFILE_KEY).ok()
}

fn get_env(var: &str, default: &str) -> String {
    match std::env::var(var) {
        Ok(val) => {
            if val.is_empty() {
                default.to_string()
            } else {
                val.to_string()
            }
        }
        Err(_) => default.to_string(),
    }
}

fn get_timeout() -> Duration {
    match std::env::var(LATTICE_RPC_TIMEOUT_KEY) {
        Ok(val) => {
            if val.is_empty() {
                Duration::from_millis(DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS)
            } else {
                Duration::from_millis(val.parse().unwrap_or(DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS))
            }
        }
        Err(_) => Duration::from_millis(DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS),
    }
}

const LATTICE_HOST_KEY: &str = "LATTICE_HOST";
// env var name
const DEFAULT_LATTICE_HOST: &str = "127.0.0.1";
// default mode is anonymous via loopback
const LATTICE_RPC_TIMEOUT_KEY: &str = "LATTICE_RPC_TIMEOUT_MILLIS";
const DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS: u64 = 600;
const LATTICE_CREDSFILE_KEY: &str = "LATTICE_CREDS_FILE";
