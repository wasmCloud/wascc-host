use crate::{BindingsList, RouteKey};
use crate::{Invocation, InvocationResponse, Result};
use crossbeam::{Receiver, Sender};
use latticeclient::{BusEvent, CloudEvent};
use nats;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};
use wascap::jwt::{Actor, Claims};
use wascc_codec::{capabilities::CapabilityDescriptor, deserialize, serialize};

const LATTICE_HOST_KEY: &str = "LATTICE_HOST"; // env var name
const DEFAULT_LATTICE_HOST: &str = "127.0.0.1"; // default mode is anonymous via loopback
const LATTICE_RPC_TIMEOUT_KEY: &str = "LATTICE_RPC_TIMEOUT_MILLIS";
const DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS: u64 = 600;
const LATTICE_CREDSFILE_KEY: &str = "LATTICE_CREDS_FILE";

const TERM_BACKOFF_MAX_TRIES: u8 = 3;
const TERM_BACKOFF_DELAY_MS: u64 = 50;

use crate::bus::event_subject;
use latticeclient::*;

pub(crate) struct DistributedBus {
    nc: Arc<RwLock<Option<nats::Connection>>>,
    subs: Arc<RwLock<HashMap<String, nats::subscription::Handler>>>,
    terminators: Arc<RwLock<HashMap<String, Sender<bool>>>>,
    req_timeout: Duration,
    host_id: String,
    ns: Option<String>,
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
                )))
            }
        };
        let lock = self.nc.read().unwrap();
        if let Some(ref nc) = lock.as_ref() {
            let _ = nc.publish(&*self.event_subject(), &payload);
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
