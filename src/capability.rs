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

use crate::Result;
use libloading::Library;
use libloading::Symbol;
use std::ffi::OsStr;
use wascc_codec::capabilities::CapabilityProvider;

/// Provides a summary of a registered capability provider
#[derive(Debug, Clone, PartialEq)]
pub struct CapabilitySummary {
    /// The human-readable name of the capability provider
    pub name: String,
    /// The capability ID (namespace) of the provider
    pub id: String,
    /// The binding name for the provider (default is "default")
    pub binding: String,
    /// Indicates whether the capability provider is portable (WASI module)
    pub portable: bool,
}

/// Represents a native capability provider compiled as a shared object library.
/// These plugins are OS- and architecture-specific, so they will be `.so` files on Linux, `.dylib`
/// files on macOS, etc.
pub struct NativeCapability {
    pub(crate) capid: String,
    pub(crate) plugin: Box<dyn CapabilityProvider>,
    pub(crate) binding_name: String,
    // This field is solely used to keep the FFI library instance allocated for the same
    // lifetime as the boxed plugin
    #[allow(dead_code)]
    library: Option<Library>,
}

impl NativeCapability {
    /// Reads a capability provider from a file. The capability provider must implement the
    /// correct FFI interface to support waSCC plugins. See [wascc.dev](https://wascc.dev) for
    /// documentation and tutorials on how to create a native capability provider
    pub fn from_file<P: AsRef<OsStr>>(filename: P, binding_name: Option<String>) -> Result<Self> {
        type PluginCreate = unsafe fn() -> *mut dyn CapabilityProvider;

        let library = Library::new(filename.as_ref())?;

        let plugin = unsafe {
            let constructor: Symbol<PluginCreate> = library.get(b"__capability_provider_create")?;
            let boxed_raw = constructor();

            Box::from_raw(boxed_raw)
        };
        info!(
            "Loaded capability: {}, native provider: {}",
            plugin.capability_id(),
            plugin.name()
        );

        let capid = plugin.capability_id().to_string();
        Ok(NativeCapability {
            capid,
            plugin,
            binding_name: binding_name.unwrap_or("default".to_string()),
            library: Some(library),
        })
    }

    /// If you know ahead of time that you want a particular capability provider to be a compile-time
    /// dependency, you can create your own provider instance and pass it to this function.
    /// WARNING: you can only register a single capability provider from an instance per runtime host because
    /// the FFI function used by the plugins is globally scoped.
    pub fn from_instance(
        instance: impl CapabilityProvider,
        binding_name: Option<String>,
    ) -> Result<Self> {
        let capid = instance.capability_id();
        Ok(NativeCapability {
            capid: capid.to_string(),
            plugin: Box::new(instance),
            binding_name: binding_name.unwrap_or("default".to_string()),
            library: None,
        })
    }

    /// Returns the capability ID (namespace) of the provider
    pub fn id(&self) -> String {
        self.capid.clone()
    }

    /// Returns the human-friendly name of the provider
    pub fn name(&self) -> String {
        self.plugin.name().to_string()
    }
}
