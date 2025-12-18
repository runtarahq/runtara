// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Core - Durable Execution Engine
//!
//! This crate provides the execution engine for durable workflows. It manages checkpoints,
//! signals, and instance events, persisting all state to PostgreSQL for crash resilience.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                         External Clients                                 │
//! │                    (runtara-management-sdk, CLI)                         │
//! └─────────────────────────────────────────────────────────────────────────┘
//!                                    │
//!                                    ▼
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                      runtara-environment                                 │
//! │            (Image Registry, Instance Lifecycle, Wake Queue)              │
//! │                           Port 7000                                      │
//! └─────────────────────────────────────────────────────────────────────────┘
//!           │                                              │
//!           │ Management Protocol                          │ Spawns
//!           ▼                                              ▼
//! ┌───────────────────────┐                    ┌─────────────────────────────┐
//! │    runtara-core       │◄───────────────────│     Workflow Instances      │
//! │  (This Crate)         │  Instance Protocol │   (using runtara-sdk)       │
//! │  Checkpoints/Signals  │                    │                             │
//! │  Port 7001 + 7002     │                    └─────────────────────────────┘
//! └───────────────────────┘
//!           │
//!           ▼
//! ┌───────────────────────┐
//! │      PostgreSQL       │
//! │  (Durable Storage)    │
//! └───────────────────────┘
//! ```
//!
//! # QUIC Servers
//!
//! Core exposes two QUIC servers:
//!
//! | Server | Port | Purpose |
//! |--------|------|---------|
//! | Instance Server | 8001 | Workflow instances connect here via runtara-sdk |
//! | Management Server | 8003 | runtara-environment connects here for coordination |
//!
//! # Instance Protocol (Port 8001)
//!
//! The instance protocol handles all communication between workflow instances and Core.
//! Instances use [`runtara-sdk`] which wraps this protocol.
//!
//! ## Operations
//!
//! | Operation | Description |
//! |-----------|-------------|
//! | `RegisterInstance` | Self-register on startup, optionally resume from checkpoint |
//! | `Checkpoint` | Save state (or return existing if checkpoint_id exists) + signal delivery |
//! | `GetCheckpoint` | Read-only checkpoint lookup |
//! | `Sleep` | Durable sleep - always handled in-process |
//! | `InstanceEvent` | Fire-and-forget events (heartbeat, completed, failed, suspended) |
//! | `GetInstanceStatus` | Query instance status |
//! | `PollSignals` | Poll for pending cancel/pause/resume signals |
//! | `SignalAck` | Acknowledge receipt of a signal |
//!
//! ## Checkpoint Semantics
//!
//! The `Checkpoint` operation is the primary durability mechanism:
//!
//! 1. **First call with checkpoint_id**: Saves state, returns empty `existing_state`
//! 2. **Subsequent calls with same checkpoint_id**: Returns existing state (for resume)
//! 3. **Signal delivery**: Returns pending signals in response for efficient poll-free detection
//!
//! ## Sleep Behavior
//!
//! All sleeps are handled in-process by runtara-core. Managed environments
//! (runtara-environment) may hibernate containers separately based on idleness.
//!
//! # Management Protocol (Port 8003)
//!
//! The management protocol handles coordination between Core and Environment.
//!
//! | Operation | Description |
//! |-----------|-------------|
//! | `HealthCheck` | Server health, version, uptime, active instance count |
//! | `SendSignal` | Deliver cancel/pause/resume signal to instance |
//! | `GetInstanceStatus` | Query instance status (proxied from Environment) |
//! | `ListInstances` | List instances with filtering and pagination |
//!
//! Note: Start/stop operations are handled by Environment, not Core.
//!
//! # Instance Status State Machine
//!
//! ```text
//!                     ┌─────────┐
//!                     │ PENDING │
//!                     └────┬────┘
//!                          │ register
//!                          ▼
//!                     ┌─────────┐
//!          ┌──────────│ RUNNING │──────────┐
//!          │          └────┬────┘          │
//!          │               │               │
//!     pause│          sleep│          cancel
//!          │               │               │
//!          ▼               ▼               ▼
//!     ┌──────────┐   ┌──────────┐   ┌───────────┐
//!     │SUSPENDED │   │SUSPENDED │   │ CANCELLED │
//!     └────┬─────┘   └────┬─────┘   └───────────┘
//!          │               │
//!     resume│          wake│
//!          │               │
//!          └───────┬───────┘
//!                  │
//!                  ▼
//!             ┌─────────┐
//!             │ RUNNING │──────────┬──────────┐
//!             └─────────┘          │          │
//!                             complete      fail
//!                                  │          │
//!                                  ▼          ▼
//!                            ┌───────────┐ ┌────────┐
//!                            │ COMPLETED │ │ FAILED │
//!                            └───────────┘ └────────┘
//! ```
//!
//! ## Status Descriptions
//!
//! | Status | Description |
//! |--------|-------------|
//! | `PENDING` | Instance created but not yet registered |
//! | `RUNNING` | Instance is actively executing |
//! | `SUSPENDED` | Instance paused (by signal) or sleeping (durable sleep) |
//! | `COMPLETED` | Instance finished successfully |
//! | `FAILED` | Instance failed with error |
//! | `CANCELLED` | Instance was cancelled via signal |
//!
//! # Configuration
//!
//! Configuration is loaded from environment variables:
//!
//! | Variable | Required | Default | Description |
//! |----------|----------|---------|-------------|
//! | `RUNTARA_DATABASE_URL` | Yes | - | PostgreSQL connection string |
//! | `RUNTARA_QUIC_PORT` | No | `8001` | Instance QUIC server port |
//! | `RUNTARA_ADMIN_PORT` | No | `8003` | Management QUIC server port |
//! | `RUNTARA_MAX_CONCURRENT_INSTANCES` | No | `32` | Maximum concurrent instances |
//!
//! # Modules
//!
//! - [`config`]: Server configuration from environment variables
//! - [`db`]: PostgreSQL persistence layer for instances, checkpoints, events, signals
//! - [`error`]: Error types with RPC error code mapping
//! - [`instance_handlers`]: Instance protocol request handlers
//! - [`management_handlers`]: Management protocol request handlers
//! - [`server`]: QUIC server implementations

#![deny(missing_docs)]

/// Server configuration loaded from environment variables.
pub mod config;

/// PostgreSQL database operations for instances, checkpoints, events, and signals.
pub mod db;

/// Error types for Core operations with RPC error code mapping.
pub mod error;

/// Instance protocol handlers (registration, checkpoints, events, signals).
pub mod instance_handlers;

/// Management protocol handlers (health, signals, status queries).
pub mod management_handlers;

/// QUIC server implementations for instance and management protocols.
pub mod server;
