use latticeclient::Client;
use std::error::Error;

pub(crate) fn lattice_single_host() -> Result<(), Box<dyn Error>> {
    use std::time::Duration;
    use wascc_host::{Host, HostBuilder};

    let host = HostBuilder::new()
        .with_label("integration", "test")
        .with_label("hostcore.arch", "FOOBAR")
        .with_lattice_namespace("singlehost")
        .build();
    
    let delay = Duration::from_millis(500);
    std::thread::sleep(delay);

    let lc = Client::new("127.0.0.1", None, delay, Some("singlehost".to_string()));
    let hosts = lc.get_hosts()?;
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].labels["hostcore.os"], std::env::consts::OS);
    assert_eq!(
        hosts[0].labels["hostcore.osfamily"],
        std::env::consts::FAMILY
    );
    assert_eq!(hosts[0].labels["hostcore.arch"], std::env::consts::ARCH);
    assert_eq!(hosts[0].labels["integration"], "test");
    host.shutdown()?;
    std::thread::sleep(delay);
    Ok(())
}

pub(crate) fn lattice_isolation() -> Result<(), Box<dyn Error>> {
    use std::time::Duration;
    use wascc_host::{Host, HostBuilder};
    let host1 = HostBuilder::new()
        .with_lattice_namespace("system.1")
        .with_label("testval", "1")
        .build();
    let host2 = HostBuilder::new()
        .with_lattice_namespace("system.2")
        .with_label("testval", "2")
        .build();

    let delay = Duration::from_millis(500);
    std::thread::sleep(delay);

    let lc1 = Client::new("127.0.0.1", None, delay, Some("system.1".to_string()));
    let hosts1 = lc1.get_hosts()?;
    assert_eq!(hosts1.len(), 1);
    assert_eq!(hosts1[0].labels["testval"], "1");

    let lc2 = Client::new("127.0.0.1", None, delay, Some("system.2".to_string()));
    let hosts2 = lc2.get_hosts()?;
    assert_eq!(hosts2.len(), 1);
    assert_eq!(hosts2[0].labels["testval"], "2");

    let lc3 = Client::new("127.0.0.1", None, delay, Some("system.nope".to_string()));
    let hosts3 = lc3.get_hosts()?;
    assert_eq!(hosts3.len(), 0);

    host1.shutdown()?;
    host2.shutdown()?;
    std::thread::sleep(delay);
    Ok(())
}

pub(crate) fn lattice_events() -> Result<(), Box<dyn Error>> {
    use crossbeam_channel::unbounded;
    use latticeclient::{BusEvent, CloudEvent};
    use std::time::Duration;
    use wascc_host::Host;

    let (s, r) = unbounded();
    let nc = nats::connect("127.0.0.1")?;

    let _sub = nc.subscribe("wasmbus.events")?.with_handler(move |msg| {
        let ce: CloudEvent = serde_json::from_slice(&msg.data).unwrap();
        let be: BusEvent = serde_json::from_str(&ce.data).unwrap();
        let _ = s.send(be);
        Ok(())
    });

    // K/V counter host:
    // add_actor x 2
    // add_native_capability
    // bind_actor x 2
    let host = crate::common::gen_kvcounter_host(3666, Host::new())?;
    let delay = Duration::from_millis(500);
    std::thread::sleep(delay);
    host.shutdown()?;
    std::thread::sleep(delay);
    nc.close();

    // * While these events are _mostly_ in deterministic order, because of the nature
    // of the fact that the capability providers are on background threads and
    // other events are issued by foreground threads, we can see certain events
    // appear "out of order", which makes for frustrating CI builds, so we do a
    // "contains" check here instead of checking on the exact delivery order.

    let a = [
        r.recv().unwrap(), // host started
        r.recv().unwrap(), // extras prov loaded
        r.recv().unwrap(), // actor starting
        r.recv().unwrap(), // actor started
        r.recv().unwrap(), // prov loaded
        r.recv().unwrap(), // prov loaded
        r.recv().unwrap(), // binding created
        r.recv().unwrap(), // binding created
        // -- shut down
        r.recv().unwrap(), // actor stopped
        r.recv().unwrap(), // provider removed
        r.recv().unwrap(), // provider removed
        r.recv().unwrap(), // provider removed (remember "extras" is an omnipresent provider)
        r.recv().unwrap(), // host stop -- the last gasp
    ];

    assert!(a.contains(&BusEvent::HostStarted(host.id())));
    assert!(a.contains(&BusEvent::ProviderLoaded {
        capid: "wascc:extras".to_string(),
        instance_name: "default".to_string(),
        host: host.id(),
    }));
    assert!(a.contains(&BusEvent::ActorStarting {
        actor: "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ".to_string(),
        host: host.id(),
    }));
    assert!(a.contains(&BusEvent::ActorStarted {
        actor: "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ".to_string(),
        host: host.id(),
    }));
    assert!(a.contains(&BusEvent::ProviderLoaded {
        capid: "wascc:http_server".to_string(),
        instance_name: "default".to_string(),
        host: host.id(),
    }));
    assert!(a.contains(&BusEvent::ProviderLoaded {
        capid: "wascc:keyvalue".to_string(),
        instance_name: "default".to_string(),
        host: host.id(),
    }));
    assert!(a.contains(&BusEvent::ActorBindingCreated {
        actor: "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ".to_string(),
        capid: "wascc:keyvalue".to_string(),
        instance_name: "default".to_string(),
        host: host.id(),
    }));
    assert!(a.contains(&BusEvent::ActorBindingCreated {
        actor: "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ".to_string(),
        capid: "wascc:http_server".to_string(),
        instance_name: "default".to_string(),
        host: host.id(),
    }));
    assert!(a.contains(&BusEvent::ActorStopped {
        actor: "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ".to_string(),
        host: host.id(),
    }));

    assert!(a.contains(&BusEvent::ProviderRemoved {
        capid: "wascc:http_server".to_string(),
        host: host.id(),
        instance_name: "default".to_string(),
    }));
    assert!(a.contains(&BusEvent::ProviderRemoved {
        capid: "wascc:keyvalue".to_string(),
        host: host.id(),
        instance_name: "default".to_string(),
    }));
    assert!(a.contains(&BusEvent::ProviderRemoved {
        capid: "wascc:extras".to_string(),
        host: host.id(),
        instance_name: "default".to_string(),
    }));
    assert!(a.contains(&BusEvent::HostStopped(host.id())));
    Ok(())
}

pub(crate) fn can_bind_from_any_host() -> Result<(), Box<dyn Error>> {
    use redis::Commands;
    use std::time::Duration;
    use wascc_host::{Actor, Host, HostBuilder, NativeCapability};

    let port = 6203_u16;
    let host = HostBuilder::new()
        .with_lattice_namespace("bindanywhere")
        .build();

    host.add_actor(Actor::from_file("./examples/.assets/kvcounter.wasm")?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_redis.so",
        None,
    )?)?;

    // Bindings in lattice mode should have a global scope, so we should
    // be able to invoke "set binding" from any host in the lattice
    // and have the binding take effect.
    let host2 = HostBuilder::new()
        .with_lattice_namespace("bindanywhere")
        .build();
    host2.set_binding(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:keyvalue",
        None,
        crate::common::redis_config(),
    )?;
    host2.set_binding(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:http_server",
        None,
        crate::common::generate_port_config(port),
    )?;

    // Pretend to "move" the actor from one host to another
    // pre-existing binding should remain unaffected
    // NOTE: if you remove the last instance of an actor from a lattice, then
    // it will take out all of the bindings for that actor... so to keep that from
    // happening we add a second actor first, then remove the other one.
    host2.add_actor(Actor::from_file("./examples/.assets/kvcounter.wasm")?)?;
    std::thread::sleep(::std::time::Duration::from_millis(300));
    host.remove_actor("MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ")?;
    std::thread::sleep(::std::time::Duration::from_millis(300));
    println!("Host actor count {}", host.actors().len());

    let key = uuid::Uuid::new_v4().to_string();
    let rkey = format!(":{}", key); // the kv wasm logic does a replace on '/' with ':'
    let url = format!("http://localhost:6203/{}", key);
    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;

    let mut resp = reqwest::blocking::get(&url)?;
    assert!(resp.status().is_success());
    reqwest::blocking::get(&url)?;
    resp = reqwest::blocking::get(&url)?; // counter should be at 3 now
    assert!(resp.status().is_success());
    assert_eq!(resp.text()?, "{\"counter\":3}");
    host.shutdown()?;
    host2.shutdown()?;
    std::thread::sleep(Duration::from_millis(500));

    let _: () = con.del(&rkey)?;
    Ok(())
}

pub(crate) fn instance_count() -> Result<(), Box<dyn Error>> {
    use latticeclient::{BusEvent, Client, CloudEvent};
    use redis::Commands;
    use std::time::Duration;
    use wascc_host::{Actor, Host, HostBuilder, NativeCapability};

    let port = 6209_u16;

    let host1 = HostBuilder::new().with_lattice_namespace("icount").build();
    host1.add_actor(Actor::from_file("./examples/.assets/kvcounter.wasm")?)?;
    host1.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;
    host1.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_redis.so",
        None,
    )?)?;

    host1.set_binding(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:keyvalue",
        None,
        crate::common::redis_config(),
    )?;
    host1.set_binding(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:http_server",
        None,
        crate::common::generate_port_config(port),
    )?;

    let host2 = HostBuilder::new().with_lattice_namespace("icount").build();
    host2.add_actor(Actor::from_file("./examples/.assets/kvcounter.wasm")?)?;

    host2.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_redis.so",
        None,
    )?)?;

    std::thread::sleep(Duration::from_millis(300));

    let nc = nats::connect("127.0.0.1")?;
    let lc = Client::with_connection(nc, Duration::from_secs(1), Some("icount".to_string()));
    let actors = lc.get_actors()?;
    let anames: Vec<_> = actors
        .values()
        .flatten()
        .map(|a| a.subject.to_string())
        .collect();
    assert_eq!(2, anames.len());

    let count = match lc.get_actors() {
        Ok(res) => {
            let count = res.values().into_iter().fold(0, |acc, x| {
                acc + x
                    .iter()
                    .filter(|c| {
                        c.subject == "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ"
                    })
                    .collect::<Vec<_>>()
                    .len()
            });
            count
        }
        Err(e) => 99,
    };
    assert_eq!(count, 2);

    host1.shutdown()?;
    host2.shutdown()?;

    std::thread::sleep(Duration::from_millis(300));
    Ok(())
}

pub(crate) fn unload_reload_actor_retains_bindings() -> Result<(), Box<dyn Error>> {
    use latticeclient::{BusEvent, CloudEvent};
    use redis::Commands;
    use std::time::Duration;
    use wascc_host::{Actor, Host, HostBuilder, NativeCapability};

    let nc = nats::connect("127.0.0.1")?;

    let port = 6201_u16;

    let host1 = HostBuilder::new()
        .with_lattice_namespace("unloadreload")
        .with_label("testval", "1")
        .build();
    host1.add_actor(Actor::from_file("./examples/.assets/kvcounter.wasm")?)?;
    host1.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;
    host1.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_redis.so",
        None,
    )?)?;

    // *** !!
    // These bindings only need to be set once per lattice, not once per host
    // *** !!
    host1.set_binding(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:keyvalue",
        None,
        crate::common::redis_config(),
    )?;
    host1.set_binding(
        "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ",
        "wascc:http_server",
        None,
        crate::common::generate_port_config(port),
    )?;

    let host2 = HostBuilder::new()
        .with_lattice_namespace("unloadreload")
        .with_label("testval", "2")
        .build();
    host2.add_actor(Actor::from_file("./examples/.assets/kvcounter.wasm")?)?;

    // The actor in host 2 should immediately be able to talk to this redis
    // provider because of the re-establish binding code
    host2.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_redis.so",
        None,
    )?)?;
    std::thread::sleep(Duration::from_millis(500));

    let key = uuid::Uuid::new_v4().to_string();
    let rkey = format!(":{}", key); // the kv wasm logic does a replace on '/' with ':'
    let url = format!("http://localhost:{}/{}", port, key);

    // Invoke it enough times to ensure that both actors get exercised...
    let mut resp = reqwest::blocking::get(&url)?;
    assert!(resp.status().is_success());
    reqwest::blocking::get(&url)?;
    reqwest::blocking::get(&url)?; // counter should be at 3 now

    host2.remove_actor("MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ")?;
    std::thread::sleep(Duration::from_millis(500)); // give actor thread time to terminate

    reqwest::blocking::get(&url)?;
    resp = reqwest::blocking::get(&url)?; // counter should be at 5 now
    assert!(resp.status().is_success());
    assert_eq!(resp.text()?, "{\"counter\":5}");

    host2.add_actor(Actor::from_file("./examples/.assets/kvcounter.wasm")?)?;
    std::thread::sleep(Duration::from_millis(500));
    // should not have to call re-bind here, because the removed actor was not the last of its kind in the lattice

    reqwest::blocking::get(&url)?;
    resp = reqwest::blocking::get(&url)?; // counter should be at 7 now
    assert!(resp.status().is_success());
    assert_eq!(resp.text()?, "{\"counter\":7}");

    // Purge the counter value from Redis
    let client = redis::Client::open("redis://127.0.0.1/")?;
    let mut con = client.get_connection()?;
    let _: () = con.del(&rkey)?;
    assert_eq!(1, host2.actors().len());
    assert_eq!(1, host1.actors().len());

    host1.shutdown()?;
    host2.shutdown()?;
    std::thread::sleep(Duration::from_millis(500));

    Ok(())
}
