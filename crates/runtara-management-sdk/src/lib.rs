// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Management SDK
//!
//! High-level SDK for managing Runtara Environment instances and images.
//!
//! This crate provides an ergonomic client for interacting with runtara-environment's
//! API over QUIC. It is the single entry point for all management operations.
//!
//! # Architecture
//!
//! The Management SDK talks ONLY to runtara-environment:
//! - Image management (register, list, delete)
//! - Instance lifecycle (start, stop, resume, status)
//! - Signals (pause, cancel - proxied to runtara-core by Environment)
//!
//! # Example
//!
//! ```no_run
//! use runtara_management_sdk::{ManagementSdk, StartInstanceOptions};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Create SDK for local development
//! let sdk = ManagementSdk::localhost()?;
//!
//! // Connect to runtara-environment
//! sdk.connect().await?;
//!
//! // Check health
//! let health = sdk.health_check().await?;
//! println!("Server version: {}", health.version);
//!
//! // Start an instance
//! let options = StartInstanceOptions::new("my-image-id", "tenant-1")
//!     .with_input(serde_json::json!({"key": "value"}));
//! let result = sdk.start_instance(options).await?;
//! println!("Started instance: {}", result.instance_id);
//!
//! // Get instance status
//! let status = sdk.get_instance_status(&result.instance_id).await?;
//! println!("Status: {:?}", status.status);
//! # Ok(())
//! # }
//! ```

mod client;
mod config;
mod error;
mod types;

pub use client::ManagementSdk;
pub use config::SdkConfig;
pub use error::{Result, SdkError};
pub use types::{
    AgentInfo, CapabilityField, CapabilityInfo, Checkpoint, CheckpointSummary, EventSummary,
    HealthStatus, ImageSummary, InstanceInfo, InstanceStatus, InstanceSummary,
    ListCheckpointsOptions, ListCheckpointsResult, ListEventsOptions, ListEventsResult,
    ListImagesOptions, ListImagesResult, ListInstancesOptions, ListInstancesOrder,
    ListInstancesResult, RegisterImageOptions, RegisterImageResult, RegisterImageStreamOptions,
    RunnerType, SignalType, StartInstanceOptions, StartInstanceResult, StopInstanceOptions,
    TestCapabilityOptions, TestCapabilityResult,
};
