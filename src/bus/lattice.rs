// Copyright 2015-2020 Capital One Services, LLC
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

use crate::{BindingsList, RouteKey};
use crate::{Invocation, InvocationResponse, Result};
use crossbeam::{Receiver, Sender};
use nats;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use wascap::jwt::{Actor, Claims};
use wascc_codec::{capabilities::CapabilityDescriptor, deserialize, serialize};

const LATTICE_HOST_KEY: &str = "LATTICE_HOST"; // env var name
const DEFAULT_LATTICE_HOST: &str = "127.0.0.1"; // default mode is anonymous via loopback
const LATTICE_RPC_TIMEOUT_KEY: &str = "LATTICE_RPC_TIMEOUT_MILLIS";
const DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS: u64 = 500;
const LATTICE_CREDSFILE_KEY: &str = "LATTICE_CREDS_FILE";

const INVENTORY_SUBJECT: &str = "wasmbus.inventory.*";

use latticeclient::*;

pub(crate) struct DistributedBus {
    nc: Arc<nats::Connection>,
    subs: Arc<RwLock<HashMap<String, nats::subscription::Handler>>>,
    req_timeout: Duration,
}

impl DistributedBus {
    pub fn new(
        host_id: String,
        claims: Arc<RwLock<HashMap<String, Claims<Actor>>>>,
        caps: Arc<RwLock<HashMap<RouteKey, CapabilityDescriptor>>>,
        bindings: Arc<RwLock<BindingsList>>,
    ) -> Self {
        let nc = Arc::new(get_connection());

        info!("Initialized Message Bus (lattice)");
        spawn_inventory_handler(
            nc.clone(),
            host_id,
            claims.clone(),
            bindings.clone(),
            caps.clone(),
        )
        .unwrap();
        DistributedBus {
            nc,
            subs: Arc::new(RwLock::new(HashMap::new())),
            req_timeout: get_timeout(),
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
            .queue_subscribe(subject, subject)?
            .with_handler(move |msg| {
                handle_invocation(&msg, sender.clone(), receiver.clone());
                Ok(())
            });
        self.subs.write().unwrap().insert(subject.to_string(), sub);
        Ok(())
    }

    pub fn invoke(&self, subject: &str, inv: Invocation) -> Result<InvocationResponse> {
        let resp = self
            .nc
            .request_timeout(&subject, &serialize(inv)?, self.req_timeout)?;
        let ir: InvocationResponse = deserialize(&resp.data)?;
        Ok(ir)
    }

    pub fn unsubscribe(&self, subject: &str) -> Result<()> {
        if let Some(sub) = self.subs.write().unwrap().remove(subject) {
            sub.unsubscribe()?;
        }
        Ok(())
    }
}

fn spawn_inventory_handler(
    nc: Arc<nats::Connection>,
    host_id: String,
    claims: Arc<RwLock<HashMap<String, Claims<Actor>>>>,
    bindings: Arc<RwLock<BindingsList>>,
    caps: Arc<RwLock<HashMap<RouteKey, CapabilityDescriptor>>>,
) -> Result<()> {
    let _ = nc.subscribe(INVENTORY_SUBJECT)?.with_handler(move |msg| {
        trace!("Handling Inventory Request");
        match msg.subject.clone().as_str() {
            INVENTORY_HOSTS => respond_with_host(msg, host_id.to_string()),
            INVENTORY_ACTORS => respond_with_actors(msg, host_id.to_string(), claims.clone()),
            INVENTORY_BINDINGS => respond_with_bindings(msg, host_id.to_string(), bindings.clone()),
            INVENTORY_CAPABILITIES => respond_with_caps(msg, host_id.to_string(), caps.clone()),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Bad inventory topic!",
            )),
        }
    });
    Ok(())
}

fn respond_with_host(
    msg: nats::Message,
    host_id: String,
) -> std::result::Result<(), std::io::Error> {
    msg.respond(serde_json::to_vec(&InventoryResponse::Host(host_id)).unwrap())
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
            binding_name: k.0.to_string(),
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
        nats::ConnectionOptions::with_credentials(creds)
    } else {
        nats::ConnectionOptions::new()
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
