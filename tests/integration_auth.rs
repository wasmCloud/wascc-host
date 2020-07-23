#[allow(dead_code)]
mod common;

use std::error::Error;
use wascc_host::{Actor, Authorizer, NativeCapability, WasccHost};

#[test]
fn default_authorizer_enforces_cap_attestations() -> Result<(), Box<dyn Error>> {
    // Attempt to bind an actor to a capability for which it isn't authorized.
    let host = WasccHost::new();
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
    let host = WasccHost::with_authorizer(DenyAuthorizer::new(false, true));

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
    let host = WasccHost::with_authorizer(DenyAuthorizer::new(true, false));
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
