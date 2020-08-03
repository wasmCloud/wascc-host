// Copyright 2015-2020 Capital One Services, LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use wascc_host::middleware::{InvocationHandler, MiddlewareResponse};
use wascc_host::{Actor, Invocation, InvocationResponse, Middleware, NativeCapability, WasccHost};

#[macro_use]
extern crate log;

type Result<T> = std::result::Result<T, wascc_host::errors::Error>;

fn main() -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();
    let host = WasccHost::new();
    host.add_actor(Actor::from_file("./examples/.assets/echo.wasm")?)?;
    host.add_actor(Actor::from_file("./examples/.assets/echo2.wasm")?)?;
    host.add_native_capability(NativeCapability::from_file(
        "./examples/.assets/libwascc_httpsrv.so",
        None,
    )?)?;

    host.add_middleware(CachingMiddleware::default());
    host.add_middleware(LoggingMiddleware::default());

    host.bind_actor(
        "MB4OLDIC3TCZ4Q4TGGOVAZC43VXFE2JQVRAXQMQFXUCREOOFEKOKZTY2",
        "wascc:http_server",
        None,
        generate_port_config(8081),
    )?;
    host.bind_actor(
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

#[derive(Default)]
struct CachingMiddleware {}

impl Middleware for CachingMiddleware {
    fn actor_pre_invoke(&self, inv: Invocation) -> wascc_host::Result<Invocation> {
        info!("CachingMiddleware-ACTOR(PRE): {}", inv.operation);
        Ok(inv)
    }

    fn actor_invoke(
        &self,
        inv: Invocation,
        _handler: InvocationHandler,
    ) -> Result<MiddlewareResponse> {
        info!("CachingMiddleware-ACTOR(INV): cached response returned, execution halted");
        Ok(MiddlewareResponse::Halt(InvocationResponse::success(
            &inv,
            "cached actor response".as_bytes().to_vec(),
        )))
    }

    fn actor_post_invoke(
        &self,
        response: InvocationResponse,
    ) -> wascc_host::Result<InvocationResponse> {
        info!(
            "CachingMiddleware-ACTOR(POST): success: {}",
            response.error.is_none()
        );
        Ok(response)
    }

    fn capability_pre_invoke(&self, inv: Invocation) -> wascc_host::Result<Invocation> {
        info!("CachingMiddleware-CAP(PRE): {}", inv.operation);
        Ok(inv)
    }

    fn capability_invoke(
        &self,
        inv: Invocation,
        _handler: InvocationHandler,
    ) -> Result<MiddlewareResponse> {
        info!("CachingMiddleware-CAP(INV): cached response returned, execution halted");
        Ok(MiddlewareResponse::Halt(InvocationResponse::success(
            &inv,
            "cached capability response".as_bytes().to_vec(),
        )))
    }

    fn capability_post_invoke(
        &self,
        response: InvocationResponse,
    ) -> wascc_host::Result<InvocationResponse> {
        info!(
            "CachingMiddleware-CAP(POST): success: {}",
            response.error.is_none()
        );
        Ok(response)
    }
}

#[derive(Default)]
struct LoggingMiddleware {}

impl Middleware for LoggingMiddleware {
    fn actor_pre_invoke(&self, inv: Invocation) -> wascc_host::Result<Invocation> {
        info!("LoggingMiddleware-ACTOR(PRE): {}", inv.operation);
        Ok(inv)
    }

    fn actor_invoke(
        &self,
        inv: Invocation,
        handler: InvocationHandler,
    ) -> Result<MiddlewareResponse> {
        // Will not be invoked since the 'CachingMiddleware' halts the middleware execution
        info!("LoggingMiddleware-ACTOR(INV): {}", inv.operation);
        Ok(MiddlewareResponse::Continue(handler.invoke(inv)))
    }

    fn actor_post_invoke(
        &self,
        response: InvocationResponse,
    ) -> wascc_host::Result<InvocationResponse> {
        info!(
            "LoggingMiddleware-ACTOR(POST): success: {}",
            response.error.is_none()
        );
        Ok(response)
    }

    fn capability_pre_invoke(&self, inv: Invocation) -> wascc_host::Result<Invocation> {
        info!("LoggingMiddleware-CAP(PRE): {}", inv.operation);
        Ok(inv)
    }

    fn capability_invoke(
        &self,
        inv: Invocation,
        handler: InvocationHandler,
    ) -> Result<MiddlewareResponse> {
        // Will not be invoked since the 'CachingMiddleware' halts the middleware execution
        info!("LoggingMiddleware-CAP(INV): {}", inv.operation);
        Ok(MiddlewareResponse::Continue(handler.invoke(inv)))
    }

    fn capability_post_invoke(
        &self,
        response: InvocationResponse,
    ) -> wascc_host::Result<InvocationResponse> {
        info!(
            "LoggingMiddleware-CAP(POST): success: {}",
            response.error.is_none()
        );
        Ok(response)
    }
}
