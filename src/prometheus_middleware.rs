use crate::{errors, Invocation, InvocationResponse, InvocationTarget, Middleware, Result};
use hyper::header::CONTENT_TYPE;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use prometheus::{labels, Encoder, IntCounter, IntGauge, Registry, TextEncoder};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

struct PrometheusMiddleware {
    metrics: Arc<RwLock<Metrics>>,
    registry: Arc<RwLock<Registry>>,
    metrics_server_handle: Option<JoinHandle<()>>,
    metrics_server_kill_switch: Option<tokio::sync::oneshot::Sender<()>>,
    metrics_push_handle: Option<JoinHandle<()>>,
    metrics_push_kill_switch: Option<tokio::sync::oneshot::Sender<()>>,
}

struct Metrics {
    /// Total number of invocations of capabilities
    cap_total_inv_count: IntCounter,
    /// Number of invocations per capability
    cap_inv_count: HashMap<String, IntCounter>,
    /// Average invocation time across all capabilities
    cap_total_average_inv_time: IntGauge,
    /// Average invocation time per capability
    cap_average_inv_time: HashMap<String, IntGauge>,
    /// Measurements of invocation time for active invocations of capabilities
    cap_active_inv_time: HashMap<String, Instant>,

    /// Total number of invocations of actors
    actor_total_inv_count: IntCounter,
    /// Number of invocations per actor
    actor_inv_count: HashMap<String, IntCounter>,
    /// Average invocation time across all actors
    actor_total_average_inv_time: IntGauge,
    /// Average invocation time per actor
    actor_average_inv_time: HashMap<String, IntGauge>,
    /// Measurements of invocation time for active invocations of actors
    actor_active_inv_time: HashMap<String, Instant>,
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
    fn new(config: PrometheusConfig) -> Result<Self>
    where
        Self: Send + Sync,
    {
        let metrics = PrometheusMiddleware::init_metrics()?;
        let registry = PrometheusMiddleware::init_registry(&metrics)?;
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
                let registry2 = registry.clone();

                // need a separate thread for pushing metrics because the reqwest sync client
                // is used in the prometheus library. reqwest sync creates a tokio runtime internally
                // so the tokio runtime created for the metrics server can't be reused.
                let thread_handle = std::thread::spawn(move || {
                    push_metrics(registry2, push_config, metrics_push_kill_switch_rx);
                });

                (Some(thread_handle), Some(metrics_push_kill_switch))
            } else {
                (None, None)
            };

        Ok(Self {
            metrics,
            registry,
            metrics_server_handle,
            metrics_server_kill_switch,
            metrics_push_handle,
            metrics_push_kill_switch,
        })
    }

    fn init_registry(metrics: &Metrics) -> Result<Registry> {
        let registry = Registry::new();
        registry.register(Box::new(metrics.cap_total_inv_count.clone()))?;
        registry.register(Box::new(metrics.cap_total_average_inv_time.clone()))?;
        registry.register(Box::new(metrics.actor_total_inv_count.clone()))?;
        registry.register(Box::new(metrics.actor_total_average_inv_time.clone()))?;
        Ok(registry)
    }

    fn init_metrics() -> Result<Metrics> {
        Ok(Metrics {
            cap_total_inv_count: IntCounter::new(
                "cap_total_inv_count",
                "total number of capability invocations",
            )?,
            cap_inv_count: HashMap::new(),
            cap_total_average_inv_time: IntGauge::new(
                "cap_total_average_inv_time",
                "average invocation time across all capabilities",
            )?,
            cap_average_inv_time: HashMap::new(),
            cap_active_inv_time: HashMap::new(),

            actor_total_inv_count: IntCounter::new(
                "actor_total_inv_count",
                "total number of actor invocations",
            )?,
            actor_inv_count: HashMap::new(),
            actor_total_average_inv_time: IntGauge::new(
                "actor_total_average_inv_time",
                "average invocation time across all actors",
            )?,
            actor_average_inv_time: HashMap::new(),
            actor_active_inv_time: HashMap::new(),
        })
    }
}

impl Middleware for PrometheusMiddleware {
    fn actor_pre_invoke(&self, inv: Invocation) -> Result<Invocation> {
        pre_invoke(&self.metrics, &self.registry, inv)
    }

    fn actor_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse> {
        post_invoke_measure_time(&self.metrics, &self.registry, response)
    }

    fn capability_pre_invoke(&self, inv: Invocation) -> Result<Invocation> {
        pre_invoke(&self.metrics, &self.registry, inv)
    }

    fn capability_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse> {
        post_invoke_measure_time(&self.metrics, &self.registry, response)
    }
}

impl From<prometheus::Error> for errors::Error {
    fn from(e: prometheus::Error) -> errors::Error {
        match e {
            prometheus::Error::AlreadyReg => errors::new(errors::ErrorKind::Middleware(
                "Metric already registered".to_owned(),
            )),
            prometheus::Error::Msg(m) => errors::new(errors::ErrorKind::Middleware(m)),
            prometheus::Error::Io(e) => errors::new(errors::ErrorKind::Middleware(e.to_string())),
            _ => errors::new(errors::ErrorKind::Middleware(
                "Unknown Prometheus error".to_owned(),
            )),
        }
    }
}

fn pre_invoke(
    metrics: &Arc<RwLock<Metrics>>,
    registry: &Arc<RwLock<Registry>>,
    inv: Invocation,
) -> Result<Invocation> {
    let mut metrics = metrics.write().unwrap();

    let target = match &inv.target {
        InvocationTarget::Actor(actor) => actor.clone(),
        InvocationTarget::Capability { capid, binding } => capid.clone() + binding,
    };
    // count invocations per sender<->receiver pair
    let key = inv.origin.clone() + &target;

    match &inv.target {
        InvocationTarget::Actor(actor) => {
            metrics.actor_total_inv_count.inc();
            if let Some(value) = metrics.actor_inv_count.get(&key) {
                value.inc();
            } else {
                let name = format!("{}_inv_count", &actor);
                let help = format!("number of invocations of {}", &actor);

                if let Err(e) =
                    register_new_counter(&mut metrics.actor_inv_count, &registry, key, &name, &help)
                {
                    error!("Error registering counter '{}': {}", &name, e);
                }
            }
        }
        InvocationTarget::Capability { capid, binding } => {
            metrics.cap_total_inv_count.inc();

            if let Some(value) = metrics.cap_inv_count.get(&key) {
                value.inc();
            } else {
                let name = format!("{}_{}_inv_count", &capid, &binding);
                let help = format!(
                    "number of invocations of capability {} with binding {}",
                    &capid, &binding
                );

                if let Err(e) =
                    register_new_counter(&mut metrics.cap_inv_count, &registry, key, &name, &help)
                {
                    error!("Error registering counter '{}': {}", &name, e);
                }
            }
        }
    }

    Ok(inv)
}

fn register_new_counter(
    counters: &mut HashMap<String, IntCounter>,
    registry: &Arc<RwLock<Registry>>,
    key: String,
    counter_name: &str,
    counter_help: &str,
) -> prometheus::Result<()> {
    let counter = IntCounter::new(counter_name.to_string(), counter_help.to_string())?;
    counter.inc();

    let reg = registry.write().unwrap();
    reg.register(Box::new(counter.clone()))?;

    counters.insert(key, counter);
    Ok(())
}

fn pre_invoke_measure_time(metrics: &Arc<RwLock<Metrics>>, inv: Invocation) -> Result<Invocation> {
    let mut metrics = metrics.write().unwrap();

    // TODO Need to use a better key for storing measurements
    let target = match &inv.target {
        InvocationTarget::Actor(actor) => actor.clone(),
        InvocationTarget::Capability { capid, binding } => capid.clone() + binding,
    };
    let key = inv.origin.clone() + &inv.operation + &target;

    match &inv.target {
        InvocationTarget::Actor(_) => {
            if let Some(_) = metrics.actor_active_inv_time.get(&key) {
                error!("Active invocation of capability found, key: {}", &key);
            } else {
                metrics.actor_active_inv_time.insert(key, Instant::now());
            }
        }
        InvocationTarget::Capability {
            capid: _,
            binding: _,
        } => {
            if let Some(_) = metrics.cap_active_inv_time.get(&key) {
                error!("Active invocation of capability found, key: {}", &key);
            } else {
                metrics.cap_active_inv_time.insert(key, Instant::now());
            }
        }
    }

    Ok(inv)
}

fn post_invoke_measure_time(
    metrics: &Arc<RwLock<Metrics>>,
    registry: &Arc<RwLock<Registry>>,
    response: InvocationResponse,
) -> Result<InvocationResponse> {
    // TODO Need someway to correlate a InvocationResponse and an Invocation to
    // measure the time.
    Ok(response)
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
    registry: Arc<RwLock<Registry>>,
    push_config: PushgatewayConfig,
    mut push_kill_switch_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let job = push_config
        .job
        .unwrap_or_else(|| String::from("wascc_push"));
    let pushgateway_addr = &push_config.pushgateway_addr;

    loop {
        match push_kill_switch_rx.try_recv() {
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            _ => {
                return;
            }
        }

        let basic_auth = if let Some(basic_auth) = &push_config.push_basic_auth {
            Some(prometheus::BasicAuthentication {
                username: basic_auth.username.clone(),
                password: basic_auth.password.clone(),
            })
        } else {
            None
        };

        // make sure the read lock is released before we start
        // pushing or sleeping on this thread
        let metric_families = {
            let reg = registry.read().unwrap();
            reg.gather()
        };

        if let Err(e) = prometheus::push_metrics(
            &job,
            labels! {},
            &pushgateway_addr,
            metric_families,
            basic_auth,
        ) {
            error!("Error pushing metrics to {}: {}", &pushgateway_addr, e);
        }

        std::thread::sleep(push_config.push_interval);
    }
}

#[cfg(test)]
mod tests {
    use crate::prometheus_middleware::{PrometheusConfig, PrometheusMiddleware, PushgatewayConfig};
    use crate::{Invocation, InvocationTarget, Middleware};
    use mockito::{mock, Matcher};
    use rand::random;
    use std::net::SocketAddr;
    use std::ops::Mul;
    use std::time::Duration;

    const CAPID: &str = "capid";
    const BINDING: &str = "binding";
    const TARGET_ACTOR: &str = "target_actor";

    fn actor_invocation() -> Invocation {
        Invocation::new(
            "actor_origin".to_owned(),
            InvocationTarget::Actor(TARGET_ACTOR.to_owned()),
            "actor_op",
            "actor_msg".into(),
        )
    }

    fn cap_invocation() -> Invocation {
        Invocation::new(
            "cap_origin".to_owned(),
            InvocationTarget::Capability {
                capid: CAPID.to_owned(),
                binding: BINDING.to_owned(),
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
        let middleware = PrometheusMiddleware::new(config).unwrap();
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

        // capabilities
        assert!(body
            .find(&format!("cap_total_inv_count {}", invocations))
            .is_some());
        assert!(body
            .find(&format!("{}_{}_inv_count {}", CAPID, BINDING, invocations))
            .is_some());
        assert!(body
            .find(&format!("cap_total_average_inv_time 0"))
            .is_some());
        // TODO Add assert for cap_average_inv_time

        // actors
        assert!(body
            .find(&format!("actor_total_inv_count {}", invocations))
            .is_some());
        assert!(body
            .find(&format!("{}_inv_count {}", TARGET_ACTOR, invocations))
            .is_some());
        assert!(body
            .find(&format!("actor_total_average_inv_time 0"))
            .is_some());
        // TODO Add assert for actor_average_inv_time

        Ok(())
    }

    #[test]
    fn test_push_metrics() {
        let invocations = random::<u8>();

        // The data format that is used is not compatible with any current Mockito
        // matcher. It doesn't currently seem to be possible to get the raw body in
        // a matcher, see https://github.com/lipanski/mockito/issues/95
        let pushed_metrics = mock("PUT", "/metrics/job/wascc_push")
            .match_header("content-type", "application/vnd.google.protobuf; proto=io.prometheus.client.MetricFamily; encoding=delimited")
            .match_header("content-length", Matcher::Any)
            .match_header("accept", "*/*")
            .match_header("host", Matcher::Any)
            // flexible number of invocations to avoid flakiness due to timing between 
            // push interval and waiting for the push to happen before asserting
            .expect_at_least(1)
            .create();

        // setup middleware for push
        let push_interval = Duration::from_millis(100);
        let config = PrometheusConfig {
            metrics_server_addr: None,
            pushgateway_config: Some(PushgatewayConfig {
                push_interval,
                pushgateway_addr: mockito::server_url(),
                job: None,
                push_basic_auth: None,
            }),
        };

        let middleware = PrometheusMiddleware::new(config).unwrap();
        let actor_invocation = actor_invocation();
        let cap_invocation = cap_invocation();

        for _ in 0..invocations {
            middleware
                .actor_pre_invoke(actor_invocation.clone())
                .unwrap();
            middleware.actor_pre_invoke(cap_invocation.clone()).unwrap();
        }

        // make sure at least one push is done before asserting
        std::thread::sleep(push_interval.mul(2));

        pushed_metrics.assert();
    }
}
