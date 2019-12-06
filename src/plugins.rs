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

use crate::dispatch::WasccNativeDispatcher;
use crate::errors::{self, ErrorKind};
use crate::host::Invocation;
use crate::host::InvocationResponse;
use crate::Result;
use libloading::{Library, Symbol};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::RwLock;
use wascc_codec::capabilities::CapabilityProvider;

lazy_static! {
    pub(crate) static ref PLUGMAN: RwLock<PluginManager> = { RwLock::new(PluginManager::new()) };
}

#[derive(Default)]
pub(crate) struct PluginManager {
    plugins: HashMap<String, Box<dyn CapabilityProvider>>,
    loaded_libraries: HashMap<String, Library>,
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
            Some(p) => match p.configure_dispatch(Box::new(dispatcher)) {
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
            Some(c) => match c.handle_call(&inv.origin, &v[1], &inv.msg) {
                Ok(msg) => Ok(InvocationResponse::success(msg)),
                Err(e) => Err(errors::new(errors::ErrorKind::HostCallFailure(e))),
            },
            None => Err(errors::new(ErrorKind::CapabilityProvider(format!(
                "No such capability ID registered: {}",
                capability_id
            )))),
        }
    }

    pub fn load_plugin<P: AsRef<OsStr>>(&mut self, filename: P) -> Result<String> {
        type PluginCreate = unsafe fn() -> *mut dyn CapabilityProvider;

        let lib = Library::new(filename.as_ref())?;

        let plugin = unsafe {
            let constructor: Symbol<PluginCreate> = lib.get(b"__capability_provider_create")?;
            let boxed_raw = constructor();

            Box::from_raw(boxed_raw)
        };
        info!(
            "Loaded capability: {}, native provider: {}",
            plugin.capability_id(),
            plugin.name()
        );

        let capid = plugin.capability_id().to_string();

        // We need to keep the library around otherwise our plugin's vtable will
        // point to garbage. We do this little dance to make sure the library
        // doesn't end up getting moved.
        self.loaded_libraries.insert(capid.clone(), lib);

        if self.plugins.contains_key(&capid) {
            panic!(
                "Duplicate providers attempted to register for {}",
                plugin.capability_id()
            );
        }

        self.plugins
            .insert(plugin.capability_id().to_string(), plugin);

        Ok(capid)
    }

    /// Unload all plugins and loaded plugin libraries, making sure to fire
    /// their `on_plugin_unload()` methods so they can do any necessary cleanup.
    pub fn unload(&mut self) {
        info!("Unloading plugins");

        for lib in self.loaded_libraries.drain() {
            drop(lib);
        }
    }
}

impl Drop for PluginManager {
    fn drop(&mut self) {
        if !self.plugins.is_empty() || !self.loaded_libraries.is_empty() {
            self.unload();
        }
    }
}
