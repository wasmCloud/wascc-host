use crate::authz;
use crate::Result;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;
use wascap::jwt::Token;

/// An actor is a WebAssembly module that can consume capabilities exposed by capability providers
#[derive(Debug)]
pub struct Actor {
    pub(crate) token: Token,
    pub(crate) bytes: Vec<u8>,
}

impl Actor {
    /// Create an actor from the bytes (must be a signed module) of a WebAssembly module
    pub fn from_bytes(buf: Vec<u8>) -> Result<Actor> {
        let token = authz::extract_and_store_claims(&buf)?;
        Ok(Actor { token, bytes: buf })
    }

    /// Create an actor from a WebAssembly (`.wasm`) file
    pub fn from_file(path: impl AsRef<Path>) -> Result<Actor> {
        let mut file = File::open(path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;

        Actor::from_bytes(buf)
    }

    /// Obtain the actor's public key. This is globally unique identifier
    pub fn public_key(&self) -> String {
        self.token.claims.subject.to_string()
    }

    /// Obtain the public key of the issuer of the actor's signed token
    pub fn issuer(&self) -> String {
        self.token.claims.issuer.to_string()
    }

    /// Obtain the list of capabilities declared in this actor's embedded token
    pub fn capabilities(&self) -> Vec<String> {
        match self.token.claims.caps {
            Some(ref caps) => caps.clone(),
            None => vec![],
        }
    }
}
