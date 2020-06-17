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

// Implementations of support functions for the `WasccHost` struct

use super::WasccHost;
use crate::Result;

use crate::bus;
use crate::bus::MessageBus;
use crate::BindingsList;
use crate::{authz, errors, Actor, NativeCapability};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use uuid::Uuid;
use wapc::prelude::*;
use wascap::jwt::Claims;
use wascc_codec::{
    capabilities::{CapabilityDescriptor, OP_GET_CAPABILITY_DESCRIPTOR},
    core::{CapabilityConfiguration, OP_BIND_ACTOR, OP_PERFORM_LIVE_UPDATE, OP_REMOVE_ACTOR},
    deserialize, serialize, SYSTEM_ACTOR,
};

pub(crate) fn unsub_all_bindings(
    bindings: Arc<RwLock<BindingsList>>,
    bus: Arc<MessageBus>,
    capid: &str,
) {
    bindings
        .read()
        .unwrap()
        .iter()
        .filter(|(_a, c, _b)| c == capid)
        .for_each(|(a, c, b)| {
            let _ = bus.unsubscribe(&bus::provider_subject_bound_actor(c, b, a));
        });
}

impl WasccHost {
    pub(crate) fn record_binding(&self, actor: &str, capid: &str, binding: &str) -> Result<()> {
        let mut lock = self.bindings.write().unwrap();
        lock.push((actor.to_string(), capid.to_string(), binding.to_string()));
        trace!(
            "Actor {} successfully bound to {},{}",
            actor,
            binding,
            capid
        );
        Ok(())
    }

    pub(crate) fn ensure_extras(&self) -> Result<()> {
        self.add_native_capability(NativeCapability::from_instance(
            crate::extras::ExtrasCapabilityProvider::default(),
            None,
        )?)?;
        Ok(())
    }
}

/// In the case of a portable capability provider, obtain its capability descriptor
pub(crate) fn get_descriptor(host: &mut WapcHost) -> Result<CapabilityDescriptor> {
    let msg = wascc_codec::core::HealthRequest { placeholder: false }; // TODO: eventually support sending an empty slice for this
    let res = host.call(OP_GET_CAPABILITY_DESCRIPTOR, &serialize(&msg)?)?;
    deserialize(&res).map_err(|e| e.into())
}

pub(crate) fn remove_cap(
    caps: Arc<RwLock<HashMap<crate::RouteKey, CapabilityDescriptor>>>,
    capid: &str,
    binding: &str,
) {
    caps.write()
        .unwrap()
        .remove(&(binding.to_string(), capid.to_string()));
}

/// Puts a "live update" message into the dispatch queue, which will be handled
/// as soon as it is pulled off the channel for the target actor
pub(crate) fn replace_actor(bus: Arc<MessageBus>, new_actor: Actor) -> Result<()> {
    let public_key = new_actor.token.claims.subject;
    let tgt_subject = crate::bus::actor_subject(&public_key);
    let inv = gen_liveupdate_invocation(&public_key, new_actor.bytes);

    match bus.invoke(&tgt_subject, inv) {
        Ok(_) => {
            info!("Actor {} replaced", public_key);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub(crate) fn live_update(guest: &mut WapcHost, inv: &Invocation) -> InvocationResponse {
    match guest.replace_module(&inv.msg) {
        Ok(_) => InvocationResponse::success(inv, vec![]),
        Err(e) => {
            error!("Failed to perform hot swap, ignoring message: {}", e);
            InvocationResponse::error(inv, "Failed to perform hot swap")
        }
    }
}

fn gen_liveupdate_invocation(target: &str, bytes: Vec<u8>) -> Invocation {
    Invocation::new(
        SYSTEM_ACTOR.to_string(),
        InvocationTarget::Actor(target.to_string()),
        OP_PERFORM_LIVE_UPDATE,
        bytes,
    )
}

/// Removes all bindings for a given actor by sending the "deconfigure" message
/// to each of the capabilities
pub(crate) fn deconfigure_actor(
    bus: Arc<MessageBus>,
    bindings: Arc<RwLock<BindingsList>>,
    key: &str,
) {
    let cfg = CapabilityConfiguration {
        module: key.to_string(),
        values: HashMap::new(),
    };
    let buf = serialize(&cfg).unwrap();
    let nbindings: Vec<_> = {
        let lock = bindings.read().unwrap();
        lock.iter()
            .filter(|(a, _cap, _bind)| a == key)
            .cloned()
            .collect()
    };

    // (actor, capid, binding)
    for (actor, capid, binding) in nbindings {
        info!("Unbinding actor {} from {},{}", actor, binding, capid);
        let _inv_r = bus.invoke(
            &bus::provider_subject_bound_actor(&capid, &binding, &actor),
            gen_remove_actor(buf.clone(), &binding, &capid),
        );
        remove_binding(bindings.clone(), key, &binding, &capid);
    }
}

/// Removes all bindings from a capability without notifying anyone
pub(crate) fn unbind_all_from_cap(bindings: Arc<RwLock<BindingsList>>, capid: &str, binding: &str) {
    let mut lock = bindings.write().unwrap();
    lock.retain(|(_a, c, b)| !(c == capid) && (binding == b));
}

pub(crate) fn remove_binding(
    bindings: Arc<RwLock<BindingsList>>,
    actor: &str,
    binding: &str,
    capid: &str,
) {
    // binding: (actor,  capid, binding)
    let mut lock = bindings.write().unwrap();
    lock.retain(|(a, c, b)| !(a == actor && b == binding && c == capid));
}

pub(crate) fn gen_remove_actor(msg: Vec<u8>, binding: &str, capid: &str) -> Invocation {
    Invocation::new(
        SYSTEM_ACTOR.to_string(),
        InvocationTarget::Capability {
            capid: capid.to_string(),
            binding: binding.to_string(),
        },
        OP_REMOVE_ACTOR,
        msg,
    )
}
/// An immutable representation of an invocation within waSCC
#[derive(Debug, Clone)]
#[cfg_attr(feature = "lattice", derive(serde::Serialize, serde::Deserialize))]
pub struct Invocation {
    pub origin: String,
    pub target: InvocationTarget,
    pub operation: String,
    pub msg: Vec<u8>,
    pub id: String,
}

/// Represents an invocation target - either an actor or a bound capability provider
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "lattice", derive(serde::Serialize, serde::Deserialize))]
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
            id: format!("{}", Uuid::new_v4()),
        }
    }
}

/// The response to an invocation
#[derive(Debug, Clone)]
#[cfg_attr(feature = "lattice", derive(serde::Serialize, serde::Deserialize))]
pub struct InvocationResponse {
    pub msg: Vec<u8>,
    pub error: Option<String>,
    pub invocation_id: String,
}

impl InvocationResponse {
    pub fn success(inv: &Invocation, msg: Vec<u8>) -> InvocationResponse {
        InvocationResponse {
            msg,
            error: None,
            invocation_id: inv.id.to_string(),
        }
    }

    pub fn error(inv: &Invocation, err: &str) -> InvocationResponse {
        InvocationResponse {
            msg: Vec::new(),
            error: Some(err.to_string()),
            invocation_id: inv.id.to_string(),
        }
    }
}

pub(crate) fn wapc_host_callback(
    claims: Claims<wascap::jwt::Actor>,
    bus: Arc<MessageBus>,
    binding: &str,
    namespace: &str,
    operation: &str,
    payload: &[u8],
) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    trace!(
        "Guest {} invoking {}:{}",
        claims.subject,
        namespace,
        operation
    );

    let capability_id = namespace;
    let inv = invocation_from_callback(&claims.subject, binding, namespace, operation, payload);

    if !authz::can_invoke(&claims, capability_id) {
        return Err(Box::new(errors::new(errors::ErrorKind::Authorization(
            format!(
                "Actor {} attempted to call {} on {},{} - PERMISSION DENIED.",
                claims.subject, operation, capability_id, binding
            ),
        ))));
    }
    // Make a request on either `wasmbus.Mxxxxx` for an actor or `wasmbus.{capid}.{binding}.{calling-actor}` for
    // a bound capability provider
    let invoke_subject = match &inv.target {
        InvocationTarget::Actor(subject) => bus::actor_subject(subject),
        InvocationTarget::Capability { capid, binding } => {
            bus::provider_subject_bound_actor(capid, binding, &claims.subject)
        }
    };
    match bus.invoke(&invoke_subject, inv) {
        Ok(inv_r) => Ok(inv_r.msg),
        Err(e) => Err(Box::new(errors::new(errors::ErrorKind::HostCallFailure(
            e.into(),
        )))),
    }
}

fn invocation_from_callback(
    origin: &str,
    bd: &str,
    ns: &str,
    op: &str,
    payload: &[u8],
) -> Invocation {
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
    Invocation::new(origin.to_string(), target, op, payload.to_vec())
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
    Invocation::new(
        SYSTEM_ACTOR.to_string(),
        InvocationTarget::Capability {
            capid: capid.to_string(),
            binding,
        },
        OP_BIND_ACTOR,
        payload,
    )
}
