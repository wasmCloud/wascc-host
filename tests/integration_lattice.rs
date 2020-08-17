#[allow(dead_code)]
mod common;

#[cfg(feature = "lattice")]
use latticeclient::Client;
#[cfg(feature = "lattice")]
use std::error::Error;

#[test]
#[cfg(feature = "lattice")]
fn lattice_single_host() -> Result<(), Box<dyn Error>> {
    use std::time::Duration;

    let host = common::gen_stock_host()?;
    host.set_label("integration", "test");
    host.set_label("hostcore.arch", "FOOBAR"); // this should be ignored
    let delay = Duration::from_millis(500);
    std::thread::sleep(delay);

    let lc = Client::new("127.0.0.1", None, delay);
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
