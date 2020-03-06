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

use crate::host::{Invocation, InvocationResponse};
use crate::Result;
use std::sync::Arc;
use std::sync::RwLock;
use wapc::WapcHost;

lazy_static! {
    pub(crate) static ref MIDDLEWARES: Arc<RwLock<Vec<Box<dyn Middleware>>>> =
        { Arc::new(RwLock::new(Vec::new())) };
}

/// The trait that must be implemented by all waSCC middleware
pub trait Middleware: Send + Sync + 'static {
    fn actor_pre_invoke(&self, inv: Invocation) -> Result<Invocation>;
    fn actor_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse>;
    fn capability_pre_invoke(&self, inv: Invocation) -> Result<Invocation>;
    fn capability_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse>;
}

pub(crate) fn invoke_capability(inv: Invocation) -> Result<InvocationResponse> {
    let inv = match run_capability_pre_invoke(inv.clone(), &MIDDLEWARES.read().unwrap()) {
        Ok(i) => i,
        Err(e) => {
            error!("Middleware failure: {}", e);
            inv
        }
    };

    let lock = crate::plugins::PLUGMAN.read().unwrap();

    let r = lock.call(&inv).unwrap();
    match run_capability_post_invoke(r.clone(), &MIDDLEWARES.read().unwrap()) {
        Ok(r) => Ok(r),
        Err(e) => {
            error!("Middleware failure: {}", e);
            Ok(r)
        }
    }
}

pub(crate) fn invoke_actor(inv: Invocation, guest: &mut WapcHost) -> Result<InvocationResponse> {
    let inv = match run_actor_pre_invoke(inv.clone(), &MIDDLEWARES.read().unwrap()) {
        Ok(i) => i,
        Err(e) => {
            error!("Middleware failure: {}", e);
            inv
        }
    };

    let inv_r = match guest.call(&inv.operation, &inv.msg) {
        Ok(v) => InvocationResponse::success(v),
        Err(e) => InvocationResponse::error(&format!("Failed to invoke guest call: {}", e)),
    };
    match run_actor_post_invoke(inv_r.clone(), &MIDDLEWARES.read().unwrap()) {
        Ok(r) => Ok(r),
        Err(e) => {
            error!("Middleware failure: {}", e);
            Ok(inv_r)
        }
    }
}

fn run_actor_pre_invoke(
    inv: Invocation,
    middlewares: &[Box<dyn Middleware>],
) -> Result<Invocation> {
    let mut cur_inv = inv;
    for m in middlewares {
        match m.actor_pre_invoke(cur_inv) {
            Ok(i) => cur_inv = i.clone(),
            Err(e) => return Err(e),
        }
    }
    Ok(cur_inv)
}

fn run_actor_post_invoke(
    resp: InvocationResponse,
    middlewares: &[Box<dyn Middleware>],
) -> Result<InvocationResponse> {
    let mut cur_resp = resp;
    for m in middlewares {
        match m.actor_post_invoke(cur_resp) {
            Ok(i) => cur_resp = i.clone(),
            Err(e) => return Err(e),
        }
    }
    Ok(cur_resp)
}

pub(crate) fn run_capability_pre_invoke(
    inv: Invocation,
    middlewares: &[Box<dyn Middleware>],
) -> Result<Invocation> {
    let mut cur_inv = inv;
    for m in middlewares {
        match m.capability_pre_invoke(cur_inv) {
            Ok(i) => cur_inv = i.clone(),
            Err(e) => return Err(e),
        }
    }
    Ok(cur_inv)
}

pub(crate) fn run_capability_post_invoke(
    resp: InvocationResponse,
    middlewares: &[Box<dyn Middleware>],
) -> Result<InvocationResponse> {
    let mut cur_resp = resp;
    for m in middlewares {
        match m.capability_post_invoke(cur_resp) {
            Ok(i) => cur_resp = i.clone(),
            Err(e) => return Err(e),
        }
    }
    Ok(cur_resp)
}

#[cfg(test)]
mod test {
    use std::sync::atomic::{AtomicUsize, Ordering};

    lazy_static! {
        pub static ref PRE: AtomicUsize = { AtomicUsize::new(0) };
        pub static ref POST: AtomicUsize = { AtomicUsize::new(0) };
        pub static ref CAP_PRE: AtomicUsize = { AtomicUsize::new(0) };
        pub static ref CAP_POST: AtomicUsize = { AtomicUsize::new(0) };
    }

    use super::Middleware;
    use crate::host::Invocation;
    use crate::host::InvocationResponse;
    use crate::Result;

    struct IncMiddleware {
        pre: &'static AtomicUsize,
        post: &'static AtomicUsize,
        cap_pre: &'static AtomicUsize,
        cap_post: &'static AtomicUsize,
    }

    impl Middleware for IncMiddleware {
        fn actor_pre_invoke(&self, inv: Invocation) -> Result<Invocation> {
            self.pre.fetch_add(1, Ordering::SeqCst);
            Ok(inv)
        }
        fn actor_post_invoke(&self, response: InvocationResponse) -> Result<InvocationResponse> {
            self.post.fetch_add(1, Ordering::SeqCst);
            Ok(response)
        }
        fn capability_pre_invoke(&self, inv: Invocation) -> Result<Invocation> {
            self.cap_pre.fetch_add(1, Ordering::SeqCst);
            Ok(inv)
        }
        fn capability_post_invoke(
            &self,
            response: InvocationResponse,
        ) -> Result<InvocationResponse> {
            self.cap_post.fetch_add(1, Ordering::SeqCst);
            Ok(response)
        }
    }

    #[test]
    fn simple_add() {
        let inc_mid = IncMiddleware {
            pre: &PRE,
            post: &POST,
            cap_pre: &CAP_PRE,
            cap_post: &CAP_POST,
        };

        let mids: Vec<Box<dyn Middleware>> = vec![Box::new(inc_mid)];
        let inv = Invocation::new(
            "test".to_string(),
            "testing:sample!Foo",
            b"abc1234".to_vec(),
        );
        let res = super::run_actor_pre_invoke(inv.clone(), &mids);
        assert!(res.is_ok());
        let res2 = super::run_actor_pre_invoke(inv, &mids);
        assert!(res2.is_ok());
        assert_eq!(PRE.fetch_add(0, Ordering::SeqCst), 2);
    }
}
