use std::collections::HashMap;
use wascc_host::{Actor, Host, NativeCapability};

fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();
    let host = Host::new();
    host.add_actor(Actor::from_file("./examples/.assets/extras.wasm")?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;

    host.set_binding(
        "MDOYAT2KHJ6N5DAY5X7JKGIBMKABTPXRX2KHUJI6APOVNKQDMRTIUSY2",
        "wascc:http_server",
        None,
        http_config(),
    )?;

    std::thread::park();

    Ok(())
}

fn http_config() -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("PORT".to_string(), "8081".to_string());

    hm
}
