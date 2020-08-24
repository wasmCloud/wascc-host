#[cfg(feature = "lattice")]
use crossbeam::Sender;

pub const URL_SCHEME: &str = "wasmbus";

#[cfg(feature = "lattice")]
use crate::{BindingsList, RouteKey};
#[cfg(feature = "lattice")]
use std::collections::HashMap;
#[cfg(feature = "lattice")]
use std::sync::{Arc, RwLock};
#[cfg(feature = "lattice")]
use wascap::jwt::{Actor, Claims};
#[cfg(feature = "lattice")]
use wascc_codec::capabilities::CapabilityDescriptor;

#[cfg(not(feature = "lattice"))]
pub(crate) mod inproc;
#[cfg(feature = "lattice")]
pub(crate) mod lattice;

#[cfg(not(feature = "lattice"))]
pub(crate) use inproc::InprocBus as MessageBus;

#[cfg(feature = "lattice")]
pub(crate) use lattice::DistributedBus as MessageBus;

#[cfg(not(feature = "lattice"))]
pub(crate) fn new() -> MessageBus {
    inproc::InprocBus::new()
}

#[cfg(feature = "lattice")]
pub(crate) fn new(
    host_id: String,
    claims: Arc<RwLock<HashMap<String, Claims<Actor>>>>,
    caps: Arc<RwLock<HashMap<RouteKey, CapabilityDescriptor>>>,
    bindings: Arc<RwLock<BindingsList>>,
    labels: Arc<RwLock<HashMap<String, String>>>,
    terminators: Arc<RwLock<HashMap<String, Sender<bool>>>>,
    ns: Option<String>,
) -> MessageBus {
    lattice::DistributedBus::new(host_id, claims, caps, bindings, labels, terminators, ns)
}

const LATTICE_NAMESPACE_ENV: &str = "LATTICE_NAMESPACE";

pub(crate) fn get_namespace_prefix() -> Option<String> {
    ::std::env::var(LATTICE_NAMESPACE_ENV).ok()
}

pub(crate) fn actor_subject(ns: Option<&str>, actor: &str) -> String {
    format!("{}.actor.{}", nsprefix(ns), actor)
}

pub(crate) fn provider_subject(ns: Option<&str>, capid: &str, binding: &str) -> String {
    format!(
        "{}.provider.{}.{}",
        nsprefix(ns),
        normalize_capid(capid),
        binding
    )
}

pub(crate) fn inventory_wildcard_subject(ns: Option<&str>) -> String {
    format!("{}.inventory.*", nsprefix(ns))
}

pub(crate) fn event_subject(ns: Option<&str>) -> String {
    format!("{}.events", nsprefix(ns))
}

// By convention most of the waSCC ecosystem uses a "group:item" string
// for the capability IDs, e.g. "wascc:messaging" or "gpio:relay". To
// accommodate message broker subjects that might not work with the ":"
// character, we normalize the segments to dot-separated.
pub(crate) fn normalize_capid(capid: &str) -> String {
    capid.to_lowercase().replace(":", ".").replace(" ", "_")
}

pub(crate) fn provider_subject_bound_actor(
    ns: Option<&str>,
    capid: &str,
    binding: &str,
    calling_actor: &str,
) -> String {
    format!(
        "{}.provider.{}.{}.{}",
        nsprefix(ns),
        normalize_capid(capid),
        binding,
        calling_actor
    )
}

fn nsprefix(ns: Option<&str>) -> String {
    match ns {
        Some(s) => format!("{}.wasmbus", s),
        None => "wasmbus".to_string(),
    }
}
