#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;

pub type Result<T> = std::result::Result<T, errors::Error>;
pub use actor::Actor;
pub use capability::Capability;
pub use middleware::Middleware;

mod actor;
mod authz;
mod capability;
pub mod dispatch;
pub mod errors;
pub mod host;
mod middleware;
pub mod plugins;
mod router;
