use crate::Result;
use libloading::Library;
use libloading::Symbol;
use std::ffi::OsStr;
use wascc_codec::{
    capabilities::{CapabilityDescriptor, CapabilityProvider, OP_GET_CAPABILITY_DESCRIPTOR},
    deserialize, SYSTEM_ACTOR,
};

/// Represents a native capability provider compiled as a shared object library.
/// These plugins are OS- and architecture-specific, so they will be `.so` files on Linux, `.dylib`
/// files on macOS, etc.
pub struct NativeCapability {
    pub(crate) plugin: Box<dyn CapabilityProvider>,
    pub(crate) binding_name: String,
    pub(crate) descriptor: CapabilityDescriptor,
    // This field is solely used to keep the FFI library instance allocated for the same
    // lifetime as the boxed plugin
    #[allow(dead_code)]
    library: Option<Library>,
}

impl NativeCapability {
    /// Reads a capability provider from a file. The capability provider must implement the
    /// correct FFI interface to support waSCC plugins. See [wascc.dev](https://wascc.dev) for
    /// documentation and tutorials on how to create a native capability provider
    pub fn from_file<P: AsRef<OsStr>>(
        filename: P,
        binding_target_name: Option<String>,
    ) -> Result<Self> {
        type PluginCreate = unsafe fn() -> *mut dyn CapabilityProvider;

        let library = Library::new(filename.as_ref())?;

        let plugin = unsafe {
            let constructor: Symbol<PluginCreate> = library.get(b"__capability_provider_create")?;
            let boxed_raw = constructor();

            Box::from_raw(boxed_raw)
        };
        let descriptor = get_descriptor(&plugin)?;
        let binding = binding_target_name.unwrap_or("default".to_string());
        info!(
            "Loaded native capability provider '{}' v{} ({}) for {}/{}",
            descriptor.name, descriptor.version, descriptor.revision, descriptor.id, binding
        );

        Ok(NativeCapability {
            plugin,
            descriptor,
            binding_name: binding,
            library: Some(library),
        })
    }

    /// This function is to be used for _capability embedding_. If you are building a custom
    /// waSCC host and have a fixed set of capabilities that you want to always be available
    /// to actors, then you can declare a dependency on the capability provider, enable
    /// the `static_plugin` feature, and provide an instance of that provider. Be sure to check
    /// that the provider supports capability embedding.    
    pub fn from_instance(
        instance: impl CapabilityProvider,
        binding_target_name: Option<String>,
    ) -> Result<Self> {
        let b: Box<dyn CapabilityProvider> = Box::new(instance);
        let descriptor = get_descriptor(&b)?;
        let binding = binding_target_name.unwrap_or("default".to_string());

        info!(
            "Loaded native capability provider '{}' v{} ({}) for {}/{}",
            descriptor.name, descriptor.version, descriptor.revision, descriptor.id, binding
        );
        Ok(NativeCapability {
            descriptor,
            plugin: b,
            binding_name: binding,
            library: None,
        })
    }

    /// Returns the capability ID (namespace) of the provider
    pub fn id(&self) -> String {
        self.descriptor.id.to_string()
    }

    /// Returns the human-friendly name of the provider
    pub fn name(&self) -> String {
        self.descriptor.name.to_string()
    }

    /// Returns the full descriptor for the capability provider
    pub fn descriptor(&self) -> &CapabilityDescriptor {
        &self.descriptor
    }
}

fn get_descriptor(plugin: &Box<dyn CapabilityProvider>) -> Result<CapabilityDescriptor> {
    let res = plugin.handle_call(SYSTEM_ACTOR, OP_GET_CAPABILITY_DESCRIPTOR, &[])?;
    let descriptor: CapabilityDescriptor = deserialize(&res)?;
    Ok(descriptor)
}
