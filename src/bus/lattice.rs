use crate::{Invocation, InvocationResponse, Result};
use crossbeam::{Receiver, Sender};
use nats;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use wascc_codec::{deserialize, serialize};

const LATTICE_HOST_KEY: &str = "LATTICE_HOST"; // env var name
const DEFAULT_LATTICE_HOST: &str = "127.0.0.1"; // default mode is anonymous via loopback
const LATTICE_RPC_TIMEOUT_KEY: &str = "LATTICE_RPC_TIMEOUT_MILLIS";
const DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS: u64 = 500;
const LATTICE_CREDSFILE_KEY: &str = "LATTICE_CREDS_FILE";

pub(crate) struct DistributedBus {
    nc: nats::Connection,
    subs: Arc<RwLock<HashMap<String, nats::subscription::Handler>>>,
    req_timeout: Duration,
}

impl DistributedBus {
    pub fn new() -> Self {
        let nc = get_connection();

        info!("Initialized Message Bus (lattice)");
        DistributedBus {
            nc,
            subs: Arc::new(RwLock::new(HashMap::new())),
            req_timeout: get_timeout(),
        }
    }

    pub fn subscribe(
        &self,
        subject: &str,
        sender: Sender<Invocation>,
        receiver: Receiver<InvocationResponse>,
    ) -> Result<()> {
        let sub = self
            .nc
            .queue_subscribe(subject, subject)?
            .with_handler(move |msg| {
                handle_invocation(&msg, sender.clone(), receiver.clone());
                Ok(())
            });
        self.subs.write().unwrap().insert(subject.to_string(), sub);
        Ok(())
    }

    pub fn invoke(&self, subject: &str, inv: Invocation) -> Result<InvocationResponse> {
        let resp = self
            .nc
            .request_timeout(&subject, &serialize(inv)?, self.req_timeout)?;
        let ir: InvocationResponse = deserialize(&resp.data)?;
        Ok(ir)
    }

    pub fn unsubscribe(&self, subject: &str) -> Result<()> {
        if let Some(sub) = self.subs.write().unwrap().remove(subject) {
            sub.unsubscribe()?;
        }
        Ok(())
    }
}

fn handle_invocation(
    msg: &nats::Message,
    sender: Sender<Invocation>,
    receiver: Receiver<InvocationResponse>,
) {
    let inv = invocation_from_msg(msg);
    sender.send(inv).unwrap();
    let inv_r = receiver.recv().unwrap();
    msg.respond(serialize(inv_r).unwrap()).unwrap();
}

fn invocation_from_msg(msg: &nats::Message) -> Invocation {
    let i: Invocation = deserialize(&msg.data).unwrap();
    i
}

fn get_credsfile() -> Option<String> {
    std::env::var(LATTICE_CREDSFILE_KEY).ok()
}

fn get_env(var: &str, default: &str) -> String {
    match std::env::var(var) {
        Ok(val) => {
            if val.is_empty() {
                default.to_string()
            } else {
                val.to_string()
            }
        }
        Err(_) => default.to_string(),
    }
}

fn get_connection() -> nats::Connection {
    let host = get_env(LATTICE_HOST_KEY, DEFAULT_LATTICE_HOST);
    let opts = nats::ConnectionOptions::new().with_name("waSCC Lattice");
    if let Some(creds) = get_credsfile() {
        opts.with_credentials(creds).connect(&host)
    } else {
        opts.connect(&host)
    }
    .unwrap()
}

fn get_timeout() -> Duration {
    match std::env::var(LATTICE_RPC_TIMEOUT_KEY) {
        Ok(val) => {
            if val.is_empty() {
                Duration::from_millis(DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS)
            } else {
                Duration::from_millis(val.parse().unwrap_or(DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS))
            }
        }
        Err(_) => Duration::from_millis(DEFAULT_LATTICE_RPC_TIMEOUT_MILLIS),
    }
}
