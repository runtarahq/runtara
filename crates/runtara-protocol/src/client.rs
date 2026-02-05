// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! QUIC client helpers for connecting to runtara-core.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use quinn::{ClientConfig, Connection, Endpoint, TransportConfig};
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::{debug, info, instrument};

use crate::frame::{Frame, FrameError, FramedStream};

/// Errors that can occur in the QUIC client
#[derive(Debug, Error)]
pub enum ClientError {
    #[error("connection error: {0}")]
    Connection(#[from] quinn::ConnectionError),

    #[error("connect error: {0}")]
    Connect(#[from] quinn::ConnectError),

    #[error("write error: {0}")]
    Write(#[from] quinn::WriteError),

    #[error("read error: {0}")]
    Read(#[from] quinn::ReadExactError),

    #[error("frame error: {0}")]
    Frame(#[from] FrameError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("stream closed: {0}")]
    ClosedStream(#[from] quinn::ClosedStream),

    #[error("no connection established")]
    NotConnected,

    #[error("invalid server name: {0}")]
    InvalidServerName(String),

    #[error("connection timed out after {0}ms")]
    Timeout(u64),
}

/// Configuration for the QUIC client
#[derive(Debug, Clone)]
pub struct RuntaraClientConfig {
    /// Server address to connect to
    pub server_addr: SocketAddr,
    /// Server name for TLS verification (use "localhost" for local dev)
    pub server_name: String,
    /// Enable 0-RTT for lower latency (requires server support)
    pub enable_0rtt: bool,
    /// Skip certificate verification (for development only!)
    pub dangerous_skip_cert_verification: bool,
    /// Keep-alive interval in milliseconds (0 to disable)
    pub keep_alive_interval_ms: u64,
    /// Idle timeout in milliseconds
    pub idle_timeout_ms: u64,
    /// Connection timeout in milliseconds
    pub connect_timeout_ms: u64,
}

impl Default for RuntaraClientConfig {
    fn default() -> Self {
        Self {
            server_addr: "127.0.0.1:8001".parse().unwrap(),
            server_name: "localhost".to_string(),
            enable_0rtt: true,
            dangerous_skip_cert_verification: false,
            keep_alive_interval_ms: 10_000,
            idle_timeout_ms: 600_000, // 10 minutes - match server timeout for long-running workflows
            connect_timeout_ms: 10_000,
        }
    }
}

/// QUIC client for communicating with runtara-core
pub struct RuntaraClient {
    endpoint: Endpoint,
    connection: Mutex<Option<Connection>>,
    config: RuntaraClientConfig,
}

impl RuntaraClient {
    /// Create a new client with the given configuration
    pub fn new(config: RuntaraClientConfig) -> Result<Self, ClientError> {
        let mut endpoint = Endpoint::client("0.0.0.0:0".parse().unwrap())?;

        let client_config = Self::build_client_config(&config)?;
        endpoint.set_default_client_config(client_config);

        Ok(Self {
            endpoint,
            connection: Mutex::new(None),
            config,
        })
    }

    /// Create a client with default configuration for local development
    pub fn localhost() -> Result<Self, ClientError> {
        Self::new(RuntaraClientConfig {
            dangerous_skip_cert_verification: true,
            ..Default::default()
        })
    }

    fn build_client_config(config: &RuntaraClientConfig) -> Result<ClientConfig, ClientError> {
        let crypto = if config.dangerous_skip_cert_verification {
            rustls::ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
                .with_no_client_auth()
        } else {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth()
        };

        let mut transport = TransportConfig::default();
        if config.keep_alive_interval_ms > 0 {
            transport.keep_alive_interval(Some(std::time::Duration::from_millis(
                config.keep_alive_interval_ms,
            )));
        }
        transport.max_idle_timeout(Some(
            std::time::Duration::from_millis(config.idle_timeout_ms)
                .try_into()
                .unwrap(),
        ));

        let mut client_config = ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(crypto).unwrap(),
        ));
        client_config.transport_config(Arc::new(transport));

        Ok(client_config)
    }

    /// Connect to the server
    #[instrument(skip(self))]
    pub async fn connect(&self) -> Result<(), ClientError> {
        let mut conn_guard = self.connection.lock().await;

        // Check if we already have a valid connection
        if let Some(ref conn) = *conn_guard
            && conn.close_reason().is_none()
        {
            debug!("reusing existing connection");
            return Ok(());
        }

        info!(addr = %self.config.server_addr, "connecting to runtara-core");

        let timeout = Duration::from_millis(self.config.connect_timeout_ms);
        let connecting = self
            .endpoint
            .connect(self.config.server_addr, &self.config.server_name)?;

        let connection = tokio::time::timeout(timeout, connecting)
            .await
            .map_err(|_| ClientError::Timeout(self.config.connect_timeout_ms))??;

        info!("connected to runtara-core");
        *conn_guard = Some(connection);
        Ok(())
    }

    /// Get the current connection, connecting if necessary
    async fn get_connection(&self) -> Result<Connection, ClientError> {
        self.connect().await?;
        let conn_guard = self.connection.lock().await;
        conn_guard.clone().ok_or(ClientError::NotConnected)
    }

    /// Open a new bidirectional stream for a request/response
    pub async fn open_stream(
        &self,
    ) -> Result<FramedStream<(quinn::SendStream, quinn::RecvStream)>, ClientError> {
        let conn = self.get_connection().await?;
        let (send, recv) = conn.open_bi().await?;
        Ok(FramedStream::new((send, recv)))
    }

    /// Open a unidirectional stream for sending (e.g., events)
    pub async fn open_uni_send(&self) -> Result<FramedStream<quinn::SendStream>, ClientError> {
        let conn = self.get_connection().await?;
        let send = conn.open_uni().await?;
        Ok(FramedStream::new(send))
    }

    /// Send a request and receive a response using a new stream
    #[instrument(skip(self, request))]
    pub async fn request<Req: prost::Message, Resp: prost::Message + Default>(
        &self,
        request: &Req,
    ) -> Result<Resp, ClientError> {
        let conn = self.get_connection().await?;
        let (mut send, mut recv) = conn.open_bi().await?;

        // Send request
        let frame = Frame::request(request)?;
        crate::frame::write_frame(&mut send, &frame).await?;
        send.finish()?;

        // Read response
        let response_frame = crate::frame::read_frame(&mut recv).await?;
        Ok(response_frame.decode()?)
    }

    /// Send a fire-and-forget request (no response expected).
    ///
    /// Use this for events that don't require acknowledgement.
    #[instrument(skip(self, request))]
    pub async fn send_fire_and_forget<Req: prost::Message>(
        &self,
        request: &Req,
    ) -> Result<(), ClientError> {
        let conn = self.get_connection().await?;
        let (mut send, _recv) = conn.open_bi().await?;

        // Send request
        let frame = Frame::request(request)?;
        crate::frame::write_frame(&mut send, &frame).await?;
        send.finish()?;

        // No response expected - just return
        Ok(())
    }

    /// Open a raw bidirectional stream for streaming operations.
    ///
    /// This returns the raw QUIC streams for advanced use cases like
    /// streaming large data that doesn't fit in a single frame.
    pub async fn open_raw_stream(
        &self,
    ) -> Result<(quinn::SendStream, quinn::RecvStream), ClientError> {
        let conn = self.get_connection().await?;
        Ok(conn.open_bi().await?)
    }

    /// Close the connection gracefully
    pub async fn close(&self) {
        let mut conn_guard = self.connection.lock().await;
        if let Some(conn) = conn_guard.take() {
            conn.close(0u32.into(), b"client closing");
        }
    }

    /// Check if the client is currently connected
    pub async fn is_connected(&self) -> bool {
        let conn_guard = self.connection.lock().await;
        if let Some(ref conn) = *conn_guard {
            conn.close_reason().is_none()
        } else {
            false
        }
    }
}

impl Drop for RuntaraClient {
    fn drop(&mut self) {
        // Close connection on drop (non-async, best effort)
        if let Ok(mut guard) = self.connection.try_lock()
            && let Some(conn) = guard.take()
        {
            conn.close(0u32.into(), b"client dropped");
        }
    }
}

/// Certificate verifier that skips all verification (for development only!)
#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ECDSA_NISTP521_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RuntaraClientConfig::default();
        assert_eq!(config.server_addr, "127.0.0.1:8001".parse().unwrap());
        assert_eq!(config.server_name, "localhost");
    }

    #[test]
    fn test_default_config_all_fields() {
        let config = RuntaraClientConfig::default();
        assert_eq!(config.server_addr, "127.0.0.1:8001".parse().unwrap());
        assert_eq!(config.server_name, "localhost");
        assert!(config.enable_0rtt);
        assert!(!config.dangerous_skip_cert_verification);
        assert_eq!(config.keep_alive_interval_ms, 10_000);
        assert_eq!(config.idle_timeout_ms, 600_000); // 10 minutes
        assert_eq!(config.connect_timeout_ms, 10_000);
    }

    #[test]
    fn test_config_clone() {
        let config = RuntaraClientConfig {
            server_addr: "192.168.1.1:9000".parse().unwrap(),
            server_name: "custom".to_string(),
            enable_0rtt: false,
            dangerous_skip_cert_verification: true,
            keep_alive_interval_ms: 5000,
            idle_timeout_ms: 60000,
            connect_timeout_ms: 3000,
        };
        let cloned = config.clone();
        assert_eq!(config.server_addr, cloned.server_addr);
        assert_eq!(config.server_name, cloned.server_name);
        assert_eq!(config.enable_0rtt, cloned.enable_0rtt);
        assert_eq!(
            config.dangerous_skip_cert_verification,
            cloned.dangerous_skip_cert_verification
        );
        assert_eq!(config.keep_alive_interval_ms, cloned.keep_alive_interval_ms);
        assert_eq!(config.idle_timeout_ms, cloned.idle_timeout_ms);
        assert_eq!(config.connect_timeout_ms, cloned.connect_timeout_ms);
    }

    #[test]
    fn test_config_debug() {
        let config = RuntaraClientConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("RuntaraClientConfig"));
        assert!(debug_str.contains("server_addr"));
        assert!(debug_str.contains("server_name"));
    }

    #[tokio::test]
    async fn test_client_creation() {
        let config = RuntaraClientConfig {
            dangerous_skip_cert_verification: true,
            ..Default::default()
        };
        let client = RuntaraClient::new(config);
        assert!(
            client.is_ok(),
            "Failed to create client: {:?}",
            client.err()
        );
    }

    #[tokio::test]
    async fn test_client_localhost() {
        let client = RuntaraClient::localhost();
        assert!(
            client.is_ok(),
            "Failed to create localhost client: {:?}",
            client.err()
        );
    }

    #[tokio::test]
    async fn test_client_with_custom_config() {
        let config = RuntaraClientConfig {
            server_addr: "10.0.0.1:8888".parse().unwrap(),
            server_name: "my-server".to_string(),
            enable_0rtt: false,
            dangerous_skip_cert_verification: true,
            keep_alive_interval_ms: 0, // Disable keep-alive
            idle_timeout_ms: 120000,
            connect_timeout_ms: 5000,
        };
        let client = RuntaraClient::new(config);
        assert!(client.is_ok());
    }

    #[tokio::test]
    async fn test_client_initial_not_connected() {
        let config = RuntaraClientConfig {
            dangerous_skip_cert_verification: true,
            ..Default::default()
        };
        let client = RuntaraClient::new(config).unwrap();
        assert!(!client.is_connected().await);
    }

    #[tokio::test]
    async fn test_client_connect_timeout() {
        let config = RuntaraClientConfig {
            server_addr: "127.0.0.1:59998".parse().unwrap(), // Unlikely to have a server
            dangerous_skip_cert_verification: true,
            connect_timeout_ms: 100, // Very short timeout
            ..Default::default()
        };
        let client = RuntaraClient::new(config).unwrap();
        let result = client.connect().await;
        // Should timeout since no server is running
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_client_close_without_connection() {
        let config = RuntaraClientConfig {
            dangerous_skip_cert_verification: true,
            ..Default::default()
        };
        let client = RuntaraClient::new(config).unwrap();
        // Closing without a connection should be safe
        client.close().await;
        assert!(!client.is_connected().await);
    }

    #[tokio::test]
    async fn test_open_stream_without_connection() {
        let config = RuntaraClientConfig {
            server_addr: "127.0.0.1:59997".parse().unwrap(),
            dangerous_skip_cert_verification: true,
            connect_timeout_ms: 100,
            ..Default::default()
        };
        let client = RuntaraClient::new(config).unwrap();
        // open_stream will try to connect first, then fail
        let result = client.open_stream().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_open_uni_send_without_connection() {
        let config = RuntaraClientConfig {
            server_addr: "127.0.0.1:59996".parse().unwrap(),
            dangerous_skip_cert_verification: true,
            connect_timeout_ms: 100,
            ..Default::default()
        };
        let client = RuntaraClient::new(config).unwrap();
        let result = client.open_uni_send().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_open_raw_stream_without_connection() {
        let config = RuntaraClientConfig {
            server_addr: "127.0.0.1:59995".parse().unwrap(),
            dangerous_skip_cert_verification: true,
            connect_timeout_ms: 100,
            ..Default::default()
        };
        let client = RuntaraClient::new(config).unwrap();
        let result = client.open_raw_stream().await;
        assert!(result.is_err());
    }

    #[test]
    fn test_client_error_display() {
        let err = ClientError::NotConnected;
        assert_eq!(format!("{}", err), "no connection established");

        let err = ClientError::Timeout(5000);
        assert_eq!(format!("{}", err), "connection timed out after 5000ms");

        let err = ClientError::InvalidServerName("bad-name".to_string());
        assert_eq!(format!("{}", err), "invalid server name: bad-name");
    }

    #[test]
    fn test_skip_server_verification_schemes() {
        use rustls::client::danger::ServerCertVerifier;
        let verifier = SkipServerVerification;
        let schemes = verifier.supported_verify_schemes();
        assert!(!schemes.is_empty());
        assert!(schemes.contains(&rustls::SignatureScheme::RSA_PKCS1_SHA256));
        assert!(schemes.contains(&rustls::SignatureScheme::ECDSA_NISTP256_SHA256));
        assert!(schemes.contains(&rustls::SignatureScheme::ED25519));
    }

    #[test]
    fn test_skip_server_verification_debug() {
        let verifier = SkipServerVerification;
        let debug_str = format!("{:?}", verifier);
        assert!(debug_str.contains("SkipServerVerification"));
    }

    #[test]
    fn test_build_client_config_with_verification() {
        let config = RuntaraClientConfig {
            dangerous_skip_cert_verification: false,
            ..Default::default()
        };
        // This should work (uses webpki_roots)
        let result = RuntaraClient::build_client_config(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_client_config_skip_verification() {
        let config = RuntaraClientConfig {
            dangerous_skip_cert_verification: true,
            ..Default::default()
        };
        let result = RuntaraClient::build_client_config(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_client_config_no_keepalive() {
        let config = RuntaraClientConfig {
            keep_alive_interval_ms: 0,
            dangerous_skip_cert_verification: true,
            ..Default::default()
        };
        let result = RuntaraClient::build_client_config(&config);
        assert!(result.is_ok());
    }
}
