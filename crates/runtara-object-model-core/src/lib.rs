//! Shared Object Model vocabulary and SQL generation, without a database driver.
//! Native stores and WASM agents use the same validation and planning code.

pub mod bulk;
pub mod config;
pub mod instance;
pub mod mapping;
pub mod planning;
pub mod schema;
pub mod sql;
pub mod types;
pub mod validation;

pub use config::{AutoColumns, StoreConfig};
pub use instance::*;
pub use schema::*;
pub use types::*;
