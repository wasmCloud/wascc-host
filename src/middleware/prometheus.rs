//! # Prometheus Middleware
//!
//! [Prometheus][prometheus] is an open-source systems monitoring and alerting toolkit. This
//! middleware makes it possible to serve metrics for scraping by [Prometheus][prometheus] or
//! by pushing metrics to the [Prometheus Pushgateway][prometheus_pushgateway]
//! that is then scraped by [Prometheus][prometheus].
//!
//! Enable this middleware using the feature `prometheus_middleware`.
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
//!     # Scrape waSCC. 'host.docker.internal' points to the host
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

use crate::{errors, Invocation, InvocationResponse, InvocationTarget, Middleware, Result};
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
    /// Average invocation time across all capabilities
    cap_total_average_inv_time: Gauge,
    /// Average invocation time per capability
    cap_average_inv_time: HashMap<String, Gauge>,

    /// Total number of invocations of actors
    actor_total_inv_count: IntCounter,
    /// Number of invocations per actor
    actor_inv_count: HashMap<String, IntCounter>,
    /// Average invocation time across all actors
    actor_total_average_inv_time: Gauge,
    /// Average invocation time per actor
    actor_average_inv_time: HashMap<String, Gauge>,

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
    /// Configure the number of invocations to include when calculating average invocation times.
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
    counter_key: String,
    target: InvocationTarget,
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
                "cap_total_inv_count",
                "Total number of capability invocations",
            )?,
            cap_inv_count: HashMap::new(),
            cap_total_average_inv_time: Gauge::new(
                "cap_total_average_inv_time",
                "Average invocation time across all capabilities",
            )?,
            cap_average_inv_time: HashMap::new(),
            actor_total_inv_count: IntCounter::new(
                "actor_total_inv_count",
                "Total number of actor invocations",
            )?,
            actor_inv_count: HashMap::new(),
            actor_total_average_inv_time: Gauge::new(
                "actor_total_average_inv_time",
                "Average invocation time across all actors",
            )?,
            actor_average_inv_time: HashMap::new(),
            active_inv_state: HashMap::new(),
            moving_average_window_size: config
                .moving_average_window_size
                .unwrap_or_else(|| DEFAULT_MOVING_AVERAGE_WINDOW_SIZE),
        })
    }
}

impl Middleware for PrometheusMiddleware {
    fn actor_pre_invoke(&self, inv: Invocation) -> Result<Invocation> {
        pre_invoke_count_inv(&self.metrics, &self.registry, &inv.origin, &inv.target);
        pre_invoke_measure_inv_time(&self.metrics, &inv);
        Ok(inv)
    }

    fn actor_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse> {
        post_invoke_measure_inv_time(&self.metrics, &self.registry, &response);
        Ok(response)
    }

    fn capability_pre_invoke(&self, inv: Invocation) -> Result<Invocation> {
        pre_invoke_count_inv(&self.metrics, &self.registry, &inv.origin, &inv.target);
        pre_invoke_measure_inv_time(&self.metrics, &inv);
        Ok(inv)
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
    origin: &str,
    target: &InvocationTarget,
) {
    let mut metrics = metrics.write().unwrap();
    let key = get_counter_key(origin, target);

    match target {
        InvocationTarget::Actor(actor) => {
            metrics.actor_total_inv_count.inc();

            if let Some(value) = metrics.actor_inv_count.get(&key) {
                value.inc();
            } else {
                let name = format!("{}_inv_count", &actor);
                let help = format!("Number of invocations of actor '{}'", &actor);

                if let Err(e) =
                    register_counter(&mut metrics.actor_inv_count, &registry, key, &name, &help)
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
                    "Number of invocations of capability '{}' with binding '{}'",
                    &capid, &binding
                );

                if let Err(e) =
                    register_counter(&mut metrics.cap_inv_count, &registry, key, &name, &help)
                {
                    error!("Error registering counter '{}': {}", &name, e);
                }
            }
        }
    }
}

fn get_counter_key(origin: &str, target: &InvocationTarget) -> String {
    let target = match target {
        InvocationTarget::Actor(actor) => actor.clone(),
        InvocationTarget::Capability { capid, binding } => capid.clone() + binding,
    };

    // count invocations per sender<->receiver pair
    origin.to_string() + &target
}

fn register_counter(
    counters: &mut HashMap<String, IntCounter>,
    registry: &Arc<RwLock<Registry>>,
    counter_lookup_key: String,
    name: &str,
    help: &str,
) -> prometheus::Result<()> {
    let counter = IntCounter::new(name.to_string(), help.to_string())?;
    counter.inc();

    let reg = registry.write().unwrap();
    reg.register(Box::new(counter.clone()))?;

    counters.insert(counter_lookup_key, counter);
    Ok(())
}

fn pre_invoke_measure_inv_time(metrics: &Arc<RwLock<Metrics>>, inv: &Invocation) {
    let mut metrics = metrics.write().unwrap();
    let inv_counter_key = get_counter_key(&inv.origin, &inv.target);
    let state = InvocationState {
        start_time: Instant::now(),
        counter_key: inv_counter_key.clone(),
        target: inv.target.clone(),
    };

    if metrics
        .active_inv_state
        .insert(inv.id.clone(), state)
        .is_some()
    {
        error!(
            "Invocation already in progress with id '{}'",
            &inv_counter_key
        );
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
            InvocationTarget::Actor(actor) => {
                let avg_time_key = actor.clone();

                if let Some(gauge) = metrics.actor_average_inv_time.get(&avg_time_key) {
                    let inv_count = metrics
                        .actor_inv_count
                        .get(&state.counter_key)
                        .map(|c| c.get())
                        .unwrap_or(1);
                    gauge.set(calc_avg(
                        metrics.moving_average_window_size,
                        inv_count,
                        inv_time,
                        gauge.get(),
                    ));
                } else {
                    let name = format!("{}_average_inv_time", actor.clone());
                    let help = format!("Average time to invoke actor '{}'", actor.clone());

                    register_gauge(
                        registry,
                        &mut metrics.actor_average_inv_time,
                        &avg_time_key,
                        &name,
                        &help,
                        inv_time,
                    );
                }
            }
            InvocationTarget::Capability { capid, binding } => {
                let avg_time_key = capid.clone() + binding;

                if let Some(gauge) = metrics.cap_average_inv_time.get(&avg_time_key) {
                    let inv_count = metrics
                        .cap_inv_count
                        .get(&state.counter_key)
                        .map(|c| c.get())
                        .unwrap_or(1);
                    gauge.set(calc_avg(
                        metrics.moving_average_window_size,
                        inv_count,
                        inv_time,
                        gauge.get(),
                    ));
                } else {
                    let name = format!("{}_{}_average_inv_time", capid, binding);
                    let help = format!(
                        "Average time to invoke capability '{}' with binding '{}'",
                        capid, binding
                    );

                    register_gauge(
                        registry,
                        &mut metrics.cap_average_inv_time,
                        &avg_time_key,
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
fn set_new_total_avg(
    metrics: &RwLockWriteGuard<Metrics>,
    target: &InvocationTarget,
    inv_time: u128,
) {
    match target {
        InvocationTarget::Actor(_) => {
            metrics.actor_total_average_inv_time.set(calc_avg(
                metrics.moving_average_window_size,
                metrics.actor_total_inv_count.get(),
                inv_time,
                metrics.actor_total_average_inv_time.get(),
            ));
        }
        InvocationTarget::Capability {
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
            error!("Error pushing metrics to '{}': {}", &pushgateway_addr, e);
        }

        std::thread::sleep(push_config.push_interval);
    }
}

#[cfg(test)]
mod tests {
    use crate::middleware::prometheus::{
        PrometheusConfig, PrometheusMiddleware, PushgatewayConfig,
    };
    use crate::{Invocation, InvocationResponse, InvocationTarget, Middleware};
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

    fn invocation_response(id: &String) -> InvocationResponse {
        InvocationResponse {
            msg: "response".as_bytes().to_vec(),
            error: None,
            invocation_id: id.clone(),
        }
    }

    #[test]
    fn test_serve_metrics() -> Result<(), reqwest::Error> {
        let server_addr: SocketAddr = ([127, 0, 0, 1], 9898).into();
        let config = PrometheusConfig {
            metrics_server_addr: Some(server_addr),
            pushgateway_config: None,
            moving_average_window_size: None,
        };

        let middleware = PrometheusMiddleware::new(config).unwrap();
        let actor_invocation = actor_invocation();
        let actor_invocation_response = invocation_response(&actor_invocation.id);
        let cap_invocation = cap_invocation();
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

        let url = format!("http://{}/metrics", &server_addr.to_string());
        let body = reqwest::blocking::get(&url)?.text()?;

        // get average times directly from the middleware and assert they are served
        let actor_total_average_time = &middleware
            .metrics
            .read()
            .unwrap()
            .actor_total_average_inv_time
            .get();
        let cap_total_average_time = &middleware
            .metrics
            .read()
            .unwrap()
            .cap_total_average_inv_time
            .get();

        // capabilities
        assert!(body
            .find(&format!("cap_total_inv_count {}", invocations))
            .is_some());
        assert!(body
            .find(&format!("{}_{}_inv_count {}", CAPID, BINDING, invocations))
            .is_some());
        assert!(body
            .find(&format!(
                "cap_total_average_inv_time {}",
                cap_total_average_time
            ))
            .is_some());
        assert!(body
            .find(&format!(
                "{}_{}_average_inv_time {}",
                CAPID, BINDING, cap_total_average_time
            ))
            .is_some());

        // actors
        assert!(body
            .find(&format!("actor_total_inv_count {}", invocations))
            .is_some());
        assert!(body
            .find(&format!("{}_inv_count {}", TARGET_ACTOR, invocations))
            .is_some());
        assert!(body
            .find(&format!(
                "actor_total_average_inv_time {}",
                actor_total_average_time
            ))
            .is_some());
        assert!(body
            .find(&format!(
                "{}_average_inv_time {}",
                TARGET_ACTOR, actor_total_average_time
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
        let pushed_metrics = mock("PUT", "/metrics/job/wascc_push")
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
        let actor_invocation = actor_invocation();
        let actor_invocation_response = invocation_response(&actor_invocation.id);
        let cap_invocation = cap_invocation();
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
        std::thread::sleep(push_interval.mul(2));

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
