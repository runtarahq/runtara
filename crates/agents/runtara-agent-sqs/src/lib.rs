//! AWS SQS agent — WebAssembly component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_sqs.meta.json` next to the
//! `.wasm` — the JSON is a build artifact, never hand-edited.
//!
//! Routing model: the `runtara-http` client reads `RUNTARA_HTTP_PROXY_URL` and
//! forwards every request through the proxy as a JSON envelope. The
//! `X-Runtara-Connection-Id` header causes the proxy to resolve the connection
//! and attach AWS SigV4 signing; the `X-Runtara-Aws-Service: sqs` header names
//! the service so a single generic `aws_credentials` connection can serve SQS
//! (and any other AWS service) without a per-service connection type. The
//! component never sees AWS credentials and never signs requests itself.
//!
//! Wire protocol: the AWS JSON protocol (JSON 1.0). Every operation is a
//! `POST /` with `X-Amz-Target: AmazonSQS.<Operation>`,
//! `Content-Type: application/x-amz-json-1.0`, and a JSON request/response
//! body — so no XML parser is needed in the component. The proxy synthesizes
//! the regional endpoint `https://sqs.{region}.amazonaws.com` from the
//! connection's region (or an explicit `endpoint` override for LocalStack /
//! VPC endpoints). Server-side encryption (SSE-KMS / SSE-SQS) is a *queue*
//! attribute set at create/update time — never per message — so custom KMS key
//! ids live on `create-queue` / `set-queue-attributes`. See
//! docs/sqs-agent-plan.md.
#![allow(clippy::result_large_err)]

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::time::Duration;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;

// ============================================================================
// Local AgentError shim (mirrors runtara-agent-s3-storage / -mailgun / -hubspot)
// ============================================================================

#[derive(Debug, Clone, Serialize)]
pub struct AgentError {
    pub code: String,
    pub message: String,
    pub category: &'static str,
    pub severity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub attributes: HashMap<String, Value>,
}

impl AgentError {
    pub fn permanent(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "permanent",
            severity: "error",
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn transient(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            category: "transient",
            severity: "warning",
            retry_after_ms: None,
            attributes: HashMap::new(),
        }
    }

    pub fn with_attr(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.attributes
            .insert(key.into(), Value::String(value.into()));
        self
    }
}

impl From<AgentError> for String {
    fn from(err: AgentError) -> Self {
        serde_json::to_string(&err).unwrap_or_else(|_| format!("[{}] {}", err.code, err.message))
    }
}

// ============================================================================
// RawConnection (local mirror of crates/runtara-agents/src/connections.rs)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawConnection {
    #[serde(default)]
    pub connection_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_subtype: Option<String>,
    pub integration_id: String,
    pub parameters: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit_config: Option<Value>,
}

// ============================================================================
// Shared SQS helpers
// ============================================================================

/// Request timeout. Covers ReceiveMessage long-polling (max 20s) plus margin.
const SQS_TIMEOUT: Duration = Duration::from_secs(65);

/// The AWS service name declared to the proxy for SigV4 signing + endpoint.
const AWS_SERVICE: &str = "sqs";

/// AWS JSON protocol content type for SQS.
const SQS_CONTENT_TYPE: &str = "application/x-amz-json-1.0";

fn require_connection(connection: &Option<RawConnection>) -> Result<&RawConnection, AgentError> {
    connection.as_ref().ok_or_else(|| {
        AgentError::permanent(
            "SQS_MISSING_CONNECTION",
            "No AWS connection configured. Add an aws_credentials connection to this step.",
        )
        .with_attr("integration", "aws_credentials")
    })
}

/// A parsed SQS response: HTTP status plus the JSON body (Null when empty).
struct SqsResp {
    status: u16,
    body: Value,
}

impl SqsResp {
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// POST an AWS JSON-protocol request to SQS via the runtara proxy. Credentials
/// and SigV4 signing are applied server-side; this component only names the
/// connection and the AWS service.
fn sqs_call(target: &str, connection_id: &str, body: &Value) -> Result<SqsResp, AgentError> {
    let payload = serde_json::to_vec(body).map_err(|e| {
        AgentError::permanent(
            "SQS_ENCODE_ERROR",
            format!("Failed to encode {target}: {e}"),
        )
    })?;

    let client = runtara_http::HttpClient::with_timeout(SQS_TIMEOUT);
    let resp = client
        .request("POST", "/")
        .header("X-Runtara-Connection-Id", connection_id)
        .header("X-Runtara-Aws-Service", AWS_SERVICE)
        .header("X-Amz-Target", target)
        .header("Content-Type", SQS_CONTENT_TYPE)
        .body_bytes(&payload)
        .call_agent()
        .map_err(|e| {
            AgentError::transient("SQS_NETWORK_ERROR", format!("{target} request failed: {e}"))
                .with_attr("integration", "aws_credentials")
        })?;

    let body = if resp.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&resp.body).unwrap_or(Value::Null)
    };
    Ok(SqsResp {
        status: resp.status,
        body,
    })
}

/// Extract a human-readable message from an SQS JSON error body
/// (`{"__type": "...#QueueDoesNotExist", "message": "..."}`).
fn sqs_error(target: &str, resp: &SqsResp) -> String {
    let ty = resp.body.get("__type").and_then(Value::as_str);
    let msg = resp
        .body
        .get("message")
        .or_else(|| resp.body.get("Message"))
        .and_then(Value::as_str);
    // `__type` is fully qualified (`com.amazonaws.sqs#QueueDoesNotExist`);
    // surface the short error name.
    let short = ty.map(|t| t.rsplit(['#', '.']).next().unwrap_or(t));
    match (short, msg) {
        (Some(t), Some(m)) => format!("{t}: {m}"),
        (Some(t), None) => t.to_string(),
        (None, Some(m)) => m.to_string(),
        (None, None) => format!("{target} failed (HTTP {})", resp.status),
    }
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Convert a flat `{name: value}` map into the SQS `MessageAttributes` wire
/// shape (`{name: {DataType: "String", StringValue: value}}`). Binary and
/// Number attribute types are a future extension; the common case is strings.
fn message_attributes_json(attrs: &HashMap<String, String>) -> Value {
    let mut map = Map::new();
    for (name, value) in attrs {
        map.insert(
            name.clone(),
            json!({ "DataType": "String", "StringValue": value }),
        );
    }
    Value::Object(map)
}

/// Merge a passthrough attribute map with the typed convenience fields into the
/// SQS `Attributes` wire shape (all values are strings). Typed fields win over
/// passthrough entries of the same key. Returns `None` when nothing is set.
#[allow(clippy::too_many_arguments)]
fn merge_queue_attributes(
    passthrough: Option<&HashMap<String, String>>,
    kms_master_key_id: Option<&str>,
    kms_data_key_reuse_period_seconds: Option<u32>,
    sqs_managed_sse_enabled: Option<bool>,
    fifo_queue: Option<bool>,
    content_based_deduplication: Option<bool>,
) -> Option<Value> {
    let mut map = Map::new();
    if let Some(p) = passthrough {
        for (k, v) in p {
            map.insert(k.clone(), Value::String(v.clone()));
        }
    }
    if let Some(k) = kms_master_key_id {
        map.insert("KmsMasterKeyId".into(), Value::String(k.to_string()));
    }
    if let Some(n) = kms_data_key_reuse_period_seconds {
        map.insert(
            "KmsDataKeyReusePeriodSeconds".into(),
            Value::String(n.to_string()),
        );
    }
    if let Some(b) = sqs_managed_sse_enabled {
        map.insert("SqsManagedSseEnabled".into(), Value::String(b.to_string()));
    }
    if let Some(b) = fifo_queue {
        map.insert("FifoQueue".into(), Value::String(b.to_string()));
    }
    if let Some(b) = content_based_deduplication {
        map.insert(
            "ContentBasedDeduplication".into(),
            Value::String(b.to_string()),
        );
    }
    if map.is_empty() {
        None
    } else {
        Some(Value::Object(map))
    }
}

fn parse_messages(v: &Value) -> Vec<SqsMessage> {
    v.get("Messages")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|e| SqsMessage {
                    message_id: str_field(e, "MessageId"),
                    receipt_handle: str_field(e, "ReceiptHandle"),
                    body: str_field(e, "Body"),
                    md5_of_body: opt_str(e, "MD5OfBody"),
                    attributes: e.get("Attributes").cloned(),
                    message_attributes: e.get("MessageAttributes").cloned(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_batch_success(v: &Value) -> Vec<BatchSuccessEntry> {
    v.get("Successful")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|e| BatchSuccessEntry {
                    id: str_field(e, "Id"),
                    message_id: opt_str(e, "MessageId"),
                    sequence_number: opt_str(e, "SequenceNumber"),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_batch_failed(v: &Value) -> Vec<BatchFailedEntry> {
    v.get("Failed")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|e| BatchFailedEntry {
                    id: str_field(e, "Id"),
                    code: opt_str(e, "Code"),
                    message: opt_str(e, "Message"),
                    sender_fault: e.get("SenderFault").and_then(Value::as_bool),
                })
                .collect()
        })
        .unwrap_or_default()
}

// ============================================================================
// Shared output / entry structs (Serialize/Deserialize only — nested types are
// referenced by name in the metadata, they don't need their own derives)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqsMessage {
    pub message_id: String,
    pub receipt_handle: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub md5_of_body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_attributes: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSuccessEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchFailedEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_fault: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendBatchEntry {
    pub id: String,
    pub message_body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_seconds: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_deduplication_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_attributes: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteBatchEntry {
    pub id: String,
    pub receipt_handle: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisibilityBatchEntry {
    pub id: String,
    pub receipt_handle: String,
    pub visibility_timeout: u32,
}

/// Shared `{success, error}` acknowledgement output for operations that return
/// no data (delete, purge, change-visibility, set-attributes, tag, untag).
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Acknowledgement")]
pub struct AckOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn ack(target: &str, resp: SqsResp) -> AckOutput {
    if resp.ok() {
        AckOutput {
            success: true,
            error: None,
        }
    } else {
        AckOutput {
            success: false,
            error: Some(sqs_error(target, &resp)),
        }
    }
}

// ============================================================================
// Capability 1: Send Message
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Send Message Input")]
pub struct SendMessageInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        description = "Full URL of the SQS queue",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,

    #[field(
        display_name = "Message Body",
        description = "The message payload (UTF-8 string, max 256 KB)",
        example = "{\"orderId\":42}"
    )]
    pub message_body: String,

    #[field(
        display_name = "Delay Seconds",
        description = "Seconds to delay delivery (0–900). Ignored on FIFO queues.",
        example = "0"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_seconds: Option<u32>,

    #[field(
        display_name = "Message Group ID",
        description = "FIFO only: groups messages that must be processed in order"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_group_id: Option<String>,

    #[field(
        display_name = "Message Deduplication ID",
        description = "FIFO only: dedup token (omit when content-based dedup is on)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_deduplication_id: Option<String>,

    #[field(
        display_name = "Message Attributes",
        description = "Optional string-valued message attributes (name → value)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_attributes: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Send Message Output")]
pub struct SendMessageOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Message ID")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,

    #[field(
        display_name = "Sequence Number",
        description = "FIFO only: the large, non-consecutive sequence number"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_number: Option<String>,

    #[field(display_name = "MD5 of Message Body")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub md5_of_message_body: Option<String>,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn send_message_body(input: &SendMessageInput) -> Value {
    let mut body = Map::new();
    body.insert("QueueUrl".into(), json!(input.queue_url));
    body.insert("MessageBody".into(), json!(input.message_body));
    if let Some(d) = input.delay_seconds {
        body.insert("DelaySeconds".into(), json!(d));
    }
    if let Some(g) = &input.message_group_id {
        body.insert("MessageGroupId".into(), json!(g));
    }
    if let Some(d) = &input.message_deduplication_id {
        body.insert("MessageDeduplicationId".into(), json!(d));
    }
    if let Some(attrs) = &input.message_attributes {
        body.insert("MessageAttributes".into(), message_attributes_json(attrs));
    }
    Value::Object(body)
}

#[capability(
    id = "queue-send-message",
    module = "sqs",
    display_name = "Send Message",
    description = "Send a message to an SQS queue.",
    side_effects = true,
    idempotent = false,
    module_display_name = "SQS",
    module_description = "AWS SQS: send, receive, and manage messages and queues, including SSE-KMS queue encryption. Credentials are injected server-side by the runtara HTTP proxy.",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "aws_credentials",
    module_secure = true
)]
pub fn queue_send_message(input: SendMessageInput) -> Result<SendMessageOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = send_message_body(&input);
    let resp = sqs_call("AmazonSQS.SendMessage", &connection.connection_id, &body)?;

    Ok(if resp.ok() {
        SendMessageOutput {
            success: true,
            message_id: opt_str(&resp.body, "MessageId"),
            sequence_number: opt_str(&resp.body, "SequenceNumber"),
            md5_of_message_body: opt_str(&resp.body, "MD5OfMessageBody"),
            error: None,
        }
    } else {
        SendMessageOutput {
            success: false,
            message_id: None,
            sequence_number: None,
            md5_of_message_body: None,
            error: Some(sqs_error("SendMessage", &resp)),
        }
    })
}

// ============================================================================
// Capability 2: Send Message Batch
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Send Message Batch Input")]
pub struct SendMessageBatchInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,

    #[field(
        display_name = "Entries",
        description = "Up to 10 messages, each with a unique batch id and a body"
    )]
    pub entries: Vec<SendBatchEntry>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Batch Result Output")]
pub struct BatchResultOutput {
    #[field(
        display_name = "Success",
        description = "True when the request itself succeeded (individual entries may still have failed)"
    )]
    pub success: bool,

    #[field(display_name = "Successful", description = "Per-entry successes")]
    pub successful: Vec<BatchSuccessEntry>,

    #[field(display_name = "Failed", description = "Per-entry failures")]
    pub failed: Vec<BatchFailedEntry>,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn send_message_batch_body(input: &SendMessageBatchInput) -> Value {
    let entries: Vec<Value> = input
        .entries
        .iter()
        .map(|e| {
            let mut m = Map::new();
            m.insert("Id".into(), json!(e.id));
            m.insert("MessageBody".into(), json!(e.message_body));
            if let Some(d) = e.delay_seconds {
                m.insert("DelaySeconds".into(), json!(d));
            }
            if let Some(g) = &e.message_group_id {
                m.insert("MessageGroupId".into(), json!(g));
            }
            if let Some(d) = &e.message_deduplication_id {
                m.insert("MessageDeduplicationId".into(), json!(d));
            }
            if let Some(attrs) = &e.message_attributes {
                m.insert("MessageAttributes".into(), message_attributes_json(attrs));
            }
            Value::Object(m)
        })
        .collect();
    json!({ "QueueUrl": input.queue_url, "Entries": entries })
}

#[capability(
    id = "queue-send-message-batch",
    module = "sqs",
    display_name = "Send Message Batch",
    description = "Send up to 10 messages to an SQS queue in a single request.",
    side_effects = true,
    idempotent = false
)]
pub fn queue_send_message_batch(
    input: SendMessageBatchInput,
) -> Result<BatchResultOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = send_message_batch_body(&input);
    let resp = sqs_call(
        "AmazonSQS.SendMessageBatch",
        &connection.connection_id,
        &body,
    )?;

    Ok(if resp.ok() {
        BatchResultOutput {
            success: true,
            successful: parse_batch_success(&resp.body),
            failed: parse_batch_failed(&resp.body),
            error: None,
        }
    } else {
        BatchResultOutput {
            success: false,
            successful: vec![],
            failed: vec![],
            error: Some(sqs_error("SendMessageBatch", &resp)),
        }
    })
}

// ============================================================================
// Capability 3: Receive Messages
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Receive Messages Input")]
pub struct ReceiveMessagesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,

    #[field(
        display_name = "Max Number of Messages",
        description = "Maximum messages to return (1–10, default 1)",
        example = "10"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_number_of_messages: Option<u32>,

    #[field(
        display_name = "Wait Time Seconds",
        description = "Long-poll duration (0–20). 0 = short poll.",
        example = "20"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_time_seconds: Option<u32>,

    #[field(
        display_name = "Visibility Timeout",
        description = "Seconds the received messages are hidden from other consumers"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility_timeout: Option<u32>,

    #[field(
        display_name = "Attribute Names",
        description = "System attributes to include, e.g. [\"All\"] or [\"SentTimestamp\"]"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute_names: Option<Vec<String>>,

    #[field(
        display_name = "Message Attribute Names",
        description = "Message attributes to include, e.g. [\"All\"]"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_attribute_names: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Receive Messages Output")]
pub struct ReceiveMessagesOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Messages",
        description = "Received messages (each carries a receipt handle for delete/visibility)"
    )]
    pub messages: Vec<SqsMessage>,

    #[field(display_name = "Count", description = "Number of messages returned")]
    pub count: u32,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn receive_messages_body(input: &ReceiveMessagesInput) -> Value {
    let mut body = Map::new();
    body.insert("QueueUrl".into(), json!(input.queue_url));
    if let Some(n) = input.max_number_of_messages {
        body.insert("MaxNumberOfMessages".into(), json!(n));
    }
    if let Some(n) = input.wait_time_seconds {
        body.insert("WaitTimeSeconds".into(), json!(n));
    }
    if let Some(n) = input.visibility_timeout {
        body.insert("VisibilityTimeout".into(), json!(n));
    }
    if let Some(names) = &input.attribute_names {
        body.insert("MessageSystemAttributeNames".into(), json!(names));
    }
    if let Some(names) = &input.message_attribute_names {
        body.insert("MessageAttributeNames".into(), json!(names));
    }
    Value::Object(body)
}

#[capability(
    id = "queue-receive-messages",
    module = "sqs",
    display_name = "Receive Messages",
    description = "Receive one or more messages from an SQS queue (supports long polling).",
    side_effects = true,
    idempotent = false
)]
pub fn queue_receive_messages(
    input: ReceiveMessagesInput,
) -> Result<ReceiveMessagesOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = receive_messages_body(&input);
    let resp = sqs_call("AmazonSQS.ReceiveMessage", &connection.connection_id, &body)?;

    Ok(if resp.ok() {
        let messages = parse_messages(&resp.body);
        let count = messages.len() as u32;
        ReceiveMessagesOutput {
            success: true,
            messages,
            count,
            error: None,
        }
    } else {
        ReceiveMessagesOutput {
            success: false,
            messages: vec![],
            count: 0,
            error: Some(sqs_error("ReceiveMessage", &resp)),
        }
    })
}

// ============================================================================
// Capability 4: Delete Message
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Message Input")]
pub struct DeleteMessageInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,

    #[field(
        display_name = "Receipt Handle",
        description = "The receipt handle returned by Receive Messages"
    )]
    pub receipt_handle: String,
}

#[capability(
    id = "queue-delete-message",
    module = "sqs",
    display_name = "Delete Message",
    description = "Delete a message from an SQS queue using its receipt handle.",
    side_effects = true,
    idempotent = true
)]
pub fn queue_delete_message(input: DeleteMessageInput) -> Result<AckOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = json!({ "QueueUrl": input.queue_url, "ReceiptHandle": input.receipt_handle });
    let resp = sqs_call("AmazonSQS.DeleteMessage", &connection.connection_id, &body)?;
    Ok(ack("DeleteMessage", resp))
}

// ============================================================================
// Capability 5: Delete Message Batch
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Message Batch Input")]
pub struct DeleteMessageBatchInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,

    #[field(
        display_name = "Entries",
        description = "Up to 10 entries, each with a unique batch id and a receipt handle"
    )]
    pub entries: Vec<DeleteBatchEntry>,
}

fn delete_message_batch_body(input: &DeleteMessageBatchInput) -> Value {
    let entries: Vec<Value> = input
        .entries
        .iter()
        .map(|e| json!({ "Id": e.id, "ReceiptHandle": e.receipt_handle }))
        .collect();
    json!({ "QueueUrl": input.queue_url, "Entries": entries })
}

#[capability(
    id = "queue-delete-message-batch",
    module = "sqs",
    display_name = "Delete Message Batch",
    description = "Delete up to 10 messages from an SQS queue in a single request.",
    side_effects = true,
    idempotent = true
)]
pub fn queue_delete_message_batch(
    input: DeleteMessageBatchInput,
) -> Result<BatchResultOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = delete_message_batch_body(&input);
    let resp = sqs_call(
        "AmazonSQS.DeleteMessageBatch",
        &connection.connection_id,
        &body,
    )?;

    Ok(if resp.ok() {
        BatchResultOutput {
            success: true,
            successful: parse_batch_success(&resp.body),
            failed: parse_batch_failed(&resp.body),
            error: None,
        }
    } else {
        BatchResultOutput {
            success: false,
            successful: vec![],
            failed: vec![],
            error: Some(sqs_error("DeleteMessageBatch", &resp)),
        }
    })
}

// ============================================================================
// Capability 6: Change Message Visibility
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Change Message Visibility Input")]
pub struct ChangeMessageVisibilityInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,

    #[field(display_name = "Receipt Handle")]
    pub receipt_handle: String,

    #[field(
        display_name = "Visibility Timeout",
        description = "New visibility timeout in seconds (0–43200)",
        example = "30"
    )]
    pub visibility_timeout: u32,
}

#[capability(
    id = "queue-change-message-visibility",
    module = "sqs",
    display_name = "Change Message Visibility",
    description = "Change the visibility timeout of a received message.",
    side_effects = true,
    idempotent = true
)]
pub fn queue_change_message_visibility(
    input: ChangeMessageVisibilityInput,
) -> Result<AckOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = json!({
        "QueueUrl": input.queue_url,
        "ReceiptHandle": input.receipt_handle,
        "VisibilityTimeout": input.visibility_timeout,
    });
    let resp = sqs_call(
        "AmazonSQS.ChangeMessageVisibility",
        &connection.connection_id,
        &body,
    )?;
    Ok(ack("ChangeMessageVisibility", resp))
}

// ============================================================================
// Capability 7: Change Message Visibility Batch
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Change Message Visibility Batch Input")]
pub struct ChangeMessageVisibilityBatchInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,

    #[field(
        display_name = "Entries",
        description = "Up to 10 entries, each with a batch id, receipt handle, and visibility timeout"
    )]
    pub entries: Vec<VisibilityBatchEntry>,
}

fn change_visibility_batch_body(input: &ChangeMessageVisibilityBatchInput) -> Value {
    let entries: Vec<Value> = input
        .entries
        .iter()
        .map(|e| {
            json!({
                "Id": e.id,
                "ReceiptHandle": e.receipt_handle,
                "VisibilityTimeout": e.visibility_timeout,
            })
        })
        .collect();
    json!({ "QueueUrl": input.queue_url, "Entries": entries })
}

#[capability(
    id = "queue-change-message-visibility-batch",
    module = "sqs",
    display_name = "Change Message Visibility Batch",
    description = "Change the visibility timeout of up to 10 messages in a single request.",
    side_effects = true,
    idempotent = true
)]
pub fn queue_change_message_visibility_batch(
    input: ChangeMessageVisibilityBatchInput,
) -> Result<BatchResultOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = change_visibility_batch_body(&input);
    let resp = sqs_call(
        "AmazonSQS.ChangeMessageVisibilityBatch",
        &connection.connection_id,
        &body,
    )?;

    Ok(if resp.ok() {
        BatchResultOutput {
            success: true,
            successful: parse_batch_success(&resp.body),
            failed: parse_batch_failed(&resp.body),
            error: None,
        }
    } else {
        BatchResultOutput {
            success: false,
            successful: vec![],
            failed: vec![],
            error: Some(sqs_error("ChangeMessageVisibilityBatch", &resp)),
        }
    })
}

// ============================================================================
// Capability 8: Create Queue
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Create Queue Input")]
pub struct CreateQueueInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue Name",
        description = "Name of the queue (FIFO queues must end in .fifo)",
        example = "my-queue"
    )]
    pub queue_name: String,

    #[field(
        display_name = "Attributes",
        description = "Raw SQS queue attributes (e.g. VisibilityTimeout, MessageRetentionPeriod, RedrivePolicy). Values are strings."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<HashMap<String, String>>,

    #[field(
        display_name = "KMS Master Key ID",
        description = "Custom KMS CMK id/ARN/alias for SSE-KMS encryption at rest",
        example = "alias/my-sqs-key"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kms_master_key_id: Option<String>,

    #[field(
        display_name = "KMS Data Key Reuse Period (s)",
        description = "How long SQS reuses a data key before calling KMS again (60–86400)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kms_data_key_reuse_period_seconds: Option<u32>,

    #[field(
        display_name = "SQS-Managed SSE",
        description = "Enable SSE-SQS (SQS-owned key). Mutually exclusive with SSE-KMS."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sqs_managed_sse_enabled: Option<bool>,

    #[field(display_name = "FIFO Queue", description = "Create a FIFO queue")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fifo_queue: Option<bool>,

    #[field(
        display_name = "Content-Based Deduplication",
        description = "FIFO only: derive the dedup id from the message body"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_based_deduplication: Option<bool>,

    #[field(
        display_name = "Tags",
        description = "Cost-allocation tags (key → value)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Queue URL Output")]
pub struct QueueUrlOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Queue URL")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_url: Option<String>,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn create_queue_body(input: &CreateQueueInput) -> Value {
    let mut body = Map::new();
    body.insert("QueueName".into(), json!(input.queue_name));
    if let Some(attrs) = merge_queue_attributes(
        input.attributes.as_ref(),
        input.kms_master_key_id.as_deref(),
        input.kms_data_key_reuse_period_seconds,
        input.sqs_managed_sse_enabled,
        input.fifo_queue,
        input.content_based_deduplication,
    ) {
        body.insert("Attributes".into(), attrs);
    }
    if let Some(tags) = &input.tags {
        body.insert("tags".into(), json!(tags));
    }
    Value::Object(body)
}

#[capability(
    id = "queue-create-queue",
    module = "sqs",
    display_name = "Create Queue",
    description = "Create an SQS queue, optionally with SSE-KMS/SSE-SQS encryption and FIFO settings.",
    side_effects = true,
    idempotent = false
)]
pub fn queue_create_queue(input: CreateQueueInput) -> Result<QueueUrlOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = create_queue_body(&input);
    let resp = sqs_call("AmazonSQS.CreateQueue", &connection.connection_id, &body)?;

    Ok(if resp.ok() {
        QueueUrlOutput {
            success: true,
            queue_url: opt_str(&resp.body, "QueueUrl"),
            error: None,
        }
    } else {
        QueueUrlOutput {
            success: false,
            queue_url: None,
            error: Some(sqs_error("CreateQueue", &resp)),
        }
    })
}

// ============================================================================
// Capability 9: Delete Queue
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Delete Queue Input")]
pub struct DeleteQueueInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,
}

#[capability(
    id = "queue-delete-queue",
    module = "sqs",
    display_name = "Delete Queue",
    description = "Delete an SQS queue and all of its messages.",
    side_effects = true,
    idempotent = true
)]
pub fn queue_delete_queue(input: DeleteQueueInput) -> Result<AckOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = json!({ "QueueUrl": input.queue_url });
    let resp = sqs_call("AmazonSQS.DeleteQueue", &connection.connection_id, &body)?;
    Ok(ack("DeleteQueue", resp))
}

// ============================================================================
// Capability 10: List Queues
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Queues Input")]
pub struct ListQueuesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue Name Prefix",
        description = "Only return queues whose name starts with this prefix"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_name_prefix: Option<String>,

    #[field(
        display_name = "Max Results",
        description = "Maximum queue URLs to return per page (1–1000)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_results: Option<u32>,

    #[field(
        display_name = "Next Token",
        description = "Pagination token from a previous call"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Queues Output")]
pub struct ListQueuesOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Queue URLs")]
    pub queue_urls: Vec<String>,

    #[field(
        display_name = "Next Token",
        description = "Token for the next page, if any"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_token: Option<String>,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn list_queues_body(input: &ListQueuesInput) -> Value {
    let mut body = Map::new();
    if let Some(p) = &input.queue_name_prefix {
        body.insert("QueueNamePrefix".into(), json!(p));
    }
    if let Some(n) = input.max_results {
        body.insert("MaxResults".into(), json!(n));
    }
    if let Some(t) = &input.next_token {
        body.insert("NextToken".into(), json!(t));
    }
    Value::Object(body)
}

#[capability(
    id = "queue-list-queues",
    module = "sqs",
    display_name = "List Queues",
    description = "List SQS queue URLs, optionally filtered by name prefix.",
    side_effects = false,
    idempotent = true
)]
pub fn queue_list_queues(input: ListQueuesInput) -> Result<ListQueuesOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = list_queues_body(&input);
    let resp = sqs_call("AmazonSQS.ListQueues", &connection.connection_id, &body)?;

    Ok(if resp.ok() {
        let queue_urls = resp
            .body
            .get("QueueUrls")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        ListQueuesOutput {
            success: true,
            queue_urls,
            next_token: opt_str(&resp.body, "NextToken"),
            error: None,
        }
    } else {
        ListQueuesOutput {
            success: false,
            queue_urls: vec![],
            next_token: None,
            error: Some(sqs_error("ListQueues", &resp)),
        }
    })
}

// ============================================================================
// Capability 11: Get Queue URL
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Queue URL Input")]
pub struct GetQueueUrlInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(display_name = "Queue Name", example = "my-queue")]
    pub queue_name: String,

    #[field(
        display_name = "Queue Owner AWS Account ID",
        description = "Account id of the queue owner, for cross-account access"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_owner_aws_account_id: Option<String>,
}

#[capability(
    id = "queue-get-queue-url",
    module = "sqs",
    display_name = "Get Queue URL",
    description = "Resolve a queue name to its URL.",
    side_effects = false,
    idempotent = true
)]
pub fn queue_get_queue_url(input: GetQueueUrlInput) -> Result<QueueUrlOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let mut body = Map::new();
    body.insert("QueueName".into(), json!(input.queue_name));
    if let Some(owner) = &input.queue_owner_aws_account_id {
        body.insert("QueueOwnerAWSAccountId".into(), json!(owner));
    }
    let resp = sqs_call(
        "AmazonSQS.GetQueueUrl",
        &connection.connection_id,
        &Value::Object(body),
    )?;

    Ok(if resp.ok() {
        QueueUrlOutput {
            success: true,
            queue_url: opt_str(&resp.body, "QueueUrl"),
            error: None,
        }
    } else {
        QueueUrlOutput {
            success: false,
            queue_url: None,
            error: Some(sqs_error("GetQueueUrl", &resp)),
        }
    })
}

// ============================================================================
// Capability 12: Get Queue Attributes
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Queue Attributes Input")]
pub struct GetQueueAttributesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,

    #[field(
        display_name = "Attribute Names",
        description = "Attributes to fetch, e.g. [\"All\"] or [\"ApproximateNumberOfMessages\", \"KmsMasterKeyId\"]"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attribute_names: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Queue Attributes Output")]
pub struct QueueAttributesOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(
        display_name = "Attributes",
        description = "Map of attribute name → value"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<Value>,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[capability(
    id = "queue-get-queue-attributes",
    module = "sqs",
    display_name = "Get Queue Attributes",
    description = "Fetch queue attributes (settings, counts, encryption config).",
    side_effects = false,
    idempotent = true
)]
pub fn queue_get_queue_attributes(
    input: GetQueueAttributesInput,
) -> Result<QueueAttributesOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let names = input
        .attribute_names
        .clone()
        .unwrap_or_else(|| vec!["All".to_string()]);
    let body = json!({ "QueueUrl": input.queue_url, "AttributeNames": names });
    let resp = sqs_call(
        "AmazonSQS.GetQueueAttributes",
        &connection.connection_id,
        &body,
    )?;

    Ok(if resp.ok() {
        QueueAttributesOutput {
            success: true,
            attributes: resp.body.get("Attributes").cloned(),
            error: None,
        }
    } else {
        QueueAttributesOutput {
            success: false,
            attributes: None,
            error: Some(sqs_error("GetQueueAttributes", &resp)),
        }
    })
}

// ============================================================================
// Capability 13: Set Queue Attributes
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Set Queue Attributes Input")]
pub struct SetQueueAttributesInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,

    #[field(
        display_name = "Attributes",
        description = "Raw SQS queue attributes to set (values are strings)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<HashMap<String, String>>,

    #[field(
        display_name = "KMS Master Key ID",
        description = "Custom KMS CMK id/ARN/alias for SSE-KMS encryption at rest",
        example = "alias/my-sqs-key"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kms_master_key_id: Option<String>,

    #[field(
        display_name = "KMS Data Key Reuse Period (s)",
        description = "How long SQS reuses a data key before calling KMS again (60–86400)"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kms_data_key_reuse_period_seconds: Option<u32>,

    #[field(
        display_name = "SQS-Managed SSE",
        description = "Enable SSE-SQS (SQS-owned key). Mutually exclusive with SSE-KMS."
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sqs_managed_sse_enabled: Option<bool>,
}

fn set_queue_attributes_body(input: &SetQueueAttributesInput) -> Value {
    let attrs = merge_queue_attributes(
        input.attributes.as_ref(),
        input.kms_master_key_id.as_deref(),
        input.kms_data_key_reuse_period_seconds,
        input.sqs_managed_sse_enabled,
        None,
        None,
    )
    .unwrap_or_else(|| Value::Object(Map::new()));
    json!({ "QueueUrl": input.queue_url, "Attributes": attrs })
}

#[capability(
    id = "queue-set-queue-attributes",
    module = "sqs",
    display_name = "Set Queue Attributes",
    description = "Update queue attributes, including SSE-KMS/SSE-SQS encryption settings.",
    side_effects = true,
    idempotent = true
)]
pub fn queue_set_queue_attributes(input: SetQueueAttributesInput) -> Result<AckOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = set_queue_attributes_body(&input);
    let resp = sqs_call(
        "AmazonSQS.SetQueueAttributes",
        &connection.connection_id,
        &body,
    )?;
    Ok(ack("SetQueueAttributes", resp))
}

// ============================================================================
// Capability 14: Purge Queue
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Purge Queue Input")]
pub struct PurgeQueueInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,
}

#[capability(
    id = "queue-purge-queue",
    module = "sqs",
    display_name = "Purge Queue",
    description = "Delete all messages in an SQS queue (the queue itself is kept).",
    side_effects = true,
    idempotent = true
)]
pub fn queue_purge_queue(input: PurgeQueueInput) -> Result<AckOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = json!({ "QueueUrl": input.queue_url });
    let resp = sqs_call("AmazonSQS.PurgeQueue", &connection.connection_id, &body)?;
    Ok(ack("PurgeQueue", resp))
}

// ============================================================================
// Capability 15: List Queue Tags
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "List Queue Tags Input")]
pub struct ListQueueTagsInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "List Queue Tags Output")]
pub struct ListQueueTagsOutput {
    #[field(display_name = "Success")]
    pub success: bool,

    #[field(display_name = "Tags", description = "Map of tag key → value")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Value>,

    #[field(display_name = "Error")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[capability(
    id = "queue-list-queue-tags",
    module = "sqs",
    display_name = "List Queue Tags",
    description = "List the cost-allocation tags on an SQS queue.",
    side_effects = false,
    idempotent = true
)]
pub fn queue_list_queue_tags(input: ListQueueTagsInput) -> Result<ListQueueTagsOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = json!({ "QueueUrl": input.queue_url });
    let resp = sqs_call("AmazonSQS.ListQueueTags", &connection.connection_id, &body)?;

    Ok(if resp.ok() {
        ListQueueTagsOutput {
            success: true,
            tags: resp.body.get("Tags").cloned(),
            error: None,
        }
    } else {
        ListQueueTagsOutput {
            success: false,
            tags: None,
            error: Some(sqs_error("ListQueueTags", &resp)),
        }
    })
}

// ============================================================================
// Capability 16: Tag Queue
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Tag Queue Input")]
pub struct TagQueueInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,

    #[field(
        display_name = "Tags",
        description = "Tags to add or update (key → value)"
    )]
    pub tags: HashMap<String, String>,
}

#[capability(
    id = "queue-tag-queue",
    module = "sqs",
    display_name = "Tag Queue",
    description = "Add or update cost-allocation tags on an SQS queue.",
    side_effects = true,
    idempotent = true
)]
pub fn queue_tag_queue(input: TagQueueInput) -> Result<AckOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = json!({ "QueueUrl": input.queue_url, "Tags": input.tags });
    let resp = sqs_call("AmazonSQS.TagQueue", &connection.connection_id, &body)?;
    Ok(ack("TagQueue", resp))
}

// ============================================================================
// Capability 17: Untag Queue
// ============================================================================

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Untag Queue Input")]
pub struct UntagQueueInput {
    #[field(skip)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "Queue URL",
        example = "https://sqs.us-east-1.amazonaws.com/123456789012/my-queue"
    )]
    pub queue_url: String,

    #[field(display_name = "Tag Keys", description = "Tag keys to remove")]
    pub tag_keys: Vec<String>,
}

#[capability(
    id = "queue-untag-queue",
    module = "sqs",
    display_name = "Untag Queue",
    description = "Remove cost-allocation tags from an SQS queue.",
    side_effects = true,
    idempotent = true
)]
pub fn queue_untag_queue(input: UntagQueueInput) -> Result<AckOutput, AgentError> {
    let connection = require_connection(&input._connection)?;
    let body = json!({ "QueueUrl": input.queue_url, "TagKeys": input.tag_keys });
    let resp = sqs_call("AmazonSQS.UntagQueue", &connection.connection_id, &body)?;
    Ok(ack("UntagQueue", resp))
}

// ============================================================================
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// ============================================================================

#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api_with_types,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] = &[
        &__CAPABILITY_META_QUEUE_SEND_MESSAGE,
        &__CAPABILITY_META_QUEUE_SEND_MESSAGE_BATCH,
        &__CAPABILITY_META_QUEUE_RECEIVE_MESSAGES,
        &__CAPABILITY_META_QUEUE_DELETE_MESSAGE,
        &__CAPABILITY_META_QUEUE_DELETE_MESSAGE_BATCH,
        &__CAPABILITY_META_QUEUE_CHANGE_MESSAGE_VISIBILITY,
        &__CAPABILITY_META_QUEUE_CHANGE_MESSAGE_VISIBILITY_BATCH,
        &__CAPABILITY_META_QUEUE_CREATE_QUEUE,
        &__CAPABILITY_META_QUEUE_DELETE_QUEUE,
        &__CAPABILITY_META_QUEUE_LIST_QUEUES,
        &__CAPABILITY_META_QUEUE_GET_QUEUE_URL,
        &__CAPABILITY_META_QUEUE_GET_QUEUE_ATTRIBUTES,
        &__CAPABILITY_META_QUEUE_SET_QUEUE_ATTRIBUTES,
        &__CAPABILITY_META_QUEUE_PURGE_QUEUE,
        &__CAPABILITY_META_QUEUE_LIST_QUEUE_TAGS,
        &__CAPABILITY_META_QUEUE_TAG_QUEUE,
        &__CAPABILITY_META_QUEUE_UNTAG_QUEUE,
    ];

    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        (
            "SendMessageInput",
            &__INPUT_META_SendMessageInput as &InputTypeMeta,
        ),
        (
            "SendMessageBatchInput",
            &__INPUT_META_SendMessageBatchInput as &InputTypeMeta,
        ),
        (
            "ReceiveMessagesInput",
            &__INPUT_META_ReceiveMessagesInput as &InputTypeMeta,
        ),
        (
            "DeleteMessageInput",
            &__INPUT_META_DeleteMessageInput as &InputTypeMeta,
        ),
        (
            "DeleteMessageBatchInput",
            &__INPUT_META_DeleteMessageBatchInput as &InputTypeMeta,
        ),
        (
            "ChangeMessageVisibilityInput",
            &__INPUT_META_ChangeMessageVisibilityInput as &InputTypeMeta,
        ),
        (
            "ChangeMessageVisibilityBatchInput",
            &__INPUT_META_ChangeMessageVisibilityBatchInput as &InputTypeMeta,
        ),
        (
            "CreateQueueInput",
            &__INPUT_META_CreateQueueInput as &InputTypeMeta,
        ),
        (
            "DeleteQueueInput",
            &__INPUT_META_DeleteQueueInput as &InputTypeMeta,
        ),
        (
            "ListQueuesInput",
            &__INPUT_META_ListQueuesInput as &InputTypeMeta,
        ),
        (
            "GetQueueUrlInput",
            &__INPUT_META_GetQueueUrlInput as &InputTypeMeta,
        ),
        (
            "GetQueueAttributesInput",
            &__INPUT_META_GetQueueAttributesInput as &InputTypeMeta,
        ),
        (
            "SetQueueAttributesInput",
            &__INPUT_META_SetQueueAttributesInput as &InputTypeMeta,
        ),
        (
            "PurgeQueueInput",
            &__INPUT_META_PurgeQueueInput as &InputTypeMeta,
        ),
        (
            "ListQueueTagsInput",
            &__INPUT_META_ListQueueTagsInput as &InputTypeMeta,
        ),
        (
            "TagQueueInput",
            &__INPUT_META_TagQueueInput as &InputTypeMeta,
        ),
        (
            "UntagQueueInput",
            &__INPUT_META_UntagQueueInput as &InputTypeMeta,
        ),
    ]
    .into_iter()
    .collect();

    let output_types: HashMap<&'static str, &'static OutputTypeMeta> = [
        (
            "SendMessageOutput",
            &__OUTPUT_META_SendMessageOutput as &OutputTypeMeta,
        ),
        (
            "BatchResultOutput",
            &__OUTPUT_META_BatchResultOutput as &OutputTypeMeta,
        ),
        (
            "ReceiveMessagesOutput",
            &__OUTPUT_META_ReceiveMessagesOutput as &OutputTypeMeta,
        ),
        ("AckOutput", &__OUTPUT_META_AckOutput as &OutputTypeMeta),
        (
            "QueueUrlOutput",
            &__OUTPUT_META_QueueUrlOutput as &OutputTypeMeta,
        ),
        (
            "ListQueuesOutput",
            &__OUTPUT_META_ListQueuesOutput as &OutputTypeMeta,
        ),
        (
            "QueueAttributesOutput",
            &__OUTPUT_META_QueueAttributesOutput as &OutputTypeMeta,
        ),
        (
            "ListQueueTagsOutput",
            &__OUTPUT_META_ListQueueTagsOutput as &OutputTypeMeta,
        ),
    ]
    .into_iter()
    .collect();

    let capabilities = caps
        .iter()
        .map(|cap| {
            capability_to_api_with_types(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
                &output_types,
            )
        })
        .collect();

    AgentInfo {
        id: "sqs".into(),
        name: "SQS".into(),
        description: "AWS SQS: send, receive, and manage messages and queues, including SSE-KMS queue encryption. Credentials are injected server-side by the runtara HTTP proxy.".into(),
        has_side_effects: true,
        supports_connections: true,
        integration_ids: vec!["aws_credentials".to_string()],
        capabilities,
    }
}

// ============================================================================
// Wasm component plumbing
// ============================================================================

#[cfg(target_arch = "wasm32")]
use bindings::exports::runtara::agent_sqs::capabilities::{ConnectionInfo, ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(
        capability_id: String,
        input: Vec<u8>,
        connection: Option<ConnectionInfo>,
    ) -> Result<Vec<u8>, ErrorInfo> {
        let mut value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;

        // Inject the WIT `connection` arg into the input JSON under `_connection`
        // so the macro-generated executor can deserialize it into the capability
        // input struct's `_connection: Option<RawConnection>` field.
        if let Some(c) = connection.as_ref() {
            if let serde_json::Value::Object(ref mut obj) = value {
                let parameters = serde_json::from_str::<serde_json::Value>(&c.parameters)
                    .unwrap_or(serde_json::Value::Null);
                let rate_limit_config = c
                    .rate_limit_config
                    .as_ref()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());
                obj.insert(
                    "_connection".into(),
                    serde_json::json!({
                        "connection_id": c.connection_id,
                        "integration_id": c.integration_id,
                        "connection_subtype": c.connection_subtype,
                        "parameters": parameters,
                        "rate_limit_config": rate_limit_config,
                    }),
                );
            }
        }

        let executor_result = match capability_id.as_str() {
            "queue-send-message" => __executor_queue_send_message(value),
            "queue-send-message-batch" => __executor_queue_send_message_batch(value),
            "queue-receive-messages" => __executor_queue_receive_messages(value),
            "queue-delete-message" => __executor_queue_delete_message(value),
            "queue-delete-message-batch" => __executor_queue_delete_message_batch(value),
            "queue-change-message-visibility" => __executor_queue_change_message_visibility(value),
            "queue-change-message-visibility-batch" => {
                __executor_queue_change_message_visibility_batch(value)
            }
            "queue-create-queue" => __executor_queue_create_queue(value),
            "queue-delete-queue" => __executor_queue_delete_queue(value),
            "queue-list-queues" => __executor_queue_list_queues(value),
            "queue-get-queue-url" => __executor_queue_get_queue_url(value),
            "queue-get-queue-attributes" => __executor_queue_get_queue_attributes(value),
            "queue-set-queue-attributes" => __executor_queue_set_queue_attributes(value),
            "queue-purge-queue" => __executor_queue_purge_queue(value),
            "queue-list-queue-tags" => __executor_queue_list_queue_tags(value),
            "queue-tag-queue" => __executor_queue_tag_queue(value),
            "queue-untag-queue" => __executor_queue_untag_queue(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("sqs agent has no capability `{other}`"),
                    category: "permanent".into(),
                    severity: "error".into(),
                    retryable: false,
                    retry_after_ms: None,
                    attributes: None,
                });
            }
        };
        executor_result
            .map_err(error_string_to_error_info)
            .and_then(|out_value| serde_json::to_vec(&out_value).map_err(bad_json))
    }
}

#[cfg(target_arch = "wasm32")]
fn bad_json(e: serde_json::Error) -> ErrorInfo {
    ErrorInfo {
        code: "INPUT_DESERIALIZATION_ERROR".into(),
        message: e.to_string(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

/// The `#[capability]` macro packages each error as a JSON-string with
/// `{ code, message, category, severity, ... }`. Parse it back into a typed
/// `ErrorInfo` for the WIT result.
#[cfg(target_arch = "wasm32")]
fn error_string_to_error_info(s: String) -> ErrorInfo {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&s) {
        let category = value
            .get("category")
            .and_then(|v| v.as_str())
            .unwrap_or("permanent")
            .to_string();
        let retryable = value
            .get("retryable")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| category == "transient");
        ErrorInfo {
            code: value
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("CAPABILITY_ERROR")
                .into(),
            message: value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or(&s)
                .into(),
            category,
            severity: value
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .into(),
            retryable,
            retry_after_ms: value.get("retry_after_ms").and_then(|v| v.as_u64()),
            attributes: value.get("attributes").map(|v| v.to_string()),
        }
    } else {
        ErrorInfo {
            code: "CAPABILITY_ERROR".into(),
            message: s,
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: None,
        }
    }
}

#[cfg(target_arch = "wasm32")]
bindings::export!(Component with_types_in bindings);

// ============================================================================
// Tests (host-side; pure request-body builders and response parsers)
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Option<RawConnection> {
        Some(RawConnection {
            connection_id: "conn-1".into(),
            connection_subtype: None,
            integration_id: "aws_credentials".into(),
            parameters: Value::Null,
            rate_limit_config: None,
        })
    }

    #[test]
    fn send_message_body_includes_only_set_fields() {
        let input = SendMessageInput {
            _connection: conn(),
            queue_url: "https://sqs.us-east-1.amazonaws.com/1/q".into(),
            message_body: "hello".into(),
            delay_seconds: None,
            message_group_id: None,
            message_deduplication_id: None,
            message_attributes: None,
        };
        let body = send_message_body(&input);
        assert_eq!(
            body["QueueUrl"],
            json!("https://sqs.us-east-1.amazonaws.com/1/q")
        );
        assert_eq!(body["MessageBody"], json!("hello"));
        assert!(body.get("DelaySeconds").is_none());
        assert!(body.get("MessageGroupId").is_none());
        assert!(body.get("MessageAttributes").is_none());
    }

    #[test]
    fn send_message_body_encodes_fifo_and_attributes() {
        let mut attrs = HashMap::new();
        attrs.insert("source".to_string(), "erp".to_string());
        let input = SendMessageInput {
            _connection: conn(),
            queue_url: "q".into(),
            message_body: "b".into(),
            delay_seconds: Some(5),
            message_group_id: Some("g1".into()),
            message_deduplication_id: Some("d1".into()),
            message_attributes: Some(attrs),
        };
        let body = send_message_body(&input);
        assert_eq!(body["DelaySeconds"], json!(5));
        assert_eq!(body["MessageGroupId"], json!("g1"));
        assert_eq!(body["MessageDeduplicationId"], json!("d1"));
        assert_eq!(
            body["MessageAttributes"]["source"],
            json!({ "DataType": "String", "StringValue": "erp" })
        );
    }

    #[test]
    fn create_queue_folds_kms_and_fifo_into_attributes() {
        let mut passthrough = HashMap::new();
        passthrough.insert("VisibilityTimeout".to_string(), "45".to_string());
        let input = CreateQueueInput {
            _connection: conn(),
            queue_name: "orders.fifo".into(),
            attributes: Some(passthrough),
            kms_master_key_id: Some("alias/my-key".into()),
            kms_data_key_reuse_period_seconds: Some(300),
            sqs_managed_sse_enabled: None,
            fifo_queue: Some(true),
            content_based_deduplication: Some(true),
            tags: None,
        };
        let body = create_queue_body(&input);
        let a = &body["Attributes"];
        assert_eq!(a["VisibilityTimeout"], json!("45"));
        assert_eq!(a["KmsMasterKeyId"], json!("alias/my-key"));
        // All attribute values must be strings on the wire.
        assert_eq!(a["KmsDataKeyReusePeriodSeconds"], json!("300"));
        assert_eq!(a["FifoQueue"], json!("true"));
        assert_eq!(a["ContentBasedDeduplication"], json!("true"));
        assert_eq!(body["QueueName"], json!("orders.fifo"));
    }

    #[test]
    fn create_queue_omits_attributes_when_none_set() {
        let input = CreateQueueInput {
            _connection: conn(),
            queue_name: "plain".into(),
            attributes: None,
            kms_master_key_id: None,
            kms_data_key_reuse_period_seconds: None,
            sqs_managed_sse_enabled: None,
            fifo_queue: None,
            content_based_deduplication: None,
            tags: None,
        };
        let body = create_queue_body(&input);
        assert!(body.get("Attributes").is_none());
    }

    #[test]
    fn set_queue_attributes_body_always_has_attributes_object() {
        let input = SetQueueAttributesInput {
            _connection: conn(),
            queue_url: "q".into(),
            attributes: None,
            kms_master_key_id: Some("alias/k".into()),
            kms_data_key_reuse_period_seconds: None,
            sqs_managed_sse_enabled: Some(true),
        };
        let body = set_queue_attributes_body(&input);
        assert_eq!(body["Attributes"]["KmsMasterKeyId"], json!("alias/k"));
        assert_eq!(body["Attributes"]["SqsManagedSseEnabled"], json!("true"));
    }

    #[test]
    fn receive_body_uses_modern_system_attribute_member() {
        let input = ReceiveMessagesInput {
            _connection: conn(),
            queue_url: "q".into(),
            max_number_of_messages: Some(10),
            wait_time_seconds: Some(20),
            visibility_timeout: None,
            attribute_names: Some(vec!["All".into()]),
            message_attribute_names: None,
        };
        let body = receive_messages_body(&input);
        assert_eq!(body["MaxNumberOfMessages"], json!(10));
        assert_eq!(body["WaitTimeSeconds"], json!(20));
        assert_eq!(body["MessageSystemAttributeNames"], json!(["All"]));
        assert!(body.get("VisibilityTimeout").is_none());
    }

    #[test]
    fn parse_messages_reads_receipt_handles_and_bodies() {
        let resp = json!({
            "Messages": [
                { "MessageId": "m1", "ReceiptHandle": "rh1", "Body": "b1", "MD5OfBody": "x" },
                { "MessageId": "m2", "ReceiptHandle": "rh2", "Body": "b2" }
            ]
        });
        let msgs = parse_messages(&resp);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].message_id, "m1");
        assert_eq!(msgs[0].receipt_handle, "rh1");
        assert_eq!(msgs[0].md5_of_body.as_deref(), Some("x"));
        assert_eq!(msgs[1].md5_of_body, None);
    }

    #[test]
    fn parse_batch_splits_successful_and_failed() {
        let resp = json!({
            "Successful": [ { "Id": "1", "MessageId": "m1" } ],
            "Failed": [ { "Id": "2", "Code": "InvalidParameterValue", "Message": "bad", "SenderFault": true } ]
        });
        let ok = parse_batch_success(&resp);
        let bad = parse_batch_failed(&resp);
        assert_eq!(ok.len(), 1);
        assert_eq!(ok[0].message_id.as_deref(), Some("m1"));
        assert_eq!(bad.len(), 1);
        assert_eq!(bad[0].code.as_deref(), Some("InvalidParameterValue"));
        assert_eq!(bad[0].sender_fault, Some(true));
    }

    #[test]
    fn sqs_error_shortens_type_and_includes_message() {
        let resp = SqsResp {
            status: 400,
            body: json!({
                "__type": "com.amazonaws.sqs#QueueDoesNotExist",
                "message": "The specified queue does not exist."
            }),
        };
        assert_eq!(
            sqs_error("GetQueueUrl", &resp),
            "QueueDoesNotExist: The specified queue does not exist."
        );
    }

    #[test]
    fn sqs_error_falls_back_to_status_when_body_empty() {
        let resp = SqsResp {
            status: 500,
            body: Value::Null,
        };
        assert_eq!(
            sqs_error("SendMessage", &resp),
            "SendMessage failed (HTTP 500)"
        );
    }

    #[test]
    fn agent_info_exposes_all_capabilities() {
        let info = agent_info();
        assert_eq!(info.id, "sqs");
        assert_eq!(info.integration_ids, vec!["aws_credentials".to_string()]);
        assert_eq!(info.capabilities.len(), 17);
    }
}
