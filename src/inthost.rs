// Implementations of support functions for the `WasccHost` struct

use super::WasccHost;
use crate::Result;

use crate::plugins::remove_plugin;
use crate::router::{get_route, remove_route, ROUTER};
use crate::{authz, errors, middleware, router, Actor, NativeCapability};
use crate::{dispatch::WasccNativeDispatcher, CapabilitySummary};
use crossbeam::{Receiver, Sender};
use crossbeam_channel as channel;
use crossbeam_utils::sync::WaitGroup;
use std::collections::HashMap;
use std::sync::RwLock;
use std::thread;
use wapc::prelude::*;
use wascap::jwt::Claims;
use wascc_codec::{
    core::{CapabilityConfiguration, OP_BIND_ACTOR, OP_PERFORM_LIVE_UPDATE, OP_REMOVE_ACTOR},
    serialize,
};
pub(crate) const ACTOR_BINDING: &str = "__actor__"; // A marker namespace for looking up the route to an actor's dispatcher

lazy_static! {
    pub(crate) static ref CAPS: RwLock<Vec<CapabilitySummary>> = { RwLock::new(Vec::new()) };
    //                                          actor,  capid, binding
    pub(crate) static ref BINDINGS: RwLock<Vec<(String, String, String)>> = { RwLock::new(Vec::new()) };
}

#[cfg(feature = "gantry")]
lazy_static! {
    pub(crate) static ref GANTRYCLIENT: RwLock<gantryclient::Client> =
        RwLock::new(gantryclient::Client::default());
}

impl WasccHost {
    /// Spawns a new thread, inside which we create an instance of the wasm module interpreter. This function
    /// handles incoming calls _targeted at_ either an actor module or a portable capability provider (both are wapc hosts under the hood).
    pub(crate) fn spawn_actor_and_listen(
        &self,
        wg: WaitGroup,
        claims: Claims<wascap::jwt::Actor>,
        buf: Vec<u8>,
        wasi: Option<WasiParams>,
        actor: bool,
        binding: String,
    ) -> Result<()> {
        thread::spawn(move || {
            info!(
                "Loading {} module...",
                if actor { "actor" } else { "capability" }
            );
            let mut guest = WapcHost::new(host_callback, &buf, wasi).unwrap();
            authz::register_claims(guest.id(), claims.clone());
            let (inv_s, inv_r): (Sender<Invocation>, Receiver<Invocation>) = channel::unbounded();
            let (resp_s, resp_r): (Sender<InvocationResponse>, Receiver<InvocationResponse>) =
                channel::unbounded();
            let (term_s, term_r): (Sender<bool>, Receiver<bool>) = channel::unbounded();
            let route_key = route_key(&claims);
            if actor {
                router::register_route(ACTOR_BINDING, &route_key, inv_s, resp_r, term_s);
            } else {
                router::register_route(&binding, &route_key, inv_s, resp_r, term_s);
            }

            drop(wg); // Let the WasccHost wrapper function return

            loop {
                select! {
                    recv(inv_r) -> inv => {
                        if let Ok(inv) = inv {
                            if inv.operation == OP_PERFORM_LIVE_UPDATE { // Preempt the middleware chain if the operation is a live update
                                resp_s.send(live_update(&mut guest, &inv)).unwrap();
                                continue;
                            }
                            let inv_r = middleware::invoke_actor(inv, &mut guest).unwrap();
                            resp_s.send(inv_r).unwrap();
                        }
                    },
                    recv(term_r) -> _term => {
                        info!("Terminating {} {}", if actor { "actor" } else { "capability" }, route_key);
                        deconfigure_actor(&route_key);
                        authz::unregister_claims(guest.id());
                        break;
                    }
                }
            }
        });
        Ok(())
    }

    pub(crate) fn record_binding(&self, actor: &str, capid: &str, binding: &str) -> Result<()> {
        let mut lock = BINDINGS.write().unwrap();
        lock.push((actor.to_string(), capid.to_string(), binding.to_string()));
        trace!(
            "Actor {} successfully bound to {},{}",
            actor,
            binding,
            capid
        );
        trace!("{}", lock.len());
        Ok(())
    }

    pub(crate) fn ensure_extras(&self) -> Result<()> {
        if crate::router::ROUTER
            .read()
            .unwrap()
            .get_route("default", "wascc:extras")
            .is_some()
        {
            return Ok(());
        }
        self.add_native_capability(NativeCapability::from_instance(
            crate::extras::ExtrasCapabilityProvider::default(),
            None,
        )?)?;
        Ok(())
    }

    /// Spawns a thread that listens on a channel for messages coming out of a capability provider
    /// and messages going into the capability provider
    pub(crate) fn spawn_capability_provider_and_listen(
        &self,
        capability: NativeCapability,
        summary: CapabilitySummary,
        wg: WaitGroup,
    ) -> Result<()> {
        CAPS.write().unwrap().push(summary);

        let capid = capability.id().to_string();
        let binding = capability.binding_name.to_string();

        crate::plugins::PLUGMAN
            .write()
            .unwrap()
            .add_plugin(capability)?;

        thread::spawn(move || {
            let (inv_s, inv_r): (Sender<Invocation>, Receiver<Invocation>) = channel::unbounded();
            let (resp_s, resp_r): (Sender<InvocationResponse>, Receiver<InvocationResponse>) =
                channel::unbounded();
            let (term_s, term_r): (Sender<bool>, Receiver<bool>) = channel::unbounded();
            let dispatcher = WasccNativeDispatcher::new(resp_r.clone(), inv_s.clone(), &capid);
            crate::plugins::PLUGMAN
                .write()
                .unwrap()
                .register_dispatcher(&binding, &capid, dispatcher)
                .unwrap();

            ROUTER
                .write()
                .unwrap()
                .add_route(&binding, &capid, inv_s, resp_r, term_s);

            info!("Native capability provider '({},{})' ready", binding, capid);
            drop(wg);

            loop {
                select! {
                    recv(inv_r) -> inv => {
                        if let Ok(ref inv) = inv {
                            let inv_r = match &inv.target {
                                InvocationTarget::Capability{capid: _tgt_capid, binding: _tgt_binding} => {
                                    // Run invocation through middleware, which will terminate at a plugin invocation
                                    middleware::invoke_capability(inv.clone()).unwrap()
                                },
                                InvocationTarget::Actor(tgt_actor) => {
                                    if !authz::can_invoke(&tgt_actor, &capid) {
                                        InvocationResponse::error(&format!(
                                            "Dispatch between actor and unauthorized capability: {} <-> {}",
                                            tgt_actor, capid
                                        ))
                                    } else {
                                        match get_route(ACTOR_BINDING, &tgt_actor) {
                                            Some(ref entry) => {
                                                match entry.invoke(inv.clone()) {
                                                    Ok(ir) => ir,
                                                    Err(e) => InvocationResponse::error(&format!(
                                                        "Capability to actor call failure: {}", e
                                                    ))
                                                }
                                            }
                                            None => InvocationResponse::error("Dispatch to unknown actor"),
                                        }
                                    }
                                }
                            };
                            resp_s.send(inv_r).unwrap();
                        }
                    },
                    recv(term_r) -> _term => {
                        info!("Terminating native capability provider {},{}", binding, capid);
                        remove_cap(&capid);
                        remove_route(&binding, &capid);
                        remove_plugin(&binding, &capid).unwrap();
                        break;
                    }
                }
            }
        });

        Ok(())
    }
}

pub(crate) fn remove_cap(capid: &str) {
    CAPS.write().unwrap().retain(|c| c.id != capid);
}

/// Puts a "live update" message into the dispatch queue, which will be handled
/// as soon as it is pulled off the channel for the target actor
pub(crate) fn replace_actor(new_actor: Actor) -> Result<()> {
    let public_key = new_actor.token.claims.subject;

    match ROUTER.read().unwrap().get_route(ACTOR_BINDING, &public_key) {
        Some(ref entry) => {
            match entry.invoke(gen_liveupdate_invocation(&public_key, new_actor.bytes)) {
                Ok(_) => {
                    info!("Actor {} replaced", public_key);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        }
        None => Err(errors::new(errors::ErrorKind::MiscHost(format!(
            "Cannot replace non-existent actor: {}",
            public_key
        )))),
    }
}

/// Produces the identifier used for a binding's route key for an actor.
/// Since actors can be either regular actor modules or providers, we
/// either return the actor's subject/pk or the declared capability from the
/// actor's signed metadata.
fn route_key(claims: &Claims<wascap::jwt::Actor>) -> String {
    let meta = claims.metadata.clone().unwrap();
    if meta.provider {
        // This is a portable capability provider, use the capability ID
        meta.caps.unwrap()[0].to_string()
    } else {
        claims.subject.to_string()
    }
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

fn gen_liveupdate_invocation(target: &str, bytes: Vec<u8>) -> Invocation {
    Invocation {
        msg: bytes,
        operation: OP_PERFORM_LIVE_UPDATE.to_string(),
        origin: "system".to_string(),
        target: InvocationTarget::Actor(target.to_string()),
    }
}

/// Removes all bindings for a given actor by sending the "deconfigure" message
/// to each of the capabilities
fn deconfigure_actor(key: &str) {
    let cfg = CapabilityConfiguration {
        module: key.to_string(),
        values: HashMap::new(),
    };
    let buf = serialize(&cfg).unwrap();
    let bindings: Vec<_> = {
        let lock = BINDINGS.read().unwrap();
        lock.iter()
            .filter(|(a, _cap, _bind)| a == key)
            .cloned()
            .collect()
    };

    // (actor, capid, binding)
    for (_actor, capid, binding) in bindings {
        if let Some(route) = crate::router::ROUTER
            .read()
            .unwrap()
            .get_route(&binding, &capid)
        {
            info!("Unbinding actor {} from {},{}", key, binding, capid);
            let _ = route.invoke(gen_remove_actor(buf.clone(), &binding, &capid));
            remove_binding(key, &binding, &capid);
        } else {
            trace!(
                "No route for {},{} - skipping remove invocation",
                binding,
                capid
            );
        }
    }
}

fn remove_binding(actor: &str, binding: &str, capid: &str) {
    // binding: (actor,  capid, binding)
    let mut lock = BINDINGS.write().unwrap();
    lock.retain(|(a, c, b)| !(a == actor && b == binding && c == capid));
}

fn gen_remove_actor(msg: Vec<u8>, binding: &str, capid: &str) -> Invocation {
    Invocation {
        origin: "system".to_string(),
        msg,
        operation: OP_REMOVE_ACTOR.to_string(),
        target: InvocationTarget::Capability {
            capid: capid.to_string(),
            binding: binding.to_string(),
        },
    }
}
/// An immutable representation of an invocation within waSCC
#[derive(Debug, Clone)]
pub struct Invocation {
    pub origin: String,
    pub target: InvocationTarget,
    pub operation: String,
    pub msg: Vec<u8>,
}

/// Represents an invocation target - either an actor or a bound capability provider
#[derive(Debug, Clone, PartialEq)]
pub enum InvocationTarget {
    Actor(String),
    Capability { capid: String, binding: String },
}

impl Invocation {
    pub fn new(origin: String, target: InvocationTarget, op: &str, msg: Vec<u8>) -> Invocation {
        Invocation {
            origin,
            target,
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

/// This function is called by the underlying waPC host in response to a guest module
/// invoking a host import according to the waPC spec.
/// The binding name uniquely identifies a capability provider (e.g. "default" or "userprofile" or "cache" etc)
/// The namespace indicates the capability provider ID (e.g. "wascc:messaging" or "wascc:keyvalue")
/// The operation is the name of the operation to perform on the provider (e.g. "PerformRequest" or "DeliverMessage")
/// NOTE: if the target namespace of the invocation is an actor subject (e.g. "M...") then the invocation target will be for an actor,
/// not a capability (this is what enables actor-to-actor comms)
fn host_callback(
    id: u64,
    bd: &str,
    ns: &str,
    op: &str,
    payload: &[u8],
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    trace!("Guest {} invoking {}:{}", id, ns, op);

    let capability_id = ns;
    let inv = invocation_from_callback(id, bd, ns, op, payload);

    if !authz::can_id_invoke(id, capability_id) {
        return Err(Box::new(errors::new(errors::ErrorKind::Authorization(
            format!(
                "Actor {} does not have permission to communicate with {}",
                id, capability_id
            ),
        ))));
    }
    match &inv.target {
        InvocationTarget::Actor(subject) => {
            // This is an actor-to-actor call
            match get_route(ACTOR_BINDING, &subject) {
                Some(entry) => match entry.invoke(inv.clone()) {
                    Ok(inv_r) => Ok(inv_r.msg),
                    Err(e) => Err(Box::new(errors::new(errors::ErrorKind::HostCallFailure(
                        e.into(),
                    )))),
                },
                None => Err("Attempted actor-to-actor call to non-existent target".into()),
            }
        }
        InvocationTarget::Capability { .. } => {
            // This is a standard actor-to-host call
            match middleware::invoke_capability(inv.clone()) {
                Ok(inv_r) => Ok(inv_r.msg),
                Err(e) => Err(Box::new(errors::new(errors::ErrorKind::HostCallFailure(
                    e.into(),
                )))),
            }
        }
    }
}

fn invocation_from_callback(id: u64, bd: &str, ns: &str, op: &str, payload: &[u8]) -> Invocation {
    let binding = if bd.trim().is_empty() {
        // Some actor SDKs may not specify a binding field by default
        "default".to_string()
    } else {
        bd.to_string()
    };
    let target = if ns.len() == 56 && ns.starts_with("M") {
        InvocationTarget::Actor(ns.to_string())
    } else {
        InvocationTarget::Capability {
            binding,
            capid: ns.to_string(),
        }
    };
    Invocation {
        msg: payload.to_vec(),
        operation: op.to_string(),
        origin: authz::pk_for_id(id),
        target,
    }
}

pub(crate) fn gen_config_invocation(
    actor: &str,
    capid: &str,
    binding: String,
    values: HashMap<String, String>,
) -> Invocation {
    let cfgvals = CapabilityConfiguration {
        module: actor.to_string(),
        values,
    };
    let payload = serialize(&cfgvals).unwrap();
    Invocation {
        msg: payload,
        operation: OP_BIND_ACTOR.to_string(),
        origin: "system".to_string(),
        target: InvocationTarget::Capability {
            capid: capid.to_string(),
            binding,
        },
    }
}
