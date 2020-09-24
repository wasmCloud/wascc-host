use std::collections::HashMap;
use wascc_host::{Actor, Host, NativeCapability};

fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();
    let host = Host::new();
    host.add_actor(Actor::from_file("./examples/.assets/as-actor.wasm")?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_redis.so",
        None,
    )?)?;

    host.set_binding(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:http_server",
        None,
        generate_port_config(8081),
    )?;

    host.set_binding(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:keyvalue",
        None,
        redis_config(),
    )?;

    std::thread::park();

    Ok(())
}

fn generate_port_config(port: u16) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("PORT".to_string(), port.to_string());

    hm
}

fn redis_config() -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("URL".to_string(), "redis://127.0.0.1:6379".to_string());

    hm
}
