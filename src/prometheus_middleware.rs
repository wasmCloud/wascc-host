use crate::{Invocation, InvocationResponse, Middleware};
use crate::{InvocationTarget, Result};
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;

struct PrometheusMiddleware {
    metrics: Arc<RwLock<Metrics>>,
    server_handle: Option<JoinHandle<std::result::Result<(), tokio::task::JoinError>>>,
    server_kill_switch: Option<tokio::sync::oneshot::Sender<()>>,
}

#[derive(Default)]
struct Metrics {
    cap_total_call_count: u128,
    cap_call_count: HashMap<String, u128>,
    cap_total_call_time: u128,
    cap_call_time: HashMap<String, u128>,
    actor_total_call_count: u128,
    actor_call_count: HashMap<String, u128>,
    actor_total_call_time: u128,
    actor_call_time: HashMap<String, u128>,
}

impl PrometheusMiddleware {
    fn new(server_addr: SocketAddr) -> Self
    where
        Self: Send + Sync,
    {
        let (tx, rx) = tokio::sync::oneshot::channel();

        let server_handle = std::thread::spawn(move || {
            let mut rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
            let server_task = rt.spawn(serve_metrics(server_addr, rx));
            rt.block_on(server_task)
        });

        Self {
            metrics: Default::default(),
            server_handle: Some(server_handle),
            server_kill_switch: Some(tx),
        }
    }
}

impl Middleware for PrometheusMiddleware {
    fn actor_pre_invoke(&self, inv: Invocation) -> Result<Invocation> {
        // Timing: Crate hash of everything in Invocation. Unique enough?
        // Store current_time in HashMap (hash -> time)
        pre_invoke(&self.metrics, inv)
    }

    fn actor_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse> {
        Ok(response)
    }

    fn capability_pre_invoke(&self, inv: Invocation) -> Result<Invocation> {
        pre_invoke(&self.metrics, inv)
    }

    fn capability_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse> {
        Ok(response)
    }
}

impl Drop for PrometheusMiddleware {
    fn drop(&mut self) {
        if let Some(kill_switch) = self.server_kill_switch.take() {
            kill_switch.send(()).unwrap();
        }

        if let Some(thread_handle) = self.server_handle.take() {
            if thread_handle.join().is_err() {
                error!("Error terminating the metrics server thread");
            }
        }
    }
}

async fn serve_request(_req: Request<Body>) -> hyper::error::Result<Response<Body>> {
    Ok(Response::new(Body::from("Hello from Middleware")))
}

async fn serve_metrics(addr: SocketAddr, kill_switch: tokio::sync::oneshot::Receiver<()>) {
    let server = Server::bind(&addr).serve(make_service_fn(|_| async {
        Ok::<_, hyper::error::Error>(service_fn(|req| serve_request(req)))
    }));
    let graceful = server.with_graceful_shutdown(async {
        kill_switch.await.ok();
    });

    if let Err(e) = graceful.await {
        error!("Metrics server error: {}", e);
    }
}

fn pre_invoke(metrics: &Arc<RwLock<Metrics>>, inv: Invocation) -> Result<Invocation> {
    let mut metrics = metrics.write().unwrap();

    match &inv.target {
        InvocationTarget::Actor(actor) => {
            metrics.actor_total_call_count += 1;
            *metrics.actor_call_count.entry(actor.clone()).or_insert(0) += 1;
        }
        InvocationTarget::Capability { capid, binding: _ } => {
            metrics.cap_total_call_count += 1;
            *metrics.cap_call_count.entry(capid.clone()).or_insert(0) += 1;
        }
    }

    Ok(inv)
}

#[cfg(test)]
mod tests {
    use crate::prometheus_middleware::PrometheusMiddleware;

    #[test]
    fn test_get_metrics() -> Result<(), reqwest::Error> {
        let server_addr = ([127, 0, 0, 1], 9898).into();
        let _middleware = PrometheusMiddleware::new(server_addr);
        let body = reqwest::blocking::get("http://127.0.0.1:9898")?.text()?;
        assert_eq!("Hello from Middleware", body);
        Ok(())
    }
}
