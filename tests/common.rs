use std::io::{Read, Write};
use std::{collections::HashMap, error::Error};
use wascc_host::{Actor, Host, NativeCapability};

pub fn get_hello_actor() -> Result<Actor, Box<dyn Error>> {
    Actor::from_file("./examples/.assets/echo.wasm").map_err(|e| e.into())
}

pub fn get_hello2_actor() -> Result<Actor, Box<dyn Error>> {
    Actor::from_file("./examples/.assets/echo2.wasm").map_err(|e| e.into())
}

pub fn gen_stock_host(first_port: u16) -> Result<Host, Box<dyn Error>> {
    let host = Host::new();
    host.add_actor(get_hello_actor()?)?;
    host.add_actor(get_hello2_actor()?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        Some("stockhost".to_string()),
    )?)?;

    host.set_binding(
        "MDFD7XZ5KBOPLPHQKHJEMPR54XIW6RAG5D7NNKN22NP7NSEWNTJZP7JN",
        "wascc:http_server",
        Some("stockhost".to_string()),
        generate_port_config(first_port),
    )?;

    host.set_binding(
        "MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2",
        "wascc:http_server",
        Some("stockhost".to_string()),
        generate_port_config(first_port + 1),
    )?;

    Ok(host)
}

pub fn gen_kvcounter_host(port: u16, host: Host) -> Result<Host, Box<dyn Error>> {
    host.add_actor(Actor::from_file("./examples/.assets/kvcounter.wasm")?)?;
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
        "wascc:keyvalue",
        None,
        redis_config(),
    )?;
    host.set_binding(
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

pub fn generate_resigned_actor(bytes: &[u8]) -> Result<Actor, Box<dyn Error>> {
    use wascap::prelude::*;

    let (issuer, module) = (KeyPair::new_account(), KeyPair::new_module());
    let claims = ClaimsBuilder::<Actor>::new()
        .issuer(&issuer.public_key())
        .subject(&module.public_key())
        .with_metadata(Actor {
            name: Some("test".to_string()),
            caps: Some(vec![
                caps::HTTP_SERVER.to_string(),
                caps::KEY_VALUE.to_string(),
            ]),
            ..Default::default()
        })
        .build();
    let embedded = wasm::embed_claims(&bytes, &claims, &issuer)?;

    Ok(wascc_host::Actor::from_slice(&embedded)?)
}
