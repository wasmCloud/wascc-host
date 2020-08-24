/*
 * The named bindings example takes an actor that performs an atomic increrement
 * on two different data stores and returns the combined result of the counts
 * in a single HTTP response. This illustrates the ability to create an actor
 * that has multiple, distinguishable capabilities of the same type without
 * ever creating a tight coupling.
 */
use std::collections::HashMap;
use wascc_host::{Actor, Host, NativeCapability};

const ACTOR_SUBJECT: &str = "MCYWHMJW5VW5U7ZRDQV7JU45GHSR2SA6OZJ2MPHQCFALR2CGFA2NAMZM";
fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();
    let host = Host::new();
    host.add_actor(Actor::from_file("./examples/.assets/multibinding.wasm")?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;

    // Add TWO Redis providers. Note that these do NOT have to be the same provider...
    // we could use Redis and an in-memory store or Redis and Cassandra or
    // Cassandra and etcd and so on
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_redis.so",
        Some("source1".to_string()),
    )?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_redis.so",
        Some("source2".to_string()),
    )?)?;

    host.bind_actor(
        ACTOR_SUBJECT,
        "wascc:http_server",
        None,
        generate_port_config(8081),
    )?;

    host.bind_actor(
        ACTOR_SUBJECT,
        "wascc:keyvalue",
        Some("source1".to_string()),
        redis_config(0),
    )?;

    host.bind_actor(
        ACTOR_SUBJECT,
        "wascc:keyvalue",
        Some("source2".to_string()),
        redis_config(1),
    )?;

    std::thread::park();

    Ok(())
}

fn redis_config(dbindex: u32) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert(
        "URL".to_string(),
        format!("redis://127.0.0.1:6379/{}", dbindex),
    );

    hm
}

fn generate_port_config(port: u16) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("PORT".to_string(), port.to_string());

    hm
}
