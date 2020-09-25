#[cfg(feature = "prometheus_middleware")]
fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    prometheus_example::run_example()
}

#[cfg(not(feature = "prometheus_middleware"))]
fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("Feature 'prometheus_middleware' needed to run this example");
    Ok(())
}

#[cfg(feature = "prometheus_middleware")]
mod prometheus_example {
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use wascc_host::middleware::prometheus::{PrometheusConfig, PrometheusMiddleware};
    use wascc_host::{Actor, Host, NativeCapability};

    pub fn run_example() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
        env_logger::init();
        let host = Host::new();
        host.add_actor(Actor::from_file("./examples/.assets/echo.wasm")?)?;
        host.add_actor(Actor::from_file("./examples/.assets/echo2.wasm")?)?;
        host.add_native_capability(NativeCapability::from_file(
            "./examples/.assets/libwascc_httpsrv.so",
            None,
        )?)?;

        let server_addr: SocketAddr = ([127, 0, 0, 1], 9898).into();
        let config = PrometheusConfig {
            metrics_server_addr: Some(server_addr),
            pushgateway_config: None,
            moving_average_window_size: None,
        };
        host.add_middleware(PrometheusMiddleware::new(config).unwrap());

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

        std::thread::park();

        Ok(())
    }

    fn generate_port_config(port: u16) -> HashMap<String, String> {
        let mut hm = HashMap::new();
        hm.insert("PORT".to_string(), port.to_string());

        hm
    }
}
