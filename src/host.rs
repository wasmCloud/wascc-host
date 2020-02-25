//! The main interface for managing a waSCC host

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

use super::router::Router;
use super::Result;
use crate::actor::Actor;
use crate::authz;
use crate::capability::{CapabilitySummary, NativeCapability};
use crate::dispatch::WasccNativeDispatcher;
use crate::errors;
use crate::middleware;
use crate::middleware::Middleware;
use crate::plugins::PLUGMAN;
use crate::{manifest::HostManifest, router::InvokerPair};
use crossbeam::{Receiver, Sender};
use crossbeam_channel as channel;
use crossbeam_utils::sync::WaitGroup;
use prost::Message;
use std::collections::HashMap;
use std::sync::RwLock;
use std::thread;
use wapc::prelude::*;
use wascap::jwt::Claims;
use wascc_codec::core::{
    CapabilityConfiguration, OP_CONFIGURE, OP_PERFORM_LIVE_UPDATE, OP_REMOVE_ACTOR,
};

pub use authz::set_auth_hook;

lazy_static! {
    pub(crate) static ref CAPS: RwLock<Vec<CapabilitySummary>> = { RwLock::new(Vec::new()) };
    pub(crate) static ref ROUTER: RwLock<Router> = { RwLock::new(Router::default()) };
    pub(crate) static ref TERMINATORS: RwLock<HashMap<String, Sender<bool>>> =
        { RwLock::new(HashMap::new()) };
}

/// Adds a middleware trait object to the middleware processing pipeline.
pub fn add_middleware(mid: impl Middleware) {
    middleware::MIDDLEWARES.write().unwrap().push(Box::new(mid));
}

/// Adds a portable capability provider wasm module to the runtime host. The identity of this provider
/// will be determined by examining the capability attestation in this actor's embedded token.
pub fn add_capability(actor: Actor, wasi: WasiParams) -> Result<()> {
    let wg = WaitGroup::new();
    spawn_actor_and_listen(
        wg.clone(),
        actor.token.claims,
        actor.bytes.clone(),
        Some(wasi),
        false,
    )?;
    wg.wait();
    Ok(())
}

/// Manually dispatches an operation and an opaque binary payload to an actor as if it
/// came from a capability. The origin of this message will be `system`. Use this function
/// to perform integration or acceptance tests on actors within a host, or if your host has
/// special invocations that it knows it must call on actors.
pub fn call(actor: &str, op: &str, msg: &[u8]) -> Result<Vec<u8>> {
    if actor.contains(':') {
        return Err(errors::new(errors::ErrorKind::MiscHost(
            "Cannot invoke capability providers through host instead of from an actor".into(),
        )));
    }
    let router = ROUTER.read().unwrap();
    let pair = router.get_pair(actor);
    match pair {
        Some(ref p) => match invoke(p, "system".to_string(), op, msg) {
            Ok(resp) => Ok(resp.msg),
            Err(e) => Err(e),
        },
        None => Err(errors::new(errors::ErrorKind::MiscHost(
            "No such actor".into(),
        ))),
    }
}

/// Removes a portable capability provider from the host.
pub fn remove_capability(cap_id: &str) -> Result<()> {
    if let Some(term_s) = TERMINATORS.read().unwrap().get(cap_id) {
        term_s.send(true).unwrap();
        Ok(())
    } else {
        Err(errors::new(errors::ErrorKind::MiscHost(
            "No such capability".into(),
        )))
    }
}

/// Adds an actor module to the runtime host. The identity of this module is determined
/// by inspecting the claims embedded in the module's custom section as a JWT. The identity
/// comes from the `subject` field on the embedded claims and is the primary key of the
/// module identity.
pub fn add_actor(actor: Actor) -> Result<()> {
    ensure_extras()?;
    let wg = WaitGroup::new();
    info!("Adding actor {} to host", actor.public_key());
    spawn_actor_and_listen(
        wg.clone(),
        actor.token.claims,
        actor.bytes.clone(),
        None,
        true,
    )?;
    wg.wait();
    Ok(())
}

/// Applies a manifest containing actors, capabilities, and actor-capability configuration
/// entries. This provides a useful shortcut for provisioning an entire host.
pub fn apply_manifest(manifest: HostManifest) -> Result<()> {
    for actor in manifest.actors {
        // for now, only support file paths
        // TODO: support public keys and fetch from gantry        
        add_actor(Actor::from_file(actor)?)?;
    }
    for cap in manifest.capabilities {
        // for now, supports only file paths        
        add_native_capability(NativeCapability::from_file(cap)?)?;
    }
    for config in manifest.config {
        configure(&config.actor, &config.capability,
        config.values)?;
    }
    Ok(())
}

fn ensure_extras() -> Result<()> {
    if ROUTER.read().unwrap().get_pair("wascc:extras").is_some() {
        return Ok(());
    }
    add_native_capability(NativeCapability::from_instance(
        crate::extras::ExtrasCapabilityProvider::default(),
    )?)?;
    Ok(())
}

/// Replaces one running actor with another live actor with no message loss. Note that
/// the time it takes to perform this replacement can cause pending messages from capability
/// providers (e.g. messages from subscriptions or HTTP requests) to build up in a backlog,
/// so make sure the new actor can handle this stream of these delayed messages
pub fn replace_actor(new_actor: Actor) -> Result<()> {
    let public_key = new_actor.token.claims.subject;

    match ROUTER.read().unwrap().get_pair(&public_key) {
        Some(ref p) => {
            match invoke(
                p,
                "system".into(),
                &format!("{}!{}", public_key, OP_PERFORM_LIVE_UPDATE),
                &new_actor.bytes,
            ) {
                Ok(_) => {
                    info!("Actor {} replaced", public_key);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        None => Err(errors::new(errors::ErrorKind::MiscHost(
            "Cannot replace non-existent actor".into(),
        ))),
    }
}

/// Removes an actor from the host. Stops the thread managing the actor and notifies
/// all capability providers to free up any associated resources being used by the actor. Because
/// this removal is _asynchronous_, the `actors` function might not immediately report
/// the change.
pub fn remove_actor(pk: &str) -> Result<()> {
    if let Some(term_s) = TERMINATORS.read().unwrap().get(pk) {
        term_s.send(true).unwrap();
        Ok(())
    } else {
        Err(errors::new(errors::ErrorKind::MiscHost(
            "No such actor".into(),
        )))
    }
}

/// Retrieves the list of all actors running in the host, returning a tuple of each
/// actor's primary key and that actor's security claims. The order in which the actors
/// appear in the resulting vector is _not guaranteed_. **NOTE** - Because actors are added and
/// removed asynchronously, this function returns a view of the actors only as seen at
/// the moment the function is invoked.
pub fn actors() -> Vec<(String, Claims<wascap::jwt::Actor>)> {
    authz::get_all_claims()
}

/// Retrieves a list of all of the currently registered capability ID
pub fn capabilities() -> Vec<CapabilitySummary> {
    let lock = CAPS.read().unwrap();
    lock.iter().cloned().collect()
}

/// Retrieves the security claims for a given actor. Returns `None` if that actor is
/// not running in the host. Actors are added and removed asynchronously, and actors are
/// not visible until fully running, so if you attempt to query an actor's claims
/// immediately after calling `add_actor`, this function might (correctly) return `None`
pub fn actor_claims(pk: &str) -> Option<Claims<wascap::jwt::Actor>> {
    authz::get_claims(pk)
}

/// Adds a native linux dynamic library (plugin) as a capability provider to the runtime host. The
/// identity and other metadata about this provider is determined by loading the plugin from disk
/// and invoking the appropriate plugin trait methods.
pub fn add_native_capability(capability: NativeCapability) -> Result<()> {
    let capid = capability.capid.clone();
    if ROUTER.read().unwrap().get_pair(&capid).is_some() {
        return Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
            "Attempt to register the same capability provider multiple times: {}",
            capid
        ))));
    }
    let summary = CapabilitySummary {
        id: capid.clone(),
        name: capability.name(),
        portable: false,
    };
    CAPS.write().unwrap().push(summary);
    crate::plugins::PLUGMAN
        .write()
        .unwrap()
        .add_plugin(capability)?;
    let wg = WaitGroup::new();
    spawn_capability_provider_and_listen(wg.clone(), &capid)?;
    wg.wait();
    Ok(())
}

/// Removes a native capability provider from the host.
pub fn remove_native_capability(capid: &str) -> Result<()> {
    if let Some(term_s) = TERMINATORS.read().unwrap().get(capid) {
        term_s.send(true).unwrap();
        Ok(())
    } else {
        Err(errors::new(errors::ErrorKind::MiscHost(
            "No such capability".into(),
        )))
    }
}

fn remove_cap(capid: &str) -> Result<()> {
    CAPS.write().unwrap().retain(|c| c.id != capid);
    Ok(())
}

/// Supply a set of key-value pairs for a given actor to a capability provider. This allows
/// the capability provider to set actor-specific data like an HTTP server port or a set of
/// subscriptions, etc.
pub fn configure(module: &str, capid: &str, config: HashMap<String, String>) -> Result<()> {
    if !authz::can_invoke(module, capid) {
        return Err(errors::new(errors::ErrorKind::Authorization(format!(
            "Actor {} is not authorized to use capability {}, configuration rejected",
            module, capid
        ))));
    }
    info!(
        "Attempting to configure actor {} for capability {}",
        module, capid
    );
    let capid = capid.to_string();
    let module = module.to_string();
    let pair = ROUTER.read().unwrap().get_pair(&capid);
    match pair {
        Some(pair) => {
            trace!("Sending configuration to {}", capid);
            let res = invoke(
                &pair,
                "system".to_string(),
                &format!("{}!{}", capid, OP_CONFIGURE),
                &gen_config_proto(&module, config),
            )?;
            if let Some(e) = res.error {
                Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                    "Failed to configure {} - {}",
                    capid, e
                ))))
            } else {
                Ok(())
            }
        }
        None => Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
            "No such capability provider: {}",
            capid
        )))),
    }
}

/// Creates a dispatcher and gives it to a native plugin, allowing that plugin to then
/// perform invocations on an actor module via the channels inside the dispatcher. Invocations
/// pulled off the channel are then invoked by looking up the target capability ID on the
/// router and invoking via the channels from the router.
fn spawn_capability_provider_and_listen(wg: WaitGroup, capid: &str) -> Result<()> {
    let capid = capid.to_string();

    thread::spawn(move || {
        let (inv_s, inv_r): (Sender<Invocation>, Receiver<Invocation>) = channel::unbounded();
        let (resp_s, resp_r): (Sender<InvocationResponse>, Receiver<InvocationResponse>) =
            channel::unbounded();
        let (term_s, term_r): (Sender<bool>, Receiver<bool>) = channel::unbounded();
        let dispatcher = WasccNativeDispatcher::new(resp_r.clone(), inv_s.clone(), &capid);
        crate::plugins::PLUGMAN
            .write()
            .unwrap()
            .register_dispatcher(&capid, dispatcher)
            .unwrap();

        ROUTER
            .write()
            .unwrap()
            .add_route(capid.to_string(), inv_s, resp_r);
        TERMINATORS.write().unwrap().insert(capid.clone(), term_s);

        info!("Native capability provider '{}' ready", capid);
        drop(wg);

        loop {
            select! {
                recv(inv_r) -> inv => {
                    if let Ok(inv) = inv {
                        let v: Vec<_> = inv.operation.split('!').collect();
                        let target = v[0];
                        info!(
                            "Capability {} received invocation for target {}",
                            capid, target
                        );

                        let inv_r = if target == capid {
                            // if target of invocation is this particular capability,
                            // then perform the invocation on the plugin
                            middleware::invoke_capability(inv).unwrap()
                        } else {
                            // Capability is handling a dispatch (delivering) to actor module
                            if !authz::can_invoke(target, &capid) {
                                InvocationResponse::error(&format!(
                                    "Dispatch between actor and unauthorized capability: {} <-> {}",
                                    target, capid
                                ))
                            } else {
                                let pair = ROUTER.read().unwrap().get_pair(target);
                                match pair {
                                    Some(ref p) => {
                                        invoke(p, capid.clone(), &inv.operation, &inv.msg).unwrap()
                                    }
                                    None => InvocationResponse::error("Dispatch to unknown actor"),
                                }
                            }
                        };
                        resp_s.send(inv_r).unwrap();
                    }
                },
                recv(term_r) -> _term => {
                    info!("Terminating native capability provider {}", capid);
                    remove_cap(&capid).unwrap();
                    PLUGMAN.write().unwrap().remove_plugin(&capid).unwrap();
                    TERMINATORS.write().unwrap().remove(&capid);
                    ROUTER.write().unwrap().remove_route(&capid).unwrap();
                    break;
                }
            }
        }
    });

    Ok(())
}

/// Spawns a new thread, inside which we create an instance of the wasm module interpreter. This function
/// handles incoming calls _targeted at_ either an actor module or a portable capability provider (both are wasm).
fn spawn_actor_and_listen(
    wg: WaitGroup,
    claims: Claims<wascap::jwt::Actor>,
    buf: Vec<u8>,
    wasi: Option<WasiParams>,
    actor: bool,
) -> Result<()> {
    thread::spawn(move || {
        info!(
            "Loading {} module...",
            if actor { "actor" } else { "capability" }
        );
        let mut guest = WapcHost::new(host_callback, &buf, wasi).unwrap();
        authz::store_claims(claims.clone()).unwrap();
        authz::map_claims(guest.id(), &claims.subject);
        let (inv_s, inv_r): (Sender<Invocation>, Receiver<Invocation>) = channel::unbounded();
        let (resp_s, resp_r): (Sender<InvocationResponse>, Receiver<InvocationResponse>) =
            channel::unbounded();
        let (term_s, term_r): (Sender<bool>, Receiver<bool>) = channel::unbounded();

        let route_key = {
            let route_key = if actor {
                claims.subject.clone()
            } else {
                claims.metadata.unwrap().caps.unwrap()[0].to_string() // If we can't unwrap this, the cap is bad, so panic is fine
            };
            ROUTER
                .write()
                .unwrap()
                .add_route(route_key.clone(), inv_s, resp_r);
            TERMINATORS
                .write()
                .unwrap()
                .insert(route_key.clone(), term_s);
            if !actor {
                CAPS.write().unwrap().push(CapabilitySummary {
                    id: route_key.clone(),
                    name: route_key.clone(),
                    portable: true,
                });
            }
            info!(
                "{} {} ready for communications",
                if actor {
                    "Actor"
                } else {
                    "Portable capability"
                },
                route_key
            );
            route_key
        };
        drop(wg); // API call that spawned this thread can now unblock

        loop {
            select! {
                recv(inv_r) -> inv => {
                    if let Ok(inv) = inv {
                        let v: Vec<_> = inv.operation.split('!').collect();
                        let inv = Invocation::new(inv.origin, v[1], inv.msg); // Remove routing prefix from operation
                        if inv.operation == OP_PERFORM_LIVE_UPDATE {
                            resp_s.send(live_update(&mut guest, &inv)).unwrap();
                            continue;
                        }
                        let inv_r = middleware::invoke_actor(inv, &mut guest).unwrap();
                        resp_s.send(inv_r).unwrap();
                    }

                },
                recv(term_r) -> _term => {
                    info!("Terminating {} {}", if actor { "actor" } else { "capability" }, route_key);
                    if actor {
                        deconfigure_actor(&route_key);
                    } else {
                        remove_cap(&route_key).unwrap();
                    }
                    authz::remove_claims(&claims.subject).unwrap();
                    TERMINATORS.write().unwrap().remove(&route_key);
                    ROUTER.write().unwrap().remove_route(&route_key).unwrap();
                    break;
                }
            }
        }
    });

    Ok(())
}

fn live_update(guest: &mut WapcHost, inv: &Invocation) -> InvocationResponse {
    match guest.replace_module(&inv.msg) {
        Ok(_) => InvocationResponse::success(vec![]),
        Err(e) => {
            error!("Failed to perform hot swap, ignoring message: {}", e);
            InvocationResponse::error("Failed to perform hot swap")
        }
    }
}

fn deconfigure_actor(key: &str) {
    let cfg = CapabilityConfiguration {
        module: key.to_string(),
        values: HashMap::new(),
    };
    let mut buf = Vec::new();
    cfg.encode(&mut buf).unwrap();
    ROUTER
        .read()
        .unwrap()
        .all_capabilities()
        .iter()
        .for_each(|(capid, (sender, receiver))| {
            let inv = Invocation {
                origin: "system".to_string(),
                msg: buf.clone(),
                operation: format!("{}!{}", capid, OP_REMOVE_ACTOR),
            };
            sender.send(inv).unwrap();
            let _res = receiver.recv().unwrap();
        });
}

/// This function is called by the underlying waPC host in response to a guest module
/// invoking a host import according to the waPC protobuf-RPC spec. The operation
/// is assumed to be a string in the form [capability_id]![operation] where `capability_id` is a
/// namespace-delimited string like `wapc:messaging` or `wapc:keyvalue`.
fn host_callback(
    id: u64,
    op: &str,
    payload: &[u8],
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    trace!("Guest {} invoking {}", id, op);
    let v: Vec<_> = op.split('!').collect();
    let capability_id = v[0];
    if !authz::can_id_invoke(id, capability_id) {
        return Err(Box::new(errors::new(errors::ErrorKind::Authorization(
            format!(
                "Actor {} does not have permission to communicate with {}",
                id, capability_id
            ),
        ))));
    }
    if capability_id.len() == 56 && capability_id.starts_with("M") {
        // This is an actor-to-actor call
        if let Some(pair) = ROUTER.read().unwrap().get_pair(capability_id) {
            match invoke(&pair, authz::pk_for_id(id), op, &payload.to_vec()) {
                Ok(ir) => {
                    if ir.error.is_some() {
                        Err(ir.error.unwrap().into())
                    } else {
                        Ok(ir.msg)
                    }
                }
                Err(e) => Err(Box::new(e)),
            }
        } else {
            Err("Attempted actor-to-actor call to non-existent target".into())
        }
    } else {
        let inv = Invocation::new(authz::pk_for_id(id), op, payload.to_vec());
        match middleware::invoke_capability(inv) {
            Ok(inv_r) => Ok(inv_r.msg),
            Err(e) => Err(Box::new(errors::new(errors::ErrorKind::HostCallFailure(
                e.into(),
            )))),
        }
    }
}

/// Send a request on the invoker channel and await a reply on the response channel
fn invoke(
    pair: &InvokerPair,
    origin: String,
    op: &str,
    payload: &[u8],
) -> Result<InvocationResponse> {
    trace!("invoking: {} from {}", op, origin);
    let (inv_s, resp_r) = pair;

    inv_s
        .send(Invocation::new(origin, op, payload.to_vec()))
        .unwrap();
    Ok(resp_r.recv().unwrap())
}

/// Converts a hashmap into the CapabilityConfiguration protobuf object to
/// be sent to a capability provider to supply configuration for an actor
fn gen_config_proto(module: &str, values: HashMap<String, String>) -> Vec<u8> {
    let mut buf = Vec::new();
    let cfgvals = CapabilityConfiguration {
        module: module.to_string(),
        values,
    };
    cfgvals.encode(&mut buf).unwrap();
    buf
}

/// An immutable representation of an invocation within waSCC
#[derive(Debug, Clone)]
pub struct Invocation {
    pub origin: String,
    pub operation: String,
    pub msg: Vec<u8>,
}

impl Invocation {
    pub fn new(origin: String, op: &str, msg: Vec<u8>) -> Invocation {
        Invocation {
            origin,
            operation: op.to_string(),
            msg,
        }
    }
}

/// The response to an invocation
#[derive(Debug, Clone)]
pub struct InvocationResponse {
    pub msg: Vec<u8>,
    pub error: Option<String>,
}

impl InvocationResponse {
    pub fn success(msg: Vec<u8>) -> InvocationResponse {
        InvocationResponse { msg, error: None }
    }

    pub fn error(err: &str) -> InvocationResponse {
        InvocationResponse {
            msg: Vec::new(),
            error: Some(err.to_string()),
        }
    }
}
