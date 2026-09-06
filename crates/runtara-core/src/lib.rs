// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Core - Durable Execution Engine
//!
//! This crate provides the execution engine for durable workflows. It manages checkpoints,
//! signals, and instance events, persisting state through a host-provided backend for crash resilience.
//!
//! # A library, not a service
//!
//! Core is transport-free. It exposes the instance protocol as plain async
//! functions over an [`Arc<dyn Persistence>`](persistence::Persistence) — no
//! HTTP, no sockets, no binary. Hosts embed it two ways:
//!
//! - **In-process**: call the [`instance_handlers`] functions directly. This is
//!   what `runtara-environment` does for workflows composed against
//!   `runtara:workflow-runtime/runtime` as a host import, which is the default.
//! - **Over HTTP**: `runtara-server` wraps the same functions in an axum router
//!   (`runtara_server::core_runtime`) and serves them on the instance port for
//!   guests that reach core through the SDK's HTTP backend.
//!
//! Whichever a host picks, the semantics below are the handlers', not the
//! transport's.
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
//! │                           Port 8002                                      │
//! └─────────────────────────────────────────────────────────────────────────┘
//!           │                                              │
//!           │ Shared Persistence                           │ Spawns
//!           ▼                                              ▼
//! ┌───────────────────────┐                    ┌─────────────────────────────┐
//! │    runtara-core       │◄───────────────────│     Workflow Instances      │
//! │  (This Crate)         │  Instance Protocol │   (using runtara-sdk)       │
//! │  Checkpoints/Signals  │  (in-process, or   │                             │
//! │  (library only)       │   HTTP via server) └─────────────────────────────┘
//! └───────────────────────┘
//!           │
//!           ▼
//! ┌───────────────────────┐
//! │  Persistence backend  │
//! │  (Durable Storage)    │
//! └───────────────────────┘
//! ```
//!
//! # Instance Protocol
//!
//! The instance protocol handles all communication between workflow instances and Core.
//! Instances use `runtara-sdk`, which wraps this protocol.
//!
//! `runtara-server` exposes it over HTTP on the instance port (8001 by
//! default); environment's in-process runner calls the same handlers directly.
//!
//! ## Operations
//!
//! | Operation | Description |
//! |-----------|-------------|
//! | `RegisterInstance` | Self-register on startup, optionally resume from checkpoint |
//! | `Checkpoint` | Save state (or return existing if checkpoint_id exists) + signal delivery |
//! | `GetCheckpoint` | Read-only checkpoint lookup |
//! | `Sleep` | Durable sleep - persists a wake deadline |
//! | `InstanceEvent` | Fire-and-forget events (heartbeat, completed, failed, suspended) |
//! | `GetInstanceStatus` | Query instance status |
//! | `PollSignals` | Poll for pending cancel/pause/shutdown signals |
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
//! ## Durable Sleep
//!
//! Wake scheduling uses the instance record’s `sleep_until` timestamp.
//! The sleep handler persists a checkpoint and waits, delivering interrupting signals.
//! Environment's wake scheduler polls for sleeping instances and relaunches them
//! when their wake time arrives. On resume, the SDK calculates remaining sleep time.
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
//! Hosts pass settings explicitly; core never reads the process environment.
//! [`instance_handlers::InstanceHandlerState::with_limits`] accepts a concurrency
//! cap (`0` disables it). Only running instances count, and resumes are exempt.
//! Transport settings, shutdown grace, and deployment configuration belong to
//! the host application.
//!
//! # Modules
//!
//! - [`domain`] — Typed instance statuses, signals, and timeline events
//! - [`persistence`] — Storage contracts for instances, checkpoints, events, and signals
//! - [`error`] — Error types and transport-independent classifications
//! - [`instance_handlers`] — Instance protocol request handlers
//!
//! Hosts supply a [`persistence::Persistence`] implementation. Backends own
//! storage schemas, encodings, queries, and setup. Core owns execution semantics.

#![deny(missing_docs)]

/// Execution domain types independent of storage and transport.
pub mod domain;

/// Pure lifecycle policy shared by storage backends and execution hosts.
pub mod lifecycle;

/// Persistence layer for instances, checkpoints, events, and signals.
pub mod persistence;

/// Error types for Core operations with RPC error code mapping.
pub mod error;

/// Instance protocol handlers (registration, checkpoints, events, signals).
pub mod instance_handlers;
