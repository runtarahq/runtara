//! Atomic managed-response queue. No unresolved envelope has a TTL.
use redis::aio::ConnectionManager;
use runtara_core::persistence::inputs::{InputReceipt, canonical_payload};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

const SCRIPT: &str = include_str!("managed.lua");
/// Completed envelopes and their deduplication identity are retained for seven days.
pub const COMPLETED_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;
const MAX_LEASE_MS: u32 = 300_000;

#[derive(Debug, thiserror::Error)]
pub enum QueueError {
    #[error("invalid queue operation")]
    Invalid,
    #[error("queue message identity conflicts with its original submission")]
    Conflict,
    #[error("queue delivery lease is no longer current")]
    LeaseLost,
    #[error("queue message not found")]
    NotFound,
    #[error("queue contains corrupt state")]
    Corrupt,
    #[error("queue unavailable")]
    Backend(#[from] redis::RedisError),
}
pub type QueueResult<T> = Result<T, QueueError>;

#[derive(Clone, Debug)]
pub struct QueueScope {
    tenant_id: String,
    session_id: String,
    prefix: String,
}
impl QueueScope {
    pub fn new(tenant_id: &str, session_id: &str) -> QueueResult<Self> {
        if [tenant_id, session_id]
            .iter()
            .any(|s| s.is_empty() || s.len() > 1024 || s.contains('\0'))
        {
            return Err(QueueError::Invalid);
        }
        // Encode the pair before hashing; colons/braces supplied by callers cannot
        // collide with another tenant/session or select a different cluster slot.
        let identity = serde_json::to_vec(&[tenant_id, session_id]).expect("string pair");
        let digest = Sha256::digest(identity);
        Ok(Self {
            tenant_id: tenant_id.into(),
            session_id: session_id.into(),
            prefix: format!("runtara:session:{{{digest:x}}}"),
        })
    }
    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    fn keys(&self) -> [String; 5] {
        ["pending", "envelopes", "operations", "completed", "owner"]
            .map(|suffix| format!("{}:{suffix}", self.prefix))
    }
    async fn run(
        &self,
        conn: &mut ConnectionManager,
        args: &[String],
    ) -> QueueResult<(String, Option<String>)> {
        let script = redis::Script::new(SCRIPT);
        let mut call = script.prepare_invoke();
        for key in self.keys() {
            call.key(key);
        }
        for arg in args {
            call.arg(arg);
        }
        call.arg(&self.tenant_id).arg(&self.session_id);
        let mut result: Vec<String> = call.invoke_async(conn).await?;
        if result.is_empty() {
            return Err(QueueError::Corrupt);
        }
        let code = result.remove(0);
        let body = result.into_iter().next();
        match code.as_str() {
            "ok" | "empty" | "busy" | "blocked" | "deferred" => Ok((code, body)),
            "conflict" => Err(QueueError::Conflict),
            "lease_lost" => Err(QueueError::LeaseLost),
            "not_found" => Err(QueueError::NotFound),
            "invalid" => Err(QueueError::Invalid),
            _ => Err(QueueError::Corrupt),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputTarget {
    pub instance_id: String,
    pub request_id: String,
}
impl InputTarget {
    fn validate(&self) -> QueueResult<()> {
        validate_id(&self.instance_id)?;
        validate_id(&self.request_id)
    }
}
/// Trusted session routing, configured by an authorized execution launch.
#[derive(Clone, Serialize, Deserialize)]
pub struct SessionRoute {
    pub workflow_id: String,
    pub instance_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    Queued,
    Leased,
    Retry,
    Blocked,
    Accepted,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryReason {
    NoTarget,
    AmbiguousTarget,
    StaleTarget,
    InvalidPayload,
    OperationConflict,
    BackendUnavailable,
    ExplicitFailure,
    LaunchRejected,
}

/// Payload stays encoded until Rust decodes it; Redis Lua only changes metadata.
#[derive(Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub message_id: String,
    pub operation_id: String,
    payload_json: String,
    pub enqueued_at_ms: u64,
    pub state: DeliveryState,
    pub target: Option<InputTarget>,
    pub attempts: u64,
    pub lease_token: Option<String>,
    pub lease_deadline_ms: Option<u64>,
    pub retry_at_ms: Option<u64>,
    pub reason: Option<DeliveryReason>,
    pub completed_at_ms: Option<u64>,
    pub receipt_id: Option<String>,
}
impl Envelope {
    pub fn payload(&self) -> QueueResult<Value> {
        serde_json::from_str(&self.payload_json).map_err(|_| QueueError::Corrupt)
    }
}
impl std::fmt::Debug for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Envelope")
            .field("message_id", &self.message_id)
            .field("state", &self.state)
            .field("target", &self.target)
            .finish_non_exhaustive()
    }
}
#[derive(Debug)]
pub enum ClaimOutcome {
    Claimed(Envelope),
    Empty,
    Busy,
    Deferred(Envelope),
    Blocked(Envelope),
}

fn validate_id(value: &str) -> QueueResult<()> {
    if value.is_empty() || value.len() > 128 || value.contains('\0') {
        Err(QueueError::Invalid)
    } else {
        Ok(())
    }
}
fn decode(body: Option<String>) -> QueueResult<Envelope> {
    let envelope: Envelope =
        serde_json::from_str(&body.ok_or(QueueError::Corrupt)?).map_err(|_| QueueError::Corrupt)?;
    envelope.payload()?;
    Ok(envelope)
}
fn envelope_result(result: (String, Option<String>)) -> QueueResult<Envelope> {
    if result.0 == "empty" {
        return Err(QueueError::NotFound);
    }
    if result.0 != "ok" {
        return Err(QueueError::Corrupt);
    }
    decode(result.1)
}

/// Keep scan progress even when one queue has corrupt owner metadata. Workers
/// report these keys for repair while continuing other sessions.
#[derive(Debug)]
pub struct ScopeScan {
    pub cursor: u64,
    pub scopes: Vec<QueueScope>,
    pub corrupt_keys: Vec<String>,
}

/// Discover retained queues after a restart, including queues without an SSE
/// subscriber. `count` is a SCAN work hint; never discard returned keys when a
/// server page exceeds it. A worker must finish one scan cursor before restarting.
pub async fn scan_scopes(
    conn: &mut ConnectionManager,
    cursor: u64,
    count: u32,
) -> QueueResult<ScopeScan> {
    if count == 0 || count > 1000 {
        return Err(QueueError::Invalid);
    }
    let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
        .arg(cursor)
        .arg("MATCH")
        .arg("runtara:session:*:owner")
        .arg("COUNT")
        .arg(count)
        .query_async(conn)
        .await?;
    let mut scan = ScopeScan {
        cursor: next,
        scopes: Vec::with_capacity(keys.len()),
        corrupt_keys: Vec::new(),
    };
    for key in keys {
        let owner: redis::RedisResult<(Option<String>, Option<String>)> = redis::cmd("HMGET")
            .arg(&key)
            .arg("tenant_id")
            .arg("session_id")
            .query_async(conn)
            .await;
        let (tenant, session) = match owner {
            Ok(owner) => owner,
            Err(error) if error.code() == Some("WRONGTYPE") => {
                scan.corrupt_keys.push(key);
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        match (tenant, session) {
            (None, None) => {
                let exists: bool = redis::cmd("EXISTS").arg(&key).query_async(conn).await?;
                if exists {
                    scan.corrupt_keys.push(key);
                }
                // Otherwise concurrent completed-queue cleanup removed this key.
            }
            (Some(tenant), Some(session)) => match QueueScope::new(&tenant, &session) {
                Ok(scope) if scope.keys()[4] == key => scan.scopes.push(scope),
                _ => scan.corrupt_keys.push(key),
            },
            _ => scan.corrupt_keys.push(key),
        }
    }
    Ok(scan)
}

pub async fn configure_route(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    route: &SessionRoute,
) -> QueueResult<()> {
    validate_id(&route.instance_id)?;
    validate_id(&route.workflow_id)?;
    let (code, _) = scope
        .run(
            conn,
            &[
                "configure_route".into(),
                serde_json::to_string(route).expect("route"),
            ],
        )
        .await?;
    if code == "ok" {
        Ok(())
    } else {
        Err(QueueError::Corrupt)
    }
}
/// Route changes/new starts must not bypass an unresolved response.
pub async fn has_unresolved(conn: &mut ConnectionManager, scope: &QueueScope) -> QueueResult<bool> {
    let (code, body) = scope.run(conn, &["has_unresolved".into()]).await?;
    if code != "ok" {
        return Err(QueueError::Corrupt);
    }
    let count: u64 = body
        .ok_or(QueueError::Corrupt)?
        .parse()
        .map_err(|_| QueueError::Corrupt)?;
    Ok(count > 0)
}

pub async fn session_route(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
) -> QueueResult<SessionRoute> {
    let (code, body) = scope.run(conn, &["session_route".into()]).await?;
    if code != "ok" {
        return Err(QueueError::Corrupt);
    }
    serde_json::from_str(&body.ok_or(QueueError::Corrupt)?).map_err(|_| QueueError::Corrupt)
}
#[derive(Deserialize)]
pub struct EnvelopePage {
    pub cursor: u64,
    pub envelopes: Vec<Envelope>,
}
pub async fn scan_envelopes(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    cursor: u64,
    count: u32,
) -> QueueResult<EnvelopePage> {
    if count == 0 || count > 100 {
        return Err(QueueError::Invalid);
    }
    let (code, body) = scope
        .run(
            conn,
            &[
                "scan_envelopes".into(),
                cursor.to_string(),
                count.to_string(),
            ],
        )
        .await?;
    if code != "ok" {
        return Err(QueueError::Corrupt);
    }
    let page: EnvelopePage =
        serde_json::from_str(&body.ok_or(QueueError::Corrupt)?).map_err(|_| QueueError::Corrupt)?;
    for envelope in &page.envelopes {
        envelope.payload()?;
    }
    Ok(page)
}

pub async fn enqueue(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    message_id: &str,
    operation_id: &str,
    payload: &Value,
) -> QueueResult<Envelope> {
    enqueue_with_target(conn, scope, message_id, operation_id, payload, None).await
}

/// Retain an explicitly selected response and its immutable target atomically.
/// The caller authorizes the target; acceptance still checks current eligibility.
/// A worker must never observe the response without its selected request binding.
pub async fn enqueue_targeted(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    message_id: &str,
    operation_id: &str,
    payload: &Value,
    target: &InputTarget,
) -> QueueResult<Envelope> {
    target.validate()?;
    enqueue_with_target(conn, scope, message_id, operation_id, payload, Some(target)).await
}

async fn enqueue_with_target(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    message_id: &str,
    operation_id: &str,
    payload: &Value,
    target: Option<&InputTarget>,
) -> QueueResult<Envelope> {
    validate_id(message_id)?;
    validate_id(operation_id)?;
    let encoded = String::from_utf8(canonical_payload(payload)).expect("JSON UTF-8");
    envelope_result(
        scope
            .run(
                conn,
                &[
                    "enqueue".into(),
                    message_id.into(),
                    operation_id.into(),
                    encoded,
                    target
                        .map(|target| serde_json::to_string(target).expect("target"))
                        .unwrap_or_default(),
                ],
            )
            .await?,
    )
}

pub async fn get(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    message_id: &str,
) -> QueueResult<Envelope> {
    validate_id(message_id)?;
    envelope_result(scope.run(conn, &["get".into(), message_id.into()]).await?)
}
pub async fn claim(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    lease_ms: u32,
) -> QueueResult<ClaimOutcome> {
    if lease_ms == 0 || lease_ms > MAX_LEASE_MS {
        return Err(QueueError::Invalid);
    }
    let (code, body) = scope
        .run(
            conn,
            &[
                "claim".into(),
                Uuid::new_v4().to_string(),
                lease_ms.to_string(),
            ],
        )
        .await?;
    match code.as_str() {
        "empty" => Ok(ClaimOutcome::Empty),
        "busy" => Ok(ClaimOutcome::Busy),
        "ok" => Ok(ClaimOutcome::Claimed(decode(body)?)),
        "blocked" => Ok(ClaimOutcome::Blocked(decode(body)?)),
        "deferred" => Ok(ClaimOutcome::Deferred(decode(body)?)),
        _ => Err(QueueError::Corrupt),
    }
}
async fn transition(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    lease: &Envelope,
    op: &str,
    value: String,
    delay_ms: u32,
) -> QueueResult<Envelope> {
    let token = lease.lease_token.clone().ok_or(QueueError::LeaseLost)?;
    let result = scope
        .run(
            conn,
            &[
                op.into(),
                lease.message_id.clone(),
                token,
                value,
                delay_ms.to_string(),
                COMPLETED_RETENTION_MS.to_string(),
            ],
        )
        .await?;
    if result.0 == "empty" {
        return Err(QueueError::LeaseLost);
    }
    envelope_result(result)
}
pub async fn bind(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    lease: &Envelope,
    target: &InputTarget,
) -> QueueResult<Envelope> {
    target.validate()?;
    transition(
        conn,
        scope,
        lease,
        "bind",
        serde_json::to_string(target).expect("target"),
        0,
    )
    .await
}
pub async fn renew(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    lease: &Envelope,
    lease_ms: u32,
) -> QueueResult<Envelope> {
    if lease_ms == 0 || lease_ms > MAX_LEASE_MS {
        return Err(QueueError::Invalid);
    }
    transition(conn, scope, lease, "renew", lease_ms.to_string(), 0).await
}
pub async fn acknowledge(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    lease: &Envelope,
    receipt: &InputReceipt,
) -> QueueResult<Envelope> {
    // Refuse to acknowledge a different payload, even if a caller mixes receipts.
    if receipt.payload != lease.payload_json.as_bytes() {
        return Err(QueueError::Conflict);
    }
    let value = serde_json::json!({"request_id":receipt.request_id,"operation_id":receipt.operation_id,"receipt_id":receipt.receipt_id});
    transition(conn, scope, lease, "ack", value.to_string(), 0).await
}
pub async fn retry(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    lease: &Envelope,
    reason: DeliveryReason,
    delay_ms: u32,
) -> QueueResult<Envelope> {
    if delay_ms == 0 || delay_ms > MAX_LEASE_MS {
        return Err(QueueError::Invalid);
    }
    transition(conn, scope, lease, "retry", reason_name(reason), delay_ms).await
}
pub async fn block(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    lease: &Envelope,
    reason: DeliveryReason,
) -> QueueResult<Envelope> {
    transition(conn, scope, lease, "block", reason_name(reason), 0).await
}
fn reason_name(reason: DeliveryReason) -> String {
    serde_json::to_value(reason)
        .expect("reason")
        .as_str()
        .expect("enum string")
        .into()
}
/// Caller must authorize the target. Only an unbound blocked head can acquire a
/// new target. An existing binding can be retried, but never changed.
pub async fn resolve(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    message_id: &str,
    target: &InputTarget,
) -> QueueResult<Envelope> {
    validate_id(message_id)?;
    target.validate()?;
    envelope_result(
        scope
            .run(
                conn,
                &[
                    "resolve".into(),
                    message_id.into(),
                    String::new(),
                    serde_json::to_string(target).expect("target"),
                ],
            )
            .await?,
    )
}
pub async fn fail_blocked(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    message_id: &str,
) -> QueueResult<Envelope> {
    validate_id(message_id)?;
    envelope_result(
        scope
            .run(
                conn,
                &[
                    "fail_blocked".into(),
                    message_id.into(),
                    String::new(),
                    String::new(),
                    String::new(),
                    COMPLETED_RETENTION_MS.to_string(),
                ],
            )
            .await?,
    )
}
/// How long a drained session keeps its route metadata before it is forgotten.
pub const IDLE_ROUTE_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// Remove expired accepted/failed envelopes. A drained queue's owner metadata
/// then starts an idle expiry window, which any new message cancels.
pub async fn prune_completed(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    limit: u32,
) -> QueueResult<u32> {
    if limit == 0 || limit > 1000 {
        return Err(QueueError::Invalid);
    }
    let (code, result) = scope
        .run(
            conn,
            &[
                "prune".into(),
                limit.to_string(),
                IDLE_ROUTE_TTL_SECS.to_string(),
            ],
        )
        .await?;
    if code != "ok" {
        return Err(QueueError::Corrupt);
    }
    result
        .ok_or(QueueError::Corrupt)?
        .parse()
        .map_err(|_| QueueError::Corrupt)
}

#[derive(Debug)]
pub enum DeliveryOutcome {
    Idle,
    Busy,
    Deferred(Envelope),
    Blocked(Envelope),
    Accepted(Envelope),
}

/// One bounded delivery attempt, shared by session/channel workers. Responses are
/// bound to their request when retained; delivery never picks a target itself.
pub async fn deliver_to_instance(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    client: &crate::runtime_client::RuntimeClient,
) -> QueueResult<DeliveryOutcome> {
    let lease = match claim(conn, scope, 30_000).await? {
        ClaimOutcome::Empty => return Ok(DeliveryOutcome::Idle),
        ClaimOutcome::Busy => return Ok(DeliveryOutcome::Busy),
        ClaimOutcome::Blocked(envelope) => return Ok(DeliveryOutcome::Blocked(envelope)),
        ClaimOutcome::Deferred(envelope) => return Ok(DeliveryOutcome::Deferred(envelope)),
        ClaimOutcome::Claimed(envelope) => envelope,
    };
    deliver_claimed(conn, scope, client, lease).await
}

/// Submit a claimed response to the request it was bound to. An unbound message
/// is blocked for explicit resolution: whichever request happens to be open at
/// delivery time is not necessarily the one the sender answered, and binding it
/// there would resurrect the stale-input bug through the queue.
pub async fn deliver_claimed(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    client: &crate::runtime_client::RuntimeClient,
    lease: Envelope,
) -> QueueResult<DeliveryOutcome> {
    use runtara_core::persistence::inputs::InputError;
    if lease.target.is_none() {
        return Ok(DeliveryOutcome::Blocked(
            block(conn, scope, &lease, DeliveryReason::NoTarget).await?,
        ));
    }
    let target = lease.target.as_ref().ok_or(QueueError::Corrupt)?;
    let outcome = client
        .submit_input_response(
            scope.tenant_id(),
            &target.instance_id,
            &target.request_id,
            &lease.operation_id,
            &lease.payload()?,
        )
        .await;
    match outcome {
        Ok(receipt) => Ok(DeliveryOutcome::Accepted(
            acknowledge(conn, scope, &lease, &receipt).await?,
        )),
        Err(error) => {
            let reason = match error {
                InputError::Storage(_) => {
                    return defer(conn, scope, &lease, DeliveryReason::BackendUnavailable).await;
                }
                // A fence rejection is permanent for this target (its owner is
                // gone), so retrying it forever would wedge the session queue.
                InputError::NotFound
                | InputError::Inactive
                | InputError::AlreadyAnswered
                | InputError::FenceRejected => DeliveryReason::StaleTarget,
                InputError::InvalidPayload(_) => DeliveryReason::InvalidPayload,
                _ => DeliveryReason::OperationConflict,
            };
            Ok(DeliveryOutcome::Blocked(
                block(conn, scope, &lease, reason).await?,
            ))
        }
    }
}
pub(super) async fn defer(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    lease: &Envelope,
    reason: DeliveryReason,
) -> QueueResult<DeliveryOutcome> {
    let delay = 1000u32
        .saturating_mul(1 << lease.attempts.min(5))
        .min(30_000);
    Ok(DeliveryOutcome::Deferred(
        retry(conn, scope, lease, reason, delay).await?,
    ))
}

#[cfg(all(test, feature = "valkey-integration-tests"))]
mod tests;
