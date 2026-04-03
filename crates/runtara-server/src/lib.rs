pub mod api;
pub mod auth;
pub mod channels;
pub mod compiler;
pub mod config;
pub mod dsl;
pub mod embedded_runtara;
pub mod metrics;
pub mod middleware;
pub mod observability;
pub mod runtime_client;
pub mod step_events;
pub mod types;
pub mod valkey;
pub mod workers;

// Link runtara_agents crate to ensure inventory collects agent metadata at runtime
// This is required for the runtime metadata API to work
extern crate runtara_agents;

// Force linker to include integration agents' inventory items by referencing the module
// Without this, the linker may optimize out the unused symbols
pub use runtara_agents::integrations;

// Re-export spec_generator from runtara-dsl
pub use runtara_dsl::spec as spec_generator;

// Re-export ObjectStoreManager for use by host applications
pub use api::repositories::object_model::ObjectStoreManager;
