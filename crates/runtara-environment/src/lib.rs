// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Environment - Instance Lifecycle Management
//!
//! This crate provides the control plane for managing workflow instances.
//! It handles image registration, instance lifecycle, workflow execution,
//! and wake scheduling for durable sleeps.
//!
//! # A library, not a service
//!
//! Environment is transport-free. Its protocol is a set of async functions in
//! [`handlers`] over a shared [`handlers::EnvironmentHandlerState`], and
//! [`runtime::EnvironmentRuntime`] owns the background workers — the wake
//! scheduler, heartbeat monitor and the cleanup trio — and nothing else.
//!
//! The management API is served over HTTP by `runtara-server`
//! (`runtara_server::environment_api`), which owns that listener and its
//! lifecycle. A host that wants the protocol without a socket calls the
//! [`handlers`] functions directly.
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
//! │                   runtara-environment (This Crate)                       │
//! │                         Port 8002                                        │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐     │
//! │  │   Image     │  │  Instance   │  │    Wake     │  │  Workflow   │     │
//! │  │  Registry   │  │  Lifecycle  │  │  Scheduler  │  │    Runner   │     │
//! │  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘     │
//! └─────────────────────────────────────────────────────────────────────────┘
//!           │                 │                              │
//!           │                 │ Proxy signals                │ Spawn
//!           │                 ▼                              ▼
//!           │       ┌───────────────────┐        ┌─────────────────────────┐
//!           │       │   runtara-core    │◄───────│   Workflow Instances    │
//!           │       │   Port 8001/8003  │        │  (in-process wasmtime)  │
//!           │       └───────────────────┘        └─────────────────────────┘
//!           │                 │
//!           ▼                 ▼
//! ┌───────────────────────────────────────────────────────────────────────┐
//! │                           PostgreSQL                                   │
//! │              (Images, Instances, Wake Queue)                          │
//! └───────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # HTTP Server (Environment Protocol - Port 8002)
//!
//! Environment exposes an HTTP server for all management operations.
//! External clients (via runtara-management-sdk) connect here.
//!
//! ## Image Operations
//!
//! | Operation | Description |
//! |-----------|-------------|
//! | `RegisterImage` | Register a new image (single-frame upload < 16MB) |
//! | `RegisterImageStream` | Register a large image via streaming upload |
//! | `ListImages` | List images with optional tenant filter and pagination |
//! | `GetImage` | Get image details by ID |
//! | `DeleteImage` | Delete an image |
//!
//! ## Instance Operations
//!
//! | Operation | Description |
//! |-----------|-------------|
//! | `StartInstance` | Start a new instance from an image |
//! | `StopInstance` | Stop a running instance with grace period |
//! | `ResumeInstance` | Resume a suspended instance |
//! | `GetInstanceStatus` | Query instance status |
//! | `ListInstances` | List instances with filtering and pagination |
//!
//! ## Signal Operations
//!
//! | Operation | Description |
//! |-----------|-------------|
//! | `SendSignal` | Send cancel/pause/resume signal to instance |
//!
//! Signals are proxied to runtara-core which stores them for the instance.
//!
//! # Runner
//!
//! Workflows execute on `EmbeddedWasmRunner`, an in-process wasmtime engine.
//! It is the only backend; `MockRunner` exists for tests. The `runner::Runner`
//! trait keeps that seam.
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
//! # Configuration
//!
//! Environment reads nothing from the environment on its own: a host supplies
//! the pool, the runner, the data directory and the rest through
//! [`runtime::EnvironmentRuntimeBuilder`]. Two variables are still consulted
//! deeper in the crate — `RUNTARA_SKIP_CERT_VERIFICATION`, forwarded to guests,
//! and the `*_CLEANUP_ENABLED` opt-outs read by the background workers.
//!
//! # Modules
//!
//! - [`config`]: Shared configuration parsing helpers
//! - [`db`]: PostgreSQL persistence for images, instances, and wake queue
//! - [`error`]: Error types for Environment operations
//! - [`handlers`]: Environment protocol request handlers
//! - [`image_registry`]: Image storage and retrieval
//! - [`container_registry`]: Running container tracking
//! - [`instance_output`]: Instance output types (legacy, used by SDK)
//! - [`runner`]: Container/process execution backends
//! - [`wake_scheduler`]: Durable sleep wake scheduling

#![deny(missing_docs)]

/// Database migrations for runtara-environment.
///
/// Environment extends runtara-core's schema. Calling `migrations::run()` will
/// apply both core and environment migrations in the correct order.
///
/// ```ignore
/// use runtara_environment::migrations;
///
/// let pool = PgPool::connect(&database_url).await?;
/// migrations::run(&pool).await?;
/// ```
pub mod migrations;

/// Server configuration loaded from environment variables.
pub mod config;

/// Typed, bounded active-execution timeout policy.
pub mod execution_timeout;

/// PostgreSQL database operations for images, instances, and wake queue.
pub mod db;

/// Durable queue state and transactions for runner launch generations.
pub mod launch_queue;

/// Background dispatcher for durable runner launch generations.
pub mod launch_dispatcher;

/// Error types for Environment operations.
pub mod error;

/// Environment protocol request handlers.
pub mod handlers;

/// Image storage and retrieval.
pub mod image_registry;

/// Running container tracking and management.
pub mod container_registry;

/// Instance output types (legacy, used by SDK).
pub mod instance_output;

/// In-process WASM execution backend.
pub mod runner;

/// Durable sleep wake scheduling.
pub mod wake_scheduler;

/// Background worker for cleaning up old run directories.
pub mod cleanup_worker;

/// Background worker for cleaning up old database records.
pub mod db_cleanup_worker;

/// Background worker for cleaning up unused images.
pub mod image_cleanup_worker;

/// Background worker for detecting and failing stale instances.
pub mod heartbeat_monitor;

/// Automatic recovery of instances killed by an Environment restart.
pub mod recovery;

/// Embeddable runtime for runtara-environment.
pub mod runtime;

/// Persistence-backed implementation of the component host's `RuntimeHost` —
/// the native replacement for the composed guest runtime's HTTP loopback.
pub mod runtime_host;

/// Shared Postgres fixtures for this crate's database-backed unit tests.
#[cfg(all(test, feature = "db-integration-tests"))]
pub(crate) mod test_support;

pub use error::Error;
