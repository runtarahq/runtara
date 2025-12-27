// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! QUIC server helpers for runtara-core.

use std::net::SocketAddr;
use std::sync::Arc;

use quinn::{Endpoint, Incoming, RecvStream, SendStream, ServerConfig, TransportConfig};
use thiserror::Error;
use tracing::{debug, error, info, instrument, warn};

use crate::frame::{Frame, FrameError, FramedStream, read_frame, write_frame};

/// Errors that can occur in the QUIC server
#[derive(Debug, Error)]
pub enum ServerError {
    #[error("bind error: {0}")]
    Bind(#[from] std::io::Error),

    #[error("connection error: {0}")]
    Connection(#[from] quinn::ConnectionError),

    #[error("frame error: {0}")]
    Frame(#[from] FrameError),

    #[error("TLS error: {0}")]
    Tls(String),

    #[error("server closed")]
    Closed,
}

/// Configuration for the QUIC server
#[derive(Debug, Clone)]
pub struct RuntaraServerConfig {
    /// Address to bind to
    pub bind_addr: SocketAddr,
    /// TLS certificate chain (PEM format)
    pub cert_pem: Vec<u8>,
    /// TLS private key (PEM format)
    pub key_pem: Vec<u8>,
    /// Maximum pending incoming connections (handshakes in progress)
    pub max_incoming: u32,
    /// Maximum concurrent bidirectional streams per connection
    pub max_bi_streams: u32,
    /// Maximum concurrent unidirectional streams per connection
    pub max_uni_streams: u32,
    /// Idle timeout in milliseconds
    pub idle_timeout_ms: u64,
    /// Server-side keep-alive interval in milliseconds (0 to disable)
    pub keep_alive_interval_ms: u64,
    /// UDP receive buffer size in bytes (0 for OS default)
    pub udp_receive_buffer_size: usize,
    /// UDP send buffer size in bytes (0 for OS default)
    pub udp_send_buffer_size: usize,
    /// Maximum concurrent connection handlers (0 for unlimited)
    pub max_concurrent_handlers: u32,
}

impl Default for RuntaraServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:7001".parse().unwrap(),
            cert_pem: Vec::new(),
            key_pem: Vec::new(),
            max_incoming: 10_000,
            max_bi_streams: 1_000,
            max_uni_streams: 100,
            idle_timeout_ms: 120_000,
            keep_alive_interval_ms: 15_000,
            udp_receive_buffer_size: 2 * 1024 * 1024, // 2MB
            udp_send_buffer_size: 2 * 1024 * 1024,    // 2MB
            max_concurrent_handlers: 0,               // unlimited by default
        }
    }
}

impl RuntaraServerConfig {
    /// Create a configuration from environment variables with defaults.
    ///
    /// Environment variables:
    /// - `RUNTARA_QUIC_MAX_INCOMING`: Max pending handshakes (default: 10000)
    /// - `RUNTARA_QUIC_MAX_BI_STREAMS`: Max bidirectional streams per connection (default: 1000)
    /// - `RUNTARA_QUIC_MAX_UNI_STREAMS`: Max unidirectional streams per connection (default: 100)
    /// - `RUNTARA_QUIC_IDLE_TIMEOUT_MS`: Idle timeout in ms (default: 120000)
    /// - `RUNTARA_QUIC_KEEP_ALIVE_MS`: Keep-alive interval in ms, 0 to disable (default: 15000)
    /// - `RUNTARA_QUIC_UDP_RECV_BUFFER`: UDP receive buffer size in bytes (default: 2097152)
    /// - `RUNTARA_QUIC_UDP_SEND_BUFFER`: UDP send buffer size in bytes (default: 2097152)
    /// - `RUNTARA_QUIC_MAX_HANDLERS`: Max concurrent connection handlers, 0 for unlimited (default: 0)
    pub fn from_env() -> Self {
        let default = Self::default();

        Self {
            bind_addr: default.bind_addr,
            cert_pem: default.cert_pem,
            key_pem: default.key_pem,
            max_incoming: std::env::var("RUNTARA_QUIC_MAX_INCOMING")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.max_incoming),
            max_bi_streams: std::env::var("RUNTARA_QUIC_MAX_BI_STREAMS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.max_bi_streams),
            max_uni_streams: std::env::var("RUNTARA_QUIC_MAX_UNI_STREAMS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.max_uni_streams),
            idle_timeout_ms: std::env::var("RUNTARA_QUIC_IDLE_TIMEOUT_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.idle_timeout_ms),
            keep_alive_interval_ms: std::env::var("RUNTARA_QUIC_KEEP_ALIVE_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.keep_alive_interval_ms),
            udp_receive_buffer_size: std::env::var("RUNTARA_QUIC_UDP_RECV_BUFFER")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.udp_receive_buffer_size),
            udp_send_buffer_size: std::env::var("RUNTARA_QUIC_UDP_SEND_BUFFER")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.udp_send_buffer_size),
            max_concurrent_handlers: std::env::var("RUNTARA_QUIC_MAX_HANDLERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default.max_concurrent_handlers),
        }
    }
}

/// QUIC server for runtara-core
pub struct RuntaraServer {
    endpoint: Endpoint,
    config: RuntaraServerConfig,
}

impl RuntaraServer {
    /// Create a new server with the given configuration
    pub fn new(config: RuntaraServerConfig) -> Result<Self, ServerError> {
        use socket2::{Domain, Protocol, Socket, Type};

        let server_config = Self::build_server_config(&config)?;

        // Create UDP socket with custom buffer sizes using socket2
        let domain = if config.bind_addr.is_ipv6() {
            Domain::IPV6
        } else {
            Domain::IPV4
        };
        let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;

        if config.udp_receive_buffer_size > 0
            && let Err(e) = socket.set_recv_buffer_size(config.udp_receive_buffer_size)
        {
            warn!(
                size = config.udp_receive_buffer_size,
                error = %e,
                "Failed to set UDP receive buffer size"
            );
        }
        if config.udp_send_buffer_size > 0
            && let Err(e) = socket.set_send_buffer_size(config.udp_send_buffer_size)
        {
            warn!(
                size = config.udp_send_buffer_size,
                error = %e,
                "Failed to set UDP send buffer size"
            );
        }

        // Bind and convert to std socket
        socket.bind(&config.bind_addr.into())?;
        let std_socket: std::net::UdpSocket = socket.into();

        let runtime = quinn::default_runtime()
            .ok_or_else(|| ServerError::Bind(std::io::Error::other("no async runtime found")))?;
        let endpoint = Endpoint::new_with_abstract_socket(
            quinn::EndpointConfig::default(),
            Some(server_config),
            runtime.wrap_udp_socket(std_socket)?,
            runtime,
        )?;

        info!(
            addr = %config.bind_addr,
            max_incoming = config.max_incoming,
            max_bi_streams = config.max_bi_streams,
            idle_timeout_ms = config.idle_timeout_ms,
            keep_alive_ms = config.keep_alive_interval_ms,
            udp_recv_buffer = config.udp_receive_buffer_size,
            udp_send_buffer = config.udp_send_buffer_size,
            max_handlers = config.max_concurrent_handlers,
            "QUIC server bound"
        );

        Ok(Self { endpoint, config })
    }

    /// Create a server with self-signed certificate for local development
    pub fn localhost(bind_addr: SocketAddr) -> Result<Self, ServerError> {
        Self::localhost_with_config(bind_addr, RuntaraServerConfig::from_env())
    }

    /// Create a server with self-signed certificate and custom config
    pub fn localhost_with_config(
        bind_addr: SocketAddr,
        mut config: RuntaraServerConfig,
    ) -> Result<Self, ServerError> {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .map_err(|e| ServerError::Tls(e.to_string()))?;

        config.bind_addr = bind_addr;
        config.cert_pem = cert.cert.pem().into_bytes();
        config.key_pem = cert.key_pair.serialize_pem().into_bytes();

        Self::new(config)
    }

    /// Get the server configuration
    pub fn config(&self) -> &RuntaraServerConfig {
        &self.config
    }

    fn build_server_config(config: &RuntaraServerConfig) -> Result<ServerConfig, ServerError> {
        let certs = rustls_pemfile::certs(&mut config.cert_pem.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| ServerError::Tls(format!("failed to parse certificates: {}", e)))?;

        let key = rustls_pemfile::private_key(&mut config.key_pem.as_slice())
            .map_err(|e| ServerError::Tls(format!("failed to parse private key: {}", e)))?
            .ok_or_else(|| ServerError::Tls("no private key found".to_string()))?;

        let crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| ServerError::Tls(e.to_string()))?;

        let mut transport = TransportConfig::default();
        transport.max_idle_timeout(Some(
            std::time::Duration::from_millis(config.idle_timeout_ms)
                .try_into()
                .unwrap(),
        ));
        transport.max_concurrent_bidi_streams(config.max_bi_streams.into());
        transport.max_concurrent_uni_streams(config.max_uni_streams.into());

        // Server-side keep-alive
        if config.keep_alive_interval_ms > 0 {
            transport.keep_alive_interval(Some(std::time::Duration::from_millis(
                config.keep_alive_interval_ms,
            )));
        }

        let mut server_config = ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(crypto)
                .map_err(|e| ServerError::Tls(e.to_string()))?,
        ));
        server_config.transport_config(Arc::new(transport));

        // Limit pending handshakes
        server_config.max_incoming(config.max_incoming as usize);

        Ok(server_config)
    }

    /// Accept the next incoming connection
    pub async fn accept(&self) -> Option<Incoming> {
        self.endpoint.accept().await
    }

    /// Get the local address the server is bound to
    pub fn local_addr(&self) -> Result<SocketAddr, ServerError> {
        Ok(self.endpoint.local_addr()?)
    }

    /// Close the server
    pub fn close(&self) {
        self.endpoint.close(0u32.into(), b"server closing");
    }

    /// Run the server with a connection handler
    #[instrument(skip(self, handler))]
    pub async fn run<H, Fut>(&self, handler: H) -> Result<(), ServerError>
    where
        H: Fn(ConnectionHandler) -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        use tokio::sync::Semaphore;

        info!("QUIC server running");

        // Create semaphore for backpressure if configured
        let semaphore = if self.config.max_concurrent_handlers > 0 {
            Some(Arc::new(Semaphore::new(
                self.config.max_concurrent_handlers as usize,
            )))
        } else {
            None
        };

        while let Some(incoming) = self.accept().await {
            let handler = handler.clone();
            let semaphore = semaphore.clone();

            tokio::spawn(async move {
                // Acquire permit if semaphore is configured
                let _permit = if let Some(ref sem) = semaphore {
                    match sem.clone().acquire_owned().await {
                        Ok(permit) => Some(permit),
                        Err(_) => {
                            warn!("semaphore closed, dropping connection");
                            return;
                        }
                    }
                } else {
                    None
                };

                match incoming.await {
                    Ok(connection) => {
                        let remote_addr = connection.remote_address();
                        debug!(%remote_addr, "accepted connection");

                        let conn_handler = ConnectionHandler::new(connection);
                        handler(conn_handler).await;
                    }
                    Err(e) => {
                        warn!("failed to accept connection: {}", e);
                    }
                }
            });
        }

        Ok(())
    }
}

/// Handler for an individual QUIC connection
pub struct ConnectionHandler {
    connection: quinn::Connection,
}

impl ConnectionHandler {
    pub fn new(connection: quinn::Connection) -> Self {
        Self { connection }
    }

    /// Get the remote address of the connection
    pub fn remote_address(&self) -> SocketAddr {
        self.connection.remote_address()
    }

    /// Accept the next bidirectional stream
    pub async fn accept_bi(&self) -> Result<(SendStream, RecvStream), ServerError> {
        Ok(self.connection.accept_bi().await?)
    }

    /// Accept the next unidirectional stream (for receiving)
    pub async fn accept_uni(&self) -> Result<RecvStream, ServerError> {
        Ok(self.connection.accept_uni().await?)
    }

    /// Open a bidirectional stream
    pub async fn open_bi(&self) -> Result<(SendStream, RecvStream), ServerError> {
        Ok(self.connection.open_bi().await?)
    }

    /// Open a unidirectional stream (for sending)
    pub async fn open_uni(&self) -> Result<SendStream, ServerError> {
        Ok(self.connection.open_uni().await?)
    }

    /// Run the connection handler with a stream handler
    #[instrument(skip(self, handler), fields(remote = %self.remote_address()))]
    pub async fn run<H, Fut>(&self, handler: H)
    where
        H: Fn(StreamHandler) -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        loop {
            tokio::select! {
                result = self.accept_bi() => {
                    match result {
                        Ok((send, recv)) => {
                            let handler = handler.clone();
                            tokio::spawn(async move {
                                let stream_handler = StreamHandler::new(send, recv);
                                handler(stream_handler).await;
                            });
                        }
                        Err(e) => {
                            match &e {
                                ServerError::Connection(quinn::ConnectionError::ApplicationClosed(_)) |
                                ServerError::Connection(quinn::ConnectionError::LocallyClosed) => {
                                    debug!("connection closed");
                                }
                                _ => {
                                    error!("error accepting stream: {}", e);
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Check if the connection is still open
    pub fn is_open(&self) -> bool {
        self.connection.close_reason().is_none()
    }

    /// Close the connection
    pub fn close(&self, code: u32, reason: &[u8]) {
        self.connection.close(code.into(), reason);
    }
}

/// Handler for an individual QUIC stream (bidirectional)
pub struct StreamHandler {
    send: SendStream,
    recv: RecvStream,
}

impl StreamHandler {
    pub fn new(send: SendStream, recv: RecvStream) -> Self {
        Self { send, recv }
    }

    /// Read the next frame from the stream
    pub async fn read_frame(&mut self) -> Result<Frame, ServerError> {
        Ok(read_frame(&mut self.recv).await?)
    }

    /// Write a frame to the stream
    pub async fn write_frame(&mut self, frame: &Frame) -> Result<(), ServerError> {
        Ok(write_frame(&mut self.send, frame).await?)
    }

    /// Handle a request/response pattern
    pub async fn handle_request<Req, Resp, H, Fut>(&mut self, handler: H) -> Result<(), ServerError>
    where
        Req: prost::Message + Default,
        Resp: prost::Message,
        H: FnOnce(Req) -> Fut,
        Fut: std::future::Future<Output = Result<Resp, ServerError>>,
    {
        // Read request
        let request_frame = self.read_frame().await?;
        let request: Req = request_frame.decode()?;

        // Process and respond
        match handler(request).await {
            Ok(response) => {
                let response_frame = Frame::response(&response)?;
                self.write_frame(&response_frame).await?;
            }
            Err(e) => {
                error!("request handler error: {}", e);
                // Send error frame with empty payload
                // The frame type itself indicates an error
                let error_frame = Frame {
                    message_type: crate::frame::MessageType::Error,
                    payload: bytes::Bytes::new(),
                };
                self.write_frame(&error_frame).await?;
            }
        }

        Ok(())
    }

    /// Convert to a FramedStream for more complex patterns
    pub fn into_framed(self) -> FramedStream<(SendStream, RecvStream)> {
        FramedStream::new((self.send, self.recv))
    }

    /// Finish the send stream (signal no more data)
    pub fn finish(&mut self) -> Result<(), ServerError> {
        self.send
            .finish()
            .map_err(|e| ServerError::Frame(FrameError::Io(std::io::Error::other(e))))?;
        Ok(())
    }

    /// Read raw bytes from the stream (for streaming uploads)
    /// Returns the number of bytes read, or 0 if EOF
    pub async fn read_bytes(&mut self, buf: &mut [u8]) -> Result<usize, ServerError> {
        match self.recv.read(buf).await {
            Ok(Some(n)) => Ok(n),
            Ok(None) => Ok(0), // EOF
            Err(e) => Err(ServerError::Frame(FrameError::Io(std::io::Error::other(
                e.to_string(),
            )))),
        }
    }

    /// Read exact number of bytes from the stream
    pub async fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), ServerError> {
        self.recv.read_exact(buf).await.map_err(|e| {
            ServerError::Frame(FrameError::Io(std::io::Error::other(e.to_string())))
        })?;
        Ok(())
    }

    /// Read all remaining bytes from the stream until EOF (with size limit)
    pub async fn read_to_end(&mut self, size_limit: usize) -> Result<Vec<u8>, ServerError> {
        self.recv
            .read_to_end(size_limit)
            .await
            .map_err(|e| ServerError::Frame(FrameError::Io(std::io::Error::other(e.to_string()))))
    }

    /// Stream bytes to a writer (for large uploads without buffering all in memory)
    pub async fn stream_to_writer<W: tokio::io::AsyncWrite + Unpin>(
        &mut self,
        writer: &mut W,
        expected_size: Option<u64>,
    ) -> Result<u64, ServerError> {
        use tokio::io::AsyncWriteExt;

        let mut total = 0u64;
        let mut buf = [0u8; 64 * 1024]; // 64KB chunks

        loop {
            let n = match self.recv.read(&mut buf).await {
                Ok(Some(n)) => n,
                Ok(None) => 0, // EOF
                Err(e) => {
                    return Err(ServerError::Frame(FrameError::Io(std::io::Error::other(
                        e.to_string(),
                    ))));
                }
            };
            if n == 0 {
                break;
            }
            writer.write_all(&buf[..n]).await?;
            total += n as u64;
        }

        if let Some(expected) = expected_size
            && total != expected
        {
            return Err(ServerError::Frame(FrameError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("Expected {} bytes, got {}", expected, total),
            ))));
        }

        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = RuntaraServerConfig::default();
        assert_eq!(config.bind_addr, "0.0.0.0:7001".parse().unwrap());
        assert_eq!(config.max_incoming, 10_000);
    }

    #[test]
    fn test_default_config_all_fields() {
        let config = RuntaraServerConfig::default();
        assert_eq!(config.bind_addr, "0.0.0.0:7001".parse().unwrap());
        assert!(config.cert_pem.is_empty());
        assert!(config.key_pem.is_empty());
        assert_eq!(config.max_incoming, 10_000);
        assert_eq!(config.max_bi_streams, 1_000);
        assert_eq!(config.max_uni_streams, 100);
        assert_eq!(config.idle_timeout_ms, 120_000);
        assert_eq!(config.keep_alive_interval_ms, 15_000);
        assert_eq!(config.udp_receive_buffer_size, 2 * 1024 * 1024);
        assert_eq!(config.udp_send_buffer_size, 2 * 1024 * 1024);
        assert_eq!(config.max_concurrent_handlers, 0);
    }

    #[test]
    fn test_config_clone() {
        let config = RuntaraServerConfig {
            bind_addr: "127.0.0.1:9000".parse().unwrap(),
            cert_pem: b"test-cert".to_vec(),
            key_pem: b"test-key".to_vec(),
            max_incoming: 5000,
            max_bi_streams: 50,
            max_uni_streams: 25,
            idle_timeout_ms: 60000,
            keep_alive_interval_ms: 10000,
            udp_receive_buffer_size: 1024 * 1024,
            udp_send_buffer_size: 1024 * 1024,
            max_concurrent_handlers: 500,
        };
        let cloned = config.clone();
        assert_eq!(config.bind_addr, cloned.bind_addr);
        assert_eq!(config.cert_pem, cloned.cert_pem);
        assert_eq!(config.key_pem, cloned.key_pem);
        assert_eq!(config.max_incoming, cloned.max_incoming);
        assert_eq!(config.max_bi_streams, cloned.max_bi_streams);
        assert_eq!(config.max_uni_streams, cloned.max_uni_streams);
        assert_eq!(config.idle_timeout_ms, cloned.idle_timeout_ms);
        assert_eq!(config.keep_alive_interval_ms, cloned.keep_alive_interval_ms);
        assert_eq!(
            config.udp_receive_buffer_size,
            cloned.udp_receive_buffer_size
        );
        assert_eq!(config.udp_send_buffer_size, cloned.udp_send_buffer_size);
        assert_eq!(
            config.max_concurrent_handlers,
            cloned.max_concurrent_handlers
        );
    }

    #[test]
    fn test_config_debug() {
        let config = RuntaraServerConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("RuntaraServerConfig"));
        assert!(debug_str.contains("bind_addr"));
        assert!(debug_str.contains("max_incoming"));
    }

    #[tokio::test]
    async fn test_server_localhost_creation() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = RuntaraServer::localhost(addr);
        assert!(
            server.is_ok(),
            "Failed to create localhost server: {:?}",
            server.err()
        );
    }

    #[tokio::test]
    async fn test_server_localhost_local_addr() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = RuntaraServer::localhost(addr).unwrap();
        let local_addr = server.local_addr();
        assert!(local_addr.is_ok());
        // Port 0 should have been assigned a real port
        assert!(local_addr.unwrap().port() > 0);
    }

    #[tokio::test]
    async fn test_server_close() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = RuntaraServer::localhost(addr).unwrap();
        // Closing should not panic
        server.close();
    }

    #[test]
    fn test_server_with_invalid_cert() {
        let config = RuntaraServerConfig {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            cert_pem: b"invalid-cert".to_vec(),
            key_pem: b"invalid-key".to_vec(),
            ..Default::default()
        };
        let server = RuntaraServer::new(config);
        assert!(server.is_err());
    }

    #[test]
    fn test_server_error_display() {
        let err = ServerError::Tls("invalid certificate".to_string());
        assert_eq!(format!("{}", err), "TLS error: invalid certificate");

        let err = ServerError::Closed;
        assert_eq!(format!("{}", err), "server closed");
    }

    #[test]
    fn test_connection_handler_new() {
        // We can't easily create a real Connection in tests without network,
        // but we can test that the struct exists and has expected methods
        // This is primarily a compile-time check
    }

    #[test]
    fn test_stream_handler_new() {
        // Similar to above - this verifies the API exists
        // Real testing requires integration tests with actual streams
    }

    #[tokio::test]
    async fn test_server_accept_after_close() {
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = RuntaraServer::localhost(addr).unwrap();
        server.close();
        // After close, accept should return None
        let result = server.accept().await;
        assert!(result.is_none());
    }

    #[test]
    fn test_build_server_config_empty_cert() {
        let config = RuntaraServerConfig {
            cert_pem: Vec::new(),
            key_pem: Vec::new(),
            ..Default::default()
        };
        let result = RuntaraServer::build_server_config(&config);
        // Empty cert should fail
        assert!(result.is_err());
    }

    #[test]
    fn test_build_server_config_missing_key() {
        // Generate a valid cert but provide no key
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let config = RuntaraServerConfig {
            cert_pem: cert.cert.pem().into_bytes(),
            key_pem: Vec::new(),
            ..Default::default()
        };
        let result = RuntaraServer::build_server_config(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_build_server_config_valid() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let config = RuntaraServerConfig {
            cert_pem: cert.cert.pem().into_bytes(),
            key_pem: cert.key_pair.serialize_pem().into_bytes(),
            ..Default::default()
        };
        let result = RuntaraServer::build_server_config(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_build_server_config_with_custom_limits() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let config = RuntaraServerConfig {
            bind_addr: "0.0.0.0:0".parse().unwrap(),
            cert_pem: cert.cert.pem().into_bytes(),
            key_pem: cert.key_pair.serialize_pem().into_bytes(),
            max_incoming: 1000,
            max_bi_streams: 200,
            max_uni_streams: 50,
            idle_timeout_ms: 120000,
            keep_alive_interval_ms: 20000,
            udp_receive_buffer_size: 4 * 1024 * 1024,
            udp_send_buffer_size: 4 * 1024 * 1024,
            max_concurrent_handlers: 200,
        };
        let result = RuntaraServer::build_server_config(&config);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_server_new_with_valid_config() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let config = RuntaraServerConfig {
            bind_addr: "127.0.0.1:0".parse().unwrap(),
            cert_pem: cert.cert.pem().into_bytes(),
            key_pem: cert.key_pair.serialize_pem().into_bytes(),
            ..Default::default()
        };
        let server = RuntaraServer::new(config);
        assert!(server.is_ok());
    }
}
