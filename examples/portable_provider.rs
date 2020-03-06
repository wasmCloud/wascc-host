use std::collections::HashMap;
use wascc_host::{host, Actor, NativeCapability, WasiParams};

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    host::add_actor(Actor::from_file("./examples/.assets/wasi_consumer.wasm")?)?;
    host::add_capability(
        Actor::from_file("./examples/.assets/wasi_provider.wasm")?,
        WasiParams::default(),
    )?; // WASI default does not map file system access

    host::add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
    )?)?;

    host::configure(
        "MDNPIQOU5EEHTP4TKY2APFOJTTEYYARN3ZIJTRWRYWHX6B4MFSO6ZCRT",
        "wascc:wasidemo",
        HashMap::new(),
    )?;

    host::configure(
        "MDNPIQOU5EEHTP4TKY2APFOJTTEYYARN3ZIJTRWRYWHX6B4MFSO6ZCRT",
        "wascc:http_server",
        generate_port_config(8081),
    )?;

    std::thread::park();

    Ok(())
}

fn generate_port_config(port: u16) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("PORT".to_string(), port.to_string());

    hm
}
