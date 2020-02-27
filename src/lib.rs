#![doc(html_logo_url = "https://avatars0.githubusercontent.com/u/52050279?s=200&v=4")]
// Copyright 2015-2019 Capital One Services, LLC
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

//! # waSCC Host
//!
//! The WebAssembly Secure Capabilities Connector (waSCC) host runtime manages actors
//! written in WebAssembly (aka _nanoprocesses_) and capability providers written in
//! WebAssembly (via WASI) or as OS-native plugin libraries. waSCC securely manages
//! communications between actors and the capabilities they need.
//!
//! To start a runtime, simply add actors and capabilities to the host. For more information,
//! take a look at the documentation and tutorials at [wascc.dev](https://wascc.dev).
//!
//! # Example
//! ```
//! use std::collections::HashMap;
//! use wascc_host::{host, Actor, NativeCapability};
//!
//! fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
//!    env_logger::init();
//!    host::add_actor(Actor::from_file("./examples/.assets/echo.wasm")?)?;
//!    host::add_actor(Actor::from_file("./examples/.assets/echo2.wasm")?)?;
//!    host::add_native_capability(NativeCapability::from_file(
//!        "./examples/.assets/libwascc_httpsrv.so",
//!    )?)?;
//!
//!    host::configure(
//!        "MDFD7XZ5KBOPLPHQKHJEMPR54XIW6RAG5D7NNKN22NP7NSEWNTJZP7JN",
//!        "wascc:http_server",
//!        generate_port_config(8082),
//!    )?;
//!
//!    host::configure(
//!        "MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2",
//!        "wascc:http_server",
//!        generate_port_config(8081),
//!    )?;
//!
//!    assert_eq!(2, host::actors().len());
//!    if let Some(ref claims) = host::actor_claims("MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2") {
//!        let md = claims.metadata.as_ref().unwrap();
//!        assert!(md.caps.as_ref().unwrap().contains(&"wascc:http_server".to_string()));   
//!    }
//!    
//!
//! # std::thread::sleep(::std::time::Duration::from_millis(10));
//!    // Need to keep the main thread from terminating immediately
//!    // std::thread::park();
//!
//!    Ok(())
//! }
//!
//! fn generate_port_config(port: u16) -> HashMap<String, String> {
//!    let mut hm = HashMap::new();
//!    hm.insert("PORT".to_string(), port.to_string());
//!
//!    hm
//! }
//!
//! ```
//!

#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;

#[macro_use]
extern crate crossbeam;

#[macro_use]
extern crate serde;

pub type Result<T> = std::result::Result<T, errors::Error>;
pub use actor::Actor;
pub use capability::NativeCapability;
pub use manifest::{ConfigEntry, HostManifest};
pub use middleware::Middleware;
pub use wapc::prelude::WasiParams;

mod actor;
mod authz;
mod capability;
mod dispatch;
pub mod errors;
mod extras;
pub mod host;
mod manifest;
mod middleware;
mod plugins;
mod router;
