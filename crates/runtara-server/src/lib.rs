pub mod api;
pub mod auth;
pub mod bind;
pub mod channels;
pub mod compiler;
pub mod config;
pub mod dsl;
pub mod embedded_runtara;
pub mod mcp;
pub mod metrics;
pub mod middleware;
pub mod observability;
pub mod runtime_client;
pub mod server;
pub mod shutdown;
pub mod step_events;
pub mod types;
pub mod valkey;
pub mod workers;

pub use server::start;

// Link runtara_agents so the static metadata registry is available at runtime.
extern crate runtara_agents;

// Keep integration agents reachable for metadata and execution APIs.
pub use runtara_agents::integrations;

// Re-export spec_generator from runtara-dsl
pub use runtara_dsl::spec as spec_generator;

// Re-export ObjectStoreManager for use by host applications
pub use api::repositories::object_model::ObjectStoreManager;
