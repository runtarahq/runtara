//! Authoritative lifecycle for external inputs, independent of debug events.
//!
//! All mutations serialize with root lifecycle transitions, invocation fences,
//! and raw signal writes. Accepted bytes are retained until instance deletion.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::invocations::{AttemptFence, InvocationLease};

/// Host-derived authority; guest code never selects its tenant or fence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputAuthority {
    /// A root without an invocation lease. Invalid once a lease exists.
    Root {
        /// Trusted tenant identity.
        tenant_id: String,
        /// Trusted root instance identity.
        instance_id: String,
    },
    /// Root IO under an exact active execution lease.
    LeasedRoot(InvocationLease),
    /// Child IO under an exact active attempt fence.
    Invocation(AttemptFence),
}

impl InputAuthority {
    /// Tenant owning this execution.
    pub fn tenant_id(&self) -> &str {
        match self {
            Self::Root { tenant_id, .. } => tenant_id,
            Self::LeasedRoot(lease) => &lease.tenant_id,
            Self::Invocation(fence) => &fence.lease.tenant_id,
        }
    }

    /// Root instance owning this execution.
    pub fn instance_id(&self) -> &str {
        match self {
            Self::Root { instance_id, .. } => instance_id,
            Self::LeasedRoot(lease) => &lease.instance_id,
            Self::Invocation(fence) => &fence.lease.instance_id,
        }
    }

    /// Logical owner; root ownership is represented by the empty path.
    pub fn invocation_path(&self) -> &str {
        match self {
            Self::Invocation(fence) => &fence.path,
            _ => "",
        }
    }
}

/// Immutable facts supplied when a logical wait first becomes available.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputRequestSpec {
    /// Full compiler-qualified deterministic wait identity, stable on replay.
    pub signal_id: String,
    /// The wait's response schema, not the workflow startup schema.
    pub response_schema: Option<Value>,
    /// Step/tool presentation, action key, correlation and context.
    pub metadata: Value,
    /// Original absolute deadline in persistence time; replay cannot move it.
    /// Hosts rebase guest deadlines with [`persistence_deadline_ms`] first.
    pub deadline: Option<DateTime<Utc>>,
}

impl InputRequestSpec {
    /// Decode the compiler's existing wait descriptor at a trusted host boundary.
    /// Ownership is deliberately absent from this guest-provided structure.
    pub fn from_descriptor(descriptor: &[u8], deadline_ms: Option<u64>) -> InputResult<Self> {
        let metadata: Value =
            serde_json::from_slice(descriptor).map_err(|_| InputError::InvalidRequest)?;
        let signal_id = metadata
            .get("signal_id")
            .and_then(Value::as_str)
            .ok_or(InputError::InvalidRequest)?
            .to_owned();
        let deadline = deadline_ms
            .map(|ms| {
                i64::try_from(ms)
                    .ok()
                    .and_then(DateTime::from_timestamp_millis)
                    .ok_or(InputError::InvalidRequest)
            })
            .transpose()?;
        let spec = Self {
            signal_id,
            response_schema: metadata
                .get("response_schema")
                .filter(|schema| !schema.is_null())
                .cloned(),
            metadata,
            deadline,
        };
        spec.validate()?;
        Ok(spec)
    }

    /// Validate identities before persisting a request.
    pub fn validate(&self) -> InputResult<()> {
        if self.signal_id.is_empty()
            || self.signal_id.len() > 65_536
            || self.signal_id.contains('\0')
            || !self.metadata.is_object()
        {
            return Err(InputError::InvalidRequest);
        }
        Ok(())
    }

    /// Bounded index key. Stores also compare the full signal to detect collision.
    pub fn request_id(&self) -> String {
        request_id(&self.signal_id)
    }
}

/// Rebase a guest deadline minted from the host clock onto persistence time.
///
/// Expiry is decided by persistence, so the remaining budget measured on the
/// guest's own clock is re-anchored at persistence `now`. Skew between the host
/// and the database therefore cannot shorten, lengthen or pre-expire a wait.
/// Registration replay keeps the first stored deadline regardless.
pub async fn persistence_deadline_ms(
    inputs: &dyn InputRequests,
    deadline_ms: Option<u64>,
    host_now_ms: u64,
) -> InputResult<Option<u64>> {
    let Some(deadline_ms) = deadline_ms else {
        return Ok(None);
    };
    let now = u64::try_from(inputs.input_clock().await?.timestamp_millis())
        .map_err(|_| InputError::Storage("persistence clock before epoch".into()))?;
    Ok(Some(
        now.saturating_add(deadline_ms.saturating_sub(host_now_ms)),
    ))
}

/// Stable bounded key for a full wait identity, scoped to one instance.
pub fn request_id(signal_id: &str) -> String {
    format!("{:x}", Sha256::digest(signal_id.as_bytes()))
}

/// Why an unanswered request can no longer accept input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputClosure {
    /// Its original deadline elapsed.
    Expired,
    /// Its owner left the wait without a response.
    Abandoned,
    /// Its logical invocation was cancelled.
    InvocationCancelled,
    /// Its logical invocation finished.
    InvocationSettled,
    /// Its root execution became terminal.
    InstanceTerminated,
}

impl InputClosure {
    /// Stable transport reason, independent of presentation/debug formatting.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "expired",
            Self::Abandoned => "abandoned",
            Self::InvocationCancelled => "invocation_cancelled",
            Self::InvocationSettled => "invocation_settled",
            Self::InstanceTerminated => "instance_terminated",
        }
    }
}

/// Stable successful outcome, replayable after root termination.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputReceipt {
    /// Stable receipt identity allocated once by persistence.
    pub receipt_id: String,
    /// Caller operation identity, unique within the tenant-owned instance.
    pub operation_id: String,
    /// Bounded request identity.
    pub request_id: String,
    /// Persistence acceptance timestamp.
    pub accepted_at: DateTime<Utc>,
    /// Canonical immutable response bytes. Never log these bytes.
    pub payload: Vec<u8>,
    /// Canonical trusted submission context. Internal persistence data, never
    /// include this or the accepted payload in a public acknowledgement DTO.
    pub acceptance_context: Option<Vec<u8>>,
}

/// Retry identity for a trusted adapter that enriches a caller's payload.
/// Its canonical bytes include the original caller payload, principal and
/// source scope; mutable enrichment is deliberately excluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InputAcceptanceContext(Vec<u8>);

impl InputAcceptanceContext {
    /// Build only after authorizing the current source scope and principal.
    /// Never populate these fields from an unverified guest identity.
    pub fn new(
        source: &str,
        principal: &str,
        scope: &Value,
        caller_payload: &Value,
    ) -> InputResult<Self> {
        validate_operation_id(source)?;
        if principal.is_empty()
            || principal.len() > 1024
            || principal.chars().any(char::is_control)
            || !scope.is_object()
        {
            return Err(InputError::InvalidRequest);
        }
        Ok(Self(canonical_payload(&serde_json::json!({
            "source": source, "principal": principal, "scope": scope, "caller_payload": caller_payload,
        }))))
    }

    /// Immutable canonical representation for atomic persistence.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Content to compare on replay. Contextual retries compare the original caller
/// intent, not an effective payload recomputed from mutable defaults.
pub enum InputReplayIdentity<'a> {
    /// Ordinary direct response, with canonical effective payload bytes.
    Payload(&'a [u8]),
    /// Trusted adapter context, including canonical original caller payload.
    Context(&'a [u8]),
}

impl InputReplayIdentity<'_> {
    /// Source transitions also conflict: a direct response cannot replay a
    /// contextual operation merely by guessing its effective payload.
    pub fn matches(&self, receipt: &InputReceipt) -> bool {
        match self {
            Self::Payload(payload) => {
                receipt.acceptance_context.is_none() && receipt.payload == *payload
            }
            Self::Context(context) => receipt.acceptance_context.as_deref() == Some(*context),
        }
    }
}

/// Durable request state. Accepted is final; consumption does not reopen it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum InputState {
    /// Awaiting input, subject to root/owner/deadline eligibility.
    Open,
    /// One response committed durably.
    Accepted {
        /// Original acceptance outcome.
        receipt: InputReceipt,
    },
    /// Resolved without a response.
    Closed {
        /// Reason acceptance is no longer possible.
        reason: InputClosure,
        /// Persistence closure timestamp.
        closed_at: DateTime<Utc>,
    },
}

/// Durable request plus immutable metadata and current lifecycle state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputRequest {
    /// Tenant ownership, resolved by the host.
    pub tenant_id: String,
    /// Root instance identity.
    pub instance_id: String,
    /// Bounded index key for the full signal identity.
    pub request_id: String,
    /// Logical invocation path; empty for a root-owned wait.
    pub invocation_path: String,
    /// Current child write fence, rebound only by legitimate replay registration.
    pub fence: Option<AttemptFence>,
    /// Immutable registration facts.
    pub spec: InputRequestSpec,
    /// Persistence registration timestamp, retained on replay.
    pub created_at: DateTime<Utc>,
    /// Authoritative state.
    pub state: InputState,
    /// Retryable notification intent; never implies permission to resume a pause.
    pub wake_pending: bool,
}

impl InputRequest {
    /// Whether this record is open at a persistence timestamp. The caller must
    /// additionally check root and logical invocation eligibility atomically.
    pub fn open_at(&self, now: DateTime<Utc>) -> bool {
        self.state == InputState::Open && self.spec.deadline.is_none_or(|at| at > now)
    }

    /// Close only unanswered requests, retaining successful receipts forever.
    pub fn close(&mut self, reason: InputClosure, now: DateTime<Utc>) {
        if self.state == InputState::Open {
            self.state = InputState::Closed {
                reason,
                closed_at: now,
            };
            self.wake_pending = false;
        }
    }
}

/// A new response validated against the exact immutable request specification.
/// Private fields prevent adapters from bypassing schema validation.
#[derive(Clone, Debug)]
pub struct ValidatedInputResponse {
    spec: InputRequestSpec,
    operation_id: String,
    payload: Vec<u8>,
    acceptance_context: Option<InputAcceptanceContext>,
}

impl ValidatedInputResponse {
    /// Validate a new operation. Receipt replay must precede this operation.
    pub fn new(spec: &InputRequestSpec, operation_id: &str, payload: &Value) -> InputResult<Self> {
        validate_operation_id(operation_id)?;
        if let Some(schema) = &spec.response_schema
            && !runtara_dsl::input_validation::is_empty_schema(schema)
        {
            runtara_dsl::input_validation::validate_inputs(payload, schema)
                .map_err(InputError::InvalidPayload)?;
        }
        Ok(Self {
            spec: spec.clone(),
            operation_id: operation_id.into(),
            payload: canonical_payload(payload),
            acceptance_context: None,
        })
    }

    /// Attach trusted context without weakening effective-payload validation.
    pub fn with_context(mut self, context: &InputAcceptanceContext) -> Self {
        self.acceptance_context = Some(context.clone());
        self
    }

    /// Canonical adapter context committed atomically with acceptance.
    pub fn acceptance_context(&self) -> Option<&[u8]> {
        self.acceptance_context
            .as_ref()
            .map(InputAcceptanceContext::as_bytes)
    }

    /// Identity used for the receipt check under the acceptance lock.
    pub fn replay_identity(&self) -> InputReplayIdentity<'_> {
        match self.acceptance_context() {
            Some(context) => InputReplayIdentity::Context(context),
            None => InputReplayIdentity::Payload(self.payload()),
        }
    }

    /// Immutable request facts that must still match at acceptance.
    pub fn spec(&self) -> &InputRequestSpec {
        &self.spec
    }

    /// Stable caller identity for retries.
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    /// Validated canonical bytes.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

/// Validate a bounded opaque caller operation identity.
pub fn validate_operation_id(id: &str) -> InputResult<()> {
    if id.is_empty() || id.len() > 128 || id.chars().any(char::is_control) {
        Err(InputError::InvalidRequest)
    } else {
        Ok(())
    }
}

/// Canonical JSON for idempotency. Sort even when serde's preserve_order feature
/// is enabled elsewhere in the workspace. Arrays and scalar types stay intact.
pub fn canonical_payload(payload: &Value) -> Vec<u8> {
    let mut value = payload.clone();
    value.sort_all_objects();
    serde_json::to_vec(&value).expect("JSON Value always serializes")
}

/// Request discovery page; count and records share the same store snapshot.
#[derive(Clone, Debug)]
pub struct InputRequestPage {
    /// Deterministically ordered actionable requests.
    pub requests: Vec<InputRequest>,
    /// Count before offset/limit over requests, not instances.
    pub total_count: u64,
}

/// Input failures are typed across persistence and transport adapters.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InputError {
    /// Missing or foreign-owned root/request; deliberately indistinguishable.
    #[error("input request not found")]
    NotFound,
    /// Bad envelope/identity.
    #[error("invalid input request")]
    InvalidRequest,
    /// Invalid response against the registered schema.
    #[error("invalid input response: {0}")]
    InvalidPayload(String),
    /// Immutable metadata or full identity differs on registration.
    #[error("input request identity conflicts with its registration")]
    IdentityConflict,
    /// A request is closed, its deadline elapsed, or its root terminated.
    #[error("input request is no longer active")]
    Inactive,
    /// A different operation already answered this request.
    #[error("input request is already answered")]
    AlreadyAnswered,
    /// An operation identity was previously used for different content/target.
    #[error("input operation identity conflicts with its receipt")]
    OperationConflict,
    /// A guest or old execution does not own this request.
    #[error("input request execution fence rejected")]
    FenceRejected,
    /// A preexisting raw mailbox value conflicts with managed registration.
    #[error("input request address already has a raw signal")]
    RawSignalConflict,
    /// Backend failure, never interpreted as empty discovery or permission.
    #[error("input request storage failed: {0}")]
    Storage(String),
}

/// Result from authoritative input operations.
pub type InputResult<T> = Result<T, InputError>;

/// Atomic managed-input capability. All writers lock the root before requests.
#[async_trait]
pub trait InputRequests: Send + Sync {
    /// Recover at most `limit` parked roots with pending accepted responses.
    /// Uses the same root lock and eligibility check as acceptance/parking;
    /// never shortens a scheduler claim or wakes an explicit pause.
    async fn reconcile_input_wakes(&self, limit: u32) -> InputResult<u64>;

    /// The clock deadlines and expiry are evaluated against.
    async fn input_clock(&self) -> InputResult<DateTime<Utc>> {
        Ok(Utc::now())
    }

    /// Register/replay a logical wait under current execution authority.
    /// Identity is the full signal id plus logical owner; a replay keeps the
    /// first registration's metadata and deadline rather than comparing them.
    async fn register_input(
        &self,
        authority: &InputAuthority,
        spec: &InputRequestSpec,
    ) -> InputResult<InputRequest>;

    /// Authorized retained record lookup, including accepted/closed requests.
    async fn get_input(
        &self,
        tenant: &str,
        instance: &str,
        request: &str,
    ) -> InputResult<InputRequest>;

    /// Fenced guest read. Expire an unanswered request using persistence time
    /// before returning its state; acceptance and this transition arbitrate
    /// under the same lock. Accepted bytes remain readable after the deadline.
    async fn poll_input(
        &self,
        authority: &InputAuthority,
        request: &str,
    ) -> InputResult<InputRequest>;

    /// Lookup and compare a retained receipt before any liveness/schema checks.
    async fn replay_input(
        &self,
        tenant: &str,
        instance: &str,
        request: &str,
        operation: &str,
        identity: InputReplayIdentity<'_>,
    ) -> InputResult<Option<InputReceipt>>;

    /// Accept atomically, rechecking the receipt, immutable spec and eligibility.
    async fn accept_input(
        &self,
        tenant: &str,
        instance: &str,
        response: &ValidatedInputResponse,
    ) -> InputResult<InputReceipt>;

    /// Close under host authority. If acceptance won, return its retained state.
    /// Expired closure only succeeds once persistence time reaches the deadline.
    async fn close_input(
        &self,
        authority: &InputAuthority,
        request: &str,
        reason: InputClosure,
    ) -> InputResult<InputRequest>;

    /// Page actionable requests for authorized roots in one tenant. Empty roots
    /// means an empty page, never an unfiltered tenant scan. Reject foreign or
    /// missing roots rather than treating them as empty.
    async fn list_inputs(
        &self,
        tenant: &str,
        instances: &[String],
        offset: u64,
        limit: u32,
    ) -> InputResult<InputRequestPage>;

    /// Return the subset of authorized roots with actionable requests in one
    /// snapshot, using the same eligibility rules as `list_inputs`. Missing or
    /// foreign roots fail the whole batch; empty input returns an empty set.
    async fn instances_with_open_inputs(
        &self,
        tenant: &str,
        instances: &[String],
    ) -> InputResult<std::collections::BTreeSet<String>>;
}

/// Shared validated submission used by every transport. Receipt replay precedes
/// current request/schema checks, and the store repeats it under the root lock.
pub async fn submit_input(
    inputs: &dyn InputRequests,
    tenant: &str,
    instance: &str,
    request: &str,
    operation: &str,
    payload: &Value,
) -> InputResult<InputReceipt> {
    submit_input_with_context(inputs, tenant, instance, request, operation, payload, None).await
}

/// Validated acceptance for trusted enriching adapters. Authorization must be
/// current before either contextual replay or a new acceptance is attempted.
#[allow(clippy::too_many_arguments)]
pub async fn submit_input_with_context(
    inputs: &dyn InputRequests,
    tenant: &str,
    instance: &str,
    request: &str,
    operation: &str,
    payload: &Value,
    context: Option<&InputAcceptanceContext>,
) -> InputResult<InputReceipt> {
    let canonical = canonical_payload(payload);
    let identity = || match context {
        Some(context) => InputReplayIdentity::Context(context.as_bytes()),
        None => InputReplayIdentity::Payload(&canonical),
    };
    if let Some(receipt) = inputs
        .replay_input(tenant, instance, request, operation, identity())
        .await?
    {
        return Ok(receipt);
    }
    let registered = inputs.get_input(tenant, instance, request).await?;
    let validated =
        ValidatedInputResponse::new(&registered.spec, operation, payload).map(|response| {
            match context {
                Some(context) => response.with_context(context),
                None => response,
            }
        });
    match validated {
        Ok(response) => inputs.accept_input(tenant, instance, &response).await,
        Err(error) => {
            // An acknowledgement can have been lost while another submitter
            // commits. An existing operation always returns its durable result.
            match inputs
                .replay_input(tenant, instance, request, operation, identity())
                .await?
            {
                Some(receipt) => Ok(receipt),
                None => Err(error),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonicalization_preserves_scalar_types_and_array_order() {
        let a: Value =
            serde_json::from_str(r#"{"b": [1, 2], "a": {"d": true, "c": null}}"#).unwrap();
        let b: Value =
            serde_json::from_str(r#"{ "a": { "c": null, "d": true }, "b": [1, 2] }"#).unwrap();
        assert_eq!(canonical_payload(&a), canonical_payload(&b));
        assert_ne!(
            canonical_payload(&json!([1, 2])),
            canonical_payload(&json!([2, 1]))
        );
        assert_ne!(canonical_payload(&json!(1)), canonical_payload(&json!("1")));
        assert_ne!(canonical_payload(&json!(1)), canonical_payload(&json!(1.0)));
    }

    #[test]
    fn response_validation_uses_both_registered_schema_formats() {
        for schema in [
            json!({"answer": {"type": "string", "required": true}}),
            json!({"type": "object", "properties": {"answer": {"type": "string"}}, "required": ["answer"]}),
        ] {
            let spec = InputRequestSpec {
                signal_id: "wait".into(),
                response_schema: Some(schema),
                metadata: json!({}),
                deadline: None,
            };
            assert!(
                ValidatedInputResponse::new(&spec, "operation", &json!({"answer":"yes"})).is_ok()
            );
            assert!(matches!(
                ValidatedInputResponse::new(&spec, "operation", &json!({"answer":false})),
                Err(InputError::InvalidPayload(_))
            ));
            assert!(matches!(
                ValidatedInputResponse::new(&spec, "operation", &json!({})),
                Err(InputError::InvalidPayload(_))
            ));
        }
    }
}
