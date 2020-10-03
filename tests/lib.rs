use std::error::Error;

mod auth;
mod common;
mod core;
#[cfg(feature = "lattice")]
mod lattice;
mod load;

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
fn fs_host_error() -> Result<(), Box<dyn Error>> {
    core::fs_host_error()
}

#[test]
#[cfg(feature = "lattice")]
fn unload_reload_actor_retains_bindings() -> Result<(), Box<dyn Error>> {
    lattice::unload_reload_actor_retains_bindings()
}

#[test]
#[cfg(feature = "lattice")]
fn can_bind_from_any_host() -> Result<(), Box<dyn Error>> {
    lattice::can_bind_from_any_host()
}

#[test]
#[cfg(feature = "lattice")]
fn instance_count() -> Result<(), Box<dyn Error>> {
    lattice::instance_count()
}

#[test]
#[cfg(feature = "lattice")]
fn lattice_single_host() -> Result<(), Box<dyn Error>> {
    lattice::lattice_single_host()
}

#[test]
#[cfg(feature = "lattice")]
fn lattice_isolation() -> Result<(), Box<dyn Error>> {
    lattice::lattice_isolation()
}

#[test]
#[cfg(feature = "lattice")]
fn lattice_events() -> Result<(), Box<dyn Error>> {
    lattice::lattice_events()
}

//#[test]
//fn simple_load() -> Result<(), Box<dyn Error>> {
//    load::simple_load()
//}

#[test]
fn actor_only_load() -> Result<(), Box<dyn Error>> {
    load::actor_only_load()
}
