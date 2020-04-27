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

use crate::inthost::{Invocation, InvocationResponse};
use crate::{errors, Result};
use crossbeam::{Receiver, Sender};
use std::collections::HashMap;

pub(crate) type RouteKey = (String, String); // (binding, id)

#[derive(Default)]
pub(crate) struct Router {
    routes: HashMap<RouteKey, RouteEntry>,
}

#[derive(Clone)]
pub(crate) struct RouteEntry {
    pub inv_s: Sender<Invocation>,
    pub resp_r: Receiver<InvocationResponse>,
    pub terminator: Sender<bool>,
}

impl RouteEntry {
    pub(crate) fn invoke(&self, inv: Invocation) -> Result<InvocationResponse> {
        trace!("Invoking: {} from {}", inv.operation, inv.origin);

        self.inv_s.send(inv).unwrap();
        Ok(self.resp_r.recv().unwrap())
    }

    pub(crate) fn terminate(&self) {
        self.terminator.send(true).unwrap();
    }
}

// Support for multiple ways to get a pair
// 1 get_pair(None, "wascc:messaging") -> pair to invoke the default messaging binding
// 2 get_pair(Some("controlplane"), "wascc:messaging") -> pair to invoke a capability
// 3 get_pair(Some("_actor_"), subject) -> pair to invoke an actor

impl Router {
    pub fn add_route(
        &mut self,
        binding: &str,
        id: &str,
        inv_s: Sender<Invocation>,
        resp_r: Receiver<InvocationResponse>,
        term_s: Sender<bool>,
    ) {
        self.routes.insert(
            route_key(binding, id),
            RouteEntry {
                inv_s,
                resp_r,
                terminator: term_s,
            },
        );
    }

    pub fn get_route(&self, binding: &str, id: &str) -> Option<RouteEntry> {
        let key = route_key(binding, id);
        match self.routes.get(&key) {
            Some(p) => Some(p.clone()),
            None => None,
        }
    }

    pub fn remove_route(&mut self, binding: &str, id: &str) {
        self.routes.remove(&route_key(binding, id));
    }

    pub fn route_exists(&self, binding: &str, id: &str) -> bool {
        let key = route_key(binding, id);
        self.routes.contains_key(&key)
    }

    pub fn terminate_route(&mut self, binding: &str, id: &str) -> Result<()> {
        let key = route_key(binding, id);
        if let Some((_key, entry)) = self.routes.remove_entry(&key) {
            entry.terminate();
            Ok(())
        } else {
            Err(errors::new(errors::ErrorKind::MiscHost(format!(
                "Failed to remove route - does not exist: {:?}",
                key
            ))))
        }
    }    
}

pub(crate) fn route_key(binding: &str, id: &str) -> RouteKey {
    (binding.to_string(), id.to_string())
}
