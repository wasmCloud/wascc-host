use wascap::jwt::Token;
use crate::errors;
use crate::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use wascap::prelude::*;

lazy_static! {
    pub(crate) static ref CLAIMS: Arc<RwLock<HashMap<String, Claims>>> =
        { Arc::new(RwLock::new(HashMap::new())) };
    pub(crate) static ref CLAIMS_MAP: Arc<RwLock<HashMap<u64, String>>> =
        { Arc::new(RwLock::new(HashMap::new())) };
    static ref AUTH_HOOK: RwLock<Option<Box<AuthHook>>> = RwLock::new(None);
}

type AuthHook = dyn Fn(&Token) -> bool + Sync + Send + 'static;

#[allow(dead_code)]
pub fn set_auth_hook<F>(hook: F) 
where F: Fn(&Token) -> bool
        + Sync
        + Send
        + 'static,
{
    *AUTH_HOOK.write().unwrap() = Some(Box::new(hook))
}

pub(crate) fn check_auth(token: &Token) -> bool {
    let lock = AUTH_HOOK.read().unwrap();
    match *lock {
        Some(ref f) => {
            f(token)
        },
        None => true
    }
}

pub(crate) fn store_claims(claims: Claims) -> Result<()> {
    CLAIMS
        .write()
        .unwrap()
        .insert(claims.subject.clone(), claims.clone());
    Ok(())
}

pub(crate) fn map_claims(id: u64, public_key: &str) {
    CLAIMS_MAP
        .write()
        .unwrap()
        .insert(id, public_key.to_string());
}

pub(crate) fn can_id_invoke(id: u64, capability_id: &str) -> bool {
    CLAIMS_MAP
        .read()
        .unwrap()
        .get(&id)
        .map_or(false, |pk| can_invoke(pk, capability_id))
}

pub(crate) fn pk_for_id(id: u64) -> String {
    CLAIMS_MAP
        .read()
        .unwrap()
        .get(&id)
        .map_or(format!("actor:{}", id), |s| s.clone())
}

pub(crate) fn can_invoke(pk: &str, capability_id: &str) -> bool {
    CLAIMS.read().unwrap().get(pk).map_or(false, |claims| {
        claims
            .caps
            .as_ref()
            .map_or(false, |caps| caps.contains(&capability_id.to_string()))
    })
}

/// Extract claims from the JWT embedded in the wasm module's custom section
/// and then store them in the static hashmap correlating module public
/// keys with their claims
pub(crate) fn extract_and_store_claims(buf: &[u8]) -> Result<wascap::jwt::Token> {
    let token = wascap::wasm::extract_claims(buf)?;
    match token {
        Some(token) => {
            enforce_validation(&token.jwt)?;
            if !check_auth(&token) {                
                return Err(errors::new(errors::ErrorKind::Authorization(
                    "Authorization hook denied access to module".into()
                )))
            }

            info!(
                "Discovered capability attestations: {}",
                token.claims.caps.clone().unwrap().join(",")
            );
            store_claims(token.claims.clone())?;
            Ok(token)
        }
        None => Err(errors::new(errors::ErrorKind::Authorization(
            "No embedded JWT in actor module".to_string(),
        ))),
    }
}

fn enforce_validation(jwt: &str) -> Result<()> {
    let v = validate_token(jwt)?;
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
