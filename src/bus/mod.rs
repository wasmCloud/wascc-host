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

pub const URL_SCHEME: &str = "wasmbus";

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
pub(crate) fn normalize_capid(capid: &str) -> String {
    capid.to_lowercase().replace(":", ".").replace(" ", "_")
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
