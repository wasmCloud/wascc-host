use crate::{Invocation, InvocationResponse, Result};

use crossbeam::{Receiver, Sender};
use nats;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use wascc_codec::{deserialize, serialize};

pub(crate) struct DistributedBus {
    nc: nats::Connection,
    subs: Arc<RwLock<HashMap<String, nats::subscription::Handler>>>,
}

impl DistributedBus {
    pub fn new() -> Self {
        let nc = nats::connect("127.0.0.1").unwrap();
        info!("Initialized Message Bus (lattice)");
        DistributedBus {
            nc,
            subs: Arc::new(RwLock::new(HashMap::new())),
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
        let resp = self.nc.request(&subject, &serialize(inv)?)?;
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
