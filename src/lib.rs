#![doc(html_logo_url = "https://avatars0.githubusercontent.com/u/52050279?s=200&v=4")]
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
//! use wascc_host::{Host, Actor, NativeCapability};
//!
//! fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
//!    env_logger::init();
//!    let host = Host::new();
//!    host.add_actor(Actor::from_file("./examples/.assets/echo.wasm")?)?;
//!    host.add_actor(Actor::from_file("./examples/.assets/echo2.wasm")?)?;
//!    host.add_native_capability(NativeCapability::from_file(
//!        "./examples/.assets/libwascc_httpsrv.so", None
//!    )?)?;
//!
//!    host.set_binding(
//!        "MDFD7XZ5KBOPLPHQKHJEMPR54XIW6RAG5D7NNKN22NP7NSEWNTJZP7JN",
//!        "wascc:http_server",
//!        None,
//!        generate_port_config(8085),
//!    )?;
//!
//!    host.set_binding(
//!        "MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2",
//!        "wascc:http_server",
//!        None,
//!        generate_port_config(8084),
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

#[cfg(feature = "lattice")]
use latticeclient::BusEvent;

#[cfg(feature = "lattice")]
use bus::lattice::ControlCommand;

pub use authz::Authorizer;
pub use middleware::Middleware;
pub use wapc::WasiParams;

pub type SubjectClaimsPair = (String, Claims<wascap::jwt::Actor>);

use bus::{get_namespace_prefix, MessageBus};
use crossbeam::Sender;
#[cfg(feature = "lattice")]
use crossbeam_channel as channel;
use crossbeam_channel::Receiver;
#[cfg(any(feature = "lattice", feature = "manifest"))]
use inthost::RESTRICTED_LABELS;
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
    serialize, SYSTEM_ACTOR,
};

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

/// A builder pattern implementation for creating a custom-configured host runtime
pub struct HostBuilder {
    labels: HashMap<String, String>,
    ns: Option<String>,
    authorizer: Box<dyn Authorizer + 'static>,
    #[cfg(feature = "lattice")]
    gantry_client: Option<gantryclient::Client>,
}

impl HostBuilder {
    /// Creates a new host builder. This builder will initialize itself with some defaults
    /// obtained from the environment. The labels list will pre-populate with the `hostcore.*`
    /// labels, the namespace will be gleaned from the `LATTICE_NAMESPACE` environment variable
    /// (if lattice mode is enabled), and the default authorizer will be set.
    pub fn new() -> HostBuilder {
        #[cfg(not(feature = "lattice"))]
        let b = HostBuilder {
            labels: inthost::detect_core_host_labels(),
            ns: get_namespace_prefix(),
            authorizer: Box::new(authz::DefaultAuthorizer::new()),
        };

        #[cfg(feature = "lattice")]
        let b = HostBuilder {
            labels: inthost::detect_core_host_labels(),
            ns: get_namespace_prefix(),
            authorizer: Box::new(authz::DefaultAuthorizer::new()),
            gantry_client: None,
        };

        b
    }

    /// Sets the lattice namespace for this host. A lattice namespace is a unit of multi-tenant
    /// isolation on a network. To reduce the risk of conflicts or subscription failures, the
    /// lattice namespace should not include any non-alphanumeric characters.
    #[cfg(feature = "lattice")]
    pub fn with_lattice_namespace(self, ns: &str) -> HostBuilder {
        if !ns.chars().all(char::is_alphanumeric) {
            panic!("Cannot use a non-alphanumeric lattice namespace name");
        }
        HostBuilder {
            ns: Some(ns.to_lowercase().to_string()),
            ..self
        }
    }

    /// Sets a custom authorizer to be used for authorizing actors, capability providers,
    /// and invocation requests. Note that the authorizer cannot be used to implement _less_
    /// strict measures than the default authorizer, it can only be used to implement
    /// _more_ strict rules
    pub fn with_authorizer(self, authorizer: impl Authorizer + 'static) -> HostBuilder {
        HostBuilder {
            authorizer: Box::new(authorizer),
            ..self
        }
    }

    /// Adds an arbitrary label->value pair of metadata to the host. Cannot override
    /// reserved labels such as those that begin with `hostcore.` Calling this twice
    /// on the same label will have no effect after the first call.
    pub fn with_label(self, key: &str, value: &str) -> HostBuilder {
        let mut hm = self.labels.clone();
        if !hm.contains_key(key) {
            hm.insert(key.to_string(), value.to_string());
        }
        HostBuilder { labels: hm, ..self }
    }

    /// Sets the gantry client to be used by the host.
    #[cfg(feature = "lattice")]
    pub fn with_gantryclient(self, client: gantryclient::Client) -> HostBuilder {
        HostBuilder {
            gantry_client: Some(client),
            ..self
        }
    }

    /// Converts the transient builder instance into a realized host runtime instance
    pub fn build(self) -> Host {
        #[cfg(not(feature = "lattice"))]
        let h = Host::generate(self.authorizer, self.labels, self.ns.clone());
        #[cfg(feature = "lattice")]
        let h = Host::generate(
            self.authorizer,
            self.labels,
            self.ns.clone(),
            self.gantry_client.clone(),
        );
        h
    }
}

/// Represents an instance of a waSCC host runtime
#[derive(Clone)]
pub struct Host {
    bus: Arc<MessageBus>,
    claims: Arc<RwLock<HashMap<String, Claims<wascap::jwt::Actor>>>>,
    plugins: Arc<RwLock<PluginManager>>,
    bindings: Arc<RwLock<BindingsList>>,
    caps: Arc<RwLock<HashMap<RouteKey, CapabilityDescriptor>>>,
    middlewares: Arc<RwLock<Vec<Box<dyn Middleware>>>>,
    // the key to this field is the subscription subject, and not either a pk or a capid
    terminators: Arc<RwLock<HashMap<String, Sender<bool>>>>,
    #[cfg(feature = "lattice")]
    gantry_client: Arc<RwLock<Option<gantryclient::Client>>>,
    key: KeyPair,
    authorizer: Arc<RwLock<Box<dyn Authorizer>>>,
    labels: Arc<RwLock<HashMap<String, String>>>,
    ns: Option<String>,
}

impl Host {
    /// Creates a new runtime host using all of the default values. Use the host builder
    /// if you want to provide more customization options
    pub fn new() -> Self {
        #[cfg(not(feature = "lattice"))]
        let h = Self::generate(
            Box::new(authz::DefaultAuthorizer::new()),
            inthost::detect_core_host_labels(),
            get_namespace_prefix(),
        );
        #[cfg(feature = "lattice")]
        let h = Self::generate(
            Box::new(authz::DefaultAuthorizer::new()),
            inthost::detect_core_host_labels(),
            get_namespace_prefix(),
            None,
        );
        h
    }

    pub(crate) fn generate(
        authz: Box<dyn Authorizer + 'static>,
        labels: HashMap<String, String>,
        ns: Option<String>,
        #[cfg(feature = "lattice")] gantry: Option<gantryclient::Client>,
    ) -> Self {
        let key = KeyPair::new_server();
        let claims = Arc::new(RwLock::new(HashMap::new()));
        let caps = Arc::new(RwLock::new(HashMap::new()));
        let bindings = Arc::new(RwLock::new(HashMap::new()));
        let labels = Arc::new(RwLock::new(labels));
        let terminators = Arc::new(RwLock::new(HashMap::new()));
        let authz = Arc::new(RwLock::new(authz));

        #[cfg(feature = "lattice")]
        let (com_s, com_r): (Sender<ControlCommand>, Receiver<ControlCommand>) =
            channel::unbounded();

        #[cfg(feature = "lattice")]
        let bus = Arc::new(bus::new(
            key.public_key(),
            claims.clone(),
            caps.clone(),
            bindings.clone(),
            labels.clone(),
            terminators.clone(),
            ns.clone(),
            com_s,
            authz.clone(),
        ));

        #[cfg(not(feature = "lattice"))]
        let bus = Arc::new(bus::new());

        #[cfg(feature = "lattice")]
        let _ = bus.publish_event(BusEvent::HostStarted(key.public_key()));

        #[cfg(feature = "lattice")]
        let host = Host {
            terminators: terminators.clone(),
            bus: bus.clone(),
            claims: claims.clone(),
            plugins: Arc::new(RwLock::new(PluginManager::default())),
            bindings,
            caps,
            middlewares: Arc::new(RwLock::new(vec![])),
            gantry_client: Arc::new(RwLock::new(gantry)),
            key: key,
            authorizer: authz,
            labels,
            ns,
        };
        #[cfg(not(feature = "lattice"))]
        let host = Host {
            terminators: terminators.clone(),
            bus: bus.clone(),
            claims: claims.clone(),
            plugins: Arc::new(RwLock::new(PluginManager::default())),
            bindings,
            middlewares: Arc::new(RwLock::new(vec![])),
            caps,
            key: key,
            authorizer: authz,
            labels,
            ns,
        };
        info!("Host ID is {} (v{})", host.key.public_key(), VERSION,);

        host.ensure_extras().unwrap();

        #[cfg(feature = "lattice")]
        let _ = bus::lattice::spawn_controlplane(&host, com_r);

        host
    }

    /// Adds an actor to the host. This will provision resources (such as a handler thread) for the actor. Actors
    /// will not be able to make use of capability providers unless bindings are added (or existed prior to the actor
    /// being added to a host, which is possible in `lattice` mode)
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

        let c = self.claims.clone();

        c.write().unwrap().insert(
            actor.token.claims.subject.to_string(),
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
            c.clone(),
            self.terminators.clone(),
            self.key.clone(),
            self.authorizer.clone(),
        )?;
        wg.wait();
        if actor.capabilities().contains(&extras::CAPABILITY_ID.into()) {
            // force a binding so that there's a private actor subject on the bus for the
            // actor to communicate with the extras provider
            self.set_binding(
                &actor.public_key(),
                extras::CAPABILITY_ID,
                None,
                HashMap::new(),
            )?;
        }

        Ok(())
    }

    /// Adds an actor to the host by looking it up in a Gantry repository, downloading
    /// the signed module bytes, and adding them to the host. The connection to a Gantry repository
    /// will be facilitated by the Gantry client supplied by a `HostBuilder`. By default, hosts
    /// do not come configured with Gantry clients, so you will have to configure one to use this function
    #[cfg(feature = "lattice")]
    pub fn add_actor_from_gantry(&self, actor: &str, revision: u32) -> Result<()> {
        if self.gantry_client.read().unwrap().is_none() {
            return Err(errors::new(errors::ErrorKind::MiscHost(
                "No gantry client configured for this host".to_string(),
            )));
        }

        let vec =
            bus::lattice::actor_bytes_from_gantry(actor, self.gantry_client.clone(), revision)?;

        self.add_actor(Actor::from_slice(&vec)?)?;
        Ok(())
    }

    /// Adds a portable capability provider (e.g. a WASI actor) to the waSCC host. Portable capability providers adhere
    /// to the same contract as native capability providers, but they are implemented as "high-privilege WASM" modules
    /// via WASI. Today, there is very little a WASI-based capability provider can do, but in the near future when
    /// WASI gets a standardized networking stack, more providers can be written as portable modules.
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
    /// (in lattice mode, this unbinding only takes place if the actor is the last instance of its
    /// kind in the lattice)
    pub fn remove_actor(&self, pk: &str) -> Result<()> {
        self.terminators.read().unwrap()
            [&bus::actor_subject(self.ns.as_ref().map(String::as_str), pk)]
            .send(true)
            .unwrap();
        Ok(())
    }

    /// Replaces one running actor with another live actor with no message loss. Note that
    /// the time it takes to perform this replacement can cause pending messages from capability
    /// providers (e.g. messages from subscriptions or HTTP requests) to build up in a backlog,
    /// so make sure the new actor can handle this stream of these delayed messages. Also ensure that
    /// the underlying WebAssembly driver (chosen via feature flag) supports hot-swapping module bytes.
    pub fn replace_actor(&self, new_actor: Actor) -> Result<()> {
        crate::inthost::replace_actor(&self.key, self.bus.clone(), new_actor)
    }

    /// Adds a middleware item to the middleware processing pipeline
    pub fn add_middleware(&self, mid: impl Middleware) {
        self.middlewares.write().unwrap().push(Box::new(mid));
    }

    /// Adds a native capability provider plugin to the host runtime. If running in lattice mode,
    /// and at least one other instance of this same capability provider is running with previous
    /// bindings, then the provider being added to this host will automatically reconstitute
    /// the binding configuration. Note that because these capabilities are native,
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
        let subject =
            bus::provider_subject(self.ns.as_ref().map(String::as_str), capability_id, &b);
        if let Some(terminator) = self.terminators.read().unwrap().get(&subject) {
            terminator.send(true).unwrap();
            Ok(())
        } else {
            Err(errors::new(errors::ErrorKind::MiscHost(
                "No such capability".into(),
            )))
        }
    }

    /// Removes a binding between an actor and the indicated capability provider. In lattice mode,
    /// this operation has a _lattice global_ scope, and so all running instances of the indicated
    /// capability provider will be asked to dispose of any resources provisioned for the given
    /// actor.
    pub fn remove_binding(
        &self,
        actor: &str,
        capid: &str,
        binding_name: Option<String>,
    ) -> Result<()> {
        let cfg = CapabilityConfiguration {
            module: actor.to_string(),
            values: HashMap::new(),
        };
        let buf = serialize(&cfg).unwrap();
        let binding = binding_name.unwrap_or("default".to_string());
        let inv_r = self.bus.invoke(
            &self.bus.provider_subject(&capid, &binding), // The OP_REMOVE_ACTOR invocation should go to _all_ instances of the provider being unbound
            crate::inthost::gen_remove_actor(&self.key, buf.clone(), &binding, &capid),
        )?;
        if let Some(s) = inv_r.error {
            Err(format!("Failed to remove binding: {}", s).into())
        } else {
            Ok(())
        }
    }

    /// Binds an actor to a capability provider with a given configuration. If the binding name
    /// is `None` then the default binding name will be used (`default`). An actor can only have one named
    /// binding per capability provider. In lattice mode, the call to this function has a _lattice global_
    /// scope, and so all running instances of the indicated provider will be notified and provision
    /// resources accordingly. For example, if you create a binding between an actor and an HTTP server
    /// provider, and there are four instances of that provider running in the lattice, each of those
    /// four hosts will start an HTTP server on the indicated port.
    pub fn set_binding(
        &self,
        actor: &str,
        capid: &str,
        binding_name: Option<String>,
        config: HashMap<String, String>,
    ) -> Result<()> {
        #[cfg(feature = "lattice")]
        let claims = self.bus.discover_claims(actor);
        #[cfg(not(feature = "lattice"))]
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
            bus::actor_subject(self.ns.as_ref().map(String::as_str), actor)
        } else {
            bus::provider_subject(self.ns.as_ref().map(String::as_str), capid, &binding)
        };
        trace!("Binding subject: {}", tgt_subject);
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
                    #[cfg(feature = "lattice")]
                    let _ = self.bus.publish_event(BusEvent::ActorBindingCreated {
                        actor: actor.to_string(),
                        capid: capid.to_string(),
                        instance_name: binding.to_string(),
                        host: self.id(),
                    });
                    Ok(())
                }
            }
            Err(e) => Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                "Failed to configure {},{} - {}",
                binding, capid, e
            )))),
        }
    }

    /// Invoke an operation handler on an actor directly. The caller is responsible for
    /// knowing ahead of time if the given actor supports the specified operation. In lattice
    /// mode, this call will still only attempt a _local_ invocation on the host and will not
    /// make a lattice-wide call. If you want to make lattice-wide invocations, please use
    /// the lattice client library.
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
        let tgt_subject = bus::actor_subject(self.ns.as_ref().map(String::as_str), actor);
        match self.bus.invoke(&tgt_subject, inv) {
            Ok(resp) => match resp.error {
                Some(e) => Err(e.into()),
                None => Ok(resp.msg),
            },
            Err(e) => Err(e),
        }
    }

    /// Returns the full set of JWT claims for a given actor, if that actor is running in the host. This
    /// call will not query other hosts in the lattice if lattice mode is enabled.
    pub fn claims_for_actor(&self, pk: &str) -> Option<Claims<wascap::jwt::Actor>> {
        let c = self.claims.read().unwrap().get(pk).cloned();

        c
    }

    /// Applies a manifest JSON or YAML file to set up a host's actors, capability providers,
    /// and actor bindings
    #[cfg(feature = "manifest")]
    pub fn apply_manifest(&self, manifest: HostManifest) -> Result<()> {
        {
            let mut labels = self.labels.write().unwrap();
            for (label, label_value) in manifest.labels {
                if !RESTRICTED_LABELS.contains(&label.as_ref()) {
                    labels.insert(label.to_string(), label_value.to_string());
                }
            }
        }
        for actor in manifest.actors {
            #[cfg(feature = "lattice")]
            self.add_actor_gantry_first(&actor)?;

            #[cfg(not(feature = "lattice"))]
            self.add_actor(Actor::from_file(&actor)?)?;
        }
        for cap in manifest.capabilities {
            // for now, supports only file paths
            self.add_native_capability(NativeCapability::from_file(cap.path, cap.binding_name)?)?;
        }
        for config in manifest.bindings {
            self.set_binding(
                &config.actor,
                &config.capability,
                config.binding,
                config.values.unwrap_or(HashMap::new()),
            )?;
        }
        Ok(())
    }

    #[cfg(feature = "lattice")]
    fn add_actor_gantry_first(&self, actor: &str) -> Result<()> {
        if actor.len() == 56 && actor.starts_with('M') {
            // This is an actor's public subject
            self.add_actor_from_gantry(actor, 0) // TODO: do not default to the latest revision when using manifest file
        } else {
            self.add_actor(Actor::from_file(&actor)?)
        }
    }

    /// Returns the list of actors registered in the host. Even if lattice mode is enabled, this function
    /// will only return the list of actors in this specific host
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
    /// supplied parameter. This function will not make a lattice-wide tag query
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
        {
            let lock = self.claims.read().unwrap();
            let actors: Vec<_> = lock.values().collect();
            for claims in actors {
                self.remove_actor(&claims.subject)?;
            }
        }
        let caps = self.capabilities();
        for (binding_name, capid) in caps.keys() {
            self.remove_native_capability(&capid, Some(binding_name.to_string()))?;
        }
        self.bus.disconnect();
        Ok(())
    }

    /// Returns the public key of the host
    pub fn id(&self) -> String {
        self.key.public_key()
    }
}
