use prost::Message;
use std::error::Error;
use std::sync::{Arc, RwLock};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};
use uuid::Uuid;
use wascc_codec::capabilities::{CapabilityProvider, Dispatcher, NullDispatcher};
use wascc_codec::extras::*;

pub(crate) struct ExtrasCapabilityProvider {
    dispatcher: Arc<RwLock<Box<dyn Dispatcher>>>,
    sequences: Arc<RwLock<HashMap<String, AtomicU64>>>,
}

impl Default for ExtrasCapabilityProvider {
    fn default() -> Self {
        ExtrasCapabilityProvider {
            dispatcher: Arc::new(RwLock::new(Box::new(NullDispatcher::new()))),
            sequences: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

const CAPABILITY_ID: &str = "wascc:extras";

impl ExtrasCapabilityProvider {
    fn generate_guid(
        &self,
        _actor: &str,
        _msg: impl Into<GeneratorRequest>,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        let uuid = Uuid::new_v4();
        let result = GeneratorResult {
            value: Some(generator_result::Value::Guid(format!("{}", uuid))),
        };
        let mut buf = Vec::new();
        result.encode(&mut buf)?;
        Ok(buf)
    }

    fn generate_random(
        &self,
        _actor: &str,
        msg: impl Into<GeneratorRequest>,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        use rand::prelude::*;
        let req = msg.into();
        let mut rng = rand::thread_rng();
        let n: u32 = rng.gen_range(req.min, req.max);
        let result = GeneratorResult {
            value: Some(generator_result::Value::RandomNo(n)),
        };
        let mut buf = Vec::new();
        result.encode(&mut buf)?;
        Ok(buf)
    }

    fn generate_sequence(
        &self,
        actor: &str,
        _msg: impl Into<GeneratorRequest>,
    ) -> Result<Vec<u8>, Box<dyn Error>> {
        let mut lock = self.sequences.write().unwrap();
        let seq = lock
            .entry(actor.to_string())
            .or_insert(AtomicU64::new(0))
            .fetch_add(1, Ordering::SeqCst);
        let result = GeneratorResult {
            value: Some(generator_result::Value::SequenceNo(seq)),
        };
        let mut buf = Vec::new();
        result.encode(&mut buf)?;
        Ok(buf)
    }
}

impl CapabilityProvider for ExtrasCapabilityProvider {
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn wascc_codec::capabilities::Dispatcher>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        trace!("Dispatcher received.");
        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    fn capability_id(&self) -> &'static str {
        CAPABILITY_ID
    }

    fn name(&self) -> &'static str {
        "waSCC Extras"
    }

    fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        trace!("Received host call from {}, operation - {}", actor, op);

        match op {
            OP_REQUEST_GUID => self.generate_guid(actor, msg.to_vec().as_ref()),
            OP_REQUEST_RANDOM => self.generate_random(actor, msg.to_vec().as_ref()),
            OP_REQUEST_SEQUENCE => self.generate_sequence(actor, msg.to_vec().as_ref()),
            _ => Err("bad dispatch".into()),
        }
    }
}
