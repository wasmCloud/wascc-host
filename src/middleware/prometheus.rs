//! # Prometheus Middleware
//!
//! [Prometheus][prometheus] is an open-source systems monitoring and alerting toolkit. This
//! middleware makes it possible to serve metrics for scraping by [Prometheus][prometheus] or
//! by pushing metrics to the [Prometheus Pushgateway][prometheus_pushgateway]
//! that is then scraped by [Prometheus][prometheus].
//!
//! Enable this middleware using the feature flag `prometheus_middleware`.
//!
//! ## Getting Started
//!
//! Here is an example of how to serve metrics for scraping:
//!
//! ```
//! # use std::net::SocketAddr;
//! let server_addr: SocketAddr = ([127, 0, 0, 1], 9898).into();
//! let config = wascc_host::middleware::prometheus::PrometheusConfig {
//!     metrics_server_addr: Some(server_addr),
//!     pushgateway_config: None,
//!     moving_average_window_size: None,
//! };
//! let middleware = wascc_host::middleware::prometheus::PrometheusMiddleware::new(config).unwrap();
//! ```
//!
//! This will expose metrics at `http://127.0.0.1:9898/metrics`. This can be
//! used as a scraping target in [Prometheus][prometheus].
//!
//! All metrics are prefixed with 'wascc_'.
//!
//! Here is a simple [Prometheus][prometheus] configuration that scrapes the above target and
//! the [Prometheus Pushgateway][prometheus_pushgateway] (save the file as `prometheus.yml`):
//!
//! ```yml
//! global:
//!   scrape_interval: 15s
//!
//! # Scrape configurations
//! scrape_configs:
//!   - job_name: "wascc"
//!     scrape_interval: 5s
//!     honor_labels: true
//!     # Scrape waSCC.
//!     # 'host.docker.internal' points to the host on Win and Mac.
//!     static_configs:
//!       - targets: ["host.docker.internal:9898"]
//!   - job_name: "pushgateway"
//!     scrape_interval: 5s
//!     honor_labels: true
//!     # Scrape the pushgateway
//!     static_configs:
//!       - targets: ["pushgateway:9091"]
//! ```
//!
//! [Docker Compose][docker_compose] can be used to run containers with
//! running [Prometheus][prometheus], [Prometheus Pushgateway][prometheus_pushgateway], and an
//! optional [Grafana][grafana] instance. It expects that the [Prometheus][prometheus] configuration file
//! is located in the same directory. Here is the `docker-compose.yml`:
//!
//! ```yml
//! version: "3.3"
//! services:
//!   prometheus:
//!     image: prom/prometheus
//!     ports:
//!       - "9090:9090"
//!     volumes:
//!       - .:/etc/prometheus/
//!
//!   pushgateway:
//!     image: prom/pushgateway
//!     ports:
//!       - "9091:9091"
//!     depends_on:
//!       - prometheus
//!
//!   grafana:
//!     image: grafana/grafana
//!     ports:
//!       - "3000:3000"
//!     depends_on:
//!       - prometheus
//! ```
//!
//! Start the containers with:
//!
//! `docker-compose -f "docker-compose.yml" up -d --build`
//!
//! [prometheus]: https://prometheus.io/
//! [prometheus_pushgateway]: https://github.com/prometheus/pushgateway/blob/master/README.md
//! [docker_compose]: https://docs.docker.com/compose/
//! [grafana]: https://grafana.com/

use crate::middleware::{InvocationHandler, MiddlewareResponse};
use crate::{errors, Invocation, InvocationResponse, Middleware, Result, WasccEntity};
use hyper::header::CONTENT_TYPE;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use prometheus::{labels, Encoder, Gauge, IntCounter, Registry, TextEncoder};
use std::cmp::min;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock, RwLockWriteGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

// The default number of invocations to include when calculating average invocation times
const DEFAULT_MOVING_AVERAGE_WINDOW_SIZE: i64 = 100;
const WASCC: &str = "wascc";

/// A Prometheus middleware that can serve or push metrics.
pub struct PrometheusMiddleware {
    metrics: Arc<RwLock<Metrics>>,
    registry: Arc<RwLock<Registry>>,
    metrics_server_handle: Option<JoinHandle<()>>,
    metrics_server_kill_switch: Option<tokio::sync::oneshot::Sender<()>>,
    metrics_push_handle: Option<JoinHandle<()>>,
    metrics_push_kill_switch: Option<tokio::sync::oneshot::Sender<()>>,
}

/// Holds all the different metrics collected by the middleware.
struct Metrics {
    /// Total number of invocations of capabilities
    cap_total_inv_count: IntCounter,
    /// Number of invocations per capability
    cap_inv_count: HashMap<String, IntCounter>,
    /// Number of invocations per operation on each capability
    cap_operation_inv_count: HashMap<String, IntCounter>,
    /// Average invocation time across all capabilities
    cap_total_average_inv_time: Gauge,
    /// Average invocation time per capability
    cap_average_inv_time: HashMap<String, Gauge>,
    /// Average invocation time per operation on each capability
    cap_operation_average_inv_time: HashMap<String, Gauge>,

    /// Total number of invocations of actors
    actor_total_inv_count: IntCounter,
    /// Number of invocations per actor
    actor_inv_count: HashMap<String, IntCounter>,
    /// Number of invocations per operation on each actor
    actor_operation_inv_count: HashMap<String, IntCounter>,
    /// Average invocation time across all actors
    actor_total_average_inv_time: Gauge,
    /// Average invocation time per actor
    actor_average_inv_time: HashMap<String, Gauge>,
    /// Average invocation time per operation on each actor
    actor_operation_average_inv_time: HashMap<String, Gauge>,

    /// State of active invocations
    active_inv_state: HashMap<String, InvocationState>,

    moving_average_window_size: i64,
}

/// Configuration parameters.
#[derive(Clone)]
pub struct PrometheusConfig {
    /// The address that Prometheus can scrape (pull model).
    pub metrics_server_addr: Option<SocketAddr>,
    /// Configuration for the Prometheus client (push model).
    pub pushgateway_config: Option<PushgatewayConfig>,
    /// The number of invocations to include when calculating all average invocation times.
    /// The default is 100.
    pub moving_average_window_size: Option<i64>,
}

/// Configuration parameters for pushing metrics to the Pushgateway.
#[derive(Clone)]
pub struct PushgatewayConfig {
    pub pushgateway_addr: String,
    pub push_interval: Duration,
    pub push_basic_auth: Option<BasicAuthentication>,
    pub job: Option<String>,
}

/// Basic authentication for authentication with a Pushgateway.
#[derive(Clone)]
pub struct BasicAuthentication {
    pub username: String,
    pub password: String,
}

/// Values needed during an invocation to compute average invocation times.
struct InvocationState {
    start_time: Instant,
    operation: String,
    target: WasccEntity,
    /// Key to find metrics for a capability or an actor
    metric_key: String,
    /// Key to find metrics for an operation on a capability or an actor
    operation_metric_key: String,
}

impl PrometheusMiddleware {
    pub fn new(config: PrometheusConfig) -> Result<Self>
    where
        Self: Send + Sync,
    {
        let metrics = PrometheusMiddleware::init_metrics(&config)?;
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

    fn init_metrics(config: &PrometheusConfig) -> Result<Metrics> {
        Ok(Metrics {
            cap_total_inv_count: IntCounter::new(
                format!("{}_cap_total_inv_count", WASCC),
                "Total number of capability invocations".to_owned(),
            )?,
            cap_inv_count: HashMap::new(),
            cap_operation_inv_count: HashMap::new(),
            cap_total_average_inv_time: Gauge::new(
                format!("{}_cap_total_average_inv_time", WASCC),
                "Average invocation time (ms) across all capabilities".to_owned(),
            )?,
            cap_average_inv_time: HashMap::new(),
            cap_operation_average_inv_time: HashMap::new(),

            actor_total_inv_count: IntCounter::new(
                format!("{}_actor_total_inv_count", WASCC),
                "Total number of actor invocations".to_owned(),
            )?,
            actor_inv_count: HashMap::new(),
            actor_operation_inv_count: HashMap::new(),
            actor_total_average_inv_time: Gauge::new(
                format!("{}_actor_total_average_inv_time", WASCC),
                "Average invocation time (ms) across all actors".to_owned(),
            )?,
            actor_average_inv_time: HashMap::new(),
            actor_operation_average_inv_time: HashMap::new(),

            active_inv_state: HashMap::new(),
            moving_average_window_size: config
                .moving_average_window_size
                .unwrap_or_else(|| DEFAULT_MOVING_AVERAGE_WINDOW_SIZE),
        })
    }
}

impl Middleware for PrometheusMiddleware {
    fn actor_pre_invoke(&self, inv: Invocation) -> Result<Invocation> {
        pre_invoke_count_inv(&self.metrics, &self.registry, &inv.target, &inv.operation);
        pre_invoke_measure_inv_time(&self.metrics, &inv);
        Ok(inv)
    }

    fn actor_invoke(
        &self,
        inv: Invocation,
        handler: InvocationHandler,
    ) -> Result<MiddlewareResponse> {
        Ok(MiddlewareResponse::Continue(handler.invoke(inv)))
    }

    fn actor_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse> {
        post_invoke_measure_inv_time(&self.metrics, &self.registry, &response);
        Ok(response)
    }

    fn capability_pre_invoke(&self, inv: Invocation) -> Result<Invocation> {
        pre_invoke_count_inv(&self.metrics, &self.registry, &inv.target, &inv.operation);
        pre_invoke_measure_inv_time(&self.metrics, &inv);
        Ok(inv)
    }

    fn capability_invoke(
        &self,
        inv: Invocation,
        handler: InvocationHandler,
    ) -> Result<MiddlewareResponse> {
        Ok(MiddlewareResponse::Continue(handler.invoke(inv)))
    }

    fn capability_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse> {
        post_invoke_measure_inv_time(&self.metrics, &self.registry, &response);
        Ok(response)
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

fn pre_invoke_count_inv(
    metrics: &Arc<RwLock<Metrics>>,
    registry: &Arc<RwLock<Registry>>,
    target: &WasccEntity,
    operation: &str,
) {
    let mut metrics = metrics.write().unwrap();

    match target {
        WasccEntity::Actor(actor) => {
            metrics.actor_total_inv_count.inc();

            let actor_key = get_metric_key(target);
            if let Some(value) = metrics.actor_inv_count.get(&actor_key) {
                value.inc();
            } else {
                let name = format!("{}_{}_inv_count", WASCC, &actor);
                let help = format!("Number of invocations of actor '{}'", &actor);
                register_counter(
                    &mut metrics.actor_inv_count,
                    &registry,
                    actor_key,
                    &name,
                    &help,
                );
            }

            let actor_operation_key = get_operation_metric_key(target, operation);
            if let Some(value) = metrics.actor_operation_inv_count.get(&actor_operation_key) {
                value.inc();
            } else {
                let name = format!("{}_{}_{}_inv_count", WASCC, &actor, &operation);
                let help = format!(
                    "Number of invocations of operation '{}' on actor '{}'",
                    &operation, &actor
                );
                register_counter(
                    &mut metrics.actor_operation_inv_count,
                    &registry,
                    actor_operation_key,
                    &name,
                    &help,
                )
            }
        }
        WasccEntity::Capability { capid, binding } => {
            metrics.cap_total_inv_count.inc();

            let cap_key = get_metric_key(target);
            if let Some(value) = metrics.cap_inv_count.get(&cap_key) {
                value.inc();
            } else {
                let name = format!("{}_{}_{}_inv_count", WASCC, &capid, &binding);
                let help = format!(
                    "Number of invocations of capability '{}' with binding '{}'",
                    &capid, &binding
                );
                register_counter(&mut metrics.cap_inv_count, &registry, cap_key, &name, &help);
            }

            let cap_operation_key = get_operation_metric_key(target, operation);
            if let Some(value) = metrics.cap_operation_inv_count.get(&cap_operation_key) {
                value.inc();
            } else {
                let name = format!("{}_{}_{}_{}_inv_count", WASCC, &capid, &binding, &operation);
                let help = format!(
                    "Number of invocations of operation '{}' on capability '{}' with binding '{}'",
                    &capid, &binding, &operation
                );
                register_counter(
                    &mut metrics.cap_operation_inv_count,
                    &registry,
                    cap_operation_key,
                    &name,
                    &help,
                );
            }
        }
    }
}

fn register_counter(
    counters: &mut HashMap<String, IntCounter>,
    registry: &Arc<RwLock<Registry>>,
    counter_lookup_key: String,
    name: &str,
    help: &str,
) {
    let counter = match IntCounter::new(name.to_string(), help.to_string()) {
        Ok(counter) => {
            counter.inc();
            counter
        }
        Err(e) => {
            error!("Error creating counter '{}': {}", &name, e);
            return;
        }
    };

    let reg = registry.write().unwrap();
    if let Err(e) = reg.register(Box::new(counter.clone())) {
        error!("Error registering counter '{}': {}", &name, e);
    }

    counters.insert(counter_lookup_key, counter);
}

fn get_operation_metric_key(target: &WasccEntity, operation: &str) -> String {
    get_metric_key(target) + operation
}

fn get_metric_key(target: &WasccEntity) -> String {
    match target {
        WasccEntity::Actor(actor) => actor.clone(),
        WasccEntity::Capability { capid, binding } => capid.clone() + binding,
    }
}

fn pre_invoke_measure_inv_time(metrics: &Arc<RwLock<Metrics>>, inv: &Invocation) {
    let mut metrics = metrics.write().unwrap();
    let state = InvocationState {
        start_time: Instant::now(),
        operation: inv.operation.clone(),
        target: inv.target.clone(),
        metric_key: get_metric_key(&inv.target),
        operation_metric_key: get_operation_metric_key(&inv.target, &inv.operation),
    };

    if metrics
        .active_inv_state
        .insert(inv.id.clone(), state)
        .is_some()
    {
        error!("Invocation already in progress with id '{}'", &inv.id);
    }
}

fn post_invoke_measure_inv_time(
    metrics: &Arc<RwLock<Metrics>>,
    registry: &Arc<RwLock<Registry>>,
    response: &InvocationResponse,
) {
    let mut metrics = metrics.write().unwrap();
    let inv_end_time = Instant::now();

    // get the state for this invocation
    if let Some(state) = metrics.active_inv_state.remove(&response.invocation_id) {
        let inv_time = if let Some(time) = inv_end_time.checked_duration_since(state.start_time) {
            time.as_millis()
        } else {
            error!(
                "Unable to compute invocation time for id '{}'",
                &response.invocation_id
            );
            return;
        };

        set_new_total_avg(&metrics, &state.target, inv_time);

        // was an actor or a capability invoked?
        match &state.target {
            WasccEntity::Actor(actor) => {
                if let Some(gauge) = metrics.actor_average_inv_time.get(&state.metric_key) {
                    set_gauge_avg(
                        gauge,
                        &metrics.actor_inv_count,
                        metrics.moving_average_window_size,
                        inv_time,
                        &state.metric_key,
                    );
                } else {
                    let name = format!("{}_{}_average_inv_time", WASCC, actor.clone());
                    let help = format!("Average time (ms) to invoke actor '{}'", actor.clone());

                    register_gauge(
                        registry,
                        &mut metrics.actor_average_inv_time,
                        &state.metric_key,
                        &name,
                        &help,
                        inv_time,
                    );
                }

                if let Some(gauge) = metrics
                    .actor_operation_average_inv_time
                    .get(&state.operation_metric_key)
                {
                    set_gauge_avg(
                        gauge,
                        &metrics.actor_operation_inv_count,
                        metrics.moving_average_window_size,
                        inv_time,
                        &state.operation_metric_key,
                    );
                } else {
                    let name = format!(
                        "{}_{}_{}_average_inv_time",
                        WASCC,
                        actor.clone(),
                        &state.operation
                    );
                    let help = format!(
                        "Average time (ms) to invoke operation '{}' on actor '{}'",
                        &state.operation,
                        actor.clone()
                    );

                    register_gauge(
                        registry,
                        &mut metrics.actor_operation_average_inv_time,
                        &state.operation_metric_key,
                        &name,
                        &help,
                        inv_time,
                    );
                }
            }
            WasccEntity::Capability { capid, binding } => {
                if let Some(gauge) = metrics.cap_average_inv_time.get(&state.metric_key) {
                    set_gauge_avg(
                        gauge,
                        &metrics.cap_inv_count,
                        metrics.moving_average_window_size,
                        inv_time,
                        &state.metric_key,
                    );
                } else {
                    let name = format!("{}_{}_{}_average_inv_time", WASCC, capid, binding);
                    let help = format!(
                        "Average time (ms) to invoke capability '{}' with binding '{}'",
                        capid, binding
                    );

                    register_gauge(
                        registry,
                        &mut metrics.cap_average_inv_time,
                        &state.metric_key,
                        &name,
                        &help,
                        inv_time,
                    );
                }

                if let Some(gauge) = metrics
                    .cap_operation_average_inv_time
                    .get(&state.operation_metric_key)
                {
                    set_gauge_avg(
                        gauge,
                        &metrics.cap_operation_inv_count,
                        metrics.moving_average_window_size,
                        inv_time,
                        &state.operation_metric_key,
                    );
                } else {
                    let name = format!(
                        "{}_{}_{}_{}_average_inv_time",
                        WASCC, capid, binding, &state.operation
                    );
                    let help = format!(
                        "Average time (ms) to invoke operation '{}' on capability '{}' with binding '{}'",
                        &state.operation, capid, binding
                    );

                    register_gauge(
                        registry,
                        &mut metrics.cap_operation_average_inv_time,
                        &state.operation_metric_key,
                        &name,
                        &help,
                        inv_time,
                    );
                }
            }
        }
    } else {
        error!("No active invocation with id '{}'", &response.invocation_id);
    }
}

fn set_gauge_avg(
    gauge: &Gauge,
    inv_count: &HashMap<String, IntCounter>,
    moving_average_window_size: i64,
    inv_time: u128,
    metric_key: &str,
) {
    let inv_count = inv_count.get(metric_key).map(|c| c.get()).unwrap_or(1);
    gauge.set(calc_avg(
        moving_average_window_size,
        inv_count,
        inv_time,
        gauge.get(),
    ));
}

fn register_gauge(
    registry: &Arc<RwLock<Registry>>,
    avg_inv_time: &mut HashMap<String, Gauge>,
    gauge_lookup_key: &str,
    name: &str,
    help: &str,
    initial_value: u128,
) {
    let initial_value = initial_value as f64;

    match Gauge::new(name.to_string(), help.to_string()) {
        Ok(gauge) => {
            gauge.set(initial_value);
            avg_inv_time.insert(gauge_lookup_key.to_string(), gauge.clone());

            if let Err(e) = registry.write().unwrap().register(Box::new(gauge)) {
                error!("Error registering gauge '{}': {}", &name, e);
            }
        }
        Err(e) => error!("Error creating gauge '{}': {}", &name, e),
    }
}

// set new average invocation time across all actors or capabilities
fn set_new_total_avg(metrics: &RwLockWriteGuard<Metrics>, target: &WasccEntity, inv_time: u128) {
    match target {
        WasccEntity::Actor(_) => {
            metrics.actor_total_average_inv_time.set(calc_avg(
                metrics.moving_average_window_size,
                metrics.actor_total_inv_count.get(),
                inv_time,
                metrics.actor_total_average_inv_time.get(),
            ));
        }
        WasccEntity::Capability {
            capid: _,
            binding: _,
        } => {
            metrics.cap_total_average_inv_time.set(calc_avg(
                metrics.moving_average_window_size,
                metrics.cap_total_inv_count.get(),
                inv_time,
                metrics.cap_total_average_inv_time.get(),
            ));
        }
    };
}

fn calc_avg(
    moving_average_window_size: i64,
    inv_count: i64,
    inv_time: u128,
    current_avg_inv_time: f64,
) -> f64 {
    let inv_time = inv_time as f64;
    let inv_count = min(inv_count, moving_average_window_size);
    // invocations are counted in pre_invoke so the `inv_count` _includes_ the current invocation
    let total_current_inv_time = current_avg_inv_time * (inv_count - 1) as f64;
    (total_current_inv_time + inv_time as f64) / inv_count as f64
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
    let job = push_config.job.unwrap_or_else(|| WASCC.to_owned());
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
            error!("Error pushing metrics to '{}': {}", &pushgateway_addr, e);
        }

        std::thread::sleep(push_config.push_interval);
    }
}

#[cfg(test)]
mod tests {
    use super::WASCC;
    use crate::middleware::prometheus::{
        PrometheusConfig, PrometheusMiddleware, PushgatewayConfig,
    };
    use crate::{Invocation, InvocationResponse, Middleware, WasccEntity};
    use mockito::{mock, Matcher};
    use rand::random;
    use std::net::SocketAddr;
    use std::ops::Mul;
    use std::time::Duration;
    use wascap::prelude::KeyPair;

    const CAPID1: &str = "capid1";
    const CAPID2: &str = "capid2";
    const BINDING1: &str = "binding1";
    const BINDING2: &str = "binding2";
    const CAP_OPERATION1: &str = "operation1";
    const CAP_OPERATION2: &str = "operation2";

    const ACTOR1: &str = "actor1";
    const ACTOR2: &str = "actor2";
    const ACTOR_OPERATION1: &str = "operation1";
    const ACTOR_OPERATION2: &str = "operation2";

    fn actor_invocation(actor: &str, operation: &str) -> Invocation {
        Invocation::new(
            &KeyPair::new_module(),
            WasccEntity::Actor(actor.to_string()),
            WasccEntity::Actor(actor.to_string()),
            operation,
            "cap_msg".into(),
        )
    }

    fn cap_invocation(capid: &str, binding: &str, operation: &str) -> Invocation {
        Invocation::new(
            &KeyPair::new_module(),
            WasccEntity::Capability {
                capid: capid.to_owned(),
                binding: binding.to_owned(),
            },
            WasccEntity::Capability {
                capid: capid.to_owned(),
                binding: binding.to_owned(),
            },
            operation,
            "cap_msg".into(),
        )
    }

    fn invocation_response(id: &String) -> InvocationResponse {
        InvocationResponse {
            msg: "response".as_bytes().to_vec(),
            error: None,
            invocation_id: id.clone(),
        }
    }

    fn invoke(
        middleware: &PrometheusMiddleware,
        inv: &Invocation,
        inv_response: &InvocationResponse,
    ) {
        middleware.actor_pre_invoke(inv.clone()).unwrap();
        // simulate that the invocation takes some time
        std::thread::sleep(Duration::from_millis(1));
        middleware.actor_post_invoke(inv_response.clone()).unwrap();
    }

    #[test]
    fn test_serve_metrics() -> Result<(), reqwest::Error> {
        let actor_invocation1 = actor_invocation(ACTOR1, ACTOR_OPERATION1);
        let actor_invocation2 = actor_invocation(ACTOR2, ACTOR_OPERATION2);
        let actor_invocation1_response = invocation_response(&actor_invocation1.id);
        let actor_invocation2_response = invocation_response(&actor_invocation2.id);

        let cap_invocation1 = cap_invocation(CAPID1, BINDING1, CAP_OPERATION1);
        let cap_invocation2 = cap_invocation(CAPID2, BINDING2, CAP_OPERATION2);
        let cap_invocation1_response = invocation_response(&cap_invocation1.id);
        let cap_invocation2_response = invocation_response(&cap_invocation2.id);

        let server_addr: SocketAddr = ([127, 0, 0, 1], 9898).into();
        let config = PrometheusConfig {
            metrics_server_addr: Some(server_addr),
            pushgateway_config: None,
            moving_average_window_size: None,
        };
        let middleware = PrometheusMiddleware::new(config).unwrap();

        let invocations_op1 = 5;
        let invocations_op2 = 7;

        for _ in 0..invocations_op1 {
            invoke(&middleware, &actor_invocation1, &actor_invocation1_response);
            invoke(&middleware, &cap_invocation1, &cap_invocation1_response);
        }

        for _ in 0..invocations_op2 {
            invoke(&middleware, &actor_invocation2, &actor_invocation2_response);
            invoke(&middleware, &cap_invocation2, &cap_invocation2_response);
        }

        let url = format!("http://{}/metrics", &server_addr.to_string());
        let body = reqwest::blocking::get(&url)?.text()?;

        let total_invocations = invocations_op1 + invocations_op2;

        // capabilities: counts
        assert!(body
            .find(&format!(
                "{}_cap_total_inv_count {}",
                WASCC, total_invocations
            ))
            .is_some());
        assert!(body
            .find(&format!(
                "{}_{}_{}_inv_count {}",
                WASCC, CAPID1, BINDING1, invocations_op1
            ))
            .is_some());
        assert!(body
            .find(&format!(
                "{}_{}_{}_inv_count {}",
                WASCC, CAPID2, BINDING2, invocations_op2
            ))
            .is_some());
        // capabilities: averages
        assert!(body
            .find(&format!("{}_cap_total_average_inv_time", WASCC))
            .is_some());
        assert!(body
            .find(&format!(
                "{}_{}_{}_average_inv_time",
                WASCC, CAPID1, BINDING1
            ))
            .is_some());
        assert!(body
            .find(&format!(
                "{}_{}_{}_average_inv_time",
                WASCC, CAPID2, BINDING2
            ))
            .is_some());
        // capabilities: averages for operations
        assert!(body
            .find(&format!(
                "{}_{}_{}_{}_average_inv_time",
                WASCC, CAPID1, BINDING1, CAP_OPERATION1
            ))
            .is_some());
        assert!(body
            .find(&format!(
                "{}_{}_{}_{}_average_inv_time",
                WASCC, CAPID2, BINDING2, CAP_OPERATION2
            ))
            .is_some());

        // actors
        assert!(body
            .find(&format!(
                "{}_actor_total_inv_count {}",
                WASCC, total_invocations
            ))
            .is_some());
        assert!(body
            .find(&format!(
                "{}_{}_inv_count {}",
                WASCC, ACTOR1, invocations_op1
            ))
            .is_some());
        assert!(body
            .find(&format!(
                "{}_{}_inv_count {}",
                WASCC, ACTOR2, invocations_op2
            ))
            .is_some());
        // actors: averages
        assert!(body
            .find(&format!("actor_total_average_inv_time"))
            .is_some());
        assert!(body
            .find(&format!("{}_{}_average_inv_time", WASCC, ACTOR1,))
            .is_some());
        assert!(body
            .find(&format!("{}_{}_average_inv_time", WASCC, ACTOR2))
            .is_some());
        // actors: averages for operations
        assert!(body
            .find(&format!(
                "{}_{}_{}_average_inv_time",
                WASCC, ACTOR1, ACTOR_OPERATION1
            ))
            .is_some());
        assert!(body
            .find(&format!(
                "{}_{}_{}_average_inv_time",
                WASCC, ACTOR2, ACTOR_OPERATION2
            ))
            .is_some());

        // check that invocation state is cleaned up
        assert!(middleware
            .metrics
            .read()
            .unwrap()
            .active_inv_state
            .is_empty());

        Ok(())
    }

    #[test]
    fn test_push_metrics() {
        // The data format that is used is not compatible with any current Mockito
        // matcher. It doesn't currently seem to be possible to get the raw body in
        // a matcher, see https://github.com/lipanski/mockito/issues/95
        let pushed_metrics = mock("PUT", "/metrics/job/wascc")
            .match_header("content-type", "application/vnd.google.protobuf; proto=io.prometheus.client.MetricFamily; encoding=delimited")
            .match_header("content-length", Matcher::Any)
            .match_header("accept", "*/*")
            .match_header("host", Matcher::Any)
            // flexible number of invocations to avoid flakiness due to timing between 
            // push interval and waiting for the push to happen before asserting
            .expect_at_least(1)
            // start serving the mock
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
            moving_average_window_size: None,
        };

        let middleware = PrometheusMiddleware::new(config).unwrap();
        let actor_invocation = actor_invocation(ACTOR1, ACTOR_OPERATION1);
        let actor_invocation_response = invocation_response(&actor_invocation.id);
        let cap_invocation = cap_invocation(CAPID1, BINDING1, CAP_OPERATION1);
        let cap_invocation_response = invocation_response(&cap_invocation.id);
        let invocations = random::<u8>();

        for _ in 0..invocations {
            middleware
                .actor_pre_invoke(actor_invocation.clone())
                .unwrap();
            middleware.actor_pre_invoke(cap_invocation.clone()).unwrap();

            // simulate that the invocation takes some time
            std::thread::sleep(Duration::from_millis(1));

            middleware
                .actor_post_invoke(cap_invocation_response.clone())
                .unwrap();
            middleware
                .actor_post_invoke(actor_invocation_response.clone())
                .unwrap();
        }

        // make sure at least one push has time to complete before asserting
        // this test fails intermittently with a interval * 2
        std::thread::sleep(push_interval.mul(3));

        pushed_metrics.assert();
        // check that invocation state is cleaned up
        assert!(middleware
            .metrics
            .read()
            .unwrap()
            .active_inv_state
            .is_empty());
    }
}
