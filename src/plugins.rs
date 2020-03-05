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

use crate::capability::NativeCapability;
use crate::dispatch::WasccNativeDispatcher;
use crate::errors::{self, ErrorKind};
use crate::host::Invocation;
use crate::host::InvocationResponse;
use crate::Result;
use std::collections::HashMap;
use std::sync::RwLock;

lazy_static! {
    pub(crate) static ref PLUGMAN: RwLock<PluginManager> = { RwLock::new(PluginManager::new()) };
}

#[derive(Default)]
pub(crate) struct PluginManager {
    plugins: HashMap<String, NativeCapability>, //plugins: HashMap<String, Box<dyn CapabilityProvider>>,
                                                //loaded_libraries: HashMap<String, Library>
}

impl PluginManager {
    pub fn new() -> PluginManager {
        Self::default()
    }

    pub fn register_dispatcher(
        &mut self,
        capid: &str,
        dispatcher: WasccNativeDispatcher,
    ) -> Result<()> {
        match self.plugins.get(capid) {
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
        let v: Vec<&str> = inv.operation.split('!').collect();
        let capability_id = v[0];
        match self.plugins.get(capability_id) {
            // native capability is registered via plugin
            Some(c) => match c.plugin.handle_call(&inv.origin, &v[1], &inv.msg) {
                Ok(msg) => Ok(InvocationResponse::success(msg)),
                Err(e) => Err(errors::new(errors::ErrorKind::HostCallFailure(e))),
            },
            // if there's no plugin, check if there's a route pointing to this capid (portable capability provider)
            None => if let Some(pair) = crate::host::ROUTER.read().unwrap().get_pair(&capability_id) {
                crate::host::invoke(&pair, inv.origin.clone(), &inv.operation, &inv.msg)
            } else {
                Err(errors::new(ErrorKind::CapabilityProvider(format!(
                    "No such capability ID registered: {}",
                    capability_id
                ))))
            }
        }
    }

    pub fn add_plugin(&mut self, plugin: NativeCapability) -> Result<()> {
        if self.plugins.contains_key(&plugin.capid) {
            Err(errors::new(errors::ErrorKind::CapabilityProvider(format!(
                "Duplicate capability ID attempted to register provider: {}",
                plugin.capid
            ))))
        } else {
            self.plugins.insert(plugin.capid.to_string(), plugin);
            Ok(())
        }
    }

    pub fn remove_plugin(&mut self, capid: &str) -> Result<()> {
        if let Some(plugin) = self.plugins.remove(capid) {
            drop(plugin);
        }
        Ok(())
    }
}
