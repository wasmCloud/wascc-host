use std::collections::HashMap;
use std::fs::File;
use std::io::prelude::*;
use wascc_host::host;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    host::add_actor(load_wasm("./examples/.assets/subscriber.wasm")?)?;
    host::add_actor(load_wasm("./examples/.assets/subscriber2.wasm")?)?;
    host::add_native_capability("./examples/.assets/libnats_provider.so")?;

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

fn load_wasm(path: &str) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut file = File::open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    Ok(buf)
}
