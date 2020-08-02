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

// An example demonstrating loading a manifest
// For this to run, you'll need both the `manifest` and `gantry`
// features enabled, you can do that as follows from the root wascc-host directory:
// cargo run --example kvcounter_manfifest --all-features

use wascc_host::{HostManifest, WasccHost};

fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();
    let host = WasccHost::new();
    host.apply_manifest(HostManifest::from_yaml(
        "./examples/sample_manifest.yaml",
        true,
    )?)?;

    std::thread::park();
    Ok(())
}
