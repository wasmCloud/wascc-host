// Copyright 2015-2019 Capital One Services, LLC
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

use crate::authz;
use crate::Result;
use crossbeam_channel::unbounded;
use std::fs::File;
use std::io::prelude::*;
use std::{
    path::Path,
    sync::{Arc, RwLock},
};
use wascap::jwt::Token;

/// An actor is a WebAssembly module that can consume capabilities exposed by capability providers
#[derive(Debug)]
pub struct Actor {
    pub(crate) token: Token<wascap::jwt::Actor>,
    pub(crate) bytes: Vec<u8>,
}

impl Actor {
    /// Create an actor from the bytes (must be a signed module) of a WebAssembly module
    pub fn from_bytes(buf: Vec<u8>) -> Result<Actor> {
        let token = authz::extract_claims(&buf)?;
        Ok(Actor { token, bytes: buf })
    }

    /// Create an actor from a WebAssembly (`.wasm`) file
    pub fn from_file(path: impl AsRef<Path>) -> Result<Actor> {
        let mut file = File::open(path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;

        Actor::from_bytes(buf)
    }

    /// Create an actor from the Gantry registry
    pub fn from_gantry(actor: &str) -> Result<Actor> {
        let (s, r) = unbounded();
        let bytevec = Arc::new(RwLock::new(Vec::new()));
        let b = bytevec.clone();
        let _ack = crate::host::GANTRYCLIENT
            .read()
            .unwrap()
            .download_actor(actor, move |chunk| {
                bytevec
                    .write()
                    .unwrap()
                    .extend_from_slice(&chunk.chunk_bytes);
                if chunk.sequence_no == chunk.total_chunks {
                    s.send(true).unwrap();
                }
                Ok(())
            });
        let _ = r.recv().unwrap();
        let vec = b.read().unwrap();
        Actor::from_bytes(vec.clone())
    }

    /// Obtain the actor's public key. This is globally unique identifier
    pub fn public_key(&self) -> String {
        self.token.claims.subject.to_string()
    }

    /// The actor's human-friendly display name
    pub fn name(&self) -> String {
        match self.token.claims.metadata.as_ref().unwrap().name {
            Some(ref n) => n.to_string(),
            None => "Unnamed".to_string(),
        }
    }

    /// Obtain the public key of the issuer of the actor's signed token
    pub fn issuer(&self) -> String {
        self.token.claims.issuer.to_string()
    }

    /// Obtain the list of capabilities declared in this actor's embedded token
    pub fn capabilities(&self) -> Vec<String> {
        match self.token.claims.metadata.as_ref().unwrap().caps {
            Some(ref caps) => caps.clone(),
            None => vec![],
        }
    }

    /// Obtain the list of tags in the actor's token
    pub fn tags(&self) -> Vec<String> {
        match self.token.claims.metadata.as_ref().unwrap().tags {
            Some(ref tags) => tags.clone(),
            None => vec![],
        }
    }
}
