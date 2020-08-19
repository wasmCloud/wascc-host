use std::error::Error;

mod auth;
mod common;
mod core;
#[cfg(feature = "lattice")]
mod lattice;

#[test]
fn default_authorizer_enforces_cap_attestations() -> Result<(), Box<dyn Error>> {
    auth::default_authorizer_enforces_cap_attestations()
}

#[test]
fn authorizer_blocks_bindings() -> Result<(), Box<dyn Error>> {
    auth::authorizer_blocks_bindings()
}

#[test]
fn authorizer_blocks_load() -> Result<(), Box<dyn Error>> {
    auth::authorizer_blocks_load()
}

#[test]
fn stock_host() -> Result<(), Box<dyn Error>> {
    core::stock_host()
}

#[test]
fn kv_host() -> Result<(), Box<dyn Error>> {
    core::kv_host()
}

#[test]
#[cfg(feature = "lattice")]
fn lattice_single_host() -> Result<(), Box<dyn Error>> {
    lattice::lattice_single_host()
}

#[test]
#[cfg(feature = "lattice")]
fn lattice_events() -> Result<(), Box<dyn Error>> {
    lattice::lattice_events()
}
