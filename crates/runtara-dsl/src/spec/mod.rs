// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Specification Generation Module
//!
//! This module provides functions to generate JSON Schema and OpenAPI specifications
//! for the DSL and agents. It also includes compatibility checking utilities.
//!
//! The specs enable:
//! - Backward compatibility checking
//! - Client SDK generation
//! - Documentation generation
//! - Static spec serving

pub mod agent_openapi;
pub mod compatibility;
pub mod dsl_schema;

pub use agent_openapi::{AGENT_VERSION, generate_agent_openapi_spec, get_agent_changelog};
pub use compatibility::{CompatibilityReport, check_agent_compatibility, check_dsl_compatibility};
pub use dsl_schema::{generate_dsl_schema, get_dsl_changelog};
