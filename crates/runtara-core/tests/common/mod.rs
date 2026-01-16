// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Common test infrastructure for runtara-core E2E tests.
//!
//! Provides TestContext for setting up database, server, and client connections.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use runtara_core::instance_handlers::InstanceHandlerState;
use runtara_core::migrations::POSTGRES as MIGRATOR;
use runtara_core::persistence::{Persistence, PostgresPersistence};
use runtara_protocol::client::{RuntaraClient, RuntaraClientConfig};

/// Test context that manages database, server, and client for E2E tests.
pub struct TestContext {
    pub pool: PgPool,
    pub persistence: Arc<PostgresPersistence>,
    pub instance_client: RuntaraClient,
    pub instance_server_addr: SocketAddr,
}

impl TestContext {
    /// Create a new test context.
    ///
    /// This sets up:
    /// 1. Database connection from TEST_RUNTARA_DATABASE_URL
    /// 2. Instance QUIC server on an available port
    /// 3. QUIC client connected to the server
    /// 4. Persistence layer for direct database operations
    pub async fn new() -> Result<Self, String> {
        // 1. Get database URL from environment
        let database_url = std::env::var("TEST_RUNTARA_DATABASE_URL")
            .map_err(|_| "TEST_RUNTARA_DATABASE_URL not set")?;

        // 2. Connect to test database
        let pool = PgPool::connect(&database_url)
            .await
            .map_err(|e| format!("Failed to connect to database: {}", e))?;

        // 2b. Run migrations to ensure schema exists
        MIGRATOR
            .run(&pool)
            .await
            .map_err(|e| format!("Failed to run migrations: {}", e))?;

        // 3. Find available port for server
        let listener1 = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("Failed to bind listener: {}", e))?;
        let instance_server_addr = listener1
            .local_addr()
            .map_err(|e| format!("Failed to get local addr: {}", e))?;
        drop(listener1);

        // 4. Create handler state with persistence
        let persistence = Arc::new(PostgresPersistence::new(pool.clone()));
        let instance_state = Arc::new(InstanceHandlerState::new(persistence.clone()));

        // 5. Start instance server in background
        let instance_server_state = instance_state.clone();
        let instance_bind_addr = instance_server_addr;
        tokio::spawn(async move {
            if let Err(e) =
                runtara_core::server::run_instance_server(instance_bind_addr, instance_server_state)
                    .await
            {
                eprintln!("Test instance server error: {}", e);
            }
        });

        // 6. Wait for server to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 7. Create instance client
        let instance_client = RuntaraClient::new(RuntaraClientConfig {
            server_addr: instance_server_addr,
            dangerous_skip_cert_verification: true,
            ..Default::default()
        })
        .map_err(|e| format!("Failed to create QUIC client: {}", e))?;

        Ok(Self {
            pool,
            persistence,
            instance_client,
            instance_server_addr,
        })
    }

    /// Create a test instance in the database (simulating launcher).
    pub async fn create_test_instance(&self, instance_id: &Uuid, tenant_id: &str) {
        sqlx::query(
            r#"
            INSERT INTO instances (instance_id, tenant_id, status)
            VALUES ($1, $2, 'pending')
            ON CONFLICT (instance_id) DO NOTHING
            "#,
        )
        .bind(instance_id.to_string())
        .bind(tenant_id)
        .execute(&self.pool)
        .await
        .expect("Failed to create test instance");
    }

    /// Create a test instance with status running.
    pub async fn create_running_instance(&self, instance_id: &Uuid, tenant_id: &str) {
        self.create_test_instance(instance_id, tenant_id).await;
        sqlx::query(
            r#"
            UPDATE instances SET status = 'running', started_at = NOW()
            WHERE instance_id = $1
            "#,
        )
        .bind(instance_id.to_string())
        .execute(&self.pool)
        .await
        .expect("Failed to update instance to running");
    }

    /// Get instance status from database.
    pub async fn get_instance_status(&self, instance_id: &Uuid) -> Option<String> {
        let row: Option<(String,)> =
            sqlx::query_as(r#"SELECT status::text FROM instances WHERE instance_id = $1"#)
                .bind(instance_id.to_string())
                .fetch_optional(&self.pool)
                .await
                .ok()?;
        row.map(|r| r.0)
    }

    /// Get instance checkpoint_id from database.
    pub async fn get_instance_checkpoint(&self, instance_id: &Uuid) -> Option<String> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as(r#"SELECT checkpoint_id FROM instances WHERE instance_id = $1"#)
                .bind(instance_id.to_string())
                .fetch_optional(&self.pool)
                .await
                .ok()?;
        row.and_then(|r| r.0)
    }

    /// Check if a wake entry exists for an instance.
    pub async fn has_wake_entry(&self, instance_id: &Uuid) -> bool {
        let row: Option<(i64,)> =
            sqlx::query_as(r#"SELECT COUNT(*) FROM wake_queue WHERE instance_id = $1"#)
                .bind(instance_id.to_string())
                .fetch_optional(&self.pool)
                .await
                .ok()
                .flatten();
        row.map(|r| r.0 > 0).unwrap_or(false)
    }

    /// Check if a pending signal exists for an instance.
    pub async fn has_pending_signal(&self, instance_id: &Uuid) -> bool {
        let row: Option<(i64,)> = sqlx::query_as(
            r#"SELECT COUNT(*) FROM pending_signals WHERE instance_id = $1 AND acknowledged_at IS NULL"#,
        )
        .bind(instance_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten();
        row.map(|r| r.0 > 0).unwrap_or(false)
    }

    /// Get pending signal type from database.
    pub async fn get_pending_signal_type(&self, instance_id: &Uuid) -> Option<String> {
        let row: Option<(String,)> = sqlx::query_as(
            r#"SELECT signal_type::text FROM pending_signals WHERE instance_id = $1 AND acknowledged_at IS NULL"#,
        )
        .bind(instance_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .ok()?;
        row.map(|r| r.0)
    }

    /// Send a signal to an instance via the persistence layer.
    /// Returns Ok(true) if successful, Ok(false) if instance not found or in terminal state.
    pub async fn send_signal(
        &self,
        instance_id: &Uuid,
        signal_type: &str,
        payload: &[u8],
    ) -> Result<bool, String> {
        // Check instance exists and is not in terminal state
        let instance = self
            .persistence
            .get_instance(&instance_id.to_string())
            .await
            .map_err(|e| format!("Failed to get instance: {}", e))?;

        let instance = match instance {
            Some(i) => i,
            None => return Ok(false),
        };

        // Check terminal states
        match instance.status.as_str() {
            "completed" | "failed" | "cancelled" => {
                return Err(format!(
                    "Instance is in terminal state: {}",
                    instance.status
                ));
            }
            _ => {}
        }

        // Insert signal via persistence
        self.persistence
            .insert_signal(&instance_id.to_string(), signal_type, payload)
            .await
            .map_err(|e| format!("Failed to insert signal: {}", e))?;

        Ok(true)
    }

    /// Clean up all test data for a specific instance.
    pub async fn cleanup_instance(&self, instance_id: &Uuid) {
        // Delete in order respecting foreign keys
        sqlx::query("DELETE FROM pending_signals WHERE instance_id = $1")
            .bind(instance_id.to_string())
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM instance_events WHERE instance_id = $1")
            .bind(instance_id.to_string())
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM wake_queue WHERE instance_id = $1")
            .bind(instance_id.to_string())
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM checkpoints WHERE instance_id = $1")
            .bind(instance_id.to_string())
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM instances WHERE instance_id = $1")
            .bind(instance_id.to_string())
            .execute(&self.pool)
            .await
            .ok();
    }

    /// Clean up all test data.
    pub async fn cleanup(&self) {
        sqlx::query("DELETE FROM pending_signals")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM instance_events")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM wake_queue")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM checkpoints")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM containers")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM instances")
            .execute(&self.pool)
            .await
            .ok();
    }
}

/// Helper macro to skip tests if TEST_RUNTARA_DATABASE_URL is not set.
#[macro_export]
macro_rules! skip_if_no_db {
    () => {
        if std::env::var("TEST_RUNTARA_DATABASE_URL").is_err() {
            eprintln!("Skipping test: TEST_RUNTARA_DATABASE_URL not set");
            return;
        }
    };
}

// ============================================================================
// Instance Protocol Helpers
// ============================================================================

pub fn wrap_register(
    req: runtara_protocol::instance_proto::RegisterInstanceRequest,
) -> runtara_protocol::instance_proto::RpcRequest {
    runtara_protocol::instance_proto::RpcRequest {
        request: Some(
            runtara_protocol::instance_proto::rpc_request::Request::RegisterInstance(req),
        ),
    }
}

pub fn wrap_checkpoint(
    req: runtara_protocol::instance_proto::CheckpointRequest,
) -> runtara_protocol::instance_proto::RpcRequest {
    runtara_protocol::instance_proto::RpcRequest {
        request: Some(runtara_protocol::instance_proto::rpc_request::Request::Checkpoint(req)),
    }
}

pub fn wrap_get_checkpoint(
    req: runtara_protocol::instance_proto::GetCheckpointRequest,
) -> runtara_protocol::instance_proto::RpcRequest {
    runtara_protocol::instance_proto::RpcRequest {
        request: Some(runtara_protocol::instance_proto::rpc_request::Request::GetCheckpoint(req)),
    }
}

pub fn wrap_sleep(
    req: runtara_protocol::instance_proto::SleepRequest,
) -> runtara_protocol::instance_proto::RpcRequest {
    runtara_protocol::instance_proto::RpcRequest {
        request: Some(runtara_protocol::instance_proto::rpc_request::Request::Sleep(req)),
    }
}

pub fn wrap_get_instance_status(
    req: runtara_protocol::instance_proto::GetInstanceStatusRequest,
) -> runtara_protocol::instance_proto::RpcRequest {
    runtara_protocol::instance_proto::RpcRequest {
        request: Some(
            runtara_protocol::instance_proto::rpc_request::Request::GetInstanceStatus(req),
        ),
    }
}

pub fn wrap_poll_signals(
    req: runtara_protocol::instance_proto::PollSignalsRequest,
) -> runtara_protocol::instance_proto::RpcRequest {
    runtara_protocol::instance_proto::RpcRequest {
        request: Some(runtara_protocol::instance_proto::rpc_request::Request::PollSignals(req)),
    }
}

pub fn wrap_signal_ack(
    req: runtara_protocol::instance_proto::SignalAck,
) -> runtara_protocol::instance_proto::RpcRequest {
    runtara_protocol::instance_proto::RpcRequest {
        request: Some(runtara_protocol::instance_proto::rpc_request::Request::SignalAck(req)),
    }
}

pub fn wrap_instance_event(
    req: runtara_protocol::instance_proto::InstanceEvent,
) -> runtara_protocol::instance_proto::RpcRequest {
    runtara_protocol::instance_proto::RpcRequest {
        request: Some(runtara_protocol::instance_proto::rpc_request::Request::InstanceEvent(req)),
    }
}
