use std::collections::HashMap;
use std::fs::File;
use std::io::prelude::*;
use wascc_host::host::{self, Invocation, InvocationResponse};
use wascc_host::Middleware;

#[macro_use]
extern crate log;

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::init();
    host::add_actor(load_wasm("./examples/.assets/echo.wasm")?)?;
    host::add_actor(load_wasm("./examples/.assets/echo2.wasm")?)?;
    host::add_native_capability("./examples/.assets/libwascc_httpsrv.so")?;

    host::add_middleware(LoggingMiddleware::default());

    host::configure(
        "MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2",
        "wascc:http_server",
        generate_port_config(8081),
    )?;
    host::configure(
        "MDFD7XZ5KBOPLPHQKHJEMPR54XIW6RAG5D7NNKN22NP7NSEWNTJZP7JN",
        "wascc:http_server",
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

fn load_wasm(path: &str) -> std::result::Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut file = File::open(path)?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;

    Ok(buf)
}

#[derive(Default)]
struct LoggingMiddleware {}

impl Middleware for LoggingMiddleware {
    fn actor_pre_invoke(&self, inv: Invocation) -> wascc_host::Result<Invocation> {
        info!("ACTOR(PRE): {}", inv.operation);
        Ok(inv)
    }
    fn actor_post_invoke(
        &self,
        response: InvocationResponse,
    ) -> wascc_host::Result<InvocationResponse> {
        info!("ACTOR(POST): success: {}", response.error.is_none());
        Ok(response)
    }
    fn capability_pre_invoke(&self, inv: Invocation) -> wascc_host::Result<Invocation> {
        info!("CAP(PRE): {}", inv.operation);
        Ok(inv)
    }
    fn capability_post_invoke(
        &self,
        response: InvocationResponse,
    ) -> wascc_host::Result<InvocationResponse> {
        info!("CAP(POST): success: {}", response.error.is_none());
        Ok(response)
    }
}
