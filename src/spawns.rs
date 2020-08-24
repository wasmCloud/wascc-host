use crate::Result;

use crate::bus;
use crate::inthost::*;
use crate::BindingsList;
use crate::{authz, middleware, NativeCapability};
use crate::{
    bus::MessageBus, dispatch::WasccNativeDispatcher, plugins::PluginManager, Authorizer,
    Invocation, InvocationResponse, Middleware, RouteKey,
};
use authz::ClaimsMap;
use crossbeam::{Receiver, Sender};
use crossbeam_channel as channel;
use crossbeam_utils::sync::WaitGroup;
#[cfg(feature = "lattice")]
use latticeclient::BusEvent;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::thread;
use wapc::prelude::*;
use wascap::{jwt::Claims, prelude::KeyPair};
use wascc_codec::{
    capabilities::{CapabilityDescriptor, OP_GET_CAPABILITY_DESCRIPTOR},
    core::{CapabilityConfiguration, OP_BIND_ACTOR, OP_PERFORM_LIVE_UPDATE, OP_REMOVE_ACTOR},
    deserialize,
};

/// Spawns a new background thread in which a new `WapcHost` is created for the actor
/// module bytes. A message bus subscription is created either for the actor's RPC
/// subject OR for the capability provider's root subject. We then select between a receive
/// invocation on the subscription's channel or a receive invocation on the terminator channel,
/// which will then trigger a cleanup of the actor's resources.
pub(crate) fn spawn_actor(
    wg: WaitGroup,
    claims: Claims<wascap::jwt::Actor>,
    buf: Vec<u8>,
    wasi: Option<WasiParams>,
    actor: bool,
    binding: Option<String>,
    bus: Arc<MessageBus>,
    mids: Arc<RwLock<Vec<Box<dyn Middleware>>>>,
    caps: Arc<RwLock<HashMap<RouteKey, CapabilityDescriptor>>>,
    bindings: Arc<RwLock<BindingsList>>,
    claimsmap: ClaimsMap,
    terminators: Arc<RwLock<HashMap<String, Sender<bool>>>>,
    hk: KeyPair,
    auth: Arc<RwLock<Box<dyn Authorizer>>>,
) -> Result<()> {
    let c = claims.clone();
    let b = bus.clone();
    let m = mids.clone();
    let bi = binding.clone();
    let hostkey = hk.clone();
    let authorizer = auth.clone();
    thread::spawn(move || {
        if actor {
            #[cfg(feature = "lattice")]
            let _ = bus.publish_event(BusEvent::ActorStarting {
                host: hostkey.public_key(),
                actor: claims.subject.to_string(),
            });
        }
        let mut guest = WapcHost::new(
            move |_id, bd, ns, op, payload| {
                wapc_host_callback(
                    hk.clone(),
                    c.clone(),
                    bus.clone(),
                    bd,
                    ns,
                    op,
                    payload,
                    authorizer.clone(),
                )
            },
            &buf,
            wasi,
        )
        .unwrap();
        let mut d: Option<CapabilityDescriptor> = None;

        let subscribe_subject = if actor {
            b.actor_subject(&claims.subject)
        } else {
            d = match get_descriptor(&mut guest) {
                Ok(d) => Some(d),
                Err(_) => None,
            };
            if d.is_none() {
                return "".to_string();
            }
            let capid = d.as_ref().unwrap().id.to_string();
            let binding = binding.unwrap();
            #[cfg(feature = "lattice")]
            let _ = b.publish_event(BusEvent::ProviderLoaded {
                host: hostkey.public_key(),
                capid: capid.to_string(),
                instance_name: binding.to_string(),
            });

            b.provider_subject(&capid, binding.as_ref())
        };

        let (inv_s, inv_r): (Sender<Invocation>, Receiver<Invocation>) = channel::unbounded();
        let (resp_s, resp_r): (Sender<InvocationResponse>, Receiver<InvocationResponse>) =
            channel::unbounded();
        let (term_s, term_r): (Sender<bool>, Receiver<bool>) = channel::unbounded();

        if subscribe_subject.is_empty() {
            return "can't subscribe to message bus".to_string();
        }
        terminators
            .write()
            .unwrap()
            .insert(subscribe_subject.clone(), term_s);
        let _ = b.subscribe(&subscribe_subject, inv_s, resp_r).unwrap();
        drop(wg); // Let the Host wrapper function return
        if actor {
            #[cfg(feature = "lattice")]
            let _ = b.publish_event(BusEvent::ActorStarted {
                host: hostkey.public_key(),
                actor: claims.subject.to_string(),
            });
        }

        loop {
            let mids = m.clone();
            let binding = bi.clone();
            let bus = b.clone();
            let c = claims.clone();

            select! {
                recv(inv_r) -> inv => {
                    if let Ok(inv) = inv {
                        if inv.operation == OP_PERFORM_LIVE_UPDATE && actor { // Preempt the middleware chain if the operation is a live update
                            #[cfg(feature = "lattice")]
                            let _ = b.publish_event(BusEvent::ActorUpdating { actor: c.subject.to_string(), host: hostkey.public_key()});
                            let r = live_update(&mut guest, &inv);
                            #[cfg(feature = "lattice")]
                            {
                                let succeeded = r.error.is_none();
                                let _ = b.publish_event(BusEvent::ActorUpdateComplete {
                                    actor: c.subject.to_string(),
                                    host: hostkey.public_key(),
                                    success: succeeded});
                            }
                            resp_s.send(r).unwrap();
                            continue;
                        }
                        let inv_r = if actor {
                            middleware::invoke_actor(mids.clone(), inv.clone(), &mut guest).unwrap()
                        } else {
                            if inv.operation != OP_BIND_ACTOR && inv.operation != OP_GET_CAPABILITY_DESCRIPTOR {
                                InvocationResponse::error(&inv, "Attempted to invoke binding-required operation on unbound provider")
                            } else {
                                middleware::invoke_portable_capability(mids.clone(), inv.clone(), &mut guest).unwrap()
                            }
                        };
                        resp_s.send(inv_r.clone()).unwrap();
                        if inv.operation == OP_BIND_ACTOR && !actor && inv_r.error.is_none() {
                            spawn_bound_portable_capability();
                        }
                    }
                },
                recv(term_r) -> _term => {
                    info!("Terminating {} {}", if actor { "actor" } else { "capability" }, &claims.subject);
                    let _ = b.unsubscribe(&subscribe_subject);
                    terminators.write().unwrap().remove(&subscribe_subject);
                    if !actor {
                        //#[cfg(feature = "lattice")]
                        //let _ = bus.publish_event(BusEvent::ProviderRemoved{ host: hostkey.public_key(), actor: claims.subject.to_string() });
                        remove_cap(caps, &d.as_ref().unwrap().id, binding.unwrap().as_ref()); // for cap providers, route key is the capid
                        unbind_all_from_cap(bindings.clone(), &d.unwrap().id, bi.unwrap().as_ref());
                    } else {
                        #[cfg(feature = "lattice")]
                        let _ = bus.publish_event(BusEvent::ActorStopped{ host: hostkey.public_key(), actor: claims.subject.to_string() });
                        deconfigure_actor(hostkey.clone(),bus.clone(), bindings.clone(), &claims.subject);
                        authz::unregister_claims(claimsmap, &claims.subject);
                    }
                    return "".to_string()
                }
            }
        }
    });
    Ok(())
}

pub(crate) fn spawn_native_capability(
    capability: NativeCapability,
    bus: Arc<MessageBus>,
    mids: Arc<RwLock<Vec<Box<dyn Middleware>>>>,
    bindings: Arc<RwLock<BindingsList>>,
    terminators: Arc<RwLock<HashMap<String, Sender<bool>>>>,
    plugins: Arc<RwLock<PluginManager>>,
    wg: WaitGroup,
    hk: Arc<KeyPair>,
) -> Result<()> {
    let capid = capability.id().to_string();
    let binding = capability.binding_name.to_string();
    let b = bus.clone();

    plugins.write().unwrap().add_plugin(capability)?;

    thread::spawn(move || {
        let (inv_s, inv_r): (Sender<Invocation>, Receiver<Invocation>) = channel::unbounded();
        let (resp_s, resp_r): (Sender<InvocationResponse>, Receiver<InvocationResponse>) =
            channel::unbounded();
        let (term_s, term_r): (Sender<bool>, Receiver<bool>) = channel::unbounded();
        let subscribe_subject = bus.provider_subject(&capid, &binding);

        let _ = bus.subscribe(&subscribe_subject, inv_s, resp_r).unwrap();
        let dispatcher = WasccNativeDispatcher::new(hk.clone(), bus.clone(), &capid, &binding);
        plugins
            .write()
            .unwrap()
            .register_dispatcher(&binding, &capid, dispatcher)
            .unwrap();
        terminators
            .write()
            .unwrap()
            .insert(subscribe_subject.to_string(), term_s);

        info!("Native capability provider '({},{})' ready", binding, capid);

        drop(wg);
        #[cfg(feature = "lattice")]
        let _ = b.publish_event(BusEvent::ProviderLoaded {
            host: hk.public_key(),
            capid: capid.to_string(),
            instance_name: binding.to_string(),
        });

        loop {
            select! {
                recv(inv_r) -> inv => {
                    if let Ok(inv) = inv {
                        let inv_r = if inv.operation != OP_BIND_ACTOR && inv.operation != OP_GET_CAPABILITY_DESCRIPTOR {
                            InvocationResponse::error(&inv, "Attempted to invoke binding-required operation on unbound provider")
                        } else {
                            middleware::invoke_native_capability(mids.clone(), inv.clone(), plugins.clone()).unwrap()
                        };
                        resp_s.send(inv_r.clone()).unwrap();
                        if inv.operation == OP_BIND_ACTOR && inv_r.error.is_none() {
                            spawn_bound_native_capability(bus.clone(), inv, &capid, &binding, mids.clone(), plugins.clone(), terminators.clone(), bindings.clone(), hk.clone());
                        }
                    }
                },
                recv(term_r) -> _term => {
                    info!("Terminating native capability provider {},{}", binding, capid);
                    unbind_all_from_cap(bindings.clone(), &capid, &binding);
                    unsub_all_bindings(bindings.clone(), bus.clone(), &capid);
                    let _ = bus.unsubscribe(&subscribe_subject);
                    plugins.write().unwrap().remove_plugin(&binding, &capid).unwrap();
                    terminators.write().unwrap().remove(&subscribe_subject);
                    #[cfg(feature="lattice")]
                    let _ = b.publish_event(BusEvent::ProviderRemoved{ host: hk.public_key(), capid: capid.to_string(), instance_name: binding.to_string()});
                    break;
                }
            }
        }
    });

    Ok(())
}

// This is a thread that handles the private conversations between an actor and a capability.
// On the lattice, this means that actor-to-provider requests occur on a topic made up of actor+provider capid+provider instance/binding name
fn spawn_bound_native_capability(
    bus: Arc<MessageBus>,
    inv: Invocation,
    capid: &str,
    binding: &str,
    middlewares: Arc<RwLock<Vec<Box<dyn Middleware>>>>,
    plugins: Arc<RwLock<PluginManager>>,
    terminators: Arc<RwLock<HashMap<String, Sender<bool>>>>,
    bindings: Arc<RwLock<BindingsList>>,
    hk: Arc<KeyPair>,
) {
    let capid = capid.to_string();
    let binding = binding.to_string();

    let config: CapabilityConfiguration = deserialize(&inv.msg).unwrap();
    let actor = config.module.to_string();
    let mids = middlewares.clone();
    let terms = terminators.clone();

    thread::spawn(move || {
        let (inv_s, inv_r): (Sender<Invocation>, Receiver<Invocation>) = channel::unbounded();
        let (resp_s, resp_r): (Sender<InvocationResponse>, Receiver<InvocationResponse>) =
            channel::unbounded();
        let (term_s, term_r): (Sender<bool>, Receiver<bool>) = channel::unbounded();
        let subscribe_subject = bus.provider_subject_bound_actor(&capid, &binding, &actor);

        let _ = bus.subscribe(&subscribe_subject, inv_s, resp_r).unwrap();
        terms
            .write()
            .unwrap()
            .insert(subscribe_subject.to_string(), term_s.clone());
        loop {
            select! {
                recv(inv_r) -> inv => {
                    if let Ok(inv) = inv {
                        let inv_r = middleware::invoke_native_capability(mids.clone(), inv.clone(), plugins.clone()).unwrap();
                        resp_s.send(inv_r).unwrap();
                        if inv.operation == OP_REMOVE_ACTOR {
                            term_s.send(true).unwrap();
                        }
                    }
                },
                recv(term_r) -> _term => {
                    let _ = bus.unsubscribe(&subscribe_subject);
                    remove_binding(bindings.clone(), &actor, &binding, &capid);
                    terminators.write().unwrap().remove(&subscribe_subject);
                    #[cfg(feature="lattice")]
                    let _ = bus.publish_event(BusEvent::ProviderRemoved{ host: hk.public_key(), capid: capid.to_string(), instance_name: binding.to_string()});
                    break;
                }
            }
        }
    });
}

fn spawn_bound_portable_capability() {
    todo!()
}
