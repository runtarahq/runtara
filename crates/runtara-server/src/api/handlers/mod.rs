// HTTP route handlers (thin layer - HTTP concerns only)
// Handlers extract parameters, call services, map responses to HTTP
// NOTE: Connection, OAuth, and rate limit handlers are now in runtara-connections crate.

pub mod agent_execution;
pub mod agent_testing;
pub mod analytics;
pub mod api_keys;
pub mod chat;
pub mod common;
pub mod csv_import_export;
pub mod events;
pub mod executions;
pub mod file_storage;
pub mod internal_agents;
pub mod internal_object_model;
pub mod internal_proxy;
pub mod metrics;
pub mod object_model;
pub mod oidc_discovery;
pub mod operators;
pub mod reports;
pub mod sessions;
pub mod specs;
pub mod step_events;
pub mod step_summaries;
pub mod triggers;
pub mod workflows;
pub mod workflows_sync;

#[cfg(feature = "embed-ui")]
pub mod ui;
