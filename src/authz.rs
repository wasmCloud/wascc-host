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
use crate::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use wascap::jwt::Token;
use wascap::prelude::*;

lazy_static! {
    static ref AUTH_HOOK: RwLock<Option<Box<AuthHook>>> = RwLock::new(None);
}

pub(crate) type ClaimsMap = Arc<RwLock<HashMap<String, Claims<wascap::jwt::Actor>>>>;

type AuthHook = dyn Fn(&Token<wascap::jwt::Actor>) -> bool + Sync + Send + 'static;

#[allow(dead_code)]
pub(crate) fn set_auth_hook<F>(hook: F)
where
    F: Fn(&Token<wascap::jwt::Actor>) -> bool + Sync + Send + 'static,
{
    *AUTH_HOOK.write().unwrap() = Some(Box::new(hook))
}

pub(crate) fn get_all_claims(map: ClaimsMap) -> Vec<(String, Claims<wascap::jwt::Actor>)> {
    map.read()
        .unwrap()
        .iter()
        .map(|(pk, claims)| (pk.clone(), claims.clone()))
        .collect()
}

pub(crate) fn check_auth(token: &Token<wascap::jwt::Actor>) -> bool {
    let lock = AUTH_HOOK.read().unwrap();
    match *lock {
        Some(ref f) => f(token),
        None => true,
    }
}

pub(crate) fn can_invoke(claims: &Claims<wascap::jwt::Actor>, capability_id: &str) -> bool {
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
            enforce_validation(&token.jwt)?; // returns an `Err` if validation fails
            if !check_auth(&token) {
                // invoke the auth hook, if there is one
                return Err(errors::new(errors::ErrorKind::Authorization(
                    "Authorization hook denied access to module".into(),
                )));
            }

            info!(
                "Discovered capability attestations: {}",
                token
                    .claims
                    .metadata
                    .as_ref()
                    .unwrap()
                    .caps
                    .clone()
                    .unwrap()
                    .join(",")
            );
            Ok(token)
        }
        None => Err(errors::new(errors::ErrorKind::Authorization(
            "No embedded JWT in actor module".to_string(),
        ))),
    }
}

fn enforce_validation(jwt: &str) -> Result<()> {
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
