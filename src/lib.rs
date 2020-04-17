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
pub use inthost::{Invocation, InvocationResponse, InvocationTarget};

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
mod inthost;

#[cfg(feature = "manifest")]
mod manifest;

mod middleware;
mod plugins;
mod router;

pub type SubjectClaimsPair = (String, Claims<wascap::jwt::Actor>);

use crate::router::{get_route, route_exists};
use inthost::ACTOR_BINDING;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use wascap::jwt::{Claims, Token};
use wascc_codec::{
    core::{CapabilityConfiguration, OP_BIND_ACTOR},
    serialize,
};

/// Represents an instance of a waSCC host
#[derive(Clone)]
pub struct WasccHost {}

impl WasccHost {
    /// Creates a new waSCC runtime host
    pub fn new() -> Self {
        let host = WasccHost {};
        host.ensure_extras().unwrap();
        host
    }

    /// Adds an actor to the host
    pub fn add_actor(&self, actor: Actor) -> Result<()> {
        let wg = crossbeam_utils::sync::WaitGroup::new();
        if route_exists(ACTOR_BINDING, &actor.public_key()) {
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
            ACTOR_BINDING.to_string(),
        )?;
        wg.wait();
        Ok(())
    }

    /// Adds a portable capability provider (WASI actor) to the waSCC host
    pub fn add_capability(
        &self,
        actor: Actor,
        binding: Option<&str>,
        wasi: WasiParams,
    ) -> Result<()> {
        let token = authz::extract_claims(&actor.bytes)?;
        let capid = token.claims.metadata.unwrap().caps.unwrap()[0].clone();
        let binding = binding.unwrap_or("default");
        if route_exists(&binding, &capid) {
            return Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                "Capability provider {} cannot be bound to the same name ({}) twice, loading failed.", capid, 
                binding)
            )));
        }
        info!("Adding portable capability to host: {},{}", binding, capid);
        let wg = crossbeam_utils::sync::WaitGroup::new();
        self.spawn_actor_and_listen(
            wg.clone(),
            actor.token.claims,
            actor.bytes.clone(),
            Some(wasi),
            false,
            binding.to_string(),
        )?;
        wg.wait();
        Ok(())
    }

    /// Removes an actor from the host. Notifies the actor's processing thread to terminate,
    /// which will in turn attempt to unbind that actor from all previously bound capability providers
    pub fn remove_actor(&self, pk: &str) -> Result<()> {
        crate::router::ROUTER
            .write()
            .unwrap()
            .terminate_route(crate::inthost::ACTOR_BINDING, pk)
    }

    /// Replaces one running actor with another live actor with no message loss. Note that
    /// the time it takes to perform this replacement can cause pending messages from capability
    /// providers (e.g. messages from subscriptions or HTTP requests) to build up in a backlog,
    /// so make sure the new actor can handle this stream of these delayed messages
    pub fn replace_actor(&self, new_actor: Actor) -> Result<()> {
        crate::inthost::replace_actor(new_actor)
    }

    /// Adds a middleware item to the middleware processing pipeline
    pub fn add_middleware(&self, mid: impl Middleware) {
        middleware::add_middleware(mid);
    }

    /// Setting a custom authorization hook allows your code to make additional decisions
    /// about whether or not a given actor is allowed to run within the host. The authorization
    /// hook takes a `Token` (JSON Web Token) as input and returns a boolean indicating the validity of the module.
    pub fn set_auth_hook<F>(&self, hook: F)
    where
        F: Fn(&Token<wascap::jwt::Actor>) -> bool + Sync + Send + 'static,
    {
        crate::authz::set_auth_hook(hook)
    }

    /// Adds a native capability provider plugin to the waSCC runtime. Note that because these capabilities are native,
    /// cross-platform support is not always guaranteed.
    pub fn add_native_capability(&self, capability: NativeCapability) -> Result<()> {
        let capid = capability.capid.clone();
        if route_exists(&capability.binding_name, &capability.id()) {
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

    /// Removes a native capability provider plugin from the waSCC runtime
    pub fn remove_native_capability(
        &self,
        capability_id: &str,
        binding_name: Option<String>,
    ) -> Result<()> {
        let b = binding_name.unwrap_or("default".to_string());
        if let Some(entry) = get_route(&b, capability_id) {
            entry.terminate();
            Ok(())
        } else {
            Err(errors::new(errors::ErrorKind::MiscHost(
                "No such capability".into(),
            )))
        }
    }

    /// Binds an actor to a capability provider with a given configuration. If the binding name
    /// is `None` then the default binding name will be used. An actor can only have one default
    /// binding per capability provider.
    pub fn bind_actor(
        &self,
        actor: &str,
        capid: &str,
        binding_name: Option<String>,
        config: HashMap<String, String>,
    ) -> Result<()> {
        if !authz::can_invoke(actor, capid) {
            return Err(errors::new(errors::ErrorKind::Authorization(format!(
                "Unauthorized binding: actor {} is not authorized to use capability {}.",
                actor, capid
            ))));
        }
        let binding = binding_name.unwrap_or("default".to_string());
        info!(
            "Attempting to bind actor {} to {},{}",
            actor, &binding, capid
        );
        match get_route(&binding, capid) {
            Some(entry) => {
                let res = entry.invoke(inthost::gen_config_invocation(
                    actor,
                    capid,
                    binding.clone(),
                    config,
                ))?;
                if let Some(e) = res.error {
                    Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                        "Failed to configure {},{} - {}",
                        binding, capid, e
                    ))))
                } else {
                    self.record_binding(actor, capid, &binding)?;
                    Ok(())
                }
            }
            None => {
                // If a binding is created where the actor and the capid are identical
                // this is a manual configuration of an actor
                if actor == capid {
                    let cfgvals = CapabilityConfiguration {
                        module: actor.to_string(),
                        values: config,
                    };
                    let payload = serialize(&cfgvals).unwrap();
                    self.call_actor(actor, OP_BIND_ACTOR, &payload).map(|_| ())
                } else {
                    Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                        "No such capability provider: {},{}",
                        binding, capid
                    ))))
                }
            }
        }
    }

    /// Configure the Gantry client connection information to be used when actors
    /// are loaded remotely via `Actor::from_gantry`
    #[cfg(feature = "gantry")]
    pub fn configure_gantry(&self, nats_urls: Vec<String>, jwt: &str, seed: &str) -> Result<()> {
        *inthost::GANTRYCLIENT.write().unwrap() = gantryclient::Client::new(nats_urls, jwt, seed);
        Ok(())
    }

    /// Invoke an operation handler on an actor directly. The caller is responsible for
    /// knowing ahead of time if the given actor supports the specified operation.
    pub fn call_actor(&self, actor: &str, operation: &str, msg: &[u8]) -> Result<Vec<u8>> {
        match get_route(ACTOR_BINDING, actor) {
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

    /// Returns the full set of JWT claims for a given actor, if that actor is running in the host
    pub fn claims_for_actor(&self, pk: &str) -> Option<Claims<wascap::jwt::Actor>> {
        authz::get_claims(pk)
    }

    /// Applies a manifest JSON or YAML file to set up a host's actors, capability providers,
    /// and actor bindings
    #[cfg(feature = "manifest")]
    pub fn apply_manifest(&self, manifest: HostManifest) -> Result<()> {
        for actor in manifest.actors {
            #[cfg(feature = "gantry")]
            self.add_actor_gantry_first(&actor)?;

            #[cfg(not(feature = "gantry"))]
            self.add_actor_from_path(&actor)?;
        }
        for cap in manifest.capabilities {
            // for now, supports only file paths
            self.add_native_capability(NativeCapability::from_file(cap.path, cap.binding_name)?)?;
        }
        for config in manifest.bindings {
            self.bind_actor(
                &config.actor,
                &config.capability,
                config.binding,
                config.values,
            )?;
        }
        Ok(())
    }

    #[cfg(feature = "gantry")]
    fn add_actor_gantry_first(&self, actor: &str) -> Result<()> {
        if actor.len() == 56 && actor.starts_with('M') {
            // This is an actor's public subject
            self.add_actor(Actor::from_gantry(&actor)?)
        } else {
            self.add_actor(Actor::from_file(&actor)?)
        }
    }

    #[cfg(not(feature = "gantry"))]
    fn add_actor_from_path(&self, actor: &str) -> Result<()> {
        self.add_actor(Actor::from_file(actor)?)
    }

    /// Returns the list of actors registered in the host
    pub fn actors(&self) -> Vec<SubjectClaimsPair> {
        authz::get_all_claims()
    }

    /// Returns the list of capability providers registered in the host
    pub fn capabilities(&self) -> Vec<CapabilitySummary> {
        let lock = inthost::CAPS.read().unwrap();
        lock.iter().cloned().collect()
    }
}
