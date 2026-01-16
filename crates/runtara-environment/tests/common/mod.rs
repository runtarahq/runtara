// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Common test infrastructure for runtara-environment E2E tests.
//!
//! Provides TestContext for setting up database, server, and client connections.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use runtara_core::persistence::PostgresPersistence;
use sqlx::PgPool;
use uuid::Uuid;

use runtara_environment::handlers::EnvironmentHandlerState;
use runtara_environment::runner::MockRunner;
use runtara_environment::runner::Runner;
use runtara_protocol::client::{RuntaraClient, RuntaraClientConfig};

/// Test context that manages database, server, and client for E2E tests.
pub struct TestContext {
    pub pool: PgPool,
    pub client: RuntaraClient,
    pub server_addr: SocketAddr,
    pub data_dir: PathBuf,
    _temp_dir: tempfile::TempDir,
}

impl TestContext {
    /// Create a new test context.
    pub async fn new() -> Result<Self, String> {
        // Get database URL from environment
        let database_url = std::env::var("TEST_RUNTARA_DATABASE_URL")
            .map_err(|_| "TEST_RUNTARA_DATABASE_URL not set")?;

        // Connect to test database
        let pool = PgPool::connect(&database_url)
            .await
            .map_err(|e| format!("Failed to connect to database: {}", e))?;

        // Run migrations (core + environment)
        runtara_environment::migrations::run(&pool)
            .await
            .map_err(|e| format!("Failed to run migrations: {}", e))?;

        // Create temp directory for data
        let temp_dir =
            tempfile::TempDir::new().map_err(|e| format!("Failed to create temp dir: {}", e))?;
        let data_dir = temp_dir.path().to_path_buf();

        // Find available port
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .map_err(|e| format!("Failed to bind listener: {}", e))?;
        let server_addr = listener
            .local_addr()
            .map_err(|e| format!("Failed to get local addr: {}", e))?;
        drop(listener);

        // Create mock runner
        let runner: Arc<dyn Runner> = Arc::new(MockRunner::new());

        // Create persistence layer
        let persistence = Arc::new(PostgresPersistence::new(pool.clone()));

        // Create handler state
        let state = Arc::new(EnvironmentHandlerState::new(
            pool.clone(),
            persistence,
            runner,
            "127.0.0.1:8001".to_string(), // Mock core address
            data_dir.clone(),
        ));

        // Start server in background
        let server_state = state.clone();
        let bind_addr = server_addr;
        tokio::spawn(async move {
            if let Err(e) =
                runtara_environment::server::run_environment_server(bind_addr, server_state).await
            {
                eprintln!("Test environment server error: {}", e);
            }
        });

        // Wait for server to start
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Create client
        let mut config = RuntaraClientConfig::default();
        config.server_addr = server_addr;
        config.dangerous_skip_cert_verification = true;
        let client = RuntaraClient::new(config)
            .map_err(|e| format!("Failed to create QUIC client: {}", e))?;

        Ok(Self {
            pool,
            client,
            server_addr,
            data_dir,
            _temp_dir: temp_dir,
        })
    }

    /// Create a test image in the database.
    pub async fn create_test_image(&self, tenant_id: &str, name: &str) -> Uuid {
        let image_id = Uuid::new_v4();
        let binary_path = self
            .data_dir
            .join("test_binary")
            .to_string_lossy()
            .to_string();
        let bundle_path = self
            .data_dir
            .join("test_bundle")
            .to_string_lossy()
            .to_string();

        sqlx::query(
            r#"
            INSERT INTO images (image_id, tenant_id, name, description, binary_path, bundle_path, runner_type)
            VALUES ($1, $2, $3, 'Test image', $4, $5, 'mock')
            "#,
        )
        .bind(image_id.to_string())
        .bind(tenant_id)
        .bind(name)
        .bind(&binary_path)
        .bind(&bundle_path)
        .execute(&self.pool)
        .await
        .expect("Failed to create test image");

        image_id
    }

    /// Get instance status from database.
    pub async fn get_instance_status(&self, instance_id: &str) -> Option<String> {
        let row: Option<(String,)> =
            sqlx::query_as(r#"SELECT status FROM instances WHERE instance_id = $1"#)
                .bind(instance_id)
                .fetch_optional(&self.pool)
                .await
                .ok()?;
        row.map(|r| r.0)
    }

    /// Get image from database.
    pub async fn get_image(&self, image_id: &Uuid) -> Option<(String, String)> {
        let row: Option<(String, String)> =
            sqlx::query_as(r#"SELECT tenant_id, name FROM images WHERE image_id = $1"#)
                .bind(image_id.to_string())
                .fetch_optional(&self.pool)
                .await
                .ok()?;
        row
    }

    /// Clean up all test data.
    pub async fn cleanup(&self) {
        sqlx::query("DELETE FROM wake_queue")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM container_registry")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM container_cancellations")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM container_status")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM container_heartbeats")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM instances")
            .execute(&self.pool)
            .await
            .ok();
        sqlx::query("DELETE FROM images")
            .execute(&self.pool)
            .await
            .ok();
    }
}

/// Helper macro to skip tests if database URL is not set.
#[macro_export]
macro_rules! skip_if_no_env_db {
    () => {
        if std::env::var("TEST_RUNTARA_DATABASE_URL").is_err() {
            eprintln!("Skipping test: TEST_RUNTARA_DATABASE_URL not set");
            return;
        }
    };
}

// Protocol helpers for environment
pub fn wrap_health_check(
    req: runtara_protocol::environment_proto::HealthCheckRequest,
) -> runtara_protocol::environment_proto::RpcRequest {
    runtara_protocol::environment_proto::RpcRequest {
        request: Some(runtara_protocol::environment_proto::rpc_request::Request::HealthCheck(req)),
    }
}

pub fn wrap_register_image(
    req: runtara_protocol::environment_proto::RegisterImageRequest,
) -> runtara_protocol::environment_proto::RpcRequest {
    runtara_protocol::environment_proto::RpcRequest {
        request: Some(
            runtara_protocol::environment_proto::rpc_request::Request::RegisterImage(req),
        ),
    }
}

pub fn wrap_get_image(
    req: runtara_protocol::environment_proto::GetImageRequest,
) -> runtara_protocol::environment_proto::RpcRequest {
    runtara_protocol::environment_proto::RpcRequest {
        request: Some(runtara_protocol::environment_proto::rpc_request::Request::GetImage(req)),
    }
}

pub fn wrap_list_images(
    req: runtara_protocol::environment_proto::ListImagesRequest,
) -> runtara_protocol::environment_proto::RpcRequest {
    runtara_protocol::environment_proto::RpcRequest {
        request: Some(runtara_protocol::environment_proto::rpc_request::Request::ListImages(req)),
    }
}

pub fn wrap_delete_image(
    req: runtara_protocol::environment_proto::DeleteImageRequest,
) -> runtara_protocol::environment_proto::RpcRequest {
    runtara_protocol::environment_proto::RpcRequest {
        request: Some(runtara_protocol::environment_proto::rpc_request::Request::DeleteImage(req)),
    }
}

pub fn wrap_start_instance(
    req: runtara_protocol::environment_proto::StartInstanceRequest,
) -> runtara_protocol::environment_proto::RpcRequest {
    runtara_protocol::environment_proto::RpcRequest {
        request: Some(
            runtara_protocol::environment_proto::rpc_request::Request::StartInstance(req),
        ),
    }
}

pub fn wrap_stop_instance(
    req: runtara_protocol::environment_proto::StopInstanceRequest,
) -> runtara_protocol::environment_proto::RpcRequest {
    runtara_protocol::environment_proto::RpcRequest {
        request: Some(runtara_protocol::environment_proto::rpc_request::Request::StopInstance(req)),
    }
}

pub fn wrap_get_instance_status(
    req: runtara_protocol::environment_proto::GetInstanceStatusRequest,
) -> runtara_protocol::environment_proto::RpcRequest {
    runtara_protocol::environment_proto::RpcRequest {
        request: Some(
            runtara_protocol::environment_proto::rpc_request::Request::GetInstanceStatus(req),
        ),
    }
}

pub fn wrap_list_instances(
    req: runtara_protocol::environment_proto::ListInstancesRequest,
) -> runtara_protocol::environment_proto::RpcRequest {
    runtara_protocol::environment_proto::RpcRequest {
        request: Some(
            runtara_protocol::environment_proto::rpc_request::Request::ListInstances(req),
        ),
    }
}
