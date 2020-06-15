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
pub(crate) fn new() -> MessageBus {
    lattice::DistributedBus::new()
}

pub(crate) fn actor_subject(actor: &str) -> String {
    format!("wasmbus.actor.{}", actor)
}

pub(crate) fn provider_subject(capid: &str, binding: &str) -> String {
    format!("wasmbus.provider.{}.{}", normalize_capid(capid), binding)
}

// By convention most of the waSCC ecosystem uses a "group:item" string
// for the capability IDs, e.g. "wascc:messaging" or "gpio:relay". To
// accommodate message broker subjects that might not work with the ":"
// character, we normalize the segments to dot-separated.
fn normalize_capid(capid: &str) -> String {
    capid.to_lowercase().replace(":", ".")
}

pub(crate) fn provider_subject_bound_actor(
    capid: &str,
    binding: &str,
    calling_actor: &str,
) -> String {
    format!(
        "wasmbus.provider.{}.{}.{}",
        normalize_capid(capid),
        binding,
        calling_actor
    )
}
