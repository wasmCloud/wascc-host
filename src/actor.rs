use crate::authz;
use crate::Result;
use std::fs::File;
use std::io::prelude::*;
use std::path::Path;
use wascap::jwt::Token;

#[derive(Debug)]
pub struct Actor {
    pub(crate) token: Token,
    pub(crate) bytes: Vec<u8>,
}

impl Actor {
    pub fn from_bytes(buf: Vec<u8>) -> Result<Actor> {
        let token = authz::extract_and_store_claims(&buf)?;
        Ok(Actor { token, bytes: buf })
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Actor> {
        let mut file = File::open(path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;

        Actor::from_bytes(buf)
    }

    pub fn public_key(&self) -> String {
        self.token.claims.subject.to_string()
    }

    pub fn issuer(&self) -> String {
        self.token.claims.issuer.to_string()
    }

    pub fn capabilities(&self) -> Vec<String> {
        match self.token.claims.caps {
            Some(ref caps) => caps.clone(),
            None => vec![],
        }
    }
}
