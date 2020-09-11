use crate::{BindingsList, RouteKey};
use crate::{Invocation, InvocationResponse, Result};
use crossbeam::{Receiver, Sender};
use crossbeam_channel as channel;
use latticeclient::{
    controlplane::{
        LaunchAck, LaunchAuctionRequest, LaunchAuctionResponse, LaunchCommand, TerminateCommand,
        AUCTION_REQ, CPLANE_PREFIX, LAUNCH_ACTOR, TERMINATE_ACTOR,
    },
    BusEvent, CloudEvent,
};
use nats;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, SystemTime};
use wapc::prelude::*;
use wascap::jwt::{Actor, Claims};
use wascc_codec::{capabilities::CapabilityDescriptor, deserialize, serialize};

const LATTICE_HOST_KEY: &str = "LATTICE_HOST";
// env var name
const DEFAULT_LATTICE_HOST: &str = "127.0.0.1";
// default mode is anonymous via loopback
const LATTICE_RPC_TIMEOUT_KEY: &str = "LATTICE_RPC_TIMEOUT_MILLIS";
const DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS: u64 = 600;
const LATTICE_CREDSFILE_KEY: &str = "LATTICE_CREDS_FILE";

const TERM_BACKOFF_MAX_TRIES: u8 = 3;
const TERM_BACKOFF_DELAY_MS: u64 = 50;

use crate::bus::event_subject;
use latticeclient::*;
use nats::Message;

#[derive(Debug, Clone)]
pub(crate) enum ControlCommand {
    TerminateActor(TerminateCommand),
    StartActor(LaunchCommand, Message),
}

pub(crate) struct DistributedBus {
    nc: Arc<RwLock<Option<nats::Connection>>>,
    subs: Arc<RwLock<HashMap<String, nats::subscription::Handler>>>,
    terminators: Arc<RwLock<HashMap<String, Sender<bool>>>>,
    req_timeout: Duration,
    host_id: String,
    pub(crate) ns: Option<String>,
}

impl DistributedBus {
    pub fn new(
        host_id: String,
        claims: Arc<RwLock<HashMap<String, Claims<Actor>>>>,
        caps: Arc<RwLock<HashMap<RouteKey, CapabilityDescriptor>>>,
        bindings: Arc<RwLock<BindingsList>>,
        labels: Arc<RwLock<HashMap<String, String>>>,
        terminators: Arc<RwLock<HashMap<String, Sender<bool>>>>,
        ns: Option<String>,
        cplane_s: Sender<ControlCommand>,
        authz: Arc<RwLock<Box<dyn crate::authz::Authorizer>>>,
    ) -> Self {
        let nc = Arc::new(RwLock::new(Some(get_connection())));

        info!(
            "Initialized Lattice Message Bus ({})",
            if let Some(ref s) = ns {
                s
            } else {
                "default namespace"
            }
        );

        spawn_controlplane_handler(
            nc.clone(),
            host_id.clone(),
            claims.clone(),
            bindings.clone(),
            caps.clone(),
            labels.clone(),
            ns.clone(),
            cplane_s,
            authz,
        )
            .unwrap();

        spawn_inventory_handler(
            nc.clone(),
            host_id.to_string(),
            claims.clone(),
            bindings.clone(),
            caps.clone(),
            SystemTime::now(),
            labels,
            ns.clone(),
        )
            .unwrap();
        DistributedBus {
            nc,
            subs: Arc::new(RwLock::new(HashMap::new())),
            terminators,
            req_timeout: get_timeout(),
            host_id,
            ns: ns.clone(),
        }
    }

    pub fn disconnect(&self) {
        // Terminate the control plane command handler
        let cpsubject = format!(
            "{}.{}.{}",
            super::nsprefix(self.ns.as_ref().map(String::as_str)),
            latticeclient::controlplane::CPLANE_PREFIX,
            self.host_id
        );
        self.terminators.read().unwrap()[&cpsubject]
            .send(true)
            .unwrap();

        let mut backoffcount = 0_u8;
        // Wait until everything that can be gracefully shut off has been shut off
        while self.terminators.read().unwrap().len() > 0 && backoffcount < TERM_BACKOFF_MAX_TRIES {
            std::thread::sleep(std::time::Duration::from_millis(TERM_BACKOFF_DELAY_MS));
            backoffcount += 1;
        }
        let _ = self.publish_event(BusEvent::HostStopped(self.host_id.to_string()));
        let mut lock = self.nc.write().unwrap();
        let conn = lock.take();
        if let Some(nc) = conn {
            nc.close();
        }
    }

    pub fn subscribe(
        &self,
        subject: &str,
        sender: Sender<Invocation>,
        receiver: Receiver<InvocationResponse>,
    ) -> Result<()> {
        let sub = self
            .nc
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .queue_subscribe(subject, subject)?
            .with_handler(move |msg| {
                handle_invocation(&msg, sender.clone(), receiver.clone());
                Ok(())
            });
        self.subs.write().unwrap().insert(subject.to_string(), sub);
        Ok(())
    }

    pub fn invoke(&self, subject: &str, inv: Invocation) -> Result<InvocationResponse> {
        let resp = self.nc.read().unwrap().as_ref().unwrap().request_timeout(
            &subject,
            &serialize(inv)?,
            self.req_timeout,
        )?;
        let ir: InvocationResponse = deserialize(&resp.data)?;
        Ok(ir)
    }

    pub fn unsubscribe(&self, subject: &str) -> Result<()> {
        if let Some(sub) = self.subs.write().unwrap().remove(subject) {
            sub.unsubscribe()?;
        }
        Ok(())
    }

    pub fn publish_event(&self, event: BusEvent) -> Result<()> {
        let cloud_event = CloudEvent::from(event);
        let payload = match serde_json::to_vec(&cloud_event) {
            Ok(p) => p,
            Err(e) => {
                return Err(crate::errors::new(crate::errors::ErrorKind::Serialization(
                    format!("{}", e),
                )));
            }
        };
        let lock = self.nc.read().unwrap();
        if let Some(ref nc) = lock.as_ref() {
            nc.publish(&*self.event_subject(), &payload)?;
            nc.flush()?;
        }
        Ok(())
    }

    pub fn actor_subject(&self, actor: &str) -> String {
        super::actor_subject(self.ns.as_ref().map(String::as_str), actor)
    }

    pub(crate) fn provider_subject(&self, capid: &str, binding: &str) -> String {
        super::provider_subject(self.ns.as_ref().map(String::as_str), capid, binding)
    }

    pub(crate) fn inventory_wildcard_subject(&self) -> String {
        super::inventory_wildcard_subject(self.ns.as_ref().map(String::as_str))
    }

    pub(crate) fn event_subject(&self) -> String {
        super::event_subject(self.ns.as_ref().map(String::as_str))
    }

    pub(crate) fn provider_subject_bound_actor(
        &self,
        capid: &str,
        binding: &str,
        calling_actor: &str,
    ) -> String {
        super::provider_subject_bound_actor(
            self.ns.as_ref().map(String::as_str),
            capid,
            binding,
            calling_actor,
        )
    }
}

pub(crate) fn controlplane_wildcard_subject(ns: Option<&str>) -> String {
    format!("{}.{}.>", super::nsprefix(ns), CPLANE_PREFIX) // e.g. wasmbus.control.* or wasmbus.control.Nxxx.*
}

pub(crate) fn spawn_controlplane(
    host: &crate::Host,
    com_r: Receiver<ControlCommand>,
) -> Result<()> {
    let wg = crossbeam_utils::sync::WaitGroup::new();
    let claims = host.claims.clone();
    let wasi: Option<WasiParams> = None;
    let actor = true;
    let binding: Option<String> = None;
    let bus = host.bus.clone();
    let mids = host.middlewares.clone();
    let caps = host.caps.clone();
    let bindings = host.bindings.clone();
    let claimsmap = host.claims.clone();
    let terminators = host.terminators.clone();
    let hk = host.key.clone();
    let auth = host.authorizer.clone();
    let gantry = host.gantry_client.clone();

    let subject = format!(
        "{}.{}.{}",
        super::nsprefix(bus.ns.as_ref().map(String::as_str)),
        latticeclient::controlplane::CPLANE_PREFIX,
        hk.public_key()
    );
    let (term_s, term_r): (Sender<bool>, Receiver<bool>) = channel::unbounded();
    terminators
        .write()
        .unwrap()
        .insert(subject.to_string(), term_s);

    thread::spawn(move || loop {
        select! {
            recv(com_r) -> cmd => {
                if let Ok(cmd) = cmd {
                    match cmd {
                        ControlCommand::StartActor(cmd, msg) => {
                            // Acknowledge before downloading the bytes. Do not keep consumers waiting for
                            // our own download process.
                            if let Err(e) = msg.respond(&serde_json::to_vec(&LaunchAck {
                                actor_id: cmd.actor_id.to_string(),
                                host: hk.public_key()
                            }).unwrap()) {
                                error!("Failed to send schedule acknowledgement reply: {}", e);
                            } else {
                                info!("Acknowledged actor start request.");
                            }
                            match fetch_actor(&cmd.actor_id, gantry.clone(), cmd.revision) {
                                Ok(a) => {
                                    let wg = crossbeam_utils::sync::WaitGroup::new();
                                    if crate::authz::enforce_validation(&a.token.jwt).is_err() {
                                        error!("Attempt to remotely schedule invalid actor.");
                                        continue;
                                    }
                                    if !auth.read().unwrap().can_load(&a.token.claims) {
                                        error!("Authorization hook denied access to remotely scheduled module.");
                                        continue;
                                    }

                                    crate::authz::register_claims(
                                        claims.clone(),
                                        &a.token.claims.subject,
                                        a.token.claims.clone(),
                                    );

                                    let _ = crate::spawns::spawn_actor(wg, a.token.claims.clone(), a.bytes,
                                        None, actor, binding.clone(), bus.clone(), mids.clone(),
                                        caps.clone(), bindings.clone(), claimsmap.clone(), terminators.clone(),
                                        hk.clone(), auth.clone());


                                },
                                Err(e) => {
                                    error!("Actor download failed for {}: {}", &cmd.actor_id, e)
                                }
                            }
                        },
                        ControlCommand::TerminateActor(cmd) => {
                            let actor_subject = bus.actor_subject(&cmd.actor_id);
                            terminators.read().unwrap()
                                [&actor_subject]
                                .send(true)
                                .unwrap();
                        }
                    }
                }
            }
            recv(term_r) -> _term => {
                terminators.write().unwrap().remove(&subject);
                break;
            }
        }
    });
    Ok(())
}

fn fetch_actor(
    actor_id: &str,
    gantry: Arc<RwLock<Option<gantryclient::Client>>>,
    revision: u32,
) -> Result<crate::actor::Actor> {
    let vec = actor_bytes_from_gantry(actor_id, gantry, revision)?;

    crate::actor::Actor::from_slice(&vec)
}

pub(crate) fn actor_bytes_from_gantry(
    actor_id: &str,
    gantry_client: Arc<RwLock<Option<gantryclient::Client>>>,
    revision: u32,
) -> Result<Vec<u8>> {
    {
        let lock = gantry_client.read().unwrap();
        if lock.as_ref().is_none() {
            return Err(crate::errors::new(crate::errors::ErrorKind::MiscHost(
                "No gantry client configured".to_string(),
            )));
        }
    }
    match gantry_client
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .download_actor(actor_id, revision)
    {
        Ok(v) => Ok(v),
        Err(e) => Err(crate::errors::new(crate::errors::ErrorKind::MiscHost(
            format!("{}", e),
        ))),
    }
}

fn spawn_controlplane_handler(
    nc: Arc<RwLock<Option<nats::Connection>>>,
    host_id: String,
    claims: Arc<RwLock<HashMap<String, Claims<Actor>>>>,
    bindings: Arc<RwLock<BindingsList>>,
    caps: Arc<RwLock<HashMap<RouteKey, CapabilityDescriptor>>>,
    labels: Arc<RwLock<HashMap<String, String>>>,
    ns: Option<String>,
    cplane_s: Sender<ControlCommand>,
    authz: Arc<RwLock<Box<dyn crate::authz::Authorizer>>>,
) -> Result<()> {
    let subject = controlplane_wildcard_subject(ns.as_ref().map(String::as_str));
    let lbs = labels.clone();

    let _ = nc
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .subscribe(&subject)?
        .with_handler(move |msg| {
            if msg.subject.ends_with(LAUNCH_ACTOR) && msg.subject.contains(&host_id) {
                // schedule the actor
                let lc: LaunchCommand = serde_json::from_slice(&msg.data).unwrap();
                cplane_s.send(ControlCommand::StartActor(lc, msg)).unwrap();
            } else if msg.subject.ends_with(TERMINATE_ACTOR) && msg.subject.contains(&host_id) {
                let tc: TerminateCommand = serde_json::from_slice(&msg.data).unwrap();
                if !claims.read().unwrap().contains_key(&tc.actor_id) {
                    warn!("Received request to terminate non-existent actor. Ignoring.");
                } else {
                    cplane_s.send(ControlCommand::TerminateActor(tc)).unwrap();
                }
            } else if msg.subject.ends_with(AUCTION_REQ) {
                let req: LaunchAuctionRequest = serde_json::from_slice(&msg.data)?;
                if claims.read().unwrap().contains_key(&req.actor_id) {
                    trace!("Skipping auction response - actor already running locally.");
                } else {
                    // TODO: validate constraint requirements
                    if !host_satifies_constraints(labels.clone(), &req.constraints) {
                        trace!("Skipping auction response - host does not satisfy constraints.");
                    } else {
                        let ar = LaunchAuctionResponse {
                            host_id: host_id.to_string(),
                        };
                        info!("Responding to actor schedule auction request");
                        let _ = msg.respond(serde_json::to_vec(&ar).unwrap());
                    }
                }
            }
            Ok(())
        });
    Ok(())
}

fn host_satifies_constraints(
    labels: Arc<RwLock<HashMap<String, String>>>,
    constraints: &HashMap<String, String>,
) -> bool {
    for (label, val) in constraints.iter() {
        match labels.read().unwrap().get_key_value(label) {
            Some((k, v)) => {
                if v != val {
                    return false;
                }
            }
            None => return false,
        }
    }
    true
}

fn spawn_inventory_handler(
    nc: Arc<RwLock<Option<nats::Connection>>>,
    host_id: String,
    claims: Arc<RwLock<HashMap<String, Claims<Actor>>>>,
    bindings: Arc<RwLock<BindingsList>>,
    caps: Arc<RwLock<HashMap<RouteKey, CapabilityDescriptor>>>,
    started: SystemTime,
    labels: Arc<RwLock<HashMap<String, String>>>,
    ns: Option<String>,
) -> Result<()> {
    let lbs = labels.clone();
    let subject = super::inventory_wildcard_subject(ns.as_ref().map(String::as_str));

    let _ = nc
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .subscribe(&subject)?
        .with_handler(move |msg| {
            trace!("Handling Inventory Request");
            if msg.subject.contains(INVENTORY_HOSTS) {
                respond_with_host(msg, host_id.to_string(), started, lbs.clone())
            } else if msg.subject.contains(INVENTORY_ACTORS) {
                respond_with_actors(msg, host_id.to_string(), claims.clone())
            } else if msg.subject.contains(INVENTORY_BINDINGS) {
                respond_with_bindings(msg, host_id.to_string(), bindings.clone())
            } else if msg.subject.contains(INVENTORY_CAPABILITIES) {
                respond_with_caps(msg, host_id.to_string(), caps.clone())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Bad inventory topic!",
                ))
            }
        });
    Ok(())
}

fn respond_with_host(
    msg: nats::Message,
    host_id: String,
    started: SystemTime,
    labels: Arc<RwLock<HashMap<String, String>>>,
) -> std::result::Result<(), std::io::Error> {
    let hp = HostProfile {
        id: host_id.to_string(),
        uptime_ms: started.elapsed().unwrap_or(Duration::new(0, 0)).as_millis(),
        labels: labels.read().unwrap().clone(),
    };
    msg.respond(serde_json::to_vec(&InventoryResponse::Host(hp)).unwrap())
}

fn respond_with_actors(
    msg: nats::Message,
    host: String,
    claims: Arc<RwLock<HashMap<String, Claims<Actor>>>>,
) -> std::result::Result<(), std::io::Error> {
    let ir = InventoryResponse::Actors {
        host,
        actors: claims.read().unwrap().values().cloned().collect(),
    };
    msg.respond(serde_json::to_vec(&ir).unwrap())
        .map_err(|e| e.into())
}

fn respond_with_bindings(
    msg: nats::Message,
    host: String,
    bindings: Arc<RwLock<BindingsList>>,
) -> std::result::Result<(), std::io::Error> {
    let mut items = Vec::<Binding>::new();
    let lock = bindings.read().unwrap();
    for (k, v) in lock.iter() {
        items.push(Binding {
            actor: k.0.to_string(),
            capability_id: k.1.to_string(),
            binding_name: k.2.to_string(),
            configuration: v.values.clone(),
        });
    }
    let ir = InventoryResponse::Bindings {
        host,
        bindings: items,
    };
    msg.respond(serde_json::to_vec(&ir).unwrap())
        .map_err(|e| e.into())
}

fn respond_with_caps(
    msg: nats::Message,
    host: String,
    caps: Arc<RwLock<HashMap<RouteKey, CapabilityDescriptor>>>,
) -> std::result::Result<(), std::io::Error> {
    let mut capabilities = vec![];
    let lock = caps.read().unwrap();
    // RouteKey - (binding, capid)
    for (k, v) in lock.iter() {
        let hc = HostedCapability {
            binding_name: k.binding_name.to_string(),
            descriptor: v.clone(),
        };
        capabilities.push(hc);
    }
    let ir = InventoryResponse::Capabilities { host, capabilities };
    msg.respond(serde_json::to_vec(&ir).unwrap())
        .map_err(|e| e.into())
}

// This function is invoked any time an invocation is _received_ by the message bus
fn handle_invocation(
    msg: &nats::Message,
    sender: Sender<Invocation>,
    receiver: Receiver<InvocationResponse>,
) {
    let inv = invocation_from_msg(msg);
    //TODO: when we implement the issue, check that the invocation's origin host is not in the block list
    if let Err(e) = inv.validate_antiforgery() {
        error!("Invocation Antiforgery check failure: {}", e);
        let inv_r = InvocationResponse::error(&inv, &format!("Antiforgery check failure: {}", e));
        msg.respond(serialize(inv_r).unwrap()).unwrap();
        // TODO: when we implement the issue, publish an antiforgery check event on wasmbus.events
        // TODO: when we implement the issue, add the host origin of the invocation to the global lattice block list
    } else {
        sender.send(inv).unwrap();
        let inv_r = receiver.recv().unwrap();
        msg.respond(serialize(inv_r).unwrap()).unwrap();
    }
}

fn invocation_from_msg(msg: &nats::Message) -> Invocation {
    let i: Invocation = deserialize(&msg.data).unwrap();
    i
}

fn get_credsfile() -> Option<String> {
    std::env::var(LATTICE_CREDSFILE_KEY).ok()
}

fn get_env(var: &str, default: &str) -> String {
    match std::env::var(var) {
        Ok(val) => {
            if val.is_empty() {
                default.to_string()
            } else {
                val.to_string()
            }
        }
        Err(_) => default.to_string(),
    }
}

fn get_connection() -> nats::Connection {
    let host = get_env(LATTICE_HOST_KEY, DEFAULT_LATTICE_HOST);
    info!("Lattice Host: {}", host);
    let mut opts = if let Some(creds) = get_credsfile() {
        nats::Options::with_credentials(creds)
    } else {
        nats::Options::new()
    };
    opts = opts.with_name("waSCC Lattice");
    opts.connect(&host).unwrap()
}

fn get_timeout() -> Duration {
    match std::env::var(LATTICE_RPC_TIMEOUT_KEY) {
        Ok(val) => {
            if val.is_empty() {
                Duration::from_millis(DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS)
            } else {
                Duration::from_millis(val.parse().unwrap_or(DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS))
            }
        }
        Err(_) => Duration::from_millis(DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS),
    }
}
