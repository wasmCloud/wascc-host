#[macro_use]
extern crate wascc_codec as codec;

use codec::blobstore::{Container, OP_REMOVE_CONTAINER};
use codec::capabilities::{
    CapabilityDescriptor, CapabilityProvider, Dispatcher, OP_GET_CAPABILITY_DESCRIPTOR,
};
use codec::core::{CapabilityConfiguration, OP_BIND_ACTOR, OP_REMOVE_ACTOR};
use codec::{deserialize, serialize};

use std::error::Error;
use std::fmt::Formatter;
use std::sync::RwLock;
#[cfg(not(feature = "static_plugin"))]
capability_provider!(DummyFsProvider, DummyFsProvider::new);

const CAPABILITY_ID: &str = "wascc:blobstore";
const SYSTEM_ACTOR: &str = "system";
const VERSION: &str = env!("CARGO_PKG_VERSION");
const REVISION: u32 = 0; // Increment for each crates publish

pub struct DummyFsProvider {
    dispatcher: RwLock<Box<dyn codec::capabilities::Dispatcher>>,
}

impl Default for DummyFsProvider {
    fn default() -> Self {
        DummyFsProvider {
            dispatcher: RwLock::new(Box::new(codec::capabilities::NullDispatcher::new())),
        }
    }
}

impl DummyFsProvider {
    pub fn new() -> Self {
        Self::default()
    }

    fn configure(
        &self,
        _config: CapabilityConfiguration,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        Ok(vec![])
    }

    fn remove_actor(
        &self,
        _config: CapabilityConfiguration,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        // Do nothing here
        Ok(vec![])
    }

    fn get_descriptor(&self) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        Ok(serialize(
            CapabilityDescriptor::builder()
                .id(CAPABILITY_ID)
                .name("Dummy FS Provider")
                .long_description("Fs provider that only throws an error")
                .version(VERSION)
                .revision(REVISION)
                .build(),
        )?)
    }

    fn remove_container(
        &self,
        _actor: &str,
        container: Container,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        Err(container.id.into())
    }
}

impl CapabilityProvider for DummyFsProvider {
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn Dispatcher>,
    ) -> Result<(), Box<dyn Error + Sync + Send>> {
        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        match op {
            OP_BIND_ACTOR if actor == SYSTEM_ACTOR => self.configure(deserialize(msg)?),
            OP_REMOVE_ACTOR if actor == SYSTEM_ACTOR => self.remove_actor(deserialize(msg)?),
            OP_GET_CAPABILITY_DESCRIPTOR if actor == SYSTEM_ACTOR => self.get_descriptor(),
            OP_REMOVE_CONTAINER => self.remove_container(actor, deserialize(msg)?),
            _ => Err("bad dispatch".into()),
        }
    }
}
