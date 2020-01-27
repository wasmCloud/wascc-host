use std::collections::HashMap;
use std::io;
use wascc_host::{host, Actor, NativeCapability};

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    host::add_actor(Actor::from_file("./examples/.assets/kvcounter.wasm")?)?;
    host::add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
    )?)?;
    host::add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libredis_provider.so",
    )?)?;

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

    println!(
        "**> curl localhost:8081/counter1 to test, then press ENTER to replace with a new version"
    );
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    // Note that we don't supply the public key of the one to replace, it
    // comes straight from the Actor being replaced. You cannot replace actors
    // that do not have the same public key as a security measure against
    // malicious code
    host::replace_actor(Actor::from_file(
        "./examples/.assets/kvcounter_tweaked.wasm",
    )?)?;
    println!("**> KV counter replaced, issue query to see the new module running.");

    println!("**> Press ENTER to remove the key-value provider");
    io::stdin().read_line(&mut input)?;
    host::remove_native_capability("wascc:keyvalue")?; 

    println!("**> Press ENTER to add an in-memory key-value provider");
    io::stdin().read_line(&mut input)?;
    host::add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libkeyvalue.so",
    )?)?;

    println!("**> Now your counter should have started over.");


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
