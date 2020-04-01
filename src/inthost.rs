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
    core::{CapabilityConfiguration, OP_CONFIGURE, OP_PERFORM_LIVE_UPDATE, OP_REMOVE_ACTOR},
    serialize,
};
pub(crate) const ACTOR_BINDING: &str = "__actor__";

lazy_static! {
    pub(crate) static ref CAPS: RwLock<Vec<CapabilitySummary>> = { RwLock::new(Vec::new()) };
}

#[cfg(feature = "gantry")]
lazy_static! {
    pub(crate) static ref GANTRYCLIENT: RwLock<gantryclient::Client> =
        RwLock::new(gantryclient::Client::default());
}

impl WasccHost {
    /// Spawns a new thread, inside which we create an instance of the wasm module interpreter. This function
    /// handles incoming calls _targeted at_ either an actor module or a portable capability provider (both are wasm).
    pub(crate) fn spawn_actor_and_listen(
        &self,
        wg: WaitGroup,
        claims: Claims<wascap::jwt::Actor>,
        buf: Vec<u8>,
        wasi: Option<WasiParams>,
        actor: bool,
        binding: Option<&str>
    ) -> Result<()> {        

        let binding = binding.unwrap_or("default").to_string();

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
                router::register_route(Some(ACTOR_BINDING), &route_key, inv_s, resp_r, term_s);
            } else {
                router::register_route(Some(&binding), &route_key, inv_s, resp_r, term_s);
            }
            
            drop(wg);

            loop {
                select! {
                    recv(inv_r) -> inv => {
                        if let Ok(inv) = inv {
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
                        deconfigure_actor(&route_key);
                        authz::unregister_claims(guest.id());
                        break;
                    }
                }
            }
        });
        Ok(())
    }

    pub(crate) fn ensure_extras(&self) -> Result<()> {
        if crate::router::ROUTER
            .read()
            .unwrap()
            .get_route(None, "wascc:extras")
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
                .add_route(Some(&binding), &capid, inv_s, resp_r, term_s);

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
                                        match get_route(Some(ACTOR_BINDING), &tgt_actor) {
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
                        remove_route(Some(&binding), &capid);
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

pub(crate) fn replace_actor(new_actor: Actor) -> Result<()> {
    let public_key = new_actor.token.claims.subject;

    match ROUTER
        .read()
        .unwrap()
        .get_route(Some(ACTOR_BINDING), &public_key)
    {
        Some(ref entry) => {
            match entry.invoke(gen_liveupdate_invocation(&public_key, new_actor.bytes)) {
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

fn deconfigure_actor(key: &str) {
    let cfg = CapabilityConfiguration {
        module: key.to_string(),
        values: HashMap::new(),
    };
    let buf = serialize(&cfg).unwrap();
    crate::router::ROUTER
        .read()
        .unwrap()
        .all_capabilities()
        .iter()
        .for_each(|(target, entry)| {
            let inv = Invocation {
                origin: "system".to_string(),
                msg: buf.clone(),
                operation: OP_REMOVE_ACTOR.to_string(),
                target: target.clone(),
            };
            entry.inv_s.send(inv).unwrap();
            let _res = entry.resp_r.recv().unwrap();
        });
}

/// An immutable representation of an invocation within waSCC
#[derive(Debug, Clone)]
pub struct Invocation {
    pub origin: String,
    pub target: InvocationTarget,
    pub operation: String,
    pub msg: Vec<u8>,
}

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
/// The binding name uniquely identifies a capability provider
/// The namespace indicates the capability provider ID
/// the operation is the name of the operation to perform on the provider
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
            match get_route(Some(ACTOR_BINDING), &subject) {
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
    let target = if ns.len() == 56 && ns.starts_with("M") {
        InvocationTarget::Actor(ns.to_string())
    } else {
        InvocationTarget::Capability {
            binding: bd.to_string(),
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
    binding: Option<&str>,
    values: HashMap<String, String>,
) -> Invocation {
    let cfgvals = CapabilityConfiguration {
        module: actor.to_string(),
        values,
    };
    let payload = serialize(&cfgvals).unwrap();
    Invocation {
        msg: payload,
        operation: OP_CONFIGURE.to_string(),
        origin: "system".to_string(),
        target: InvocationTarget::Capability {
            capid: capid.to_string(),
            binding: binding.unwrap_or("default").to_string(),
        },
    }
}
