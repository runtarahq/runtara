// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP backend for runtara-sdk.
//!
//! Implements `SdkBackend` using HTTP/JSON to communicate with runtara-core's
//! HTTP instance API.
//!
//! Used by:
//! - Native scenarios with `RUNTARA_SDK_BACKEND=http`
//! - WASM scenarios (future, via wasi-http)

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::backend::SdkBackend;
use crate::error::{Result, SdkError};
use crate::types::{
    CheckpointResult, CustomSignal, InstanceStatus, Signal, SignalType, StatusResponse,
};

/// Configuration for the HTTP backend.
#[derive(Debug, Clone)]
pub struct HttpSdkConfig {
    /// Instance ID (required).
    pub instance_id: String,
    /// Tenant ID (required).
    pub tenant_id: String,
    /// Base URL for runtara-core HTTP API (e.g., `http://127.0.0.1:8003`).
    pub base_url: String,
    /// Request timeout in milliseconds (default: 30000).
    pub request_timeout_ms: u64,
    /// Signal poll interval in milliseconds (default: 1000).
    pub signal_poll_interval_ms: u64,
    /// Heartbeat interval in milliseconds (default: 30000, 0 to disable).
    pub heartbeat_interval_ms: u64,
}

impl HttpSdkConfig {
    /// Create config from environment variables.
    ///
    /// Required: `RUNTARA_INSTANCE_ID`, `RUNTARA_TENANT_ID`, `RUNTARA_HTTP_URL`
    pub fn from_env() -> Result<Self> {
        let instance_id = std::env::var("RUNTARA_INSTANCE_ID")
            .map_err(|_| SdkError::Config("RUNTARA_INSTANCE_ID not set".into()))?;
        let tenant_id = std::env::var("RUNTARA_TENANT_ID")
            .map_err(|_| SdkError::Config("RUNTARA_TENANT_ID not set".into()))?;

        // Try RUNTARA_HTTP_URL first, then derive from RUNTARA_SERVER_ADDR
        let base_url = if let Ok(url) = std::env::var("RUNTARA_HTTP_URL") {
            url
        } else if let Ok(addr) = std::env::var("RUNTARA_SERVER_ADDR") {
            // RUNTARA_SERVER_ADDR is host:port. HTTP is typically on port+2.
            let parts: Vec<&str> = addr.split(':').collect();
            let host = parts.first().unwrap_or(&"127.0.0.1");
            let base_port: u16 = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(8001);
            let http_port = std::env::var("RUNTARA_CORE_HTTP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(base_port + 2); // Default: base port + 2 (8001 → 8003)
            format!("http://{}:{}", host, http_port)
        } else {
            "http://127.0.0.1:8003".to_string()
        };

        let request_timeout_ms = std::env::var("RUNTARA_REQUEST_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30_000);

        let signal_poll_interval_ms = std::env::var("RUNTARA_SIGNAL_POLL_INTERVAL_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1_000);

        let heartbeat_interval_ms = std::env::var("RUNTARA_HEARTBEAT_INTERVAL_MS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(30_000);

        Ok(Self {
            instance_id,
            tenant_id,
            base_url,
            request_timeout_ms,
            signal_poll_interval_ms,
            heartbeat_interval_ms,
        })
    }
}

/// HTTP backend for the SDK.
///
/// Uses `runtara_http::HttpClient` for HTTP calls to runtara-core's HTTP instance API.
/// All operations are request-response over HTTP/JSON with base64-encoded binary data.
pub struct HttpBackend {
    instance_id: String,
    tenant_id: String,
    base_url: String,
    client: runtara_http::HttpClient,
    connected: AtomicBool,
}

impl HttpBackend {
    /// Create a new HTTP backend from config.
    pub fn new(config: &HttpSdkConfig) -> Result<Self> {
        let client = runtara_http::HttpClient::with_timeout(Duration::from_millis(
            config.request_timeout_ms,
        ));

        Ok(Self {
            instance_id: config.instance_id.clone(),
            tenant_id: config.tenant_id.clone(),
            base_url: config.base_url.trim_end_matches('/').to_string(),
            client,
            connected: AtomicBool::new(false),
        })
    }

    /// Build URL for an instance endpoint.
    fn url(&self, path: &str) -> String {
        format!(
            "{}/api/v1/instances/{}/{}",
            self.base_url, self.instance_id, path
        )
    }

    /// POST JSON to an endpoint and deserialize the response.
    fn post<T: Serialize, R: for<'de> Deserialize<'de>>(&self, url: &str, body: &T) -> Result<R> {
        let json_value = serde_json::to_value(body)
            .map_err(|e| SdkError::Internal(format!("Failed to serialize request body: {}", e)))?;

        let response = self
            .client
            .request("POST", url)
            .header("Content-Type", "application/json")
            .header("X-Runtara-Tenant-Id", &self.tenant_id)
            .header("X-Runtara-Instance-Id", &self.instance_id)
            .body_json(&json_value)
            .call()
            .map_err(|e| SdkError::Internal(format!("HTTP request failed: {}", e)))?;

        if response.status >= 400 {
            let body_text = String::from_utf8_lossy(&response.body).to_string();
            return Err(SdkError::Internal(format!(
                "HTTP request failed with status {}: {}",
                response.status, body_text
            )));
        }

        let result: R = response.into_json().map_err(|e| {
            SdkError::UnexpectedResponse(format!("Failed to parse response: {}", e))
        })?;

        Ok(result)
    }

    /// GET from an endpoint and deserialize the response.
    fn get<R: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<R> {
        let response = self
            .client
            .request("GET", url)
            .header("X-Runtara-Tenant-Id", &self.tenant_id)
            .header("X-Runtara-Instance-Id", &self.instance_id)
            .call()
            .map_err(|e| SdkError::Internal(format!("HTTP request failed: {}", e)))?;

        if response.status >= 400 {
            let body_text = String::from_utf8_lossy(&response.body).to_string();
            return Err(SdkError::Internal(format!(
                "HTTP request failed with status {}: {}",
                response.status, body_text
            )));
        }

        let result: R = response.into_json().map_err(|e| {
            SdkError::UnexpectedResponse(format!("Failed to parse response: {}", e))
        })?;

        Ok(result)
    }

    /// POST JSON fire-and-forget (ignore response body, just check status).
    fn post_fire_and_forget<T: Serialize>(&self, url: &str, body: &T) -> Result<()> {
        let json_value = serde_json::to_value(body)
            .map_err(|e| SdkError::Internal(format!("Failed to serialize request body: {}", e)))?;

        match self
            .client
            .request("POST", url)
            .header("Content-Type", "application/json")
            .header("X-Runtara-Tenant-Id", &self.tenant_id)
            .header("X-Runtara-Instance-Id", &self.instance_id)
            .body_json(&json_value)
            .call()
        {
            Ok(_) => {}
            Err(e) => {
                warn!("Fire-and-forget request failed: {}", e);
            }
        }

        Ok(())
    }
}

// ============================================================================
// JSON types for HTTP API communication
// ============================================================================

#[derive(Serialize)]
struct RegisterBody {
    tenant_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint_id: Option<String>,
}

#[derive(Deserialize)]
struct RegisterResp {
    success: bool,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Serialize)]
struct CheckpointBody {
    checkpoint_id: String,
    state: String, // base64
}

#[derive(Deserialize)]
struct CheckpointResp {
    found: bool,
    #[serde(default)]
    state: Option<String>, // base64
    #[serde(default)]
    signal: Option<SignalResp>,
    #[serde(default)]
    custom_signal: Option<CustomSignalResp>,
}

#[derive(Deserialize)]
struct SignalResp {
    signal_type: String,
    #[serde(default)]
    payload: Option<String>, // base64
}

#[derive(Deserialize)]
struct CustomSignalResp {
    checkpoint_id: String,
    #[serde(default)]
    payload: Option<String>, // base64
}

#[derive(Deserialize)]
struct PollSignalsResp {
    #[serde(default)]
    signal: Option<SignalResp>,
    #[serde(default)]
    custom_signal: Option<CustomSignalResp>,
}

#[derive(Serialize)]
struct EventBody {
    event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload: Option<String>, // base64
    #[serde(skip_serializing_if = "Option::is_none")]
    subtype: Option<String>,
}

#[derive(Serialize)]
struct SleepBody {
    duration_ms: u64,
    checkpoint_id: String,
    state: String, // base64
}

#[derive(Serialize)]
struct SignalAckBody {
    signal_type: String,
}

#[derive(Serialize)]
struct RetryBody {
    checkpoint_id: String,
    attempt: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_message: Option<String>,
}

#[derive(Deserialize)]
struct SuccessResp {
    success: bool,
}

#[derive(Deserialize)]
struct StatusResp {
    found: bool,
    #[serde(default)]
    status: String,
    #[serde(default)]
    checkpoint_id: Option<String>,
    #[serde(default)]
    output: Option<String>, // base64
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct InputResp {
    #[serde(default)]
    input: Option<String>, // base64
}

// ============================================================================
// Helper: convert signal types
// ============================================================================

fn parse_instance_status(s: &str) -> InstanceStatus {
    match s {
        "pending" => InstanceStatus::Pending,
        "running" => InstanceStatus::Running,
        "suspended" => InstanceStatus::Suspended,
        "completed" => InstanceStatus::Completed,
        "failed" => InstanceStatus::Failed,
        _ => InstanceStatus::Unknown,
    }
}

fn parse_signal_type(s: &str) -> SignalType {
    match s {
        "cancel" => SignalType::Cancel,
        "pause" => SignalType::Pause,
        "resume" => SignalType::Resume,
        "shutdown" => SignalType::Shutdown,
        _ => SignalType::Cancel, // safe default
    }
}

fn signal_type_str(st: &SignalType) -> &'static str {
    match st {
        SignalType::Cancel => "cancel",
        SignalType::Pause => "pause",
        SignalType::Resume => "resume",
        SignalType::Shutdown => "shutdown",
    }
}

/// Percent-encode a string for use in a URL path segment.
/// Encodes characters that are not allowed in path segments (e.g., `/`, `:`, `?`, `#`).
fn encode_url_path(s: &str) -> String {
    use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
    // Encode everything that's not unreserved per RFC 3986, plus `/` and `:`
    const PATH_SEGMENT: &AsciiSet = &CONTROLS
        .add(b' ')
        .add(b'"')
        .add(b'#')
        .add(b'%')
        .add(b'/')
        .add(b':')
        .add(b'<')
        .add(b'>')
        .add(b'?')
        .add(b'@')
        .add(b'[')
        .add(b']')
        .add(b'^')
        .add(b'{')
        .add(b'|')
        .add(b'}');
    utf8_percent_encode(s, PATH_SEGMENT).to_string()
}

fn decode_b64(s: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .unwrap_or_default()
}

fn encode_b64(data: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn parse_signal(resp: &SignalResp) -> Signal {
    Signal {
        signal_type: parse_signal_type(&resp.signal_type),
        payload: resp.payload.as_deref().map(decode_b64).unwrap_or_default(),
        checkpoint_id: None,
    }
}

fn parse_custom_signal(resp: &CustomSignalResp) -> CustomSignal {
    CustomSignal {
        checkpoint_id: resp.checkpoint_id.clone(),
        payload: resp.payload.as_deref().map(decode_b64).unwrap_or_default(),
    }
}

// ============================================================================
// SdkBackend implementation
// ============================================================================

impl SdkBackend for HttpBackend {
    fn connect(&self) -> Result<()> {
        // HTTP is connectionless — verify reachability with a health check
        let url = format!("{}/health", self.base_url);
        let resp = self.client.request("GET", &url).call().map_err(|e| {
            SdkError::Internal(format!("Cannot reach runtara-core HTTP API: {}", e))
        })?;

        if resp.status >= 200 && resp.status < 300 {
            self.connected.store(true, Ordering::SeqCst);
            info!(base_url = %self.base_url, "Connected to runtara-core HTTP API");
            Ok(())
        } else {
            Err(SdkError::Config(format!(
                "Health check returned {}",
                resp.status
            )))
        }
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn close(&self) {
        self.connected.store(false, Ordering::SeqCst);
        debug!("HTTP backend closed");
    }

    fn register(&self, checkpoint_id: Option<&str>) -> Result<()> {
        let body = RegisterBody {
            tenant_id: self.tenant_id.clone(),
            checkpoint_id: checkpoint_id.map(|s| s.to_string()),
        };

        let resp: RegisterResp = self.post(&self.url("register"), &body)?;

        if resp.success {
            info!("Instance registered via HTTP");
            Ok(())
        } else {
            Err(SdkError::UnexpectedResponse(format!(
                "Registration failed: {}",
                resp.error.unwrap_or_default()
            )))
        }
    }

    fn instance_id(&self) -> &str {
        &self.instance_id
    }

    fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    fn checkpoint(&self, checkpoint_id: &str, state: &[u8]) -> Result<CheckpointResult> {
        let body = CheckpointBody {
            checkpoint_id: checkpoint_id.to_string(),
            state: encode_b64(state),
        };

        let resp: CheckpointResp = self.post(&self.url("checkpoint"), &body)?;

        Ok(CheckpointResult {
            found: resp.found,
            state: resp.state.as_deref().map(decode_b64).unwrap_or_default(),
            pending_signal: resp.signal.as_ref().map(parse_signal),
            custom_signal: resp.custom_signal.as_ref().map(parse_custom_signal),
        })
    }

    fn get_checkpoint(&self, checkpoint_id: &str) -> Result<Option<Vec<u8>>> {
        // Use checkpoint endpoint with empty state to check if exists
        // The HTTP API's checkpoint endpoint handles this: if checkpoint exists, returns it
        let body = CheckpointBody {
            checkpoint_id: checkpoint_id.to_string(),
            state: encode_b64(&[]),
        };

        let resp: CheckpointResp = self.post(&self.url("checkpoint"), &body)?;

        if resp.found {
            Ok(Some(
                resp.state.as_deref().map(decode_b64).unwrap_or_default(),
            ))
        } else {
            Ok(None)
        }
    }

    fn heartbeat(&self) -> Result<()> {
        let body = EventBody {
            event_type: "heartbeat".to_string(),
            checkpoint_id: None,
            payload: None,
            subtype: None,
        };

        self.post_fire_and_forget(&self.url("events"), &body)
    }

    fn completed(&self, output: &[u8]) -> Result<()> {
        let body = serde_json::json!({ "output": encode_b64(output) });
        let resp: SuccessResp = self.post(&self.url("completed"), &body)?;

        if resp.success {
            Ok(())
        } else {
            Err(SdkError::UnexpectedResponse(
                "Failed to report completion".into(),
            ))
        }
    }

    fn failed(&self, error: &str) -> Result<()> {
        let body = serde_json::json!({ "error": error });
        let resp: SuccessResp = self.post(&self.url("failed"), &body)?;

        if resp.success {
            Ok(())
        } else {
            Err(SdkError::UnexpectedResponse(
                "Failed to report failure".into(),
            ))
        }
    }

    fn suspended(&self) -> Result<()> {
        let resp: SuccessResp = self.post(&self.url("suspended"), &serde_json::json!({}))?;

        if resp.success {
            Ok(())
        } else {
            Err(SdkError::UnexpectedResponse(
                "Failed to report suspension".into(),
            ))
        }
    }

    fn sleep_until(&self, checkpoint_id: &str, wake_at: DateTime<Utc>, state: &[u8]) -> Result<()> {
        let now = Utc::now();
        let duration_ms = if wake_at > now {
            (wake_at - now).num_milliseconds() as u64
        } else {
            0
        };

        self.durable_sleep(Duration::from_millis(duration_ms), checkpoint_id, state)
    }

    fn durable_sleep(&self, duration: Duration, checkpoint_id: &str, state: &[u8]) -> Result<()> {
        let body = SleepBody {
            duration_ms: duration.as_millis() as u64,
            checkpoint_id: checkpoint_id.to_string(),
            state: encode_b64(state),
        };

        let resp: SuccessResp = self.post(&self.url("sleep"), &body)?;

        if resp.success {
            Ok(())
        } else {
            Err(SdkError::UnexpectedResponse(
                "Durable sleep request failed".into(),
            ))
        }
    }

    fn set_sleep_until(&self, _sleep_until: DateTime<Utc>) -> Result<()> {
        // Server-side managed — no-op for HTTP backend
        Ok(())
    }

    fn clear_sleep(&self) -> Result<()> {
        // Server-side managed — no-op for HTTP backend
        Ok(())
    }

    fn get_sleep_until(&self) -> Result<Option<DateTime<Utc>>> {
        // Would need a separate endpoint; not currently needed by SDK
        Ok(None)
    }

    fn send_custom_event(&self, subtype: &str, payload: Vec<u8>) -> Result<()> {
        let body = EventBody {
            event_type: "custom".to_string(),
            checkpoint_id: None,
            payload: Some(encode_b64(&payload)),
            subtype: Some(subtype.to_string()),
        };

        let resp: SuccessResp = self.post(&self.url("events"), &body)?;

        if resp.success {
            Ok(())
        } else {
            Err(SdkError::UnexpectedResponse("Custom event failed".into()))
        }
    }

    fn record_retry_attempt(
        &self,
        checkpoint_id: &str,
        attempt_number: u32,
        error_message: Option<&str>,
    ) -> Result<()> {
        let body = RetryBody {
            checkpoint_id: checkpoint_id.to_string(),
            attempt: attempt_number,
            error_message: error_message.map(|s| s.to_string()),
        };

        self.post_fire_and_forget(&self.url("retry"), &body)
    }

    fn get_status(&self) -> Result<StatusResponse> {
        self.get_instance_status(&self.instance_id)
    }

    fn poll_signals(
        &self,
        checkpoint_id: Option<&str>,
    ) -> Result<(Option<Signal>, Option<CustomSignal>)> {
        let url = match checkpoint_id {
            Some(cp_id) => format!(
                "{}/api/v1/instances/{}/signals/{}",
                self.base_url,
                self.instance_id,
                encode_url_path(cp_id)
            ),
            None => format!(
                "{}/api/v1/instances/{}/signals",
                self.base_url, self.instance_id
            ),
        };

        let resp: PollSignalsResp = self.get(&url)?;
        let signal = resp.signal.as_ref().map(parse_signal);
        let custom = resp.custom_signal.as_ref().map(parse_custom_signal);
        Ok((signal, custom))
    }

    fn acknowledge_signal(&self, signal_type: SignalType) -> Result<()> {
        let body = SignalAckBody {
            signal_type: signal_type_str(&signal_type).to_string(),
        };

        let _: SuccessResp = self.post(&self.url("signals/ack"), &body)?;
        Ok(())
    }

    fn get_instance_status(&self, instance_id: &str) -> Result<StatusResponse> {
        let url = format!("{}/api/v1/instances/{}/status", self.base_url, instance_id);

        let resp: StatusResp = self.get(&url)?;

        Ok(StatusResponse {
            found: resp.found,
            status: parse_instance_status(&resp.status),
            checkpoint_id: resp.checkpoint_id,
            output: resp.output.as_deref().map(decode_b64),
            error: resp.error,
        })
    }

    fn load_input(&self) -> Result<Option<Vec<u8>>> {
        let url = format!(
            "{}/api/v1/instances/{}/input",
            self.base_url, self.instance_id
        );

        let resp: InputResp = self.get(&url)?;
        Ok(resp.input.as_deref().map(decode_b64))
    }
}

impl std::fmt::Debug for HttpBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpBackend")
            .field("instance_id", &self.instance_id)
            .field("tenant_id", &self.tenant_id)
            .field("base_url", &self.base_url)
            .field("connected", &self.connected.load(Ordering::SeqCst))
            .finish()
    }
}
