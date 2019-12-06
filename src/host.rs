use super::router::Router;
use super::Result;
use crate::authz;
use crate::dispatch::WasccNativeDispatcher;
use crate::errors;
use crate::router::InvokerPair;
use crossbeam::{Receiver, Sender};
use crossbeam_channel as channel;
use crossbeam_utils::sync::WaitGroup;
use prost::Message;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::Arc;
use std::sync::RwLock;
use std::thread;
use wapc::prelude::*;
use wascap::jwt::Claims;
use wascc_codec::core::CapabilityConfiguration;
use wascc_codec::core::OP_CONFIGURE;
use wascc_codec::core::{CapabilityIdResponse, OP_IDENTIFY_CAPABILITY};

lazy_static! {
    pub static ref ROUTER: Arc<RwLock<Router>> = {
        wapc::set_host_callback(host_callback);
        Arc::new(RwLock::new(Router::default()))
    };
}

/// Adds a portable capability provider wasm module to the runtime host. The identity of this provider
/// will be read by invoking the `OP_IDENTIFY_CAPABILITY` waPC call, which will result
/// in, among other things, the capability ID of the plugin (e.g. `wascc:messaging`)
pub fn add_capability(buf: Vec<u8>, wasi: WasiParams) -> Result<()> {
    let token = authz::extract_and_store_claims(&buf)?;
    let wg = WaitGroup::new();
    let router = ROUTER.clone();
    listen_for_invocations(wg.clone(), token.claims, router, buf, Some(wasi), false)?;
    wg.wait();
    Ok(())
}

/// Adds an actor module to the runtime host. The identity of this module is determined
/// by inspecting the claims embedded in the module's custom section as a JWT. The identity
/// comes from the `subject` field on the embedded claims and is the primary key of the
/// module identity.
pub fn add_actor(buf: Vec<u8>) -> Result<()> {
    let token = authz::extract_and_store_claims(&buf)?;
    let wg = WaitGroup::new();
    info!("Adding actor {} to host", token.claims.subject);
    let router = ROUTER.clone();
    listen_for_invocations(wg.clone(), token.claims, router, buf, None, true)?;
    wg.wait();
    Ok(())
}

/// Adds a native linux dynamic library (plugin) as a capability provider to the runtime host. The
/// identity and other metadata about this provider is determined by loading the plugin from disk
/// and invoking the appropriate plugin trait methods.
pub fn add_native_capability<P: AsRef<OsStr>>(filename: P) -> Result<()> {
    let capid = crate::plugins::PLUGMAN
        .write()
        .unwrap()
        .load_plugin(filename)?;
    let wg = WaitGroup::new();
    let router = ROUTER.clone();
    if router.read().unwrap().get_pair(&capid).is_some() {
        return Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
            "Attempt to register the same capability provider multiple times: {}",
            capid
        ))));
    }
    listen_for_native_invocations(wg.clone(), router, &capid)?;
    wg.wait();
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
    let router = ROUTER.clone();
    let pair = router.read().unwrap().get_pair(&capid);
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
fn listen_for_native_invocations(
    wg: WaitGroup,
    router: Arc<RwLock<Router>>,
    capid: &str,
) -> Result<()> {
    let capid = capid.to_string();

    thread::spawn(move || {
        let (inv_s, inv_r): (Sender<Invocation>, Receiver<Invocation>) = channel::unbounded();
        let (resp_s, resp_r): (Sender<InvocationResponse>, Receiver<InvocationResponse>) =
            channel::unbounded();
        let dispatcher = WasccNativeDispatcher::new(resp_r.clone(), inv_s.clone(), &capid);
        crate::plugins::PLUGMAN
            .write()
            .unwrap()
            .register_dispatcher(&capid, dispatcher)
            .unwrap();

        {
            let mut lock = router.write().unwrap();
            lock.add_route(capid.to_string(), inv_s, resp_r);
        }

        info!("Native capability provider '{}' ready", capid);
        drop(wg);

        loop {
            if let Ok(inv) = inv_r.recv() {
                let v: Vec<_> = inv.operation.split('!').collect();
                let target = v[0];
                info!(
                    "Capability {} received invocation for target {}",
                    capid, target
                );

                let inv_r = if target == capid {
                    // if target of invocation is this particular capability,
                    // then perform the invocation on the plugin
                    let lock = crate::plugins::PLUGMAN.read().unwrap();
                    lock.call(&inv).unwrap()
                } else {
                    // Capability is handling a dispatch (delivering) to actor module
                    if !authz::can_invoke(target, &capid) {
                        InvocationResponse::error(&format!(
                            "Dispatch between actor and unauthorized capability: {} <-> {}",
                            target, capid
                        ))
                    } else {
                        let pair = router.read().unwrap().get_pair(target);
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
        }
    });

    Ok(())
}

/// Spawns a new thread, inside which we create an instance of the wasm module interpreter. This function
/// handles incoming calls targeted at either an actor module or a portable capability provider (both are wasm).
fn listen_for_invocations(
    wg: WaitGroup,
    claims: Claims,
    router: Arc<RwLock<Router>>,
    buf: Vec<u8>,
    wasi: Option<WasiParams>,
    actor: bool,
) -> Result<()> {
    thread::spawn(move || {
        info!(
            "Loading {} module...",
            if actor { "actor" } else { "capability" }
        );
        let mut guest = WapcHost::new(&buf, wasi).unwrap();
        authz::map_claims(guest.id(), &claims.subject);
        let (inv_s, inv_r): (Sender<Invocation>, Receiver<Invocation>) = channel::unbounded();
        let (resp_s, resp_r): (Sender<InvocationResponse>, Receiver<InvocationResponse>) =
            channel::unbounded();

        {
            let route_key = if actor {
                claims.subject
            } else {
                get_capability_id(router.clone(), &mut guest).unwrap() // NOTE: this is an intentional panic
            };
            let mut lock = router.write().unwrap();
            lock.add_route(route_key.clone(), inv_s, resp_r);
            info!(
                "Actor {} ready for communications, capability: {}",
                route_key, !actor
            );
        }
        drop(wg); // API call that spawned this thread can now unblock

        loop {
            if let Ok(inv) = inv_r.recv() {
                let v: Vec<_> = inv.operation.split('!').collect();
                let inv = Invocation::new(inv.origin, v[1], inv.msg); // Remove routing prefix from operation
                match guest.call(&inv.operation, &inv.msg) {
                    Ok(v) => {
                        resp_s.send(InvocationResponse::success(v)).unwrap();
                    }
                    Err(e) => {
                        resp_s
                            .send(InvocationResponse::error(&format!(
                                "Failed to invoke guest call: {}",
                                e
                            )))
                            .unwrap();
                    }
                }
            }
        }
    });

    Ok(())
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
    info!("Guest {} invoking {}", id, op);
    let v: Vec<_> = op.split('!').collect();
    let capability_id = v[0];
    if !authz::can_id_invoke(id, capability_id) {
        return Err(Box::new(errors::new(errors::ErrorKind::Authorization(
            format!(
                "Actor {} does not have permission to use capability {}",
                id, capability_id
            ),
        ))));
    }
    let pair = ROUTER.read().unwrap().get_pair(capability_id);
    match pair {
        Some((inv_s, resp_r)) => {
            info!("invoking");
            inv_s.send(Invocation::new(authz::pk_for_id(id), op, payload.to_vec()))?;
            match resp_r.recv() {
                Ok(ir) => Ok(ir.msg),
                Err(e) => Err(Box::new(errors::new(errors::ErrorKind::HostCallFailure(
                    e.into(),
                )))),
            }
        }
        None => Err(Box::new(errors::new(errors::ErrorKind::HostCallFailure(
            "dispatch to non-existent capability".into(),
        )))),
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

/// Used for WASI-based capability providers. Invoke the `IdentifyCapability` operation
/// on the capability provider. Returns the same kind of metadata that the native
/// plugins return upon being probed
fn get_capability_id(router: Arc<RwLock<Router>>, host: &mut WapcHost) -> Result<String> {
    let res = host.call(OP_IDENTIFY_CAPABILITY, &[])?;
    let capinfo = CapabilityIdResponse::decode(&res)?;
    {
        let lock = router.read().unwrap();
        if lock.get_pair(&capinfo.capability_id).is_some() {
            return Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                "Attempt to register duplicate capability provider: {}",
                capinfo.capability_id
            ))));
        }
    }

    Ok(capinfo.capability_id)
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

#[derive(Debug)]
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
