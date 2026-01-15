// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Workflow Standard Library
//!
//! Unified library for workflow binaries. Combines agents and runtime
//! into a single crate that workflows link against.
//!
//! This library integrates with runtara-core via runtara-sdk for:
//! - Instance registration and lifecycle management
//! - Checkpointing for crash recovery
//! - Signal handling (pause, cancel, resume)
//! - Heartbeat/tick for liveness monitoring
//!
//! Usage in generated workflow code:
//! ```rust
//! extern crate runtara_workflow_stdlib;
//! use runtara_workflow_stdlib::prelude::*;
//! use runtara_workflow_stdlib::agents::*;
//! ```

// Re-export the agents crate
pub use runtara_agents as agents;

// Runtime module (wraps runtara-sdk)
pub mod runtime;

// Condition helpers for generated conditional steps
pub mod conditions;

// Connection management (fetches connections from external service)
pub mod connections;

// Instance output handling (for Environment communication)
pub mod instance_output;

// Re-export serde at top level
pub use serde;
pub use serde_json;

// Re-export libc for stderr redirection in generated workflows
pub use libc;

// Re-export tokio for async runtime in generated workflows
pub use tokio;

// Re-export runtara-sdk for direct use
pub use runtara_sdk;

// Re-export tracing for structured logging in generated workflows
pub use tracing;
pub use tracing_subscriber;

// Prelude for convenient imports
pub mod prelude {
    // Runtime types
    pub use crate::runtime::{Error, Result};

    // SDK types for durability
    pub use crate::runtime::{RuntaraSdk, SdkConfig, durable, register_sdk, sdk};

    // Condition helpers for generated conditional steps
    pub use crate::conditions::{is_truthy, to_number, values_equal};

    // Connection types
    pub use crate::connections::{
        ConnectionError, ConnectionResponse, RateLimitState, fetch_connection,
    };

    // Instance output types (for Environment communication)
    pub use crate::instance_output::{
        InstanceOutput, InstanceOutputStatus, write_cancelled, write_completed, write_failed,
        write_sleeping, write_suspended,
    };

    // Serde types
    pub use serde::{Deserialize, Serialize};
    pub use serde_json;

    // Agent registry
    pub use runtara_agents::registry;
}

// Direct access to commonly used modules
pub use runtara_agents::registry;
pub use runtime::{Error, Result};
