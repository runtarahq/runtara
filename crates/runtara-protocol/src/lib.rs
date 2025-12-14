// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtara Protocol - QUIC + Protobuf communication layer
//!
//! This crate provides the wire protocol for communication between:
//! - Instances and runtara-core (instance protocol)
//! - External clients and runtara-core (management protocol)
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    runtara-protocol                         │
//! ├─────────────────────────────────────────────────────────────┤
//! │  RPC Layer: Request/Response + Bidirectional Streaming      │
//! ├─────────────────────────────────────────────────────────────┤
//! │  Serialization: Protobuf (prost)                            │
//! ├─────────────────────────────────────────────────────────────┤
//! │  Transport: QUIC (quinn)                                    │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Protocols
//!
//! ## Instance Protocol (`instance_proto`)
//!
//! Used by instances to communicate with runtara-core:
//! - Registration, checkpointing, sleep/wake
//! - Events (started, progress, completed, failed)
//! - Signal polling and acknowledgment
//!
//! ## Management Protocol (`management_proto`)
//!
//! Used by external clients to manage runtara-core:
//! - Health checks
//! - Send signals to instances
//! - Query instance status
//! - List instances
//!
//! # Usage
//!
//! ## Instance Client (scenarios)
//!
//! ```ignore
//! use runtara_protocol::{RuntaraClient, RuntaraClientConfig, instance_proto};
//!
//! let client = RuntaraClient::localhost()?;
//! client.connect().await?;
//!
//! let request = instance_proto::RegisterInstanceRequest {
//!     instance_id: "my-instance".to_string(),
//!     tenant_id: "tenant-1".to_string(),
//!     checkpoint_id: None,
//! };
//!
//! let rpc_request = instance_proto::RpcRequest {
//!     request: Some(instance_proto::rpc_request::Request::RegisterInstance(request)),
//! };
//!
//! let response: instance_proto::RpcResponse = client.request(&rpc_request).await?;
//! ```
//!
//! ## Management Client
//!
//! ```ignore
//! use runtara_protocol::{RuntaraClient, management_proto};
//!
//! let client = RuntaraClient::localhost()?;
//! client.connect().await?;
//!
//! let request = management_proto::HealthCheckRequest {};
//! let rpc_request = management_proto::RpcRequest {
//!     request: Some(management_proto::rpc_request::Request::HealthCheck(request)),
//! };
//!
//! let response: management_proto::RpcResponse = client.request(&rpc_request).await?;
//! ```

pub mod client;
pub mod frame;
pub mod server;

// Re-export generated protobuf types for instance protocol
pub mod instance_proto {
    include!(concat!(env!("OUT_DIR"), "/runtara.instance.rs"));
}

// Re-export generated protobuf types for management protocol (internal API for Core)
pub mod management_proto {
    include!(concat!(env!("OUT_DIR"), "/runtara.management.rs"));
}

// Re-export generated protobuf types for environment protocol (main management API)
pub mod environment_proto {
    include!(concat!(env!("OUT_DIR"), "/runtara.environment.rs"));
}

// Re-export main types
pub use client::{ClientError, RuntaraClient, RuntaraClientConfig};
pub use frame::{Frame, FrameError, FramedStream, MessageType};
pub use server::{
    ConnectionHandler, RuntaraServer, RuntaraServerConfig, ServerError, StreamHandler,
};
