use std::collections::HashMap;
use std::fs::File;
use std::io::prelude::*;
use wascc_host::host;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    host::add_actor(load_wasm("./examples/.assets/kvcounter.wasm")?)?;
    host::add_native_capability("./examples/.assets/libwascc_httpsrv.so")?;
    host::add_native_capability("./examples/.assets/libredis_provider.so")?;

    host::configure(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:keyvalue",
        redis_config(),
    )?;
    host::configure(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:http_server",
        http_config(),
    )?;

    std::thread::park();

    Ok(())
}

fn redis_config() -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("URL".to_string(), "redis://127.0.0.1:6379".to_string());

    hm
}

fn http_config() -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("PORT".to_string(), "8081".to_string());

    hm
}

fn load_wasm(path: &str) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut file = File::open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    Ok(buf)
}
