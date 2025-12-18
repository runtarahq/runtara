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
use runtara_core::management_handlers::ManagementHandlerState;
use runtara_protocol::client::{RuntaraClient, RuntaraClientConfig};

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Test context that manages database, server, and client for E2E tests.
pub struct TestContext {
    pub pool: PgPool,
    pub instance_client: RuntaraClient,
    pub management_client: RuntaraClient,
    pub instance_server_addr: SocketAddr,
    pub management_server_addr: SocketAddr,
}

impl TestContext {
    /// Create a new test context.
    ///
    /// This sets up:
    /// 1. Database connection from TEST_DATABASE_URL
    /// 2. Instance QUIC server on an available port
    /// 3. Management QUIC server on an available port
    /// 4. QUIC clients connected to both servers
    pub async fn new() -> Option<Self> {
        // 1. Get database URL from environment
        let database_url = std::env::var("TEST_DATABASE_URL").ok()?;

        // 2. Connect to test database
        let pool = PgPool::connect(&database_url).await.ok()?;

        // 2b. Run migrations to ensure schema exists
        MIGRATOR.run(&pool).await.ok()?;

        // 3. Find available ports for both servers
        let listener1 = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
        let instance_server_addr = listener1.local_addr().ok()?;
        drop(listener1);

        let listener2 = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
        let management_server_addr = listener2.local_addr().ok()?;
        drop(listener2);

        // 4. Create handler states
        let instance_state = Arc::new(InstanceHandlerState::new(pool.clone()));
        let management_state = Arc::new(ManagementHandlerState::new(pool.clone()));

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

        // 6. Start management server in background
        let management_server_state = management_state.clone();
        let management_bind_addr = management_server_addr;
        tokio::spawn(async move {
            if let Err(e) = runtara_core::server::run_management_server(
                management_bind_addr,
                management_server_state,
            )
            .await
            {
                eprintln!("Test management server error: {}", e);
            }
        });

        // 7. Wait for servers to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 8. Create instance client
        let instance_client = RuntaraClient::new(RuntaraClientConfig {
            server_addr: instance_server_addr,
            dangerous_skip_cert_verification: true,
            ..Default::default()
        })
        .ok()?;

        // 9. Create management client
        let management_client = RuntaraClient::new(RuntaraClientConfig {
            server_addr: management_server_addr,
            dangerous_skip_cert_verification: true,
            ..Default::default()
        })
        .ok()?;

        Some(Self {
            pool,
            instance_client,
            management_client,
            instance_server_addr,
            management_server_addr,
        })
    }

    /// Create a test instance in the database (simulating launcher).
    pub async fn create_test_instance(&self, instance_id: &Uuid, tenant_id: &str) {
        let definition_id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO instances (instance_id, tenant_id, definition_id, definition_version, status)
            VALUES ($1, $2, $3, 1, 'pending')
            ON CONFLICT (instance_id) DO NOTHING
            "#,
        )
        .bind(instance_id.to_string())
        .bind(tenant_id)
        .bind(definition_id) // definition_id is still UUID type
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

/// Helper macro to skip tests if TEST_DATABASE_URL is not set.
#[macro_export]
macro_rules! skip_if_no_db {
    () => {
        if std::env::var("TEST_DATABASE_URL").is_err() {
            eprintln!("Skipping test: TEST_DATABASE_URL not set");
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

// ============================================================================
// Management Protocol Helpers
// ============================================================================

pub fn wrap_health_check(
    req: runtara_protocol::management_proto::HealthCheckRequest,
) -> runtara_protocol::management_proto::RpcRequest {
    runtara_protocol::management_proto::RpcRequest {
        request: Some(runtara_protocol::management_proto::rpc_request::Request::HealthCheck(req)),
    }
}

pub fn wrap_send_signal(
    req: runtara_protocol::management_proto::SendSignalRequest,
) -> runtara_protocol::management_proto::RpcRequest {
    runtara_protocol::management_proto::RpcRequest {
        request: Some(runtara_protocol::management_proto::rpc_request::Request::SendSignal(req)),
    }
}

pub fn wrap_mgmt_get_instance_status(
    req: runtara_protocol::management_proto::GetInstanceStatusRequest,
) -> runtara_protocol::management_proto::RpcRequest {
    runtara_protocol::management_proto::RpcRequest {
        request: Some(
            runtara_protocol::management_proto::rpc_request::Request::GetInstanceStatus(req),
        ),
    }
}

pub fn wrap_list_instances(
    req: runtara_protocol::management_proto::ListInstancesRequest,
) -> runtara_protocol::management_proto::RpcRequest {
    runtara_protocol::management_proto::RpcRequest {
        request: Some(runtara_protocol::management_proto::rpc_request::Request::ListInstances(req)),
    }
}
