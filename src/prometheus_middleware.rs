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
    metrics_server_handle: Option<JoinHandle<()>>,
    metrics_server_kill_switch: Option<tokio::sync::oneshot::Sender<()>>,
    metrics_push_handle: Option<JoinHandle<()>>,
    metrics_push_kill_switch: Option<tokio::sync::oneshot::Sender<()>>,
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
    // TODO Add proper configuration
    fn new(prometheus_addr: Option<SocketAddr>, pushgateway_addr: Option<String>) -> Self
    where
        Self: Send + Sync,
    {
        let metrics = PrometheusMiddleware::init_metrics();
        PrometheusMiddleware::init_registry(&metrics);
        let metrics = Arc::new(RwLock::new(metrics));

        let (metrics_server_handle, metrics_server_kill_switch) =
            if let Some(addr) = prometheus_addr {
                let (metrics_server_kill_switch, mut server_kill_switch_rx) =
                    tokio::sync::oneshot::channel();

                let thread_handle = std::thread::spawn(move || {
                    let mut rt =
                        tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
                    rt.block_on(async {
                        tokio::select! {
                            _ = serve_metrics(addr) => {}
                            _ = (&mut server_kill_switch_rx) => {}
                        }
                    })
                });

                (Some(thread_handle), Some(metrics_server_kill_switch))
            } else {
                (None, None)
            };

        let (metrics_push_handle, metrics_push_kill_switch) = if let Some(addr) = pushgateway_addr {
            let (metrics_push_kill_switch, metrics_push_kill_switch_rx) =
                tokio::sync::oneshot::channel();

            // need a separate thread for pushing metrics because the reqwest sync client
            // is used in the prometheus library. reqwest sync creates a tokio runtime internally
            // so the tokio runtime created for the metrics server can't be reused.
            let thread_handle = std::thread::spawn(move || {
                push_metrics(addr, metrics_push_kill_switch_rx);
            });

            (Some(thread_handle), Some(metrics_push_kill_switch))
        } else {
            (None, None)
        };

        Self {
            metrics,
            metrics_server_handle,
            metrics_server_kill_switch,
            metrics_push_handle,
            metrics_push_kill_switch,
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
            if kill_switch.send(()).is_err() {
                error!("Error terminating the metrics server");
            }
        }

        if let Some(thread_handle) = self.metrics_server_handle.take() {
            if thread_handle.join().is_err() {
                error!("Error terminating the metrics server thread");
            }
        }

        if let Some(kill_switch) = self.metrics_push_kill_switch.take() {
            if kill_switch.send(()).is_err() {
                error!("Error terminating push of metrics");
            }
        }

        if let Some(thread_handle) = self.metrics_push_handle.take() {
            if thread_handle.join().is_err() {
                error!("Error terminating the metrics push thread");
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

async fn serve_metrics(addr: SocketAddr) {
    let server = Server::bind(&addr).serve(make_service_fn(move |_| async move {
        Ok::<_, hyper::error::Error>(service_fn(|req| serve_request(req)))
    }));

    if let Err(e) = server.await {
        error!("Metrics server error: {}", e);
    }
}

fn push_metrics(
    pushgateway_addr: String,
    mut push_kill_switch_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let push_interval = Duration::from_secs(5);

    loop {
        match push_kill_switch_rx.try_recv() {
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            _ => {
                return;
            }
        }

        std::thread::sleep(push_interval.clone());

        let metric_families = prometheus::gather();
        let push_res = prometheus::push_metrics(
            "wascc_push",
            labels! {},
            &pushgateway_addr,
            metric_families,
            None,
        );

        match push_res {
            Err(e) => error!("Error pushing metrics to {}: {}", &pushgateway_addr, e),
            _ => {}
        }
    }
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
            "actor_origin_pull".to_owned(),
            InvocationTarget::Actor("actor_id_pull".to_owned()),
            "actor_op_pull",
            "actor_msg_pull".into(),
        );
        let cap_invocation = Invocation::new(
            "cap_origin_pull".to_owned(),
            InvocationTarget::Capability {
                capid: "capid_pull".to_owned(),
                binding: "binding_pull".to_owned(),
            },
            "cap_op_pull",
            "cap_msg_pull".into(),
        );
        let invocations = random::<u16>();

        for _ in 0..invocations {
            middleware
                .actor_pre_invoke(actor_invocation.clone())
                .unwrap();
            middleware.actor_pre_invoke(cap_invocation.clone()).unwrap();
            // TODO TEMP for manual debug/testing
            // std::thread::sleep(Duration::from_secs(1));
        }

        let url = format!("http://{}/metrics", &prometheus_addr.to_string());
        // TODO TEMP for manual debug/testing
        // std::thread::sleep(Duration::from_secs(3000));
        let body = reqwest::blocking::get(&url)?.text()?;

        assert!(body
            .find(&format!("cap_total_call_count {}", invocations))
            .is_some());
        assert!(body
            .find(&format!("actor_total_call_count {}", invocations))
            .is_some());
        Ok(())
    }

    // TODO The prometheus::default_registry is global static so the counters
    //      are registered by the first test. Subsequent tests fail on register(..). Use custom registry?
    // TODO Start a stubbed server that the metrics can be pushed to and asserted
    #[ignore] // requires a running pushgateway
    #[test]
    fn test_push_metrics() {
        let pushgateway_addr = "http://127.0.0.1:9091";
        let middleware = PrometheusMiddleware::new(None, Some(pushgateway_addr.to_owned()));
        let actor_invocation = Invocation::new(
            "actor_origin_push".to_owned(),
            InvocationTarget::Actor("actor_id_push".to_owned()),
            "actor_op_push",
            "actor_msg_push".into(),
        );
        let cap_invocation = Invocation::new(
            "cap_origin_push".to_owned(),
            InvocationTarget::Capability {
                capid: "capid_push".to_owned(),
                binding: "binding_push".to_owned(),
            },
            "cap_op_push",
            "cap_msg_push".into(),
        );
        let invocations = random::<u16>();

        for _ in 0..invocations {
            middleware
                .actor_pre_invoke(actor_invocation.clone())
                .unwrap();
            middleware.actor_pre_invoke(cap_invocation.clone()).unwrap();
        }

        // TODO Assert pushed metrics
    }
}