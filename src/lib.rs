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
//! use wascc_host::{WasccHost, Actor, NativeCapability};
//!
//! fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
//!    env_logger::init();
//!    let host = WasccHost::new();
//!    host.add_actor(Actor::from_file("./examples/.assets/echo.wasm")?)?;
//!    host.add_actor(Actor::from_file("./examples/.assets/echo2.wasm")?)?;
//!    host.add_native_capability(NativeCapability::from_file(
//!        "./examples/.assets/libwascc_httpsrv.so", None
//!    )?)?;
//!
//!    host.bind_actor(
//!        "MDFD7XZ5KBOPLPHQKHJEMPR54XIW6RAG5D7NNKN22NP7NSEWNTJZP7JN",
//!        "wascc:http_server",
//!        None,
//!        generate_port_config(8082),
//!    )?;
//!
//!    host.bind_actor(
//!        "MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2",
//!        "wascc:http_server",
//!        None,
//!        generate_port_config(8081),
//!    )?;
//!
//!    assert_eq!(2, host.actors().len());
//!    if let Some(ref claims) = host.claims_for_actor("MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2") {
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

pub type Result<T> = std::result::Result<T, errors::Error>;
pub use actor::Actor;
pub use capability::{CapabilitySummary, NativeCapability};
pub use inthost::{Invocation, InvocationResponse};

#[cfg(feature = "manifest")]
pub use manifest::{BindingEntry, HostManifest};

pub use middleware::Middleware;
pub use wapc::prelude::WasiParams;

mod actor;
mod authz;
mod capability;
mod dispatch;
pub mod errors;
mod extras;
pub mod host;
mod inthost;

#[cfg(feature = "manifest")]
mod manifest;

mod middleware;
mod plugins;
mod router;

pub type SubjectClaimsPair = (String, Claims<wascap::jwt::Actor>);

use crate::router::{route_exists, get_route};
use inthost::{InvocationTarget, ACTOR_BINDING};
use std::collections::HashMap;
use wascap::jwt::{Claims, Token};

#[derive(Clone)]
pub struct WasccHost {}

impl WasccHost {
    pub fn new() -> Self {
        let host = WasccHost {};
        host.ensure_extras().unwrap();
        host
    }

    pub fn add_actor(&self, actor: Actor) -> Result<()> {
        let wg = crossbeam_utils::sync::WaitGroup::new();
        if route_exists(Some(ACTOR_BINDING), &actor.public_key()) {
            return Err(errors::new(errors::ErrorKind::MiscHost(format!(
                "Actor {} is already running in this host, failed to add.",
                actor.public_key()
            ))));
        }
        info!("Adding actor {} to host", actor.public_key());
        self.spawn_actor_and_listen(
            wg.clone(),
            actor.token.claims,
            actor.bytes.clone(),
            None,
            true,
            None,
        )?;
        wg.wait();
        Ok(())
    }

    pub fn add_capability(
        &self,
        actor: Actor,
        binding: Option<&str>,
        wasi: WasiParams,
    ) -> Result<()> {
        let token = authz::extract_claims(&actor.bytes)?;
        let capid = token.claims.metadata.unwrap().caps.unwrap()[0].clone();
        if route_exists(binding, &capid) {
            return Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                "Capability provider {} cannot be bound to the same name ({}) twice, loading failed.", capid, 
                binding.unwrap_or("default")              
            ))));
        }
        info!(
            "Adding portable capability to host: {},{}",
            binding.unwrap_or("default"),
            capid
        );
        let wg = crossbeam_utils::sync::WaitGroup::new();
        self.spawn_actor_and_listen(
            wg.clone(),
            actor.token.claims,
            actor.bytes.clone(),
            Some(wasi),
            false,
            binding
        )?;
        wg.wait();
        Ok(())
    }

    /// Removes an actor from the host. Notifies the actor's processing thread to terminate,
    /// which will in turn attempt to de-configure that actor for all capabilities
    pub fn remove_actor(&self, pk: &str) -> Result<()> {
        crate::router::ROUTER
            .write()
            .unwrap()
            .terminate_route(Some(crate::inthost::ACTOR_BINDING), pk)
    }

    /// Replaces one running actor with another live actor with no message loss. Note that
    /// the time it takes to perform this replacement can cause pending messages from capability
    /// providers (e.g. messages from subscriptions or HTTP requests) to build up in a backlog,
    /// so make sure the new actor can handle this stream of these delayed messages
    pub fn replace_actor(&self, new_actor: Actor) -> Result<()> {
        crate::inthost::replace_actor(new_actor)
    }

    pub fn add_middleware(&self, mid: impl Middleware) {
        middleware::add_middleware(mid);
    }

    pub fn set_auth_hook<F>(&self, hook: F)
    where
        F: Fn(&Token<wascap::jwt::Actor>) -> bool + Sync + Send + 'static,
    {
        crate::authz::set_auth_hook(hook)
    }

    pub fn add_native_capability(&self, capability: NativeCapability) -> Result<()> {
        let capid = capability.capid.clone();
        if route_exists(Some(&capability.binding_name), &capability.id()) {
            return Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                "Capability provider {} cannot be bound to the same name ({}) twice, loading failed.", capid, capability.binding_name                
            ))));
        }
        let summary = CapabilitySummary {
            id: capid.clone(),
            name: capability.name(),
            binding: capability.binding_name.to_string(),
            portable: false,
        };
        let wg = crossbeam_utils::sync::WaitGroup::new();
        self.spawn_capability_provider_and_listen(capability, summary, wg.clone())?;
        wg.wait();
        Ok(())
    }

    pub fn remove_native_capability(
        &self,
        capability_id: &str,
        binding_name: Option<&str>,
    ) -> Result<()> {
        if let Some(entry) = get_route(binding_name, capability_id) {
            entry.terminate();
            Ok(())
        } else {
            Err(errors::new(errors::ErrorKind::MiscHost(
                "No such capability".into(),
            )))
        }
    }

    pub fn bind_actor(
        &self,
        actor: &str,
        capid: &str,
        binding_name: Option<&str>,
        config: HashMap<String, String>,
    ) -> Result<()> {
        if !authz::can_invoke(actor, capid) {
            return Err(errors::new(errors::ErrorKind::Authorization(format!(
                "Unauthorized binding: actor {} is not authorized to use capability {}.",
                actor, capid
            ))));
        }
        let binding = binding_name.unwrap_or("default").to_string();
        info!(
            "Attempting to bind actor {} to {},{}",
            actor, &binding, capid
        );
        match get_route(binding_name, capid) {
            Some(entry) => {
                let res = entry.invoke(inthost::gen_config_invocation(
                    actor,
                    capid,
                    binding_name,
                    config,
                ))?;
                if let Some(e) = res.error {
                    Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                        "Failed to configure {},{} - {}",
                        binding, capid, e
                    ))))
                } else {
                    Ok(())
                }
            }
            None => Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                "No such capability provider: {},{}",
                binding, capid
            )))),
        }
    }

    /// Connect to a Gantry server
    #[cfg(feature = "gantry")]
    pub fn configure_gantry(&self, nats_urls: Vec<String>, jwt: &str, seed: &str) -> Result<()> {
        *inthost::GANTRYCLIENT.write().unwrap() = gantryclient::Client::new(nats_urls, jwt, seed);
        Ok(())
    }

    pub fn call_actor(&self, actor: &str, operation: &str, msg: &[u8]) -> Result<Vec<u8>> {
        match get_route(Some(ACTOR_BINDING), actor) {
            Some(entry) => {
                match entry.invoke(Invocation::new(
                    "system".to_string(),
                    InvocationTarget::Actor(actor.to_string()),
                    operation,
                    msg.to_vec(),
                )) {
                    Ok(resp) => Ok(resp.msg),
                    Err(e) => Err(e),
                }
            }
            None => Err(errors::new(errors::ErrorKind::MiscHost(
                "No such actor".into(),
            ))),
        }
    }

    pub fn claims_for_actor(&self, pk: &str) -> Option<Claims<wascap::jwt::Actor>> {
        authz::get_claims(pk)
    }

    #[cfg(feature = "manifest")]
    pub fn apply_manifest(&self, manifest: HostManifest) -> Result<()> {
        Ok(())
    }

    pub fn actors(&self) -> Vec<SubjectClaimsPair> {
        authz::get_all_claims()
    }

    pub fn capabilities(&self) -> Vec<CapabilitySummary> {
        let lock = inthost::CAPS.read().unwrap();
        lock.iter().cloned().collect()
    }
}
