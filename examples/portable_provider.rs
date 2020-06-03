use std::collections::HashMap;
use wascc_host::{Actor, NativeCapability, WasccHost, WasiParams};

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    let host = WasccHost::new();
    host.add_actor(Actor::from_file("./examples/.assets/wasi_consumer.wasm")?)?;
    host.add_capability(
        Actor::from_file("./examples/.assets/wasi_provider.wasm")?,
        None,
        WasiParams::default(),
    )?; // WASI default does not map file system access

    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;

    host.bind_actor(
        "MDNPIQOU5EEHTP4TKY2APFOJTTEYYARN3ZIJTRWRYWHX6B4MFSO6ZCRT",
        "wascc:wasidemo",
        None,
        HashMap::new(),
    )?;

    host.bind_actor(
        "MDNPIQOU5EEHTP4TKY2APFOJTTEYYARN3ZIJTRWRYWHX6B4MFSO6ZCRT",
        "wascc:http_server",
        None,
        generate_port_config(8081),
    )?;

    for ((_binding, _id), descriptor) in host.capabilities() {
        println!("  **  Capability providers in Host:\n");
        println!(
            "\t'{}' v{} ({}) for {}",
            descriptor.name, descriptor.version, descriptor.revision, descriptor.id
        );
    }
    for (actor, _claims) in host.actors() {
        println!("  **  Actors in Host:\n");
        println!("\t{}", actor);
    }

    std::thread::park();

    Ok(())
}

fn generate_port_config(port: u16) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("PORT".to_string(), port.to_string());

    hm
}
