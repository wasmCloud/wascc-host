#![doc(html_logo_url = "https://avatars0.githubusercontent.com/u/52050279?s=200&v=4")]
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
extern crate log;

#[macro_use]
extern crate crossbeam;

mod actor;
mod authz;
mod bus;
mod capability;
mod dispatch;
pub mod errors;
mod extras;
mod inthost;
#[cfg(feature = "manifest")]
mod manifest;
pub mod middleware;
mod plugins;
mod spawns;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REVISION: u32 = 2;

pub type Result<T> = std::result::Result<T, errors::Error>;
pub use actor::Actor;
pub use capability::NativeCapability;
pub use inthost::{Invocation, InvocationResponse, WasccEntity};

#[cfg(feature = "manifest")]
pub use manifest::{BindingEntry, HostManifest};

#[cfg(feature = "prometheus_middleware")]
pub use middleware::prometheus;

pub use authz::Authorizer;
pub use middleware::Middleware;
pub use wapc::{prelude::WasiParams, WapcHost};

pub type SubjectClaimsPair = (String, Claims<wascap::jwt::Actor>);

use bus::MessageBus;
use crossbeam::Sender;
use plugins::PluginManager;
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use wascap::jwt::Claims;
use wascap::prelude::KeyPair;
use wascc_codec::{
    capabilities::CapabilityDescriptor,
    core::{CapabilityConfiguration, OP_BIND_ACTOR},
    SYSTEM_ACTOR,
};

//type BindingsList = Vec<(String, String, String)>;
type BindingsList = HashMap<BindingTuple, CapabilityConfiguration>;
type BindingTuple = (String, String, String); // (from-actor, to-capid, to-binding-name)

/// A routing key is a combination of a capability ID and the binding name used for
/// that capability. Think of it as a unique or primary key for a capid+binding.
#[derive(PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Clone)]
pub(crate) struct RouteKey {
    pub binding_name: String,
    pub capid: String,
}

impl RouteKey {
    pub fn new(binding_name: &str, capid: &str) -> RouteKey {
        RouteKey {
            binding_name: binding_name.to_string(),
            capid: capid.to_string(),
        }
    }
}

/// Represents an instance of a waSCC host
#[derive(Clone)]
pub struct WasccHost {
    bus: Arc<MessageBus>,
    claims: Arc<RwLock<HashMap<String, Claims<wascap::jwt::Actor>>>>,
    plugins: Arc<RwLock<PluginManager>>,
    bindings: Arc<RwLock<BindingsList>>,
    caps: Arc<RwLock<HashMap<RouteKey, CapabilityDescriptor>>>,
    middlewares: Arc<RwLock<Vec<Box<dyn Middleware>>>>,
    // the key to this field is the subscription subject, and not either a pk or a capid
    terminators: Arc<RwLock<HashMap<String, Sender<bool>>>>,
    #[cfg(feature = "gantry")]
    gantry_client: Arc<RwLock<Option<gantryclient::Client>>>,
    key: KeyPair,
    authorizer: Arc<RwLock<Box<dyn Authorizer>>>,
}

impl WasccHost {
    /// Creates a new waSCC runtime host
    pub fn new() -> Self {
        Self::with_authorizer(authz::DefaultAuthorizer::new())
    }

    /// Creates a waSCC host with the given authorizer to be used for supplemental authorization checks
    /// *after* the base claims check is performed. The authorizer can add further restrictions
    /// to actors consuming capabilities, and can determine whether one actor is allowed to
    /// invoke another (including itself, which can occur during manual configuration by a host).
    pub fn with_authorizer(authz: impl Authorizer + 'static) -> Self {
        let key = KeyPair::new_server();
        let claims = Arc::new(RwLock::new(HashMap::new()));
        let caps = Arc::new(RwLock::new(HashMap::new()));
        let bindings = Arc::new(RwLock::new(HashMap::new()));
        #[cfg(feature = "lattice")]
        let bus = bus::new(
            key.public_key(),
            claims.clone(),
            caps.clone(),
            bindings.clone(),
        );
        #[cfg(not(feature = "lattice"))]
        let bus = bus::new();

        #[cfg(feature = "gantry")]
        let host = WasccHost {
            terminators: Arc::new(RwLock::new(HashMap::new())),
            bus: Arc::new(bus),
            claims,
            plugins: Arc::new(RwLock::new(PluginManager::default())),
            bindings,
            caps,
            middlewares: Arc::new(RwLock::new(vec![])),
            gantry_client: Arc::new(RwLock::new(None)),
            key: key,
            authorizer: Arc::new(RwLock::new(Box::new(authz))),
        };
        #[cfg(not(feature = "gantry"))]
        let host = WasccHost {
            terminators: Arc::new(RwLock::new(HashMap::new())),
            bus: Arc::new(bus),
            claims,
            plugins: Arc::new(RwLock::new(PluginManager::default())),
            bindings,
            middlewares: Arc::new(RwLock::new(vec![])),
            caps,
            key: key,
            authorizer: Arc::new(RwLock::new(Box::new(authz))),
        };
        info!("Host ID is {} (v{})", host.key.public_key(), VERSION,);
        host.ensure_extras().unwrap();
        host
    }

    /// Adds an actor to the host
    pub fn add_actor(&self, actor: Actor) -> Result<()> {
        if self
            .claims
            .read()
            .unwrap()
            .contains_key(&actor.public_key())
        {
            return Err(errors::new(errors::ErrorKind::MiscHost(
                format!("Actor {} is already in this host. Cannot host multiple instances of the same actor in the same host", actor.public_key())
            )));
        }
        authz::enforce_validation(&actor.token.jwt)?; // returns an `Err` if validation fails
        if !self.check_auth(&actor.token) {
            // invoke the auth hook, if there is one
            return Err(errors::new(errors::ErrorKind::Authorization(
                "Authorization hook denied access to module".into(),
            )));
        }

        authz::register_claims(
            self.claims.clone(),
            &actor.token.claims.subject,
            actor.token.claims.clone(),
        );

        let wg = crossbeam_utils::sync::WaitGroup::new();
        // Spin up a new thread that listens to "wasmbus.Mxxxx" calls on the message bus
        spawns::spawn_actor(
            wg.clone(),
            actor.token.claims.clone(),
            actor.bytes.clone(),
            None,
            true,
            None,
            self.bus.clone(),
            self.middlewares.clone(),
            self.caps.clone(),
            self.bindings.clone(),
            self.claims.clone(),
            self.terminators.clone(),
            self.key.clone(),
            self.authorizer.clone(),
        )?;
        wg.wait();
        if actor.capabilities().contains(&extras::CAPABILITY_ID.into()) {
            // force a binding so that there's a private actor subject on the bus for the
            // actor to communicate with the extras provider
            self.bind_actor(
                &actor.public_key(),
                extras::CAPABILITY_ID,
                None,
                HashMap::new(),
            )?;
        }

        Ok(())
    }

    /// Adds an actor to the host by looking it up in a Gantry repository, downloading
    /// the signed module bytes, and adding them to the host
    #[cfg(feature = "gantry")]
    pub fn add_actor_from_gantry(&self, actor: &str) -> Result<()> {
        {
            let lock = self.gantry_client.read().unwrap();
            if lock.as_ref().is_none() {
                return Err(errors::new(errors::ErrorKind::MiscHost(
                    "No gantry client configured".to_string(),
                )));
            }
        }
        use crossbeam_channel::unbounded;
        let (s, r) = unbounded();
        let bytevec = Arc::new(RwLock::new(Vec::new()));
        let b = bytevec.clone();
        let _ack = self
            .gantry_client
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .download_actor(actor, move |chunk| {
                bytevec
                    .write()
                    .unwrap()
                    .extend_from_slice(&chunk.chunk_bytes);
                if chunk.sequence_no == chunk.total_chunks {
                    s.send(true).unwrap();
                }
                Ok(())
            });
        let _ = r.recv().unwrap();
        let vec = b.read().unwrap();
        self.add_actor(Actor::from_bytes(vec.clone())?)
    }

    /// Adds a portable capability provider (e.g. a WASI actor) to the waSCC host
    pub fn add_capability(
        &self,
        actor: Actor,
        binding: Option<&str>,
        wasi: WasiParams,
    ) -> Result<()> {
        let binding = binding.unwrap_or("default");

        let wg = crossbeam_utils::sync::WaitGroup::new();
        // Spins up a new thread subscribed to the "wasmbus.{capid}.{binding}" subject
        spawns::spawn_actor(
            wg.clone(),
            actor.token.claims,
            actor.bytes.clone(),
            Some(wasi),
            false,
            Some(binding.to_string()),
            self.bus.clone(),
            self.middlewares.clone(),
            self.caps.clone(),
            self.bindings.clone(),
            self.claims.clone(),
            self.terminators.clone(),
            self.key.clone(),
            self.authorizer.clone(),
        )?;
        wg.wait();
        Ok(())
    }

    /// Removes an actor from the host. Notifies the actor's processing thread to terminate,
    /// which will in turn attempt to unbind that actor from all previously bound capability providers
    pub fn remove_actor(&self, pk: &str) -> Result<()> {
        self.terminators.read().unwrap()[&bus::actor_subject(pk)]
            .send(true)
            .unwrap();
        Ok(())
    }

    /// Replaces one running actor with another live actor with no message loss. Note that
    /// the time it takes to perform this replacement can cause pending messages from capability
    /// providers (e.g. messages from subscriptions or HTTP requests) to build up in a backlog,
    /// so make sure the new actor can handle this stream of these delayed messages
    pub fn replace_actor(&self, new_actor: Actor) -> Result<()> {
        crate::inthost::replace_actor(&self.key, self.bus.clone(), new_actor)
    }

    /// Adds a middleware item to the middleware processing pipeline
    pub fn add_middleware(&self, mid: impl Middleware) {
        self.middlewares.write().unwrap().push(Box::new(mid));
    }

    /// Adds a native capability provider plugin to the waSCC runtime. Note that because these capabilities are native,
    /// cross-platform support is not always guaranteed.
    pub fn add_native_capability(&self, capability: NativeCapability) -> Result<()> {
        let capid = capability.id();
        if self
            .caps
            .read()
            .unwrap()
            .contains_key(&RouteKey::new(&capability.binding_name, &capability.id()))
        {
            return Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                "Capability provider {} cannot be bound to the same name ({}) twice, loading failed.", capid, capability.binding_name                
            ))));
        }
        self.caps.write().unwrap().insert(
            RouteKey::new(&capability.binding_name, &capability.descriptor.id),
            capability.descriptor().clone(),
        );
        let wg = crossbeam_utils::sync::WaitGroup::new();
        spawns::spawn_native_capability(
            capability,
            self.bus.clone(),
            self.middlewares.clone(),
            self.bindings.clone(),
            self.terminators.clone(),
            self.plugins.clone(),
            wg.clone(),
            Arc::new(self.key.clone()),
        )?;
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
        let subject = bus::provider_subject(capability_id, &b);
        if let Some(terminator) = self.terminators.read().unwrap().get(&subject) {
            terminator.send(true).unwrap();
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
        let claims = self.claims.read().unwrap().get(actor).cloned();
        if claims.is_none() {
            return Err(errors::new(errors::ErrorKind::MiscHost(
                "Attempted to bind non-existent actor".to_string(),
            )));
        }
        let c = claims.unwrap().clone();
        let binding = binding_name.unwrap_or("default".to_string());
        if !authz::can_invoke(&c, capid, OP_BIND_ACTOR) {
            return Err(errors::new(errors::ErrorKind::Authorization(format!(
                "Unauthorized binding: actor {} is not authorized to use capability {}.",
                actor, capid
            ))));
        } else {
            if !self.authorizer.read().unwrap().can_invoke(
                &c,
                &WasccEntity::Capability {
                    capid: capid.to_string(),
                    binding: binding.to_string(),
                },
                OP_BIND_ACTOR,
            ) {
                return Err(errors::new(errors::ErrorKind::Authorization(format!(
                    "Unauthorized binding: actor {} is not authorized to use capability {}.",
                    actor, capid
                ))));
            }
        }

        info!(
            "Attempting to bind actor {} to {},{}",
            actor, &binding, capid
        );

        let tgt_subject = if (actor == capid || actor == SYSTEM_ACTOR) && capid.starts_with("M") {
            // manually injected actor configuration
            bus::actor_subject(actor)
        } else {
            bus::provider_subject(capid, &binding)
        };
        info!("Binding subject: {}", tgt_subject);
        let inv = inthost::gen_config_invocation(
            &self.key,
            actor,
            capid,
            c.clone(),
            binding.clone(),
            config.clone(),
        );
        match self.bus.invoke(&tgt_subject, inv) {
            Ok(inv_r) => {
                if let Some(e) = inv_r.error {
                    Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                        "Failed to configure {},{} - {}",
                        binding, capid, e
                    ))))
                } else {
                    self.record_binding(
                        actor,
                        capid,
                        &binding,
                        &CapabilityConfiguration {
                            module: actor.to_string(),
                            values: config,
                        },
                    )?;
                    Ok(())
                }
            }
            Err(e) => Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                "Failed to configure {},{} - {}",
                binding, capid, e
            )))),
        }
    }

    /// Configure the Gantry client connection information to be used when actors
    /// are loaded remotely via `Actor::from_gantry`
    #[cfg(feature = "gantry")]
    pub fn configure_gantry(&self, nats_urls: Vec<String>, jwt: &str, seed: &str) -> Result<()> {
        *self.gantry_client.write().unwrap() =
            Some(gantryclient::Client::new(nats_urls, jwt, seed));
        Ok(())
    }

    /// Invoke an operation handler on an actor directly. The caller is responsible for
    /// knowing ahead of time if the given actor supports the specified operation.
    pub fn call_actor(&self, actor: &str, operation: &str, msg: &[u8]) -> Result<Vec<u8>> {
        if !self.claims.read().unwrap().contains_key(actor) {
            return Err(errors::new(errors::ErrorKind::MiscHost(
                "No such actor".into(),
            )));
        }
        let inv = Invocation::new(
            &self.key,
            WasccEntity::Actor(SYSTEM_ACTOR.to_string()),
            WasccEntity::Actor(actor.to_string()),
            operation,
            msg.to_vec(),
        );
        let tgt_subject = bus::actor_subject(actor);
        match self.bus.invoke(&tgt_subject, inv) {
            Ok(resp) => Ok(resp.msg),
            Err(e) => Err(e),
        }
    }

    /// Returns the full set of JWT claims for a given actor, if that actor is running in the host
    pub fn claims_for_actor(&self, pk: &str) -> Option<Claims<wascap::jwt::Actor>> {
        self.claims.read().unwrap().get(pk).cloned()
    }

    /// Applies a manifest JSON or YAML file to set up a host's actors, capability providers,
    /// and actor bindings
    #[cfg(feature = "manifest")]
    pub fn apply_manifest(&self, manifest: HostManifest) -> Result<()> {
        for actor in manifest.actors {
            #[cfg(feature = "gantry")]
            self.add_actor_gantry_first(&actor)?;

            #[cfg(not(feature = "gantry"))]
            self.add_actor(Actor::from_file(&actor)?)?;
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
                config.values.unwrap_or(HashMap::new()),
            )?;
        }
        Ok(())
    }

    #[cfg(feature = "gantry")]
    fn add_actor_gantry_first(&self, actor: &str) -> Result<()> {
        if actor.len() == 56 && actor.starts_with('M') {
            // This is an actor's public subject
            self.add_actor_from_gantry(actor)
        } else {
            self.add_actor(Actor::from_file(&actor)?)
        }
    }

    /// Returns the list of actors registered in the host
    pub fn actors(&self) -> Vec<SubjectClaimsPair> {
        authz::get_all_claims(self.claims.clone())
    }

    /// Returns the list of capability providers registered in the host. The key is a tuple of (binding, capability ID)
    pub fn capabilities(&self) -> HashMap<(String, String), CapabilityDescriptor> {
        let lock = self.caps.read().unwrap();
        let mut res = HashMap::new();
        for (rk, descriptor) in lock.iter() {
            res.insert(
                (rk.binding_name.to_string(), rk.capid.to_string()),
                descriptor.clone(),
            );
        }
        res
    }

    /// Returns the list of actors in the host that contain all of the tags in the
    /// supplied parameter
    pub fn actors_by_tag(&self, tags: &[&str]) -> Vec<String> {
        let mut actors = vec![];

        for (actor, claims) in self.claims.read().unwrap().iter() {
            if let Some(actor_tags) = claims.metadata.as_ref().and_then(|m| m.tags.as_ref()) {
                if tags.iter().all(|&t| actor_tags.contains(&t.to_string())) {
                    actors.push(actor.to_string())
                }
            }
        }

        actors
    }

    /// Attempts to perform a graceful shutdown of the host by removing all actors in
    /// the host and then removing all capability providers. This function is not guaranteed to
    /// block and wait for the shutdown to finish
    pub fn shutdown(&self) -> Result<()> {
        let actors = self.actors();
        for (pk, _claims) in actors {
            self.remove_actor(&pk)?;
        }
        let caps = self.capabilities();
        for (binding_name, capid) in caps.keys() {
            self.remove_native_capability(&capid, Some(binding_name.to_string()))?;
        }
        Ok(())
    }
}
