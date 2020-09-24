use std::collections::HashMap;
use wascc_host::{Actor, Host, NativeCapability};

fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();
    let host = Host::new();
    host.add_actor(Actor::from_file("./examples/.assets/logger.wasm")?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_logging.so",
        None,
    )?)?;

    host.set_binding(
        "MDW7BWQDVYBRC6WKSJRRZL27R73EVBWQINYLPFDRCWDZDFQO4JMO4U6J",
        "wascc:http_server",
        None,
        generate_port_config(8081),
    )?;

    // As of waSCC 0.9.0, an actor cannot communicate with a capability
    // provider _at all_ unless an explicit bind takes place, even if there is
    // no configuration data. This is because bindings in 0.9.0 can be global
    // entities, spanning clouds, data centers, and devices.
    host.set_binding(
        "MDW7BWQDVYBRC6WKSJRRZL27R73EVBWQINYLPFDRCWDZDFQO4JMO4U6J",
        "wascc:logging",
        None,
        HashMap::new(),
    )?;

    std::thread::park();

    Ok(())
}

fn generate_port_config(port: u16) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("PORT".to_string(), port.to_string());

    hm
}
