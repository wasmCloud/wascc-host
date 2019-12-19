use std::collections::HashMap;
use wascc_host::{host, Actor, NativeCapability};

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    host::add_actor(Actor::from_file("./examples/.assets/subscriber.wasm")?)?;
    host::add_actor(Actor::from_file("./examples/.assets/subscriber2.wasm")?)?;
    host::add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libnats_provider.so",
    )?)?;

    host::configure(
        "MBHRSJORBXAPRCALK6EKOBBCNAPMRTM6ODLXNLOV5TKPDMPXMTCMR4DW",
        "wascc:messaging",
        generate_config("test"),
    )?;

    host::configure(
        "MDJUPIQFWEHWE4XHPWHOJLW42SJDPVBQVDC2NV3T3O4ELXXVOXLA5M4I",
        "wascc:messaging",
        generate_config("second_test"),
    )?;
    std::thread::park();

    Ok(())
}

fn generate_config(sub: &str) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("SUBSCRIPTION".to_string(), sub.to_string());
    hm.insert("URL".to_string(), "nats://localhost:4222".to_string());

    hm
}
