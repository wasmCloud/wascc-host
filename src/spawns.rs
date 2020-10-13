use crate::Result;

use crate::inthost::*;
use crate::BindingsList;
use crate::{
    bus::MessageBus, dispatch::WasccNativeDispatcher, plugins::PluginManager, Authorizer,
    Invocation, InvocationResponse, Middleware, RouteKey,
};
use crate::{middleware, NativeCapability};

use crossbeam::{Receiver, Sender};
use crossbeam_channel as channel;
use crossbeam_utils::sync::WaitGroup;
#[cfg(feature = "lattice")]
use latticeclient::BusEvent;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::thread;
use wapc::{WapcHost, WasiParams};
use wascap::{jwt::Claims, prelude::KeyPair};
use wascc_codec::{
    capabilities::{CapabilityDescriptor, OP_GET_CAPABILITY_DESCRIPTOR},
    core::{CapabilityConfiguration, OP_BIND_ACTOR, OP_REMOVE_ACTOR},
    deserialize, serialize, SYSTEM_ACTOR,
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
    claimsmap: Arc<RwLock<HashMap<String, Claims<wascap::jwt::Actor>>>>,
    terminators: Arc<RwLock<HashMap<String, Sender<bool>>>>,
    hk: KeyPair,
    auth: Arc<RwLock<Box<dyn Authorizer>>>,
    image_map: Arc<RwLock<HashMap<String, String>>>,
    imgref: Option<String>,
) -> Result<()> {
    let c = claims.clone();
    let b = bus.clone();
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
        #[cfg(feature = "wasmtime")]
        let engine = wasmtime_provider::WasmtimeEngineProvider::new(&buf, wasi);
        #[cfg(feature = "wasm3")]
        let engine = wasm3_provider::Wasm3EngineProvider::new(&buf);

        let mut guest = WapcHost::new(Box::new(engine), move |_id, bd, ns, op, payload| {
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
        })
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
            let bname = binding.clone().unwrap();
            #[cfg(feature = "lattice")]
            let _ = b.publish_event(BusEvent::ProviderLoaded {
                host: hostkey.public_key(),
                capid: capid.to_string(),
                instance_name: bname.to_string(),
            });

            b.provider_subject(&capid, &bname)
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
            info!("Actor {} up and running.", &claims.subject);
        }
        loop {
            select! {
                recv(inv_r) -> inv => {
                    if let Ok(inv) = inv {
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
                        remove_cap(caps.clone(), &d.as_ref().unwrap().id, binding.as_ref().unwrap()); // for cap providers, route key is the capid
                        unbind_all_from_cap(bindings.clone(), &d.unwrap().id, binding.as_ref().unwrap());
                    } else {
                        #[cfg(feature = "lattice")]
                        let _ = b.publish_event(BusEvent::ActorStopped{ host: hostkey.public_key(), actor: claims.subject.to_string() });

                        let mut lock = claimsmap.write().unwrap();
                        let _ = lock.remove(&claims.subject);
                        drop(lock);
                        if let Some(ref ir) = imgref { // if this actor was added via OCI image ref, remove the mapping
                            let mut lock = image_map.write().unwrap();
                            let _ = lock.remove(ir);
                            drop(lock);
                        }
                        deconfigure_actor(hostkey.clone(),b.clone(), bindings.clone(), &claims.subject);
                    }
                    break "".to_string(); // TODO: WHY WHY WHY does this recv arm need to return a value?!?!?
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

    let b2 = bus.clone();
    let mid2 = mids.clone();
    let binding2 = bindings.clone();
    let plugin2 = plugins.clone();
    let h2 = hk.clone();
    let t2 = terminators.clone();
    let capid2 = capid.clone();
    let bindingname2 = binding.clone();

    plugins.write().unwrap().add_plugin(capability)?;

    thread::spawn(move || {
        let (inv_s, inv_r): (Sender<Invocation>, Receiver<Invocation>) = channel::unbounded();
        let (resp_s, resp_r): (Sender<InvocationResponse>, Receiver<InvocationResponse>) =
            channel::unbounded();
        let (term_s, term_r): (Sender<bool>, Receiver<bool>) = channel::unbounded();
        let subscribe_subject = bus.provider_subject(&capid, &binding);

        let _ = bus.nqsubscribe(&subscribe_subject, inv_s, resp_r).unwrap();
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
                        let inv_r = if inv.operation != OP_BIND_ACTOR && inv.operation != OP_GET_CAPABILITY_DESCRIPTOR && inv.operation != OP_REMOVE_ACTOR {
                            InvocationResponse::error(&inv, "Attempted to invoke binding-required operation on unbound provider")
                        } else {
                            middleware::invoke_native_capability(mids.clone(), inv.clone(), plugins.clone()).unwrap()
                        };
                        resp_s.send(inv_r.clone()).unwrap();
                        if inv.operation == OP_BIND_ACTOR && inv_r.error.is_none() {
                            spawn_bound_native_capability(bus.clone(), inv.clone(), &capid, &binding, mids.clone(), plugins.clone(), terminators.clone(), bindings.clone(), hk.clone());
                        }
                        if inv.operation == OP_REMOVE_ACTOR && inv_r.error.is_none() {
                            let actor = actor_from_config(&inv.msg);
                            let key = bus.provider_subject_bound_actor(&capid, &binding, &actor);
                            terminators.read().unwrap()[&key].send(true).unwrap();
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

    #[cfg(feature = "lattice")]
    reestablish_bindings(
        b2.clone(),
        mid2.clone(),
        binding2.clone(),
        plugin2.clone(),
        t2.clone(),
        h2.clone(),
        &capid2,
        &bindingname2,
    );
    Ok(())
}

#[cfg(feature = "lattice")]
fn reestablish_bindings(
    bus: Arc<MessageBus>,
    mids: Arc<RwLock<Vec<Box<dyn Middleware>>>>,
    bindings: Arc<RwLock<BindingsList>>,
    plugins: Arc<RwLock<PluginManager>>,
    terminators: Arc<RwLock<HashMap<String, Sender<bool>>>>,
    hk: Arc<KeyPair>,
    capid: &str,
    binding_name: &str,
) {
    // 1. load pre-existing bindings from bus
    // 2. for each binding, invoke OP_BIND_ACTOR on the root capability
    // 3.    if successful,  spawn the bound actor-capability comms thread
    if let Ok(blist) = bus.query_bindings() {
        for b in blist {
            if b.capability_id == capid && b.binding_name == binding_name {
                let cfgvals = CapabilityConfiguration {
                    module: b.actor.to_string(),
                    values: b.configuration.clone(),
                };
                let payload = serialize(&cfgvals).unwrap();
                let inv = Invocation::new(
                    &hk,
                    WasccEntity::Actor(SYSTEM_ACTOR.to_string()),
                    WasccEntity::Capability {
                        capid: capid.to_string(),
                        binding: binding_name.to_string(),
                    },
                    OP_BIND_ACTOR,
                    payload,
                );
                let inv_r = middleware::invoke_native_capability(
                    mids.clone(),
                    inv.clone(),
                    plugins.clone(),
                )
                .unwrap();
                if inv_r.error.is_none() {
                    info!(
                        "Re-establishing binding between {} and {},{}",
                        &b.actor, &capid, &binding_name
                    );
                    spawn_bound_native_capability(
                        bus.clone(),
                        inv.clone(),
                        &capid,
                        &binding_name,
                        mids.clone(),
                        plugins.clone(),
                        terminators.clone(),
                        bindings.clone(),
                        hk.clone(),
                    );
                }
            }
        }
    }
}

fn actor_from_config(bytes: &[u8]) -> String {
    let config: CapabilityConfiguration = deserialize(bytes).unwrap();
    config.module
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
        let subscribe_subject = bus.provider_subject_bound_actor(&capid, &binding, &actor);
        let (term_s, term_r): (Sender<bool>, Receiver<bool>) = channel::unbounded();

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
