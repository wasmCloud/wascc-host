use latticeclient::Client;
use std::error::Error;

pub(crate) fn lattice_single_host() -> Result<(), Box<dyn Error>> {
    use std::time::Duration;
    use wascc_host::{Host, HostBuilder};

    let host = HostBuilder::new()
        .with_lattice_namespace("singlehost")
        .build();

    host.set_label("integration", "test");
    host.set_label("hostcore.arch", "FOOBAR"); // this should be ignored
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
        host: host.id()
    }));
    assert!(a.contains(&BusEvent::ActorStarting {
        actor: "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ".to_string(),
        host: host.id()
    }));
    assert!(a.contains(&BusEvent::ActorStarted {
        actor: "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ".to_string(),
        host: host.id()
    }));
    assert!(a.contains(&BusEvent::ProviderLoaded {
        capid: "wascc:http_server".to_string(),
        instance_name: "default".to_string(),
        host: host.id()
    }));
    assert!(a.contains(&BusEvent::ProviderLoaded {
        capid: "wascc:keyvalue".to_string(),
        instance_name: "default".to_string(),
        host: host.id()
    }));
    assert!(a.contains(&BusEvent::ActorBindingCreated {
        actor: "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ".to_string(),
        capid: "wascc:keyvalue".to_string(),
        instance_name: "default".to_string(),
        host: host.id()
    }));
    assert!(a.contains(&BusEvent::ActorBindingCreated {
        actor: "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ".to_string(),
        capid: "wascc:http_server".to_string(),
        instance_name: "default".to_string(),
        host: host.id()
    }));
    assert!(a.contains(&BusEvent::ActorStopped {
        actor: "MASCXFM4R6X63UD5MSCDZYCJNPBVSIU6RKMXUPXRKAOSBQ6UY3VT3NPZ".to_string(),
        host: host.id()
    }));

    assert!(a.contains(&BusEvent::ProviderRemoved {
        capid: "wascc:http_server".to_string(),
        host: host.id(),
        instance_name: "default".to_string()
    }));
    assert!(a.contains(&BusEvent::ProviderRemoved {
        capid: "wascc:keyvalue".to_string(),
        host: host.id(),
        instance_name: "default".to_string()
    }));
    assert!(a.contains(&BusEvent::ProviderRemoved {
        capid: "wascc:extras".to_string(),
        host: host.id(),
        instance_name: "default".to_string()
    }));
    assert!(a.contains(&BusEvent::HostStopped(host.id())));
    Ok(())
}
