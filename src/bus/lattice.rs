use crate::{BindingsList, NativeCapability, RouteKey};
use crate::{Invocation, InvocationResponse, Result};
use crossbeam::{Receiver, Sender};
use crossbeam_channel as channel;
use latticeclient::{
    controlplane::{
        LaunchAck, LaunchAuctionRequest, LaunchAuctionResponse, LaunchCommand, ProviderLaunchAck,
        TerminateCommand, AUCTION_REQ, CPLANE_PREFIX, LAUNCH_ACTOR, TERMINATE_ACTOR,
    },
    BusEvent, CloudEvent,
};
use nats;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, SystemTime};

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

use crate::inthost::{CORELABEL_ARCH, CORELABEL_OS};
use latticeclient::controlplane::{
    LaunchProviderCommand, ProviderAuctionRequest, ProviderAuctionResponse,
    TerminateProviderCommand, LAUNCH_PROVIDER, PROVIDER_AUCTION_REQ, TERMINATE_PROVIDER,
};
use latticeclient::*;
use nats::Message;
use std::fs::File;
use std::path::Path;
use wapc::WasiParams;
use wascap::prelude::KeyPair;

#[derive(Debug, Clone)]
pub(crate) enum ControlCommand {
    TerminateActor(TerminateCommand),
    TerminateProvider(TerminateProviderCommand),
    StartActor(LaunchCommand, Message),
    StartProvider(LaunchProviderCommand, Message),
}

pub(crate) struct DistributedBus {
    nc: Arc<RwLock<Option<nats::Connection>>>,
    subs: Arc<RwLock<HashMap<String, nats::subscription::Handler>>>,
    terminators: Arc<RwLock<HashMap<String, Sender<bool>>>>,
    req_timeout: Duration,
    host_id: String,
    lc: Arc<RwLock<latticeclient::Client>>,
    pub(crate) ns: Option<String>,
    claims: Arc<RwLock<HashMap<String, Claims<Actor>>>>,
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
        image_map: Arc<RwLock<HashMap<String, String>>>,
    ) -> Self {
        let con = get_connection();
        let to = get_timeout();
        let lc = Arc::new(RwLock::new(latticeclient::Client::with_connection(
            con.clone(),
            to,
            ns.clone(),
        )));
        let nc = Arc::new(RwLock::new(Some(con)));

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
            image_map.clone(),
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
            image_map.clone(),
        )
        .unwrap();
        DistributedBus {
            nc,
            subs: Arc::new(RwLock::new(HashMap::new())),
            terminators,
            req_timeout: to,
            host_id,
            lc,
            ns: ns.clone(),
            claims,
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
        std::thread::sleep(Duration::from_millis(300));
        let mut lock = self.nc.write().unwrap();
        let conn = lock.take();
        if let Some(nc) = conn {
            nc.close();
        }
    }

    pub fn instance_count(&self, actor: &str) -> Result<usize> {
        match self.lc.read().unwrap().get_actors() {
            Ok(res) => {
                let count = res.values().into_iter().fold(0, |acc, x| {
                    acc + x
                        .iter()
                        .filter(|c| c.subject == actor)
                        .collect::<Vec<_>>()
                        .len()
                });
                Ok(count)
            }
            Err(e) => Err(format!("Failed to determine instance count for actor: {}", e).into()),
        }
    }

    pub fn discover_claims(&self, actor: &str) -> Option<Claims<wascap::jwt::Actor>> {
        let res = match self.lc.read().unwrap().get_actors() {
            Ok(res) => {
                let flattened = res.values().into_iter().flatten().collect::<Vec<_>>();
                let mut claims = flattened.into_iter().filter(|c| c.subject == actor).take(1);
                match claims.next() {
                    Some(c) => Some(c.clone()),
                    None => None,
                }
            }
            Err(_e) => None,
        };
        if res.is_some() {
            res
        } else {
            self.claims.read().unwrap().get(actor).cloned()
        }
    }

    pub fn query_bindings(&self) -> Result<Vec<latticeclient::Binding>> {
        match self.lc.read().unwrap().get_bindings() {
            Ok(r) => {
                let v: Vec<_> = r.values().fold(vec![], |mut acc, x| {
                    acc.extend_from_slice(x);
                    acc
                });
                Ok(v)
            }
            Err(e) => Err(format!("Failed to query bindings from lattice : {}", e).into()),
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

    pub fn nqsubscribe(
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
            .subscribe(subject)?
            .with_handler(move |msg| {
                handle_invocation(&msg, sender.clone(), receiver.clone());
                Ok(())
            });
        self.subs.write().unwrap().insert(subject.to_string(), sub);
        Ok(())
    }

    pub fn invoke(&self, subject: &str, inv: Invocation) -> Result<InvocationResponse> {
        if self.nc.read().unwrap().as_ref().is_none() {
            error!(
                "Attempted bus invoke with no bus connection: {} {:?}->{:?}",
                inv.operation, inv.origin, inv.target
            );
            Err(crate::errors::new(crate::errors::ErrorKind::MiscHost(
                "Attempted a bus invocation without a live bus connection".to_string(),
            )))
        } else {
            let resp = self.nc.read().unwrap().as_ref().unwrap().request_timeout(
                &subject,
                &serialize(inv)?,
                self.req_timeout,
            )?;
            let ir: InvocationResponse = deserialize(&resp.data)?;
            Ok(ir)
        }
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
    let plugins = host.plugins.clone();
    let mids = host.middlewares.clone();
    let caps = host.caps.clone();
    let bindings = host.bindings.clone();
    let claimsmap = host.claims.clone();
    let terminators = host.terminators.clone();
    let hk = KeyPair::from_seed(&host.sk).unwrap();
    let auth = host.authorizer.clone();
    let image_map = host.image_map.clone();
    let labels = host.labels.clone();

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
        let key = KeyPair::from_seed(&hk.seed().unwrap()).unwrap();
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
                            // As of 0.14.0, the "actor_id" here is actually an OCI registry image reference
                            match crate::inthost::fetch_actor(&cmd.actor_id) {
                                Ok(a) => {
                                    image_map.write().unwrap().insert(cmd.actor_id.to_string(), a.public_key());
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
                                        key, auth.clone(), image_map.clone(), Some(cmd.actor_id.to_string()));


                                },
                                Err(e) => {
                                    error!("Actor download failed for {}: {}", &cmd.actor_id, e)
                                }
                            }
                        },
                        ControlCommand::TerminateActor(cmd) => {
                            let pk = image_map.read().unwrap()[&cmd.actor_id].to_string(); // get PK from the OCI ref
                            image_map.write().unwrap().remove(&pk);
                            let actor_subject = bus.actor_subject(&pk);
                            terminators.read().unwrap()
                                [&actor_subject]
                                .send(true)
                                .unwrap();
                        },
                        ControlCommand::TerminateProvider(cmd) => {
                            // TODO: this command will continue to be a no-op until the "async rewrite",
                            // which should include a re-organization of how providers subscribe to the bus
                        },
                        ControlCommand::StartProvider(cmd, msg) => {
                            if let Err(e) = msg.respond(&serde_json::to_vec(&ProviderLaunchAck {
                                provider_ref: cmd.provider_ref.to_string(),
                                host: hk.public_key()
                            }).unwrap()) {
                                error!("Failed to send schedule provider acknowledgement reply: {}", e);
                            } else {
                                info!("Acknowledged provider start request.");
                            }
                            match crate::inthost::fetch_provider(&cmd.provider_ref, &cmd.binding_name, labels.clone()) {
                                Ok((p, c)) => {
                                    if caps
                                       .read()
                                       .unwrap()
                                       .contains_key(&RouteKey::new(&cmd.binding_name, &p.id()))
                                    {
                                        error!(
                                          "Capability provider {} cannot be bound to the same name ({}) twice, loading failed.", p.id(), &cmd.binding_name
                                        );
                                    }
                                    caps.write().unwrap().insert(
                                        RouteKey::new(&cmd.binding_name, &p.descriptor.id),
                                        p.descriptor().clone(),
                                    );
                                    let wg = crossbeam_utils::sync::WaitGroup::new();
                                    let key = KeyPair::from_seed(&hk.seed().unwrap()).unwrap();
                                    let _ = crate::spawns::spawn_native_capability(
                                        p,
                                        bus.clone(),
                                        mids.clone(),
                                        bindings.clone(),
                                        terminators.clone(),
                                        plugins.clone(),
                                        wg.clone(),
                                        Arc::new(key),
                                    );
                                    wg.wait();
                                },
                                Err(e) => {
                                    error!("Provider download failed to {}: {}", &cmd.provider_ref, e)
                                }
                            }
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

// This thread handles control plane commands or demands, e.g. "launch actor" and "launch provider"
// It also responds to provider and actor auctions
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
    image_map: Arc<RwLock<HashMap<String, String>>>,
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
                if !image_map.read().unwrap().contains_key(&tc.actor_id) {
                    // actor IDs are OCI image references in the requests
                    warn!("Received request to terminate non-existent actor. Ignoring.");
                } else {
                    cplane_s.send(ControlCommand::TerminateActor(tc)).unwrap();
                }
            } else if msg.subject.ends_with(LAUNCH_PROVIDER) && msg.subject.contains(&host_id) {
                // schedule the provider
                let lc: LaunchProviderCommand =serde_json::from_slice(&msg.data).unwrap();
                cplane_s.send(ControlCommand::StartProvider(lc, msg)).unwrap();
            } else if msg.subject.ends_with(TERMINATE_PROVIDER) && msg.subject.contains(&host_id) {
                let tc: TerminateProviderCommand = serde_json::from_slice(&msg.data).unwrap();
                if !image_map.read().unwrap().contains_key(&tc.provider_ref) {
                    warn!("Received request to terminate non-existent provider. Ignoring.");
                } else {
                    cplane_s.send(ControlCommand::TerminateProvider(tc)).unwrap();
                }
            } else if msg.subject.ends_with(PROVIDER_AUCTION_REQ) { // ** WARNING ** ORDER OF COMPARISON IS IMPORTANT HERE
                let req: ProviderAuctionRequest = serde_json::from_slice(&msg.data)?;
                if image_map.read().unwrap().contains_key(&req.provider_ref) {
                    trace!("Skipping provider auction response - provider is in local image map");
                } else {
                    if !host_satifies_constraints(labels.clone(), &req.constraints) {
                        trace!("Skipping provider auction response - host does not satisfy constraints.");
                    } else {
                        let ar = ProviderAuctionResponse {
                          provider_ref: req.provider_ref.to_string(),
                            host_id: host_id.to_string(),
                        };
                        info!("Responding to provider schedule auction request");
                        let _ = msg.respond(serde_json::to_vec(&ar).unwrap());
                    }
                }
            } else if msg.subject.ends_with(AUCTION_REQ) {
                let req: LaunchAuctionRequest = serde_json::from_slice(&msg.data)?;
                if image_map.read().unwrap().contains_key(&req.actor_id) {
                    trace!("Skipping auction response - actor already running locally.");
                } else {
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
    image_map: Arc<RwLock<HashMap<String, String>>>,
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
    let actorlist = {
        let lock = claims.read().unwrap();
        lock.values().cloned().collect()
    };
    let ir = InventoryResponse::Actors {
        host,
        actors: actorlist,
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
        if let Ok(()) = sender.send(inv) {
            let inv_r = receiver.recv().unwrap();
            msg.respond(serialize(inv_r).unwrap()).unwrap();
        } else {
            warn!("Received invocation but its destination thread is no longer running.");
        }
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
