use crate::{Invocation, InvocationResponse, Middleware};
use crate::{InvocationTarget, Result};
use hyper::header::CONTENT_TYPE;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server};
use prometheus::core::{AtomicI64, GenericCounter};
use prometheus::{labels, register_int_counter, Encoder, Opts, TextEncoder};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

struct PrometheusMiddleware {
    metrics: Arc<RwLock<Metrics>>,
    metrics_server_handle: Option<JoinHandle<std::result::Result<(), tokio::task::JoinError>>>,
    metrics_server_kill_switch: Option<tokio::sync::oneshot::Sender<()>>,
    // metrics_push_handle: Option<JoinHandle<std::result::Result<(), tokio::task::JoinError>>>,
}

struct Metrics {
    cap_total_call_count: GenericCounter<AtomicI64>,
    cap_call_count: HashMap<String, GenericCounter<AtomicI64>>,
    cap_total_call_time: GenericCounter<AtomicI64>,
    cap_call_time: HashMap<String, GenericCounter<AtomicI64>>,
    actor_total_call_count: GenericCounter<AtomicI64>,
    actor_call_count: HashMap<String, GenericCounter<AtomicI64>>,
    actor_total_call_time: GenericCounter<AtomicI64>,
    actor_call_time: HashMap<String, GenericCounter<AtomicI64>>,
}

impl PrometheusMiddleware {
    fn new(prometheus_addr: Option<SocketAddr>, pushgateway_addr: Option<String>) -> Self
    where
        Self: Send + Sync,
    {
        let metrics = PrometheusMiddleware::init_metrics();
        PrometheusMiddleware::init_registry(&metrics);
        let metrics = Arc::new(RwLock::new(metrics));

        // TODO Can we reuse the same thread?
        let (metrics_server_handle, metrics_server_kill_switch) =
            if let Some(addr) = prometheus_addr {
                let (kill_switch_sender, kill_switch_receiver) = tokio::sync::oneshot::channel();

                let metrics_server_handle = std::thread::spawn(move || {
                    let mut rt =
                        tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
                    let server_task = rt.spawn(serve_metrics(addr, kill_switch_receiver));
                    // let _c = rt.spawn(async move {
                    //     let addr = pushgateway_addr.clone();
                    //
                    //     loop {
                    //         push_metrics(&addr.clone().unwrap());
                    //         // tokio::time::delay_for(Duration::from_secs(1)).await;
                    //     }
                    // });
                    rt.block_on(server_task)
                    // rt.block_on(_c)
                });

                (Some(metrics_server_handle), Some(kill_switch_sender))
            } else {
                (None, None)
            };

        // let metrics_push_handle = if let Some(addr) = pushgateway_addr {
        //     let metrics_push_handle = std::thread::spawn(move || {
        //         push_metrics();
        //     });
        //
        //     metrics_push_handle
        // };

        Self {
            metrics,
            metrics_server_handle,
            metrics_server_kill_switch,
            // metrics_push_handle,
        }
    }

    fn init_registry(metrics: &Metrics) {
        let registry = prometheus::default_registry();
        registry
            .register(Box::new(metrics.cap_total_call_count.clone()))
            .expect("Failed to register counter 'cap_total_call_count'");
        registry
            .register(Box::new(metrics.cap_total_call_time.clone()))
            .expect("Failed to register counter 'cap_total_call_time'");
        registry
            .register(Box::new(metrics.actor_total_call_count.clone()))
            .expect("Failed to register counter 'actor_total_call_count'");
        registry
            .register(Box::new(metrics.actor_total_call_time.clone()))
            .expect("Failed to register counter 'actor_total_call_time'");
    }

    fn init_metrics() -> Metrics {
        Metrics {
            cap_total_call_count: GenericCounter::with_opts(Opts::new(
                "cap_total_call_count",
                "total number of capability invocations",
            ))
            .unwrap(),
            cap_call_count: HashMap::new(),
            cap_total_call_time: GenericCounter::with_opts(Opts::new(
                "cap_total_call_time",
                "total call time for all capabilities",
            ))
            .unwrap(),
            cap_call_time: HashMap::new(),
            actor_total_call_count: GenericCounter::with_opts(Opts::new(
                "actor_total_call_count",
                "total number of actor invocations",
            ))
            .unwrap(),
            actor_call_count: HashMap::new(),
            actor_total_call_time: GenericCounter::with_opts(Opts::new(
                "actor_total_call_time",
                "total call time for all actors",
            ))
            .unwrap(),
            actor_call_time: HashMap::new(),
        }
    }
}

// TODO Add timing of invocations
impl Middleware for PrometheusMiddleware {
    fn actor_pre_invoke(&self, inv: Invocation) -> Result<Invocation> {
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

fn pre_invoke(metrics: &Arc<RwLock<Metrics>>, inv: Invocation) -> Result<Invocation> {
    let mut metrics = metrics.write().unwrap();
    let target = match &inv.target {
        InvocationTarget::Actor(actor) => actor.clone(),
        InvocationTarget::Capability { capid, binding } => capid.clone() + binding,
    };
    let key = inv.origin.clone() + &inv.operation + &target;

    match &inv.target {
        InvocationTarget::Actor(actor) => {
            metrics.actor_total_call_count.inc();
            metrics
                .actor_call_count
                .entry(key)
                .and_modify(|e| e.inc())
                .or_insert_with(|| {
                    let counter = register_int_counter!(
                        format!("{}_call_count", &actor),
                        format!("number of invocations of {}", &actor)
                    )
                    .unwrap();
                    counter.inc();
                    counter
                });
        }
        InvocationTarget::Capability { capid, binding } => {
            metrics.cap_total_call_count.inc();
            metrics
                .cap_call_count
                .entry(key)
                .and_modify(|e| e.inc())
                .or_insert_with(|| {
                    let counter = register_int_counter!(
                        format!("{}_{}_call_count", &capid, &binding),
                        format!(
                            "number of invocations of capability {} with binding {}",
                            &capid, &binding
                        )
                    )
                    .unwrap();
                    counter.inc();
                    counter
                });
        }
    }

    Ok(inv)
}

impl Drop for PrometheusMiddleware {
    fn drop(&mut self) {
        if let Some(kill_switch) = self.metrics_server_kill_switch.take() {
            kill_switch.send(()).unwrap();
        }

        if let Some(thread_handle) = self.metrics_server_handle.take() {
            if thread_handle.join().is_err() {
                error!("Error terminating the metrics server thread");
            }
        }
    }
}

async fn serve_request(req: Request<Body>) -> hyper::error::Result<Response<Body>> {
    match (req.method(), req.uri().path()) {
        (&Method::GET, "/metrics") => {
            let metric_families = prometheus::gather();
            let mut buffer = vec![];
            let encoder = TextEncoder::new();
            encoder.encode(&metric_families, &mut buffer).unwrap();

            Ok(Response::builder()
                .status(200)
                .header(CONTENT_TYPE, encoder.format_type())
                .body(Body::from(buffer))
                .unwrap())
        }
        _ => Ok(Response::builder().status(404).body(Body::empty()).unwrap()),
    }
}

async fn serve_metrics(addr: SocketAddr, kill_switch: tokio::sync::oneshot::Receiver<()>) {
    let server = Server::bind(&addr).serve(make_service_fn(move |_| async move {
        Ok::<_, hyper::error::Error>(service_fn(|req| serve_request(req)))
    }));
    let graceful = server.with_graceful_shutdown(async {
        kill_switch.await.ok();
    });

    if let Err(e) = graceful.await {
        error!("Metrics server error: {}", e);
    }
}

fn push_metrics(pushgateway_addr: &String) {
    dbg!("push_metrics");

    let metric_families = prometheus::gather();

    prometheus::push_metrics(
        "wascc_push",
        labels! {},
        &pushgateway_addr,
        metric_families,
        None,
    )
    .unwrap();
}

#[cfg(test)]
mod tests {
    use crate::prometheus_middleware::PrometheusMiddleware;
    use crate::{Invocation, InvocationTarget, Middleware};
    use rand::random;
    use std::time::Duration;

    #[test]
    fn test_get_metrics() -> Result<(), reqwest::Error> {
        let prometheus_addr = ([127, 0, 0, 1], 9898).into();
        let middleware = PrometheusMiddleware::new(Some(prometheus_addr), None);
        let actor_invocation = Invocation::new(
            "actor_origin".to_owned(),
            InvocationTarget::Actor("actor_id".to_owned()),
            "actor_op",
            "actor_msg".into(),
        );
        let cap_invocation = Invocation::new(
            "cap_origin".to_owned(),
            InvocationTarget::Capability {
                capid: "capid".to_owned(),
                binding: "binding".to_owned(),
            },
            "cap_op",
            "cap_msg".into(),
        );
        let invocations = random::<u16>();

        for _ in 0..invocations {
            middleware
                .actor_pre_invoke(actor_invocation.clone())
                .unwrap();
            middleware.actor_pre_invoke(cap_invocation.clone()).unwrap();
            // TODO TEMP for manual debug/testing
            std::thread::sleep(Duration::from_secs(1));
        }

        let url = format!("http://{}/metrics", &prometheus_addr.to_string());
        // TODO TEMP for manual debug/testing
        std::thread::sleep(Duration::from_secs(3000));
        let body = reqwest::blocking::get(&url)?.text()?;

        assert!(body
            .find(&format!("cap_total_call_count {}", invocations))
            .is_some());
        assert!(body
            .find(&format!("actor_total_call_count {}", invocations))
            .is_some());
        Ok(())
    }

    #[test]
    fn test_push_metrics() {
        let prometheus_addr = ([127, 0, 0, 1], 9898).into();
        let pushgateway_addr = "http://127.0.0.1/9091";
        let middleware =
            PrometheusMiddleware::new(Some(prometheus_addr), Some(pushgateway_addr.to_owned()));
        let actor_invocation = Invocation::new(
            "actor_origin".to_owned(),
            InvocationTarget::Actor("actor_id".to_owned()),
            "actor_op",
            "actor_msg".into(),
        );
        let cap_invocation = Invocation::new(
            "cap_origin".to_owned(),
            InvocationTarget::Capability {
                capid: "capid".to_owned(),
                binding: "binding".to_owned(),
            },
            "cap_op",
            "cap_msg".into(),
        );
        let invocations = random::<u16>();

        for _ in 0..invocations {
            middleware
                .actor_pre_invoke(actor_invocation.clone())
                .unwrap();
            middleware.actor_pre_invoke(cap_invocation.clone()).unwrap();
            // TODO TEMP for manual debug/testing
            std::thread::sleep(Duration::from_secs(1));
        }

        std::thread::sleep(Duration::from_secs(1000));
    }
}
