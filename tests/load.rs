use crate::common;
use std::error::Error;
use std::time::Instant;
use wascc_host::{Actor, Host, HostBuilder, NativeCapability};

pub(crate) fn actor_only_load() -> Result<(), Box<dyn Error>> {
    #[cfg(feature = "lattice")]
    let host = HostBuilder::new()
        .with_lattice_namespace("actoronlyload")
        .build();

    #[cfg(not(feature = "lattice"))]
    let host = HostBuilder::new().build();

    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_redis.so",
        None,
    )?)?;

    use std::fs::File;
    use std::io::Read;

    let bytes = {
        let mut f = File::open("./examples/.assets/kvcounter.wasm")?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)?;
        bytes
    };

    let start = Instant::now();
    // Change this value to run heavier loads on your workstation. It's
    // low to keep the CI builds from crashing
    for i in 0..50 {
        let a = common::generate_resigned_actor(&bytes)?;
        let actor_id = a.public_key();
        host.add_actor(a)?;
        if (i % 10 == 0) {
            println!("Actor {} added", i);
        }
    }
    let elapsed = start.elapsed().as_secs();
    println!(
        "Finished loading actors: {} seconds ({} avg)",
        elapsed,
        elapsed / 50
    );
    assert_eq!(50, host.actors().len());
    host.shutdown()?;
    Ok(())
}

pub(crate) fn simple_load() -> Result<(), Box<dyn Error>> {
    use redis::Commands;

    #[cfg(feature = "lattice")]
    let host = HostBuilder::new()
        .with_lattice_namespace("simpleload")
        .build();

    #[cfg(not(feature = "lattice"))]
    let host = HostBuilder::new().build();

    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_redis.so",
        None,
    )?)?;

    use std::fs::File;
    use std::io::Read;

    let bytes = {
        let mut f = File::open("./examples/.assets/kvcounter.wasm")?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes)?;
        bytes
    };

    let base_port = 32000;
    for i in 0..100 {
        let a = common::generate_resigned_actor(&bytes)?;
        let actor_id = a.public_key();
        host.add_actor(a)?;

        host.set_binding(
            &actor_id,
            "wascc:http_server",
            None,
            common::generate_port_config(base_port + i),
        )?;
        host.set_binding(&actor_id, "wascc:keyvalue", None, common::redis_config())?;
    }
    println!("** LOADED ALL ACTORS ** ");

    // Query all the HTTP ports
    for i in 0..100 {
        let key = format!("loadcounter{}", i);
        let url = format!("http://localhost:{}/{}", base_port + i, key);
        let mut resp = reqwest::blocking::get(&url)?;
        assert!(resp.status().is_success());
        let counter: serde_json::Value = serde_json::from_str(&resp.text()?)?;

        let expected: i64 = i as i64 + 1;
        assert_eq!(expected, counter["counter"].as_i64().unwrap());
    }

    host.shutdown()?;

    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    for i in 0..100 {
        let rkey = format!("loadcounter{}", i);
        let _: () = con.del(&rkey)?;
    }

    Ok(())
}
