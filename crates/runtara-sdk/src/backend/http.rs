// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! HTTP backend for runtara-sdk.
//!
//! Implements `SdkBackend` using HTTP/JSON to communicate with runtara-core's
//! HTTP instance API.
//!
//! Used by:
//! - Native workflows with `RUNTARA_SDK_BACKEND=http`
//! - WASM workflows (future, via wasi-http)

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::tracing_compat::{debug, info, warn};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
    /// Required: `RUNTARA_INSTANCE_ID`, `RUNTARA_TENANT_ID`.
    /// Optional: `RUNTARA_HTTP_URL` (default `http://127.0.0.1:8003`).
    pub fn from_env() -> Result<Self> {
        let instance_id = std::env::var("RUNTARA_INSTANCE_ID")
            .map_err(|_| SdkError::Config("RUNTARA_INSTANCE_ID not set".into()))?;
        let tenant_id = std::env::var("RUNTARA_TENANT_ID")
            .map_err(|_| SdkError::Config("RUNTARA_TENANT_ID not set".into()))?;

        let base_url = std::env::var("RUNTARA_HTTP_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8003".to_string());

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
    /// The deadline the client applies to every request, kept alongside the
    /// client that enforces it because durable sleep has to reason about it —
    /// see [`HttpBackend::reject_sleep_beyond_request_timeout`].
    request_timeout_ms: u64,
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
            request_timeout_ms: config.request_timeout_ms,
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

    /// Refuse a durable sleep the client deadline cannot outlast.
    ///
    /// `POST .../sleep` does not answer until the sleep is over: core holds the
    /// response open for the whole duration. This backend's client, meanwhile,
    /// aborts any request that produces no first byte within
    /// `request_timeout_ms`. A sleep at or beyond that ceiling therefore cannot
    /// succeed — the abort always lands first, and it lands at the timeout, not
    /// at the requested duration.
    ///
    /// Left to run, the failure names neither sleep nor the ceiling: it reads as
    /// a server that stopped answering. Refusing up front costs milliseconds
    /// instead of the whole timeout and says which knob moves the ceiling.
    ///
    /// A zero timeout is left alone deliberately. It is a degenerate
    /// configuration under which no request of any kind can complete, so
    /// reporting it against the sleep would blame the sleep for something that
    /// is not about sleeping at all.
    fn reject_sleep_beyond_request_timeout(&self, duration_ms: u64) -> Result<()> {
        let timeout_ms = self.request_timeout_ms;
        if timeout_ms == 0 || duration_ms < timeout_ms {
            return Ok(());
        }

        Err(SdkError::Sleep(format!(
            "durable sleep of {duration_ms}ms does not fit inside the {timeout_ms}ms client \
             request timeout: this backend holds the sleep request open for the whole duration, \
             so the client aborts it at {timeout_ms}ms before the server ever answers. Raise \
             RUNTARA_REQUEST_TIMEOUT_MS above the sleep duration, or run the workflow on the \
             host-import binding, whose durable sleep issues no request at all."
        )))
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
    #[cfg_attr(not(feature = "tracing"), allow(unused_variables))]
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
        let duration_ms = duration.as_millis() as u64;
        self.reject_sleep_beyond_request_timeout(duration_ms)?;

        let body = SleepBody {
            duration_ms,
            checkpoint_id: checkpoint_id.to_string(),
            state: encode_b64(state),
        };

        // Re-label a transport failure as a sleep failure. The generic text is
        // "HTTP request failed", which for the one request that is *meant* to
        // take minutes reads as an unrelated network fault; naming the sleep and
        // its duration is what points a reader at the right thing.
        let resp: SuccessResp = self.post(&self.url("sleep"), &body).map_err(|error| {
            SdkError::Sleep(format!("durable sleep of {duration_ms}ms failed: {error}"))
        })?;

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

#[cfg(test)]
mod config_tests {
    use super::HttpSdkConfig;
    use std::sync::Mutex;

    // Env access in tests is mutex-serialized.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        vars: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn new() -> Self {
            Self { vars: Vec::new() }
        }

        fn set(&mut self, key: &str, value: &str) {
            let old = std::env::var(key).ok();
            self.vars.push((key.to_string(), old));
            // SAFETY: serialized by ENV_MUTEX.
            unsafe { std::env::set_var(key, value) };
        }

        fn remove(&mut self, key: &str) {
            let old = std::env::var(key).ok();
            self.vars.push((key.to_string(), old));
            // SAFETY: serialized by ENV_MUTEX.
            unsafe { std::env::remove_var(key) };
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in self.vars.drain(..).rev() {
                // SAFETY: serialized by ENV_MUTEX.
                unsafe {
                    match value {
                        Some(v) => std::env::set_var(&key, v),
                        None => std::env::remove_var(&key),
                    }
                }
            }
        }
    }

    #[test]
    fn test_http_sdk_config_ignores_server_addr() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();
        guard.set("RUNTARA_INSTANCE_ID", "test-instance");
        guard.set("RUNTARA_TENANT_ID", "test-tenant");
        guard.remove("RUNTARA_HTTP_URL");
        guard.set("RUNTARA_SERVER_ADDR", "10.0.0.1:9999");
        guard.set("RUNTARA_CORE_HTTP_PORT", "9001");

        let cfg = HttpSdkConfig::from_env().unwrap();

        assert_eq!(cfg.base_url, "http://127.0.0.1:8003");
    }

    #[test]
    fn test_http_sdk_config_uses_http_url() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut guard = EnvGuard::new();
        guard.set("RUNTARA_INSTANCE_ID", "test-instance");
        guard.set("RUNTARA_TENANT_ID", "test-tenant");
        guard.set("RUNTARA_HTTP_URL", "http://example.test:1234");

        let cfg = HttpSdkConfig::from_env().unwrap();

        assert_eq!(cfg.base_url, "http://example.test:1234");
    }
}

#[cfg(test)]
mod sleep_ceiling_tests {
    use super::{HttpBackend, HttpSdkConfig};
    use crate::backend::SdkBackend;
    use crate::error::SdkError;
    use std::time::Duration;

    fn backend_at(base_url: String, request_timeout_ms: u64) -> HttpBackend {
        HttpBackend::new(&HttpSdkConfig {
            instance_id: "test-instance".to_string(),
            tenant_id: "test-tenant".to_string(),
            base_url,
            request_timeout_ms,
            signal_poll_interval_ms: 1_000,
            heartbeat_interval_ms: 30_000,
        })
        .expect("backend construction is infallible for a well-formed config")
    }

    /// `base_url` is deliberately a dead port: these assertions are about sleeps
    /// that never get as far as building a request, so a backend that stopped
    /// refusing would fail by dialling rather than pass silently.
    fn backend(request_timeout_ms: u64) -> HttpBackend {
        backend_at("http://127.0.0.1:1".to_string(), request_timeout_ms)
    }

    /// A stand-in core that accepts the connection and then answers nothing —
    /// what a sleep looks like on the wire for its whole duration, since
    /// `handle_sleep` writes no byte until the deadline passes.
    ///
    /// The accept loop runs detached and holds each connection open; it lives
    /// for the process, which outlasts anything that would notice.
    fn silent_core() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind an ephemeral port");
        let addr = listener.local_addr().expect("bound address");
        std::thread::spawn(move || {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept() {
                held.push(stream);
            }
        });
        format!("http://{addr}")
    }

    /// The wiring, not just the predicate: `durable_sleep` itself must refuse,
    /// and the refusal must carry both numbers and the knob that moves the
    /// ceiling. The failure it replaces named neither — it arrived at the
    /// timeout, not at the requested duration, and read as a dead server.
    #[test]
    fn durable_sleep_refuses_a_duration_the_request_timeout_cannot_outlast() {
        let error = backend(30_000)
            .durable_sleep(Duration::from_millis(35_000), "delay", b"")
            .expect_err("a 35s sleep cannot survive a 30s request timeout");

        assert!(matches!(error, SdkError::Sleep(_)), "got {error:?}");
        let message = error.to_string();
        assert!(message.contains("35000ms"), "{message}");
        assert!(message.contains("30000ms"), "{message}");
        assert!(message.contains("RUNTARA_REQUEST_TIMEOUT_MS"), "{message}");
    }

    /// The boundary is inclusive. A sleep of exactly the timeout still loses:
    /// the response cannot be written until the sleep is over, which is a moment
    /// after the client has already given up.
    #[test]
    fn a_sleep_of_exactly_the_timeout_is_refused_and_one_below_it_is_not() {
        assert!(
            backend(30_000)
                .reject_sleep_beyond_request_timeout(30_000)
                .is_err()
        );
        assert!(
            backend(30_000)
                .reject_sleep_beyond_request_timeout(29_999)
                .is_ok()
        );
    }

    /// Raising the timeout raises the ceiling with it, so a deployment
    /// configured for long sleeps keeps working.
    #[test]
    fn a_raised_request_timeout_raises_the_ceiling() {
        assert!(
            backend(300_000)
                .reject_sleep_beyond_request_timeout(35_000)
                .is_ok()
        );
    }

    /// Zero is not a ceiling of zero. It is a configuration under which no
    /// request of any kind completes, so reporting it against the sleep would
    /// blame the sleep for something that has nothing to do with sleeping.
    #[test]
    fn a_zero_timeout_is_not_treated_as_a_ceiling() {
        assert!(
            backend(0)
                .reject_sleep_beyond_request_timeout(35_000)
                .is_ok()
        );
    }

    /// The refusal is a SHORT-CIRCUIT, not a nicer message on the same wait.
    /// Against a core that answers nothing, an over-ceiling sleep must come back
    /// long before the deadline it could never have met — which is the whole
    /// point: the old failure arrived at the timeout, making a misconfigured
    /// sleep look like a server that had stopped responding.
    #[test]
    fn an_over_ceiling_sleep_is_refused_without_waiting_out_the_timeout() {
        let backend = backend_at(silent_core(), 1_500);

        let started = std::time::Instant::now();
        let error = backend
            .durable_sleep(Duration::from_millis(2_000), "delay", b"")
            .expect_err("a 2s sleep cannot survive a 1.5s request timeout");
        let elapsed = started.elapsed();

        assert!(matches!(error, SdkError::Sleep(_)), "got {error:?}");
        assert!(
            elapsed < Duration::from_millis(1_000),
            "the refusal must not wait out the timeout it is predicting; took {elapsed:?}"
        );
    }

    /// The other half: a sleep INSIDE the ceiling is still issued, and when the
    /// request dies anyway the failure names the sleep instead of reading as an
    /// unrelated transport fault. This is the case the pre-flight cannot catch —
    /// a duration the client would have tolerated, on a request that overran
    /// regardless.
    #[test]
    fn a_sleep_inside_the_ceiling_is_issued_and_its_failure_names_the_sleep() {
        let backend = backend_at(silent_core(), 1_000);

        let error = backend
            .durable_sleep(Duration::from_millis(500), "delay", b"")
            .expect_err("the stand-in core never answers, so the request times out");

        assert!(matches!(error, SdkError::Sleep(_)), "got {error:?}");
        let message = error.to_string();
        assert!(
            message.contains("durable sleep of 500ms failed"),
            "{message}"
        );
    }
}
