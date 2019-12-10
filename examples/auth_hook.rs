use std::collections::HashMap;
use wascap::jwt::Token;
use wascc_host::{host, Actor, Capability};

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    host::set_auth_hook(verify_issuer); // This MUST be set before you load an actor, otherwise it won't run
    host::add_actor(Actor::from_file("./examples/.assets/echo.wasm")?)?;
    host::add_native_capability(Capability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
    )?)?;

    host::configure(
        "MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2",
        "wascc:http_server",
        generate_port_config(8081),
    )?;

    std::thread::park();

    Ok(())
}

fn verify_issuer(token: &Token) -> bool {
    token.claims.issuer == "AAGRUXXTGSP4C27RWPTMCHCJF56JD53EQPA2R7RPC5VI4E274KPRMMJ5"
    //token.claims.issuer == "some unknown account here"
}

fn generate_port_config(port: u16) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("PORT".to_string(), port.to_string());

    hm
}
