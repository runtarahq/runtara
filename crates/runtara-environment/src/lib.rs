// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Environment - Instance Lifecycle Management
//!
//! This crate provides the control plane for managing workflow instances.
//! It handles image registration, instance lifecycle, container execution,
//! and wake scheduling for durable sleeps.
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
//! │  │   Image     │  │  Instance   │  │    Wake     │  │  Container  │     │
//! │  │  Registry   │  │  Lifecycle  │  │  Scheduler  │  │   Runner    │     │
//! │  └─────────────┘  └─────────────┘  └─────────────┘  └─────────────┘     │
//! └─────────────────────────────────────────────────────────────────────────┘
//!           │                 │                              │
//!           │                 │ Proxy signals                │ Spawn
//!           │                 ▼                              ▼
//!           │       ┌───────────────────┐        ┌─────────────────────────┐
//!           │       │   runtara-core    │◄───────│   Workflow Instances    │
//!           │       │   Port 8001/8003  │        │   (OCI containers)      │
//!           │       └───────────────────┘        └─────────────────────────┘
//!           │                 │
//!           ▼                 ▼
//! ┌───────────────────────────────────────────────────────────────────────┐
//! │                           PostgreSQL                                   │
//! │              (Images, Instances, Wake Queue)                          │
//! └───────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # QUIC Server (Environment Protocol - Port 8002)
//!
//! Environment exposes a single QUIC server for all management operations.
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
//! # Runner Types
//!
//! Environment supports multiple runner backends for executing workflow binaries:
//!
//! | Runner | Description |
//! |--------|-------------|
//! | OCI (default) | Execute in OCI containers via runc |
//! | Native | Execute as direct processes (development) |
//! | Wasm | Execute as WebAssembly modules (planned) |
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
//! Configuration is loaded from environment variables:
//!
//! | Variable | Required | Default | Description |
//! |----------|----------|---------|-------------|
//! | `RUNTARA_ENVIRONMENT_DATABASE_URL` | Yes* | - | PostgreSQL connection string |
//! | `RUNTARA_DATABASE_URL` | Yes* | - | Fallback if above not set |
//! | `RUNTARA_ENV_QUIC_PORT` | No | `8002` | QUIC server port |
//! | `RUNTARA_CORE_ADDR` | No | `127.0.0.1:8001` | runtara-core address |
//! | `DATA_DIR` | No | `.data` | Data directory for images and bundles |
//! | `RUNTARA_SKIP_CERT_VERIFICATION` | No | `false` | Skip TLS verification |
//!
//! # Modules
//!
//! - [`config`]: Server configuration from environment variables
//! - [`db`]: PostgreSQL persistence for images, instances, and wake queue
//! - [`error`]: Error types for Environment operations
//! - [`handlers`]: Environment protocol request handlers
//! - [`image_registry`]: Image storage and retrieval
//! - [`container_registry`]: Running container tracking
//! - [`instance_output`]: Reading output.json from completed instances
//! - [`runner`]: Container/process execution backends
//! - [`server`]: QUIC server implementation
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

/// PostgreSQL database operations for images, instances, and wake queue.
pub mod db;

/// Error types for Environment operations.
pub mod error;

/// Environment protocol request handlers.
pub mod handlers;

/// Image storage and retrieval.
pub mod image_registry;

/// Running container tracking and management.
pub mod container_registry;

/// Reading and parsing instance output.json files.
pub mod instance_output;

/// Container/process execution backends (OCI, Native, Wasm).
pub mod runner;

/// QUIC server for the Environment protocol.
pub mod server;

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

/// Embeddable runtime for runtara-environment.
pub mod runtime;

pub use config::Config;
pub use error::Error;
