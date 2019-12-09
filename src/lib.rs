#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;

pub type Result<T> = std::result::Result<T, errors::Error>;
pub use middleware::Middleware;

mod authz;
pub mod dispatch;
pub mod errors;
pub mod host;
mod middleware;
pub mod plugins;
mod router;
