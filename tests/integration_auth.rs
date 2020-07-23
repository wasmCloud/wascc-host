#[allow(dead_code)]
mod common;

use reqwest;
use std::error::Error;
use wascc_host::{Actor, Authorizer, NativeCapability, WasccHost};

#[test]
fn authorizer_blocks_actor_to_cap() -> Result<(), Box<dyn Error>> {
    let host = common::gen_kvcounter_host(8084)?;
    // Important note: we don't set the authorizer until _after_ the `bind_actor` function
    // has been called, which allows the binding to take place. If we set the authorizer prior,
    // then we'd get an Err result from bind_actor (permission denied). Instead, we want the
    // permission denied to only happen when we invoke the HTTP request handler so we can test
    // that edge case.

    host.set_authorizer(DenyAuthorizer::new(false, true))?;
    let key = uuid::Uuid::new_v4().to_string();
    let url = format!("http://localhost:8084/{}", key);
    let resp = reqwest::blocking::get(&url)?;
    assert!(
        &resp.status().is_server_error(),
        "Did not get correct status code, code was {}",
        resp.status().as_u16()
    );
    assert_eq!(resp.text()?, "Failed to handle HTTP request");

    host.shutdown()?;
    std::thread::sleep(::std::time::Duration::from_millis(100));

    Ok(())
}

#[test]
fn default_authorizer_enforces_cap_attestations() -> Result<(), Box<dyn Error>> {
    // Attempt to bind an actor to a capability for which it isn't authorized.
    let host = common::gen_kvcounter_host(8085)?;
    let res = host.bind_actor(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:messaging",
        None,
        common::empty_config(),
    );
    assert!(res.is_err());
    Ok(())
}

#[test]
fn authorizer_blocks_bindings() -> Result<(), Box<dyn Error>> {
    // Set the authorizer before calling a bind_actor, and bind_actor should
    // return permission denied / Err if the authorizer denies that invocation.
    let host = WasccHost::new();
    host.set_authorizer(DenyAuthorizer::new(false, true))?;
    host.add_actor(Actor::from_file("./examples/.assets/kvcounter.wasm")?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_redis.so",
        None,
    )?)?;

    let res = host.bind_actor(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:keyvalue",
        None,
        common::redis_config(),
    );

    assert!(res.is_err());
    Ok(())
}

#[test]
fn authorizer_blocks_load() -> Result<(), Box<dyn Error>> {
    // Set the authorizer before calling a bind_actor, and bind_actor should
    // return permission denied / Err if the authorizer denies that invocation.
    let host = WasccHost::new();
    host.set_authorizer(DenyAuthorizer::new(true, false))?;
    let res = host.add_actor(Actor::from_file("./examples/.assets/kvcounter.wasm")?);

    assert!(res.is_err());
    Ok(())
}

struct DenyAuthorizer {
    deny_load: bool,
    deny_invoke: bool,
}

impl DenyAuthorizer {
    pub(crate) fn new(deny_load: bool, deny_invoke: bool) -> DenyAuthorizer {
        DenyAuthorizer {
            deny_load,
            deny_invoke,
        }
    }
}
impl Authorizer for DenyAuthorizer {
    fn can_load(&self, _claims: &wascap::prelude::Claims<wascap::prelude::Actor>) -> bool {
        !self.deny_load
    }
    fn can_invoke(
        &self,
        _claims: &wascap::prelude::Claims<wascap::prelude::Actor>,
        _target: &wascc_host::WasccEntity,
        _operation: &str,
    ) -> bool {
        !self.deny_invoke
    }
}
