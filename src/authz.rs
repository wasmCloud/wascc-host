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

use crate::errors;
use crate::{Result, WasccEntity, WasccHost};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use wascap::jwt::Token;
use wascap::prelude::*;

pub(crate) type ClaimsMap = Arc<RwLock<HashMap<String, Claims<wascap::jwt::Actor>>>>;

/// An authorizer is responsible for determining whether an actor can be loaded as well as
/// whether an actor can invoke another entity. For invocation checks, the authorizer is only ever invoked _after_
/// an initial capability attestation check has been performed and _passed_. This has the net effect of making it
/// impossible to override the base behavior of checking that an actor's embedded JWT contains the right
/// capability attestations.
pub trait Authorizer: Sync + Send {
    /// This check is performed during the `add_actor` call, allowing the custom authorizer to do things
    /// like verify a provenance chain, make external calls, etc.
    fn can_load(&self, claims: &Claims<Actor>) -> bool;
    /// This check will be performed for _every_ invocation that has passed the base capability check,
    /// including the operation that occurs during `bind_actor`. Developers should be aware of this because
    /// if `set_authorizer` is done _after_ actor binding, it could potentially allow an unauthorized binding.
    fn can_invoke(&self, claims: &Claims<Actor>, target: &WasccEntity, operation: &str) -> bool;
}

pub(crate) struct DefaultAuthorizer {}

impl DefaultAuthorizer {
    pub fn new() -> impl Authorizer {
        DefaultAuthorizer {}
    }
}

impl Authorizer for DefaultAuthorizer {
    fn can_load(&self, _claims: &Claims<Actor>) -> bool {
        true
    }

    // This doesn't actually mean everyone can invoke everything. Remember that the host itself
    // will _always_ enforce the claims check on an actor having the required capability
    // attestation
    fn can_invoke(&self, _claims: &Claims<Actor>, target: &WasccEntity, _operation: &str) -> bool {
        match target {
            WasccEntity::Actor(_a) => true,
            WasccEntity::Capability { .. } => true,
        }
    }
}

pub(crate) fn get_all_claims(map: ClaimsMap) -> Vec<(String, Claims<wascap::jwt::Actor>)> {
    map.read()
        .unwrap()
        .iter()
        .map(|(pk, claims)| (pk.clone(), claims.clone()))
        .collect()
}

// We don't (yet) support per-operation security constraints, but when we do, this
// function will be ready to support that without breaking everyone else's calls
pub(crate) fn can_invoke(
    claims: &Claims<wascap::jwt::Actor>,
    capability_id: &str,
    _operation: &str,
) -> bool {
    // Edge case - deliver configuration to an actor directly,
    // so "self invocation" needs to be authorized
    if claims.subject == capability_id {
        return true;
    }
    claims
        .metadata
        .as_ref()
        .unwrap()
        .caps
        .as_ref()
        .map_or(false, |caps| caps.contains(&capability_id.to_string()))
}

// Extract claims from the JWT embedded in the wasm module's custom section
pub(crate) fn extract_claims(buf: &[u8]) -> Result<wascap::jwt::Token<wascap::jwt::Actor>> {
    let token = wascap::wasm::extract_claims(buf)?;
    match token {
        Some(token) => {
            let claims = token.claims.clone();
            let caps = claims.metadata.as_ref().unwrap().caps.clone();
            info!(
                "Discovered capability attestations for actor {}: {}",
                &claims.subject,
                caps.unwrap_or(vec![]).join(",")
            );
            Ok(token)
        }
        None => Err(errors::new(errors::ErrorKind::Authorization(
            "No embedded JWT in actor module".to_string(),
        ))),
    }
}

pub(crate) fn enforce_validation(jwt: &str) -> Result<()> {
    let v = validate_token::<wascap::jwt::Actor>(jwt)?;
    if v.expired {
        Err(errors::new(errors::ErrorKind::Authorization(
            "Expired token".to_string(),
        )))
    } else if v.cannot_use_yet {
        Err(errors::new(errors::ErrorKind::Authorization(format!(
            "Module cannot be used before {}",
            v.not_before_human
        ))))
    } else {
        Ok(())
    }
}

pub(crate) fn register_claims(
    claims_map: ClaimsMap,
    subject: &str,
    claims: Claims<wascap::jwt::Actor>,
) {
    claims_map
        .write()
        .unwrap()
        .insert(subject.to_string(), claims);
}

pub(crate) fn unregister_claims(claims_map: ClaimsMap, subject: &str) {
    claims_map.write().unwrap().remove(subject);
}

impl WasccHost {
    pub(crate) fn check_auth(&self, token: &Token<wascap::jwt::Actor>) -> bool {
        self.authorizer.read().unwrap().can_load(&token.claims)
    }
}
