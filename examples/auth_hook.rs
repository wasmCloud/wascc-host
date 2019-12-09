use std::collections::HashMap;
use std::fs::File;
use std::io::prelude::*;
use wascc_host::host;
use wascap::jwt::Claims;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    host::set_auth_hook(verify_issuer); // This MUST be set before you load an actor, otherwise it won't run
    host::add_actor(load_wasm("./examples/.assets/echo.wasm")?)?;    
    host::add_native_capability("./examples/.assets/libwascc_httpsrv.so")?;

    
    host::configure(
        "MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2",
        "wascc:http_server",
        generate_port_config(8081),
    )?;

    std::thread::park();

    Ok(())
}

fn verify_issuer(claims: &Claims) -> bool {
    claims.issuer == "AAGRUXXTGSP4C27RWPTMCHCJF56JD53EQPA2R7RPC5VI4E274KPRMMJ5"
    //claims.issuer == "some unknown account here"
}

fn generate_port_config(port: u16) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("PORT".to_string(), port.to_string());

    hm
}

fn load_wasm(path: &str) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut file = File::open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    Ok(buf)
}
