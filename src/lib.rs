#[macro_use]
extern crate lazy_static;

#[macro_use]
extern crate log;

pub type Result<T> = std::result::Result<T, errors::Error>;

mod authz;
pub mod dispatch;
pub mod errors;
pub mod host;
pub mod plugins;
mod router;
