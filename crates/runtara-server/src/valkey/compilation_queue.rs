//! Compilation Queue
//!
//! A Valkey-based queue for workflow compilation requests.
//! Uses a Redis SET for deduplication (only one entry per workflow:version)
//! and a LIST for ordered processing.
//!
//! Key features:
//! - Unique entries: same workflow/version won't be queued twice
//! - Ordered processing: FIFO queue semantics
//! - Atomic operations: uses Lua scripts for thread safety
//! - Polling support: for waiting until compilation completes

use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Script};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::{debug, info, warn};

/// A compilation request in the queue
//
// Note: no `Eq`/`Hash`/`PartialEq` — `product_event` can carry a `ProductEvent` whose
// `serde_json::Value` properties are neither. Dedup uses `unique_key()` (a string), not the
// struct's identity, so these derives weren't needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompilationRequest {
    pub tenant_id: String,
    pub workflow_id: String,
    pub version: i32,
    #[serde(default)]
    pub force_recompile: bool,
    /// Optional pre-built, attributed `workflow.compiled` event supplied by the enqueuer (which
    /// has the caller/surface context). The worker fills in the outcome (`success`,
    /// `occurred_at`) and emits it. `None` → the worker emits its own no-user, `Worker`-source
    /// default. Either way the worker is the single emit point, so the event lands exactly once.
    #[serde(default)]
    pub product_event: Option<crate::product_events::ProductEvent>,
}

impl CompilationRequest {
    pub fn new(tenant_id: String, workflow_id: String, version: i32) -> Self {
        Self {
            tenant_id,
            workflow_id,
            version,
            force_recompile: false,
            product_event: None,
        }
    }

    pub fn new_with_force(
        tenant_id: String,
        workflow_id: String,
        version: i32,
        force_recompile: bool,
    ) -> Self {
        Self {
            tenant_id,
            workflow_id,
            version,
            force_recompile,
            product_event: None,
        }
    }

    /// Attach a pre-built, attributed `workflow.compiled` event for the worker to emit on
    /// completion. Builder-style.
    pub fn with_product_event(
        mut self,
        product_event: Option<crate::product_events::ProductEvent>,
    ) -> Self {
        self.product_event = product_event;
        self
    }

    /// Create a unique key for this request (used for deduplication)
    pub fn unique_key(&self) -> String {
        format!("{}:{}:{}", self.tenant_id, self.workflow_id, self.version)
    }

    /// Serialize the full request for the queue payload store.
    pub fn payload(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Parse from unique key
    pub fn from_unique_key(key: &str) -> Option<Self> {
        let parts: Vec<&str> = key.splitn(3, ':').collect();
        if parts.len() == 3 {
            Some(Self {
                tenant_id: parts[0].to_string(),
                workflow_id: parts[1].to_string(),
                version: parts[2].parse().ok()?,
                force_recompile: false,
                product_event: None,
            })
        } else {
            None
        }
    }

    /// Parse a full queue payload. Falls back to the legacy key format.
    pub fn from_payload(payload: &str) -> Option<Self> {
        serde_json::from_str(payload)
            .ok()
            .or_else(|| Self::from_unique_key(payload))
    }
}

/// Compilation queue backed by Redis/Valkey
///
/// Uses three Redis keys per tenant, so processes serving different tenants
/// can share one Valkey without taking each other's work:
/// - `{tenant}:compilation:queue` - LIST for ordered processing
/// - `{tenant}:compilation:pending` - SET for deduplication and tracking
/// - `{tenant}:compilation:requests` - HASH of full request payloads
pub struct CompilationQueue {
    /// Shared Redis connection manager (built once at startup and cloned per
    /// operation — no new TCP connection per call).
    manager: ConnectionManager,
    /// The tenants whose queues this process consumes.
    tenants: Vec<String>,
    /// Which tenant's queue `dequeue` tries first, rotated so one busy tenant
    /// does not starve the others.
    next: std::sync::atomic::AtomicUsize,
}

/// The single-Valkey-per-tenant keys of earlier versions, drained into the
/// tenant-prefixed ones at startup.
const LEGACY_QUEUE_KEY: &str = "runtara:compilation:queue";
const LEGACY_PENDING_KEY: &str = "runtara:compilation:pending";
const LEGACY_REQUESTS_KEY: &str = "runtara:compilation:requests";
const QUEUE_SUFFIX: &str = ":compilation:queue";

fn queue_key(tenant: &str) -> String {
    format!("{tenant}{QUEUE_SUFFIX}")
}

fn pending_key(tenant: &str) -> String {
    format!("{tenant}:compilation:pending")
}

fn requests_key(tenant: &str) -> String {
    format!("{tenant}:compilation:requests")
}

impl CompilationQueue {
    /// Create a compilation queue consuming `tenants`' work from a shared
    /// connection manager.
    pub fn new(manager: ConnectionManager, tenants: Vec<String>) -> Self {
        Self {
            manager,
            tenants,
            next: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Get a connection from the shared manager (cheap clone — no TCP).
    fn get_connection(&self) -> ConnectionManager {
        self.manager.clone()
    }

    /// Enqueue a compilation request if not already pending
    ///
    /// Returns:
    /// - `Ok(true)` if the request was added to the queue
    /// - `Ok(false)` if the request was already pending (deduped)
    pub async fn enqueue(
        &self,
        request: &CompilationRequest,
    ) -> Result<bool, CompilationQueueError> {
        let mut conn = self.get_connection();

        let key = request.unique_key();
        let payload = request
            .payload()
            .map_err(|e| CompilationQueueError::ParseError(e.to_string()))?;
        let force_recompile = if request.force_recompile { "1" } else { "0" };

        // Lua script for atomic enqueue with deduplication:
        // 1. Check if key exists in pending set
        // 2. If not, add to pending set and push to queue
        // 3. Store full payload separately so options like force_recompile survive worker restarts
        // 4. Return 1 if added, 0 if already pending
        let script = Script::new(
            r#"
            local pending_key = KEYS[1]
            local queue_key = KEYS[2]
            local requests_key = KEYS[3]
            local request_key = ARGV[1]
            local request_payload = ARGV[2]
            local force_recompile = ARGV[3]

            -- Check if already pending
            if redis.call('SISMEMBER', pending_key, request_key) == 1 then
                if force_recompile == '1' then
                    redis.call('HSET', requests_key, request_key, request_payload)
                end
                return 0
            end

            -- Add to pending set and queue
            redis.call('SADD', pending_key, request_key)
            redis.call('HSET', requests_key, request_key, request_payload)
            redis.call('RPUSH', queue_key, request_key)
            return 1
            "#,
        );

        let added: i32 = script
            .key(pending_key(&request.tenant_id))
            .key(queue_key(&request.tenant_id))
            .key(requests_key(&request.tenant_id))
            .arg(&key)
            .arg(&payload)
            .arg(force_recompile)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;

        if added == 1 {
            info!(
                tenant_id = %request.tenant_id,
                workflow_id = %request.workflow_id,
                version = request.version,
                force_recompile = request.force_recompile,
                "Enqueued compilation request"
            );
            Ok(true)
        } else {
            debug!(
                tenant_id = %request.tenant_id,
                workflow_id = %request.workflow_id,
                version = request.version,
                force_recompile = request.force_recompile,
                "Compilation request already pending (deduped)"
            );
            Ok(false)
        }
    }

    /// Dequeue the next compilation request (blocking with timeout)
    ///
    /// Returns `None` if no request is available within the timeout.
    pub async fn dequeue(
        &self,
        timeout: Duration,
    ) -> Result<Option<CompilationRequest>, CompilationQueueError> {
        let mut conn = self.get_connection();
        if self.tenants.is_empty() {
            tokio::time::sleep(timeout).await;
            return Ok(None);
        }

        // BLPOP over every served tenant's queue, starting from a rotating one
        // (returns the list name and value).
        let start = self.next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut command = redis::cmd("BLPOP");
        for i in 0..self.tenants.len() {
            command.arg(queue_key(&self.tenants[(start + i) % self.tenants.len()]));
        }
        let result: Option<(String, String)> = command
            .arg(timeout.as_secs())
            .query_async(&mut conn)
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;

        match result {
            Some((queue_name, key)) => {
                let tenant = queue_name
                    .strip_suffix(QUEUE_SUFFIX)
                    .ok_or_else(|| CompilationQueueError::ParseError(queue_name.clone()))?;
                let payload: Option<String> = conn
                    .hget(requests_key(tenant), &key)
                    .await
                    .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;
                let request = payload
                    .as_deref()
                    .and_then(CompilationRequest::from_payload)
                    .or_else(|| CompilationRequest::from_payload(&key))
                    .ok_or_else(|| CompilationQueueError::ParseError(key.clone()))?;

                debug!(
                    tenant_id = %request.tenant_id,
                    workflow_id = %request.workflow_id,
                    version = request.version,
                    force_recompile = request.force_recompile,
                    "Dequeued compilation request"
                );

                Ok(Some(request))
            }
            None => Ok(None),
        }
    }

    /// Mark a compilation as complete (removes from pending set)
    ///
    /// Call this after compilation succeeds or fails.
    pub async fn complete(
        &self,
        request: &CompilationRequest,
    ) -> Result<(), CompilationQueueError> {
        let mut conn = self.get_connection();

        let key = request.unique_key();

        // Remove from pending set
        let _: () = conn
            .srem(pending_key(&request.tenant_id), &key)
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;
        let _: () = conn
            .hdel(requests_key(&request.tenant_id), &key)
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;

        info!(
            tenant_id = %request.tenant_id,
            workflow_id = %request.workflow_id,
            version = request.version,
            force_recompile = request.force_recompile,
            "Marked compilation as complete"
        );

        Ok(())
    }

    /// Check if a compilation is pending (in queue or being processed)
    pub async fn is_pending(
        &self,
        request: &CompilationRequest,
    ) -> Result<bool, CompilationQueueError> {
        let mut conn = self.get_connection();

        let key = request.unique_key();

        let is_member: bool = conn
            .sismember(pending_key(&request.tenant_id), &key)
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;

        Ok(is_member)
    }

    /// Get the number of pending compilations across the served tenants
    pub async fn pending_count(&self) -> Result<usize, CompilationQueueError> {
        let mut conn = self.get_connection();

        let mut count = 0usize;
        for tenant in &self.tenants {
            let pending: usize = conn
                .scard(pending_key(tenant))
                .await
                .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;
            count += pending;
        }

        Ok(count)
    }

    /// Wait until a specific compilation is complete
    ///
    /// Polls the pending set until the request is no longer pending.
    /// Returns `true` if compilation completed, `false` if timeout.
    pub async fn wait_for_completion(
        &self,
        request: &CompilationRequest,
        timeout: Duration,
        poll_interval: Duration,
    ) -> Result<bool, CompilationQueueError> {
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            if !self.is_pending(request).await? {
                return Ok(true);
            }
            tokio::time::sleep(poll_interval).await;
        }

        warn!(
            tenant_id = %request.tenant_id,
            workflow_id = %request.workflow_id,
            version = request.version,
            timeout_secs = timeout.as_secs(),
            "Timed out waiting for compilation"
        );

        Ok(false)
    }

    /// Recover orphaned pending compilations
    ///
    /// This should be called on worker startup to handle the case where
    /// a worker crashed after dequeuing an item but before completing it.
    ///
    /// It moves any pending items that are NOT in the queue back to the queue.
    pub async fn recover_orphaned(&self) -> Result<usize, CompilationQueueError> {
        self.drain_legacy().await?;
        let mut recovered = 0;
        for tenant in &self.tenants {
            recovered += self.recover_orphaned_for(tenant).await?;
        }
        Ok(recovered)
    }

    /// Move the served tenants' requests from the unprefixed keys of earlier
    /// versions into their own queues. Another tenant's requests are left.
    async fn drain_legacy(&self) -> Result<usize, CompilationQueueError> {
        let mut conn = self.get_connection();
        let pending: Vec<String> = conn
            .smembers(LEGACY_PENDING_KEY)
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;
        let mut moved = 0;
        for key in pending {
            let Some(parsed) = CompilationRequest::from_unique_key(&key) else {
                continue;
            };
            if !self.tenants.contains(&parsed.tenant_id) {
                continue;
            }
            let payload: Option<String> = conn
                .hget(LEGACY_REQUESTS_KEY, &key)
                .await
                .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;
            let request = payload
                .as_deref()
                .and_then(CompilationRequest::from_payload)
                .unwrap_or(parsed);
            self.enqueue(&request).await?;
            let _: () = redis::pipe()
                .lrem(LEGACY_QUEUE_KEY, 0, &key)
                .srem(LEGACY_PENDING_KEY, &key)
                .hdel(LEGACY_REQUESTS_KEY, &key)
                .query_async(&mut conn)
                .await
                .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;
            moved += 1;
        }
        if moved > 0 {
            info!(
                count = moved,
                "Moved compilation requests from the unprefixed queue"
            );
        }
        Ok(moved)
    }

    async fn recover_orphaned_for(&self, tenant: &str) -> Result<usize, CompilationQueueError> {
        let mut conn = self.get_connection();

        // Get all pending items
        let pending: Vec<String> = conn
            .smembers(pending_key(tenant))
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;

        if pending.is_empty() {
            return Ok(0);
        }

        // Get all items in the queue
        let queue_items: Vec<String> = conn
            .lrange(queue_key(tenant), 0, -1)
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;

        // Find orphaned items (in pending but not in queue)
        let orphaned: Vec<&String> = pending
            .iter()
            .filter(|item| !queue_items.contains(item))
            .collect();

        if orphaned.is_empty() {
            return Ok(0);
        }

        info!(
            count = orphaned.len(),
            "Found orphaned pending compilations, re-queueing"
        );

        // Re-add orphaned items to the queue
        for item in &orphaned {
            let _: () = conn
                .rpush(queue_key(tenant), item.as_str())
                .await
                .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;

            warn!(
                item = %item,
                "Re-queued orphaned compilation request"
            );
        }

        Ok(orphaned.len())
    }
}

/// Errors that can occur with the compilation queue
#[derive(Debug)]
pub enum CompilationQueueError {
    /// Failed to connect to Redis
    ConnectionError(String),
    /// Redis operation failed
    RedisError(String),
    /// Failed to parse queue entry
    ParseError(String),
}

impl std::fmt::Display for CompilationQueueError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompilationQueueError::ConnectionError(msg) => {
                write!(f, "Redis connection error: {}", msg)
            }
            CompilationQueueError::RedisError(msg) => write!(f, "Redis error: {}", msg),
            CompilationQueueError::ParseError(msg) => write!(f, "Parse error: {}", msg),
        }
    }
}

impl std::error::Error for CompilationQueueError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unique_key_roundtrip() {
        let request =
            CompilationRequest::new("tenant-1".to_string(), "workflow-123".to_string(), 42);

        let key = request.unique_key();
        assert_eq!(key, "tenant-1:workflow-123:42");

        let parsed = CompilationRequest::from_unique_key(&key).unwrap();
        assert_eq!(parsed.tenant_id, "tenant-1");
        assert_eq!(parsed.workflow_id, "workflow-123");
        assert_eq!(parsed.version, 42);
        assert!(!parsed.force_recompile);
    }

    #[test]
    fn test_unique_key_with_special_chars() {
        // Workflow IDs are UUIDs, so no special chars, but test tenant_id edge cases
        let request = CompilationRequest::new(
            "org_abc123".to_string(),
            "d93b2a2f-d4a9-427f-ad1a-3dae942ffb9a".to_string(),
            1,
        );

        let key = request.unique_key();
        let parsed = CompilationRequest::from_unique_key(&key).unwrap();
        assert_eq!(parsed.tenant_id, "org_abc123");
        assert_eq!(parsed.workflow_id, "d93b2a2f-d4a9-427f-ad1a-3dae942ffb9a");
        assert_eq!(parsed.version, 1);
        assert!(!parsed.force_recompile);
    }

    #[test]
    fn test_payload_roundtrip_with_force_recompile() {
        let request = CompilationRequest::new_with_force(
            "tenant-1".to_string(),
            "workflow-123".to_string(),
            42,
            true,
        );

        let payload = request.payload().unwrap();
        let parsed = CompilationRequest::from_payload(&payload).unwrap();

        assert_eq!(parsed.tenant_id, "tenant-1");
        assert_eq!(parsed.workflow_id, "workflow-123");
        assert_eq!(parsed.version, 42);
        assert!(parsed.force_recompile);
    }

    // =========================================================================
    // CompilationQueueError Display tests
    // =========================================================================

    #[test]
    fn test_compilation_queue_error_connection_display() {
        let error = CompilationQueueError::ConnectionError("timeout".to_string());
        let display = format!("{}", error);
        assert!(display.contains("Redis connection error"));
        assert!(display.contains("timeout"));
    }

    #[test]
    fn test_compilation_queue_error_redis_display() {
        let error = CompilationQueueError::RedisError("WRONGTYPE".to_string());
        let display = format!("{}", error);
        assert!(display.contains("Redis error"));
        assert!(display.contains("WRONGTYPE"));
    }

    #[test]
    fn test_compilation_queue_error_parse_display() {
        let error = CompilationQueueError::ParseError("invalid:key".to_string());
        let display = format!("{}", error);
        assert!(display.contains("Parse error"));
        assert!(display.contains("invalid:key"));
    }

    // =========================================================================
    // CompilationRequest parsing edge cases
    // =========================================================================

    #[test]
    fn test_from_unique_key_invalid_format() {
        // Missing version
        let result = CompilationRequest::from_unique_key("tenant:workflow");
        assert!(result.is_none());
    }

    #[test]
    fn test_from_unique_key_invalid_version() {
        // Version is not a number
        let result = CompilationRequest::from_unique_key("tenant:workflow:abc");
        assert!(result.is_none());
    }

    #[test]
    fn test_from_unique_key_empty_string() {
        let result = CompilationRequest::from_unique_key("");
        assert!(result.is_none());
    }

    #[test]
    fn test_from_unique_key_single_part() {
        let result = CompilationRequest::from_unique_key("onlyonepart");
        assert!(result.is_none());
    }

    #[test]
    fn test_from_unique_key_with_colons_in_workflow_id() {
        // splitn(3, ':') means third part captures everything after second colon
        // So "tenant:workflow:with:colons:1" would parse workflow_id as "with:colons:1"
        // This tests the actual behavior of splitn
        let result = CompilationRequest::from_unique_key("tenant:workflow:1:extra:parts");
        // With splitn(3), this becomes ["tenant", "workflow", "1:extra:parts"]
        // "1:extra:parts" won't parse as i32, so should be None
        assert!(result.is_none());
    }

    #[test]
    fn test_compilation_request_dedup_key() {
        // Dedup keys off `unique_key()` (tenant:workflow:version), not struct identity.
        let req1 = CompilationRequest::new("t1".to_string(), "s1".to_string(), 1);
        let req2 = CompilationRequest::new("t1".to_string(), "s1".to_string(), 1);
        let req3 = CompilationRequest::new("t1".to_string(), "s1".to_string(), 2);

        assert_eq!(req1.unique_key(), req2.unique_key());
        assert_ne!(req1.unique_key(), req3.unique_key());
    }

    #[test]
    fn test_compilation_request_clone() {
        let original = CompilationRequest::new("tenant".to_string(), "workflow".to_string(), 5);
        let cloned = original.clone();

        assert_eq!(original.tenant_id, cloned.tenant_id);
        assert_eq!(original.workflow_id, cloned.workflow_id);
        assert_eq!(original.version, cloned.version);
    }

    #[test]
    fn test_compilation_request_debug() {
        let request = CompilationRequest::new("t1".to_string(), "s1".to_string(), 1);
        let debug_str = format!("{:?}", request);

        assert!(debug_str.contains("CompilationRequest"));
        assert!(debug_str.contains("tenant_id"));
        assert!(debug_str.contains("workflow_id"));
        assert!(debug_str.contains("version"));
    }

    #[test]
    fn legacy_payload_without_product_event_deserializes_to_none() {
        // Old queue payloads (pre-SYN-436) lack `product_event`; `#[serde(default)]` must make
        // them deserialize cleanly so in-flight messages survive a deploy.
        let legacy = r#"{"tenant_id":"t","workflow_id":"w","version":3}"#;
        let req = CompilationRequest::from_payload(legacy).expect("legacy payload parses");
        assert_eq!(req.tenant_id, "t");
        assert_eq!(req.workflow_id, "w");
        assert_eq!(req.version, 3);
        assert!(req.product_event.is_none());
        assert!(!req.force_recompile);
    }

    #[test]
    fn request_with_product_event_round_trips_through_payload() {
        // A pre-built attributed event must survive the queue's JSON payload round-trip.
        let req = CompilationRequest::new("t".to_string(), "w".to_string(), 1)
            .with_product_event(Some(sample_product_event()));
        let payload = req.payload().expect("serialize payload");
        let back = CompilationRequest::from_payload(&payload).expect("parse payload");
        let event = back.product_event.expect("product_event survived");
        assert_eq!(
            event.event_type,
            crate::product_events::EventType::WorkflowCompiled
        );
        assert_eq!(event.resource_id.as_deref(), Some("w"));
    }

    /// A `ProductEvent` built without the global config (struct literal, not `::new`).
    fn sample_product_event() -> crate::product_events::ProductEvent {
        use crate::product_events::{EventSource, EventType, ProductEvent};
        ProductEvent {
            event_id: uuid::Uuid::new_v4(),
            occurred_at: chrono::Utc::now(),
            event_type: EventType::WorkflowCompiled,
            event_version: 1,
            tenant_id: "t".to_string(),
            user_id: None,
            actor_id: None,
            actor_type: None,
            resource_id: Some("w".to_string()),
            resource_type: Some("workflow".to_string()),
            properties: serde_json::json!({}),
            session_id: None,
            request_id: None,
            source: Some(EventSource::Ui),
        }
    }
}
