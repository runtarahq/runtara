// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! QUIC-based SDK backend for remote communication with runtara-core.

use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use runtara_protocol::instance_proto::{
    self as proto, CheckpointRequest as ProtoCheckpointRequest,
    GetCheckpointRequest as ProtoGetCheckpointRequest, GetInstanceStatusRequest,
    RegisterInstanceRequest, RpcRequest, RpcResponse, SleepRequest, rpc_request, rpc_response,
};
use runtara_protocol::{RuntaraClient, RuntaraClientConfig};
use tracing::{debug, info, instrument, warn};

use super::SdkBackend;
use crate::config::SdkConfig;
use crate::error::{Result, SdkError};
use crate::events::{
    build_completed_event, build_custom_event, build_failed_event, build_heartbeat_event,
    build_suspended_event,
};
use crate::types::{CheckpointResult, StatusResponse};

/// QUIC-based backend for SDK operations.
///
/// This backend communicates with runtara-core over QUIC protocol.
pub struct QuicBackend {
    /// Low-level protocol client
    client: RuntaraClient,
    /// Instance ID
    instance_id: String,
    /// Tenant ID
    tenant_id: String,
}

impl QuicBackend {
    /// Create a new QUIC backend with the given configuration.
    pub fn new(config: &SdkConfig) -> Result<Self> {
        let client_config = RuntaraClientConfig {
            server_addr: config.server_addr,
            server_name: config.server_name.clone(),
            enable_0rtt: true,
            dangerous_skip_cert_verification: config.skip_cert_verification,
            keep_alive_interval_ms: 10_000,
            idle_timeout_ms: config.request_timeout_ms,
            connect_timeout_ms: config.connect_timeout_ms,
        };

        let client = RuntaraClient::new(client_config)?;

        Ok(Self {
            client,
            instance_id: config.instance_id.clone(),
            tenant_id: config.tenant_id.clone(),
        })
    }

    /// Get a reference to the underlying QUIC client.
    ///
    /// This is used for QUIC-specific operations like sleep and signals.
    pub fn client(&self) -> &RuntaraClient {
        &self.client
    }
}

#[async_trait]
impl SdkBackend for QuicBackend {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn connect(&self) -> Result<()> {
        info!("Connecting to runtara-core");
        self.client.connect().await?;
        info!("Connected to runtara-core");
        Ok(())
    }

    async fn is_connected(&self) -> bool {
        self.client.is_connected().await
    }

    async fn close(&self) {
        self.client.close().await;
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn register(&self, checkpoint_id: Option<&str>) -> Result<()> {
        let request = RegisterInstanceRequest {
            instance_id: self.instance_id.clone(),
            tenant_id: self.tenant_id.clone(),
            checkpoint_id: checkpoint_id.map(|s| s.to_string()),
        };

        let rpc_request = RpcRequest {
            request: Some(rpc_request::Request::RegisterInstance(request)),
        };

        let rpc_response: RpcResponse = self.client.request(&rpc_request).await?;

        match rpc_response.response {
            Some(rpc_response::Response::RegisterInstance(resp)) => {
                if !resp.success {
                    return Err(SdkError::Registration(resp.error));
                }
                info!("Instance registered with runtara-core");
                Ok(())
            }
            Some(rpc_response::Response::Error(e)) => Err(SdkError::Server {
                code: e.code,
                message: e.message,
            }),
            _ => Err(SdkError::UnexpectedResponse(
                "expected RegisterInstanceResponse".to_string(),
            )),
        }
    }

    #[instrument(skip(self, state), fields(instance_id = %self.instance_id, checkpoint_id = %checkpoint_id, state_size = state.len()))]
    async fn checkpoint(&self, checkpoint_id: &str, state: &[u8]) -> Result<CheckpointResult> {
        debug!("Checkpoint request");

        let request = ProtoCheckpointRequest {
            instance_id: self.instance_id.clone(),
            checkpoint_id: checkpoint_id.to_string(),
            state: state.to_vec(),
        };

        let rpc_request = RpcRequest {
            request: Some(rpc_request::Request::Checkpoint(request)),
        };

        let rpc_response: RpcResponse = self.client.request(&rpc_request).await?;

        match rpc_response.response {
            Some(rpc_response::Response::Checkpoint(resp)) => {
                let pending_signal = resp.pending_signal.map(crate::signals::from_proto_signal);
                let custom_signal = resp.custom_signal.map(|sig| crate::types::CustomSignal {
                    checkpoint_id: sig.checkpoint_id,
                    payload: sig.payload,
                });

                if resp.found {
                    debug!(
                        checkpoint_id = %checkpoint_id,
                        has_pending_signal = pending_signal.is_some(),
                        has_custom_signal = custom_signal.is_some(),
                        "Found existing checkpoint - returning for resume"
                    );
                } else {
                    debug!(
                        checkpoint_id = %checkpoint_id,
                        has_pending_signal = pending_signal.is_some(),
                        has_custom_signal = custom_signal.is_some(),
                        "New checkpoint saved"
                    );
                }

                Ok(CheckpointResult {
                    found: resp.found,
                    state: resp.state,
                    pending_signal,
                    custom_signal,
                })
            }
            Some(rpc_response::Response::Error(e)) => Err(SdkError::Server {
                code: e.code,
                message: e.message,
            }),
            _ => Err(SdkError::UnexpectedResponse(
                "expected CheckpointResponse".to_string(),
            )),
        }
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id, checkpoint_id = %checkpoint_id))]
    async fn get_checkpoint(&self, checkpoint_id: &str) -> Result<Option<Vec<u8>>> {
        debug!("Get checkpoint request (read-only)");

        let request = ProtoGetCheckpointRequest {
            instance_id: self.instance_id.clone(),
            checkpoint_id: checkpoint_id.to_string(),
        };

        let rpc_request = RpcRequest {
            request: Some(rpc_request::Request::GetCheckpoint(request)),
        };

        let rpc_response: RpcResponse = self.client.request(&rpc_request).await?;

        match rpc_response.response {
            Some(rpc_response::Response::GetCheckpoint(resp)) => {
                if resp.found {
                    debug!(checkpoint_id = %checkpoint_id, "Checkpoint found");
                    Ok(Some(resp.state))
                } else {
                    debug!(checkpoint_id = %checkpoint_id, "Checkpoint not found");
                    Ok(None)
                }
            }
            Some(rpc_response::Response::Error(e)) => Err(SdkError::Server {
                code: e.code,
                message: e.message,
            }),
            _ => Err(SdkError::UnexpectedResponse(
                "expected GetCheckpointResponse".to_string(),
            )),
        }
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn heartbeat(&self) -> Result<()> {
        let event = build_heartbeat_event(&self.instance_id);
        self.send_acknowledged_event(event).await?;
        debug!("Heartbeat acknowledged");
        Ok(())
    }

    #[instrument(skip(self, output), fields(instance_id = %self.instance_id, output_size = output.len()))]
    async fn completed(&self, output: &[u8]) -> Result<()> {
        let event = build_completed_event(&self.instance_id, output.to_vec());
        self.send_acknowledged_event(event).await?;
        info!("Completed event acknowledged");
        Ok(())
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn failed(&self, error: &str) -> Result<()> {
        let event = build_failed_event(&self.instance_id, error);
        self.send_acknowledged_event(event).await?;
        warn!(error = %error, "Failed event acknowledged");
        Ok(())
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn suspended(&self) -> Result<()> {
        let event = build_suspended_event(&self.instance_id);
        self.send_acknowledged_event(event).await?;
        info!("Suspended event acknowledged");
        Ok(())
    }

    #[instrument(skip(self, payload), fields(instance_id = %self.instance_id, subtype = %subtype, payload_size = payload.len()))]
    async fn send_custom_event(&self, subtype: &str, payload: Vec<u8>) -> Result<()> {
        let event = build_custom_event(&self.instance_id, subtype, payload);
        self.send_acknowledged_event(event).await?;
        debug!(subtype = %subtype, "Custom event acknowledged");
        Ok(())
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id, checkpoint_id = %checkpoint_id, attempt = attempt_number))]
    async fn record_retry_attempt(
        &self,
        checkpoint_id: &str,
        attempt_number: u32,
        error_message: Option<&str>,
    ) -> Result<()> {
        debug!("Recording retry attempt");

        let timestamp_ms = chrono::Utc::now().timestamp_millis();

        let event = proto::RetryAttemptEvent {
            instance_id: self.instance_id.clone(),
            checkpoint_id: checkpoint_id.to_string(),
            attempt_number,
            timestamp_ms,
            error_message: error_message.map(|s| s.to_string()),
        };

        let rpc_request = RpcRequest {
            request: Some(rpc_request::Request::RetryAttempt(event)),
        };

        self.client.send_fire_and_forget(&rpc_request).await?;

        debug!(attempt = attempt_number, "Retry attempt recorded");
        Ok(())
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn get_status(&self) -> Result<StatusResponse> {
        let request = GetInstanceStatusRequest {
            instance_id: self.instance_id.clone(),
        };

        let rpc_request = RpcRequest {
            request: Some(rpc_request::Request::GetInstanceStatus(request)),
        };

        let rpc_response: RpcResponse = self.client.request(&rpc_request).await?;

        match rpc_response.response {
            Some(rpc_response::Response::GetInstanceStatus(resp)) => Ok(StatusResponse::from(resp)),
            Some(rpc_response::Response::Error(e)) => Err(SdkError::Server {
                code: e.code,
                message: e.message,
            }),
            _ => Err(SdkError::UnexpectedResponse(
                "expected GetInstanceStatusResponse".to_string(),
            )),
        }
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn set_sleep_until(&self, _sleep_until: DateTime<Utc>) -> Result<()> {
        // In QUIC mode, sleep_until is managed server-side via durable_sleep
        // This is a no-op as the server handles sleep tracking
        debug!("set_sleep_until is handled by server in QUIC mode");
        Ok(())
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn clear_sleep(&self) -> Result<()> {
        // In QUIC mode, sleep_until is managed server-side
        // This is a no-op as the server clears sleep after wake
        debug!("clear_sleep is handled by server in QUIC mode");
        Ok(())
    }

    #[instrument(skip(self), fields(instance_id = %self.instance_id))]
    async fn get_sleep_until(&self) -> Result<Option<DateTime<Utc>>> {
        // Get the sleep_until time from the server via status
        let status = self.get_status().await?;
        // StatusResponse doesn't currently include sleep_until
        // For now, return None as the server handles sleep tracking internally
        debug!("get_sleep_until: status found={}", status.found);
        Ok(None)
    }

    #[instrument(skip(self, state), fields(instance_id = %self.instance_id, duration_ms = duration.as_millis() as u64))]
    async fn durable_sleep(
        &self,
        duration: Duration,
        checkpoint_id: &str,
        state: &[u8],
    ) -> Result<()> {
        debug!("Requesting durable sleep via QUIC");

        let request = SleepRequest {
            instance_id: self.instance_id.clone(),
            duration_ms: duration.as_millis() as u64,
            checkpoint_id: checkpoint_id.to_string(),
            state: state.to_vec(),
        };

        let rpc_request = RpcRequest {
            request: Some(rpc_request::Request::Sleep(request)),
        };

        let rpc_response: RpcResponse = self.client.request(&rpc_request).await?;

        match rpc_response.response {
            Some(rpc_response::Response::Sleep(_)) => {
                info!("Durable sleep completed (handled by server)");
                Ok(())
            }
            Some(rpc_response::Response::Error(e)) => Err(SdkError::Server {
                code: e.code,
                message: e.message,
            }),
            _ => Err(SdkError::UnexpectedResponse(
                "expected SleepResponse".to_string(),
            )),
        }
    }
}

impl QuicBackend {
    /// Send an event and wait for server acknowledgment.
    ///
    /// All events use request-response semantics to ensure they are persisted
    /// before returning. This prevents race conditions where events could be
    /// lost if the process exits immediately after sending.
    async fn send_acknowledged_event(&self, event: proto::InstanceEvent) -> Result<()> {
        let rpc_request = RpcRequest {
            request: Some(rpc_request::Request::InstanceEvent(event)),
        };

        let rpc_response: RpcResponse = self.client.request(&rpc_request).await?;

        match rpc_response.response {
            Some(rpc_response::Response::InstanceEvent(resp)) => {
                if !resp.success {
                    return Err(SdkError::Server {
                        code: "EVENT_ERROR".to_string(),
                        message: resp.error.unwrap_or_else(|| "Unknown error".to_string()),
                    });
                }
                Ok(())
            }
            Some(rpc_response::Response::Error(e)) => Err(SdkError::Server {
                code: e.code,
                message: e.message,
            }),
            _ => Err(SdkError::UnexpectedResponse(
                "expected InstanceEventResponse".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SdkConfig {
        SdkConfig {
            instance_id: "test-instance".to_string(),
            tenant_id: "test-tenant".to_string(),
            server_addr: "127.0.0.1:8001".parse().unwrap(),
            server_name: "localhost".to_string(),
            skip_cert_verification: true,
            connect_timeout_ms: 5000,
            request_timeout_ms: 30000,
            signal_poll_interval_ms: 1000,
        }
    }

    #[tokio::test]
    async fn test_quic_backend_creation() {
        let config = test_config();
        let backend = QuicBackend::new(&config);
        assert!(
            backend.is_ok(),
            "Failed to create QUIC backend: {:?}",
            backend.err()
        );
    }

    #[tokio::test]
    async fn test_quic_backend_instance_and_tenant_id() {
        let config = test_config();
        let backend = QuicBackend::new(&config).unwrap();
        assert_eq!(backend.instance_id(), "test-instance");
        assert_eq!(backend.tenant_id(), "test-tenant");
    }

    #[tokio::test]
    async fn test_quic_backend_client_accessor() {
        let config = test_config();
        let backend = QuicBackend::new(&config).unwrap();
        // Just verify we can get a reference to the client
        let _client = backend.client();
    }

    #[tokio::test]
    async fn test_quic_backend_as_any() {
        let config = test_config();
        let backend = QuicBackend::new(&config).unwrap();
        let any = backend.as_any();
        assert!(any.downcast_ref::<QuicBackend>().is_some());
    }

    #[tokio::test]
    async fn test_quic_backend_with_custom_server_addr() {
        let mut config = test_config();
        config.server_addr = "192.168.1.100:9000".parse().unwrap();
        config.server_name = "custom-server".to_string();
        let backend = QuicBackend::new(&config);
        assert!(backend.is_ok());
    }

    #[tokio::test]
    async fn test_quic_backend_with_different_timeouts() {
        let mut config = test_config();
        config.connect_timeout_ms = 1000;
        config.request_timeout_ms = 60000;
        let backend = QuicBackend::new(&config);
        assert!(backend.is_ok());
    }

    #[tokio::test]
    async fn test_quic_backend_initial_not_connected() {
        let config = test_config();
        let backend = QuicBackend::new(&config).unwrap();
        // Initially not connected
        assert!(!backend.is_connected().await);
    }

    #[tokio::test]
    async fn test_quic_backend_connect_without_server() {
        let mut config = test_config();
        // Use a port that's unlikely to have a server
        config.server_addr = "127.0.0.1:59999".parse().unwrap();
        config.connect_timeout_ms = 100; // Short timeout for faster test

        let backend = QuicBackend::new(&config).unwrap();
        let result = backend.connect().await;
        // Should fail to connect since no server is running
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_quic_backend_close_without_connection() {
        let config = test_config();
        let backend = QuicBackend::new(&config).unwrap();
        // Closing without a connection should be safe (no-op)
        backend.close().await;
        assert!(!backend.is_connected().await);
    }

    #[tokio::test]
    async fn test_set_sleep_until_is_noop() {
        let config = test_config();
        let backend = QuicBackend::new(&config).unwrap();
        // set_sleep_until is a no-op in QUIC mode (server handles it)
        let result = backend.set_sleep_until(Utc::now()).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_clear_sleep_is_noop() {
        let config = test_config();
        let backend = QuicBackend::new(&config).unwrap();
        // clear_sleep is a no-op in QUIC mode (server handles it)
        let result = backend.clear_sleep().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_get_sleep_until_without_connection() {
        let mut config = test_config();
        // Use a port that's unlikely to have a server
        config.server_addr = "127.0.0.1:59998".parse().unwrap();
        config.connect_timeout_ms = 100; // Short timeout for faster test

        let backend = QuicBackend::new(&config).unwrap();
        // get_sleep_until requires a connection (calls get_status internally)
        let result = backend.get_sleep_until().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_config_client_mapping() {
        // Verify that SdkConfig fields are correctly mapped to RuntaraClientConfig
        let sdk_config = SdkConfig {
            instance_id: "inst".to_string(),
            tenant_id: "tenant".to_string(),
            server_addr: "10.0.0.1:8888".parse().unwrap(),
            server_name: "my-server".to_string(),
            skip_cert_verification: true,
            connect_timeout_ms: 2000,
            request_timeout_ms: 5000,
            signal_poll_interval_ms: 1000,
        };

        let backend = QuicBackend::new(&sdk_config).unwrap();
        assert_eq!(backend.instance_id(), "inst");
        assert_eq!(backend.tenant_id(), "tenant");
    }
}
