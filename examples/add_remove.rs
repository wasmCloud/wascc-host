use std::collections::HashMap;
use wascc_host::{Actor, Host, NativeCapability};

fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();
    let host = Host::new();
    host.add_actor(Actor::from_file("./examples/.assets/echo.wasm")?)?;
    host.add_actor(Actor::from_file("./examples/.assets/echo2.wasm")?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;

    host.set_binding(
        "MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2",
        "wascc:http_server",
        None,
        generate_port_config(8081),
    )?;
    host.set_binding(
        "MDFD7XZ5KBOPLPHQKHJEMPR54XIW6RAG5D7NNKN22NP7NSEWNTJZP7JN",
        "wascc:http_server",
        None,
        generate_port_config(8082),
    )?;

    println!("Actors (before removal):");
    for (id, _claims) in host.actors() {
        println!(" - {}", id);
    }
    println!("Capabilities (before removal)");
    for ((binding_name, capid), _descriptor) in host.capabilities() {
        println!("- {},{}", binding_name, capid);
    }

    // Need to wait until the HTTP server finishes starting before we
    // should try and kill it.
    println!("Sleeping 1s...");
    std::thread::sleep(std::time::Duration::from_millis(1000));

    println!("Removing echo actor 2...");
    // This will terminate the actor and free up the HTTP port
    host.remove_actor("MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2")?;

    println!("Sleeping 2s...");
    std::thread::sleep(std::time::Duration::from_millis(1000));

    println!("Actors (after removal of second echo):");
    for (id, _claims) in host.actors() {
        println!(" - {}", id);
    }

    // ..
    // at this point, curling on port 8081 should fail w/connection refused
    // while curling on port 8082 should work just fine

    std::thread::park();

    Ok(())
}

fn generate_port_config(port: u16) -> HashMap<String, String> {
    let mut hm = HashMap::new();
    hm.insert("PORT".to_string(), port.to_string());

    hm
}
