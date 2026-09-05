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
//! Environment is transport-free: no listener, no binary, no wire format.
//! Its protocol is a set of async functions in [`handlers`] over a shared
//! [`handlers::EnvironmentHandlerState`], and [`runtime::EnvironmentRuntime`]
//! owns the background workers — the wake scheduler, the launch dispatcher,
//! the heartbeat monitor and the cleanup trio — and nothing else.
//!
//! `runtara-server` is the only consumer. It embeds `EnvironmentRuntime` in
//! the same process, on the same tokio runtime and the same connection pool,
//! and calls the [`handlers`] functions directly.
//!
//! # Architecture
//!
//! ```text
//!                        runtara-server (the only host)
//!                                    │
//!                    calls handlers:: functions in-process
//!                                    │
//!                                    ▼
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                   runtara-environment (this crate)                      │
//! │                                                                         │
//! │   ┌───────────┐  ┌───────────┐  ┌────────────┐  ┌──────────────────┐    │
//! │   │   Image   │  │ Instance  │  │   Launch   │  │ Wake scheduler + │    │
//! │   │ registry  │  │ lifecycle │  │   queue    │  │ cleanup workers  │    │
//! │   └───────────┘  └───────────┘  └─────┬──────┘  └──────────────────┘    │
//! └───────────────────────────────────────┼─────────────────────────────────┘
//!            │                            │ hands one generation to
//!            │ Arc<dyn Persistence>       ▼ the runner
//!            │                  ┌──────────────────────────┐
//!            │                  │  EmbeddedWasmRunner      │
//!            │                  │  (in-process wasmtime)   │
//!            │                  └────────────┬─────────────┘
//!            │                               │ host imports, satisfied by
//!            │                               ▼ runtime_host (no HTTP)
//!            │                  ┌──────────────────────────┐
//!            ├─────────────────►│      runtara-core        │
//!            │                  └────────────┬─────────────┘
//!            ▼                               ▼
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                              PostgreSQL                                 │
//! │        images · instances · instance_launches · container_registry      │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Operations
//!
//! Images: register (single-frame or streamed), list, get, delete.
//!
//! Instances: start, stop with a grace period, resume, query status, list,
//! count by status. Cancel/pause/resume signals are written to Core, which
//! stores them for the running instance to consume at its next checkpoint.
//!
//! Reads served on Core's behalf: checkpoints, events, step summaries, scope
//! ancestors and per-tenant metrics.
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
//! A host supplies the pool, the runner, the persistence layer, the data
//! directory and the execution-timeout policy through
//! [`runtime::EnvironmentRuntimeBuilder`]. Everything the builder does not
//! name is read from the process environment, and the two do not overlap: a
//! builder setter always wins over an environment variable for the same
//! setting, because the variables below are only consulted while constructing
//! the default the setter replaces.
//!
//! Wake scheduler (read at `EnvironmentRuntimeBuilder::default()`, each
//! overridable by the matching setter): `RUNTARA_WAKE_POLL_INTERVAL_MS`,
//! `RUNTARA_WAKE_BATCH_SIZE`, `RUNTARA_WAKE_CONCURRENCY`,
//! `RUNTARA_WAKE_CLAIM_LEASE_SECS`.
//!
//! Runner (read inside [`runner::build_runner`], with no builder path):
//! `DATA_DIR`, `EXECUTION_TIMEOUT_SECS`, `RUNTARA_SKIP_CERT_VERIFICATION`,
//! `RUNTARA_CONNECTION_SERVICE_URL` (falling back to
//! `CONNECTION_SERVICE_URL`), `RUNTARA_SDK_BACKEND`,
//! `RUNTARA_INSTANCE_MEMORY_MAX_BYTES`, `RUNTARA_MAX_CONCURRENT_RUNS`,
//! `RUNTARA_PREPARATION_CONCURRENCY`, `RUNTARA_PRECOMPILE_CHILD_CONCURRENCY`.
//! `RUNTARA_RUNNER` is accepted and ignored with a warning.
//!
//! Background workers: the `RUNTARA_{RUN_DIR,DB,IMAGE}_CLEANUP_*` families,
//! `RUNTARA_EVENT_DEBUG_RETENTION_HOURS`, `RUNTARA_AUTO_RECOVER` and
//! `RUNTARA_MAX_AUTO_RESTARTS`. Every `*_ENABLED` switch and
//! `RUNTARA_AUTO_RECOVER` share `runtara_core::config::parse_enabled_env`, so
//! all of them answer to `false`/`0`/`no`/`off`/`disabled` and default to on.
//!
//! # Modules
//!
//! - [`handlers`]: the protocol — lifecycle operations and Core-backed reads
//! - [`launch_queue`] / [`launch_dispatcher`]: the durable launch generation
//!   state machine, and the only worker that hands one to a runner
//! - [`runner`]: the in-process wasmtime execution backend and its trait
//! - [`runtime_host`]: guest host imports, served straight from persistence
//! - [`db`] / [`image_registry`] / [`container_registry`]: PostgreSQL access
//! - [`wake_scheduler`] / [`heartbeat_monitor`] / [`recovery`]: reconcilers
//! - [`cleanup_worker`] / [`db_cleanup_worker`] / [`image_cleanup_worker`]:
//!   retention
//! - [`runtime`]: the builder that starts and drains all of the above

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

/// In-process WASM execution backend.
pub mod runner;

/// The workflow event vocabulary handed to `runtara-core` per query.
pub mod step_vocabulary;

/// Durable sleep wake scheduling.
pub mod wake_scheduler;

/// Background worker for cleaning up old run directories.
pub mod cleanup_worker;

/// Background worker for cleaning up old database records.
pub mod db_cleanup_worker;
pub mod metrics;
pub mod recovery_marks;

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
