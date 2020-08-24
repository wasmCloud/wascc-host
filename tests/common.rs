use std::{collections::HashMap, error::Error};
use wascc_host::{Actor, NativeCapability, WasccHost};

pub fn get_hello_actor() -> Result<Actor, Box<dyn Error>> {
    Actor::from_file("./examples/.assets/echo.wasm").map_err(|e| e.into())
}

pub fn get_hello2_actor() -> Result<Actor, Box<dyn Error>> {
    Actor::from_file("./examples/.assets/echo2.wasm").map_err(|e| e.into())
}

pub fn gen_stock_host(first_port: u16) -> Result<WasccHost, Box<dyn Error>> {
    let host = WasccHost::new();
    host.add_actor(get_hello_actor()?)?;
    host.add_actor(get_hello2_actor()?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        Some("stockhost".to_string()),
    )?)?;

    host.bind_actor(
        "MDFD7XZ5KBOPLPHQKHJEMPR54XIW6RAG5D7NNKN22NP7NSEWNTJZP7JN",
        "wascc:http_server",
        Some("stockhost".to_string()),
        generate_port_config(first_port),
    )?;

    host.bind_actor(
        "MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2",
        "wascc:http_server",
        Some("stockhost".to_string()),
        generate_port_config(first_port + 1),
    )?;

    Ok(host)
}

pub fn gen_kvcounter_host(port: u16, host: WasccHost) -> Result<WasccHost, Box<dyn Error>> {
    host.add_actor(Actor::from_file("./examples/.assets/kvcounter.wasm")?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_redis.so",
        None,
    )?)?;

    host.bind_actor(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:keyvalue",
        None,
        redis_config(),
    )?;
    host.bind_actor(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:http_server",
        None,
        generate_port_config(port),
    )?;

    Ok(host)
}

pub fn redis_config() -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("URL".to_string(), "redis://127.0.0.1:6379".to_string());

    hm
}

pub fn empty_config() -> HashMap<String, String> {
    HashMap::new()
}

pub fn generate_port_config(port: u16) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("PORT".to_string(), port.to_string());

    hm
}
