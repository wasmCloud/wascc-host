use std::collections::HashMap;
use wascc_host::{host, Actor, NativeCapability};

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    host::add_actor(Actor::from_file("./examples/.assets/echo.wasm")?)?;
    host::add_actor(Actor::from_file("./examples/.assets/echo2.wasm")?)?;
    host::add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
    )?)?;

    host::configure(
        "MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2",
        "wascc:http_server",
        generate_port_config(8081),
    )?;
    host::configure(
        "MDFD7XZ5KBOPLPHQKHJEMPR54XIW6RAG5D7NNKN22NP7NSEWNTJZP7JN",
        "wascc:http_server",
        generate_port_config(8082),
    )?;

    host::remove_actor("MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2")?;

    std::thread::park();

    Ok(())
}

fn generate_port_config(port: u16) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("PORT".to_string(), port.to_string());

    hm
}
