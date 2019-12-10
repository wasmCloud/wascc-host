use crate::Result;
use libloading::Library;
use libloading::Symbol;
use std::ffi::OsStr;
use wascc_codec::capabilities::CapabilityProvider;

/// Represents a native capability provider compiled as a shared object library
pub struct Capability {
    pub(crate) capid: String,
    pub(crate) plugin: Box<dyn CapabilityProvider>,
    library: Library,
}

impl Capability {
    /// Reads a capability provider from a file
    pub fn from_file<P: AsRef<OsStr>>(filename: P) -> Result<Self> {
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
        Ok(Capability {
            capid,
            plugin,
            library,
        })
    }
}
