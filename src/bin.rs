// Copyright 2015-2020 Capital One Services, LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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

#[cfg(feature = "manifest")]
fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Cli::from_args();
    let cmd = args.command;
    let _ = env_logger::builder().format_module_path(false).try_init();
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

#[cfg(not(feature = "manifest"))]
fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!(
        "The general-purpose waSCC host application needs to be built with the `manifest` feature."
    );
    Ok(())
}
