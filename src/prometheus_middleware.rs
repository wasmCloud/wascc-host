use crate::{Invocation, InvocationResponse, Middleware};
use crate::{InvocationTarget, Result};
use hyper::header::CONTENT_TYPE;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use prometheus::core::{AtomicI64, GenericCounter};
use prometheus::{labels, register_int_counter, Encoder, Opts, Registry, TextEncoder};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock, RwLockWriteGuard};
use std::thread::JoinHandle;
use std::time::Duration;

struct PrometheusMiddleware {
    metrics: Arc<RwLock<Metrics>>,
    registry: Arc<RwLock<Registry>>,
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

#[derive(Clone)]
struct PrometheusConfig {
    /// The address that Prometheus can scrape (pull model).
    metrics_server_addr: Option<SocketAddr>,
    /// Configuration for the Prometheus client (push model).
    pushgateway_config: Option<PushgatewayConfig>,
}

#[derive(Clone)]
struct PushgatewayConfig {
    pushgateway_addr: String,
    push_interval: Duration,
    push_basic_auth: Option<BasicAuthentication>,
    job: Option<String>,
}

#[derive(Clone)]
pub struct BasicAuthentication {
    pub username: String,
    pub password: String,
}

impl PrometheusMiddleware {
    fn new(config: PrometheusConfig) -> Self
    where
        Self: Send + Sync,
    {
        let metrics = PrometheusMiddleware::init_metrics();
        let registry = PrometheusMiddleware::init_registry(&metrics);
        let metrics = Arc::new(RwLock::new(metrics));
        let registry = Arc::new(RwLock::new(registry));

        let (metrics_server_handle, metrics_server_kill_switch) =
            if let Some(addr) = config.metrics_server_addr {
                let (metrics_server_kill_switch, mut server_kill_switch_rx) =
                    tokio::sync::oneshot::channel();
                let registry2 = registry.clone();

                let thread_handle = std::thread::spawn(move || {
                    let mut rt =
                        tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
                    rt.block_on(async {
                        tokio::select! {
                            _ = serve_metrics(addr, registry2) => {}
                            _ = (&mut server_kill_switch_rx) => {}
                        }
                    })
                });

                (Some(thread_handle), Some(metrics_server_kill_switch))
            } else {
                (None, None)
            };

        let (metrics_push_handle, metrics_push_kill_switch) =
            if let Some(push_config) = config.pushgateway_config {
                let (metrics_push_kill_switch, metrics_push_kill_switch_rx) =
                    tokio::sync::oneshot::channel();

                // need a separate thread for pushing metrics because the reqwest sync client
                // is used in the prometheus library. reqwest sync creates a tokio runtime internally
                // so the tokio runtime created for the metrics server can't be reused.
                let thread_handle = std::thread::spawn(move || {
                    push_metrics(push_config, metrics_push_kill_switch_rx);
                });

                (Some(thread_handle), Some(metrics_push_kill_switch))
            } else {
                (None, None)
            };

        Self {
            metrics,
            registry,
            metrics_server_handle,
            metrics_server_kill_switch,
            metrics_push_handle,
            metrics_push_kill_switch,
        }
    }

    fn init_registry(metrics: &Metrics) -> Registry {
        let registry = Registry::new();
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
        registry
    }

    fn init_metrics() -> Metrics {
        Metrics {
            cap_total_call_count: GenericCounter::new(
                "cap_total_call_count",
                "total number of capability invocations",
            )
            .unwrap(),
            cap_call_count: HashMap::new(),
            cap_total_call_time: GenericCounter::new(
                "cap_total_call_time",
                "total call time for all capabilities",
            )
            .unwrap(),
            cap_call_time: HashMap::new(),
            actor_total_call_count: GenericCounter::new(
                "actor_total_call_count",
                "total number of actor invocations",
            )
            .unwrap(),
            actor_call_count: HashMap::new(),
            actor_total_call_time: GenericCounter::new(
                "actor_total_call_time",
                "total call time for all actors",
            )
            .unwrap(),
            actor_call_time: HashMap::new(),
        }
    }
}

// TODO Add timing of invocations
impl Middleware for PrometheusMiddleware {
    fn actor_pre_invoke(&self, inv: Invocation) -> Result<Invocation> {
        pre_invoke(&self.metrics, &self.registry, inv)
    }

    fn actor_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse> {
        Ok(response)
    }

    fn capability_pre_invoke(&self, inv: Invocation) -> Result<Invocation> {
        pre_invoke(&self.metrics, &self.registry, inv)
    }

    fn capability_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse> {
        Ok(response)
    }
}

fn pre_invoke(
    metrics: &Arc<RwLock<Metrics>>,
    registry: &Arc<RwLock<Registry>>,
    inv: Invocation,
) -> Result<Invocation> {
    let mut metrics = metrics.write().unwrap();
    let registry = registry.write().unwrap();
    let target = match &inv.target {
        InvocationTarget::Actor(actor) => actor.clone(),
        InvocationTarget::Capability { capid, binding } => capid.clone() + binding,
    };
    let key = inv.origin.clone() + &inv.operation + &target;

    match &inv.target {
        InvocationTarget::Actor(actor) => {
            metrics.actor_total_call_count.inc();
            if let Some(value) = metrics.actor_call_count.get(&key) {
                value.inc();
            } else {
                let name = format!("{}_call_count", &actor);
                let help = format!("number of invocations of {}", &actor);

                match register_new_counter(&mut metrics, &registry, key, &name, &help) {
                    Err(e) => error!("Error registering counter '{}': {}", &name, e),
                    _ => (),
                };
            }
        }
        InvocationTarget::Capability { capid, binding } => {
            metrics.cap_total_call_count.inc();

            if let Some(value) = metrics.cap_call_count.get(&key) {
                value.inc();
            } else {
                let name = format!("{}_{}_call_count", &capid, &binding);
                let help = format!(
                    "number of invocations of capability {} with binding {}",
                    &capid, &binding
                );

                match register_new_counter(&mut metrics, &registry, key, &name, &help) {
                    Err(e) => error!("Error registering counter '{}': {}", &name, e),
                    _ => (),
                };
            }
        }
    }

    Ok(inv)
}

fn register_new_counter(
    metrics: &mut RwLockWriteGuard<Metrics>,
    registry: &RwLockWriteGuard<Registry>,
    key: String,
    counter_name: &String,
    counter_help: &String,
) -> prometheus::Result<()> {
    let counter = GenericCounter::new(counter_name.clone(), counter_help.clone())?;
    // TODO Is this correct handling of the counter?
    //  Would not like to clone really... Want to point to the
    //  same counter from two places. Store an Arc in the HashMap?
    counter.inc();
    registry.register(Box::new(counter.clone()))?;
    metrics.actor_call_count.insert(key, counter);
    Ok(())
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

async fn serve_request(
    req: Request<Body>,
    registry: Arc<RwLock<Registry>>,
) -> hyper::error::Result<Response<Body>> {
    match (req.method(), req.uri().path()) {
        (&Method::GET, "/metrics") => {
            let reg = registry
                .read()
                .expect("Failed to to get read lock on registry");
            let metric_families = reg.gather();
            let mut buffer = vec![];
            let encoder = TextEncoder::new();
            encoder.encode(&metric_families, &mut buffer).unwrap();

            Ok(Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, encoder.format_type())
                .body(Body::from(buffer))
                .unwrap())
        }
        _ => Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap()),
    }
}

async fn serve_metrics(addr: SocketAddr, registry: Arc<RwLock<Registry>>) {
    let server = Server::bind(&addr).serve(make_service_fn(move |_| {
        let registry2 = registry.clone();
        async move {
            Ok::<_, hyper::error::Error>(service_fn(move |req| {
                serve_request(req, registry2.clone())
            }))
        }
    }));

    if let Err(e) = server.await {
        error!("Metrics server error: {}", e);
    }
}

fn push_metrics(
    push_config: PushgatewayConfig,
    mut push_kill_switch_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let job = push_config.job.unwrap_or(String::from("wascc_push"));
    let pushgateway_addr = &push_config.pushgateway_addr;

    loop {
        match push_kill_switch_rx.try_recv() {
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            _ => {
                return;
            }
        }

        let metric_families = prometheus::gather();

        let basic_auth = if let Some(basic_auth) = &push_config.push_basic_auth {
            Some(prometheus::BasicAuthentication {
                username: basic_auth.username.clone(),
                password: basic_auth.password.clone(),
            })
        } else {
            None
        };

        let push_res = prometheus::push_metrics(
            &job,
            labels! {},
            &pushgateway_addr,
            metric_families,
            basic_auth,
        );

        match push_res {
            Err(e) => error!("Error pushing metrics to {}: {}", &pushgateway_addr, e),
            _ => {}
        }

        std::thread::sleep(push_config.push_interval.clone());
    }
}

#[cfg(test)]
mod tests {
    use crate::prometheus_middleware::{PrometheusConfig, PrometheusMiddleware, PushgatewayConfig};
    use crate::{Invocation, InvocationTarget, Middleware};
    use mockito::{mock, Matcher};
    use rand::random;
    use std::net::SocketAddr;
    use std::time::Duration;

    fn actor_invocation() -> Invocation {
        Invocation::new(
            "actor_origin".to_owned(),
            InvocationTarget::Actor("actor_id".to_owned()),
            "actor_op",
            "actor_msg".into(),
        )
    }

    fn cap_invocation() -> Invocation {
        Invocation::new(
            "cap_origin".to_owned(),
            InvocationTarget::Capability {
                capid: "capid".to_owned(),
                binding: "binding".to_owned(),
            },
            "cap_op",
            "cap_msg".into(),
        )
    }

    #[test]
    fn test_serve_metrics() -> Result<(), reqwest::Error> {
        let server_addr: SocketAddr = ([127, 0, 0, 1], 9898).into();
        let config = PrometheusConfig {
            metrics_server_addr: Some(server_addr),
            pushgateway_config: None,
        };
        let middleware = PrometheusMiddleware::new(config);
        let invocations = random::<u8>();
        let actor_invocation = actor_invocation();
        let cap_invocation = cap_invocation();

        for _ in 0..invocations {
            middleware
                .actor_pre_invoke(actor_invocation.clone())
                .unwrap();
            middleware.actor_pre_invoke(cap_invocation.clone()).unwrap();
        }

        let url = format!("http://{}/metrics", &server_addr.to_string());
        let body = reqwest::blocking::get(&url)?.text()?;

        // TODO Add asserts for all metrics
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
        let invocations = random::<u8>();

        let actor_total_call_count_matcher = format!("actor_total_call_count {}", invocations);
        let actor_total_call_count = mock("PUT", "/metrics/job/wascc_push")
            .match_header("content-type", "application/vnd.google.protobuf; proto=io.prometheus.client.MetricFamily; encoding=delimited")
            .match_header("content-length", Matcher::Any)
            .match_header("accept", "*/*")
            .match_header("host", Matcher::Any)
            // .match_body(Matcher::Regex(r"/actor_total_call_count/".to_owned()))
            .match_body(Matcher::Regex(actor_total_call_count_matcher))
            // flexible number of invocations to avoid flakiness
            // due to timing between push interval and waiting for the push to
            // happen before asserting
            .expect_at_least(1)
            .create();

        // setup middleware for push
        let push_interval = Duration::from_millis(500);
        let config = PrometheusConfig {
            metrics_server_addr: None,
            pushgateway_config: Some(PushgatewayConfig {
                push_interval,
                pushgateway_addr: mockito::server_url(),
                job: None,
                push_basic_auth: None,
            }),
        };
        let middleware = PrometheusMiddleware::new(config);
        let actor_invocation = actor_invocation();
        let cap_invocation = cap_invocation();

        for _ in 0..invocations {
            middleware
                .actor_pre_invoke(actor_invocation.clone())
                .unwrap();
            middleware.actor_pre_invoke(cap_invocation.clone()).unwrap();
        }

        // wait a little to make sure the push has been done before asserting
        std::thread::sleep(Duration::from_secs(1));

        actor_total_call_count.assert();
    }
}
