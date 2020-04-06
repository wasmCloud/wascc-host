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

use crate::capability::NativeCapability;
use crate::dispatch::WasccNativeDispatcher;
use crate::errors::{self, ErrorKind};
use crate::inthost::Invocation;
use crate::inthost::{InvocationResponse, InvocationTarget};
use crate::{
    router::{get_route, route_key, RouteKey},
    Result,
};
use std::collections::HashMap;
use std::sync::RwLock;

lazy_static! {
    pub(crate) static ref PLUGMAN: RwLock<PluginManager> = { RwLock::new(PluginManager::new()) };
}

#[derive(Default)]
pub(crate) struct PluginManager {
    plugins: HashMap<RouteKey, NativeCapability>,
}

impl PluginManager {
    pub fn new() -> PluginManager {
        Self::default()
    }

    pub fn register_dispatcher(
        &mut self,
        binding: &str,
        capid: &str,
        dispatcher: WasccNativeDispatcher,
    ) -> Result<()> {
        let key = route_key(binding, capid);
        match self.plugins.get(&key) {
            Some(p) => match p.plugin.configure_dispatch(Box::new(dispatcher)) {
                Ok(_) => Ok(()),
                Err(_) => Err(errors::new(ErrorKind::CapabilityProvider(
                    "Failed to configure dispatch on provider".into(),
                ))),
            },
            None => Err(errors::new(ErrorKind::CapabilityProvider(
                "attempt to register dispatcher for non-existent plugin".into(),
            ))),
        }
    }

    pub fn call(&self, inv: &Invocation) -> Result<InvocationResponse> {
        if let InvocationTarget::Capability { capid, binding } = &inv.target {
            let route_key = route_key(&binding, &capid);
            match self.plugins.get(&route_key) {
                // native capability is registered via plugin
                Some(c) => match c.plugin.handle_call(&inv.origin, &inv.operation, &inv.msg) {
                    Ok(msg) => Ok(InvocationResponse::success(msg)),
                    Err(e) => Err(errors::new(errors::ErrorKind::HostCallFailure(e))),
                },
                // if there's no plugin, check if there's a route pointing to this capid (portable capability provider)
                None => {
                    if let Some(entry) = get_route(&binding, &capid) {
                        entry.invoke(inv.clone())
                    } else {
                        Err(errors::new(ErrorKind::CapabilityProvider(format!(
                            "No such capability ID registered as native plug-in or portable provider: {:?}",
                            route_key
                        ))))
                    }
                }
            }
        } else {
            Err(errors::new(ErrorKind::MiscHost(
                "Attempted to invoke a capability provider plugin as though it were an actor. Bad route?".into()
            )))
        }
    }

    pub fn add_plugin(&mut self, plugin: NativeCapability) -> Result<()> {
        let key = route_key(&plugin.binding_name, &plugin.capid);
        if self.plugins.contains_key(&key) {
            Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                "Duplicate capability ID attempted to register provider: ({},{})",
                plugin.binding_name, plugin.capid
            ))))
        } else {
            self.plugins.insert(key, plugin);
            Ok(())
        }
    }

    pub fn remove_plugin(&mut self, binding: &str, capid: &str) -> Result<()> {
        let key = route_key(&binding, &capid);
        if let Some(plugin) = self.plugins.remove(&key) {
            drop(plugin);
        }
        Ok(())
    }
}

pub(crate) fn remove_plugin(binding: &str, capid: &str) -> Result<()> {
    PLUGMAN.write().unwrap().remove_plugin(binding, capid)
}
