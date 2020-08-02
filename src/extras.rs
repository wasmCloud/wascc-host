// Copyright 2015-2020 Capital One Services, LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// A default implementation of the "wascc:extras" provider that is always included
// with the host runtime. This provides functionality for generating random numbers,
// generating a guid, and generating a sequence number... things that a standalone
// WASM module cannot do.

use crate::{REVISION, VERSION};
use std::error::Error;
use std::sync::{Arc, RwLock};
use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
};
use uuid::Uuid;
use wascc_codec::capabilities::{
    CapabilityDescriptor, CapabilityProvider, Dispatcher, NullDispatcher, OperationDirection,
    OP_GET_CAPABILITY_DESCRIPTOR,
};
use wascc_codec::core::OP_BIND_ACTOR;
use wascc_codec::extras::*;
use wascc_codec::{deserialize, serialize, SYSTEM_ACTOR};

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

pub(crate) const CAPABILITY_ID: &str = "wascc:extras";

impl ExtrasCapabilityProvider {
    fn generate_guid(
        &self,
        _actor: &str,
        _msg: GeneratorRequest,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let uuid = Uuid::new_v4();
        let result = GeneratorResult {
            guid: Some(format!("{}", uuid)),
            random_number: 0,
            sequence_number: 0,
        };

        Ok(serialize(&result)?)
    }

    fn generate_random(
        &self,
        _actor: &str,
        msg: GeneratorRequest,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        use rand::prelude::*;
        let mut rng = rand::thread_rng();
        let result = if let GeneratorRequest {
            random: true,
            min,
            max,
            ..
        } = msg
        {
            let n: u32 = rng.gen_range(min, max);
            GeneratorResult {
                random_number: n,
                sequence_number: 0,
                guid: None,
            }
        } else {
            GeneratorResult::default()
        };

        Ok(serialize(result)?)
    }

    fn generate_sequence(
        &self,
        actor: &str,
        _msg: GeneratorRequest,
    ) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        let mut lock = self.sequences.write().unwrap();
        let seq = lock
            .entry(actor.to_string())
            .or_insert(AtomicU64::new(0))
            .fetch_add(1, Ordering::SeqCst);
        let result = GeneratorResult {
            sequence_number: seq,
            random_number: 0,
            guid: None,
        };
        Ok(serialize(&result)?)
    }

    fn get_descriptor(&self) -> Result<Vec<u8>, Box<dyn Error + Sync + Send>> {
        Ok(serialize(
            CapabilityDescriptor::builder()
                .id(CAPABILITY_ID)
                .name("waSCC Extras (Internal)")
                .long_description(
                    "A capability provider exposing miscellaneous utility functions to actors",
                )
                .version(VERSION)
                .revision(REVISION)
                .with_operation(
                    OP_REQUEST_GUID,
                    OperationDirection::ToProvider,
                    "Requests the generation of a new GUID",
                )
                .with_operation(
                    OP_REQUEST_RANDOM,
                    OperationDirection::ToProvider,
                    "Requests the generation of a randum number",
                )
                .with_operation(
                    OP_REQUEST_SEQUENCE,
                    OperationDirection::ToProvider,
                    "Requests the next number in a process-wide global sequence number",
                )
                .build(),
        )?)
    }
}

impl CapabilityProvider for ExtrasCapabilityProvider {
    fn configure_dispatch(
        &self,
        dispatcher: Box<dyn wascc_codec::capabilities::Dispatcher>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        trace!("Dispatcher received.");
        let mut lock = self.dispatcher.write().unwrap();
        *lock = dispatcher;

        Ok(())
    }

    fn handle_call(
        &self,
        actor: &str,
        op: &str,
        msg: &[u8],
    ) -> Result<Vec<u8>, Box<dyn std::error::Error + Sync + Send>> {
        trace!("Received host call from {}, operation - {}", actor, op);

        match op {
            OP_GET_CAPABILITY_DESCRIPTOR if actor == SYSTEM_ACTOR => self.get_descriptor(),
            OP_REQUEST_GUID => self.generate_guid(actor, deserialize(msg)?),
            OP_REQUEST_RANDOM => self.generate_random(actor, deserialize(msg)?),
            OP_REQUEST_SEQUENCE => self.generate_sequence(actor, deserialize(msg)?),
            OP_BIND_ACTOR => Ok(vec![]),
            _ => Err("bad dispatch".into()),
        }
    }
}
