//! Compilation Queue
//!
//! A Valkey-based queue for scenario compilation requests.
//! Uses a Redis SET for deduplication (only one entry per scenario:version)
//! and a LIST for ordered processing.
//!
//! Key features:
//! - Unique entries: same scenario/version won't be queued twice
//! - Ordered processing: FIFO queue semantics
//! - Atomic operations: uses Lua scripts for thread safety
//! - Polling support: for waiting until compilation completes

use redis::{AsyncCommands, Script};
use std::time::Duration;
use tracing::{debug, info, warn};

/// A compilation request in the queue
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CompilationRequest {
    pub tenant_id: String,
    pub scenario_id: String,
    pub version: i32,
}

impl CompilationRequest {
    pub fn new(tenant_id: String, scenario_id: String, version: i32) -> Self {
        Self {
            tenant_id,
            scenario_id,
            version,
        }
    }

    /// Create a unique key for this request (used for deduplication)
    pub fn unique_key(&self) -> String {
        format!("{}:{}:{}", self.tenant_id, self.scenario_id, self.version)
    }

    /// Parse from unique key
    pub fn from_unique_key(key: &str) -> Option<Self> {
        let parts: Vec<&str> = key.splitn(3, ':').collect();
        if parts.len() == 3 {
            Some(Self {
                tenant_id: parts[0].to_string(),
                scenario_id: parts[1].to_string(),
                version: parts[2].parse().ok()?,
            })
        } else {
            None
        }
    }
}

/// Compilation queue backed by Redis/Valkey
///
/// Uses two Redis keys:
/// - `runtara:compilation:queue` - LIST for ordered processing
/// - `runtara:compilation:pending` - SET for deduplication and tracking
pub struct CompilationQueue {
    /// Redis client (reused across operations to avoid parsing URL repeatedly)
    client: redis::Client,
}

impl CompilationQueue {
    /// Queue key for the LIST (FIFO order)
    const QUEUE_KEY: &'static str = "runtara:compilation:queue";
    /// Set key for tracking pending compilations (deduplication)
    const PENDING_KEY: &'static str = "runtara:compilation:pending";

    /// Create a new compilation queue
    ///
    /// Returns an error if the Redis URL is invalid.
    pub fn new(redis_url: String) -> Result<Self, CompilationQueueError> {
        let client = redis::Client::open(redis_url.as_str())
            .map_err(|e| CompilationQueueError::ConnectionError(e.to_string()))?;
        Ok(Self { client })
    }

    /// Get a multiplexed async connection from the client
    async fn get_connection(
        &self,
    ) -> Result<redis::aio::MultiplexedConnection, CompilationQueueError> {
        self.client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| CompilationQueueError::ConnectionError(e.to_string()))
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
        let mut conn = self.get_connection().await?;

        let key = request.unique_key();

        // Lua script for atomic enqueue with deduplication:
        // 1. Check if key exists in pending set
        // 2. If not, add to pending set and push to queue
        // 3. Return 1 if added, 0 if already pending
        let script = Script::new(
            r#"
            local pending_key = KEYS[1]
            local queue_key = KEYS[2]
            local request_key = ARGV[1]

            -- Check if already pending
            if redis.call('SISMEMBER', pending_key, request_key) == 1 then
                return 0
            end

            -- Add to pending set and queue
            redis.call('SADD', pending_key, request_key)
            redis.call('RPUSH', queue_key, request_key)
            return 1
            "#,
        );

        let added: i32 = script
            .key(Self::PENDING_KEY)
            .key(Self::QUEUE_KEY)
            .arg(&key)
            .invoke_async(&mut conn)
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;

        if added == 1 {
            info!(
                tenant_id = %request.tenant_id,
                scenario_id = %request.scenario_id,
                version = request.version,
                "Enqueued compilation request"
            );
            Ok(true)
        } else {
            debug!(
                tenant_id = %request.tenant_id,
                scenario_id = %request.scenario_id,
                version = request.version,
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
        let mut conn = self.get_connection().await?;

        // BLPOP with timeout (returns list name and value)
        let result: Option<(String, String)> = redis::cmd("BLPOP")
            .arg(Self::QUEUE_KEY)
            .arg(timeout.as_secs())
            .query_async(&mut conn)
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;

        match result {
            Some((_queue_name, key)) => {
                let request = CompilationRequest::from_unique_key(&key)
                    .ok_or_else(|| CompilationQueueError::ParseError(key.clone()))?;

                debug!(
                    tenant_id = %request.tenant_id,
                    scenario_id = %request.scenario_id,
                    version = request.version,
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
        let mut conn = self.get_connection().await?;

        let key = request.unique_key();

        // Remove from pending set
        let _: () = conn
            .srem(Self::PENDING_KEY, &key)
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;

        info!(
            tenant_id = %request.tenant_id,
            scenario_id = %request.scenario_id,
            version = request.version,
            "Marked compilation as complete"
        );

        Ok(())
    }

    /// Check if a compilation is pending (in queue or being processed)
    pub async fn is_pending(
        &self,
        request: &CompilationRequest,
    ) -> Result<bool, CompilationQueueError> {
        let mut conn = self.get_connection().await?;

        let key = request.unique_key();

        let is_member: bool = conn
            .sismember(Self::PENDING_KEY, &key)
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;

        Ok(is_member)
    }

    /// Get the number of pending compilations
    pub async fn pending_count(&self) -> Result<usize, CompilationQueueError> {
        let mut conn = self.get_connection().await?;

        let count: usize = conn
            .scard(Self::PENDING_KEY)
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;

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
            scenario_id = %request.scenario_id,
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
        let mut conn = self.get_connection().await?;

        // Get all pending items
        let pending: Vec<String> = conn
            .smembers(Self::PENDING_KEY)
            .await
            .map_err(|e| CompilationQueueError::RedisError(e.to_string()))?;

        if pending.is_empty() {
            return Ok(0);
        }

        // Get all items in the queue
        let queue_items: Vec<String> = conn
            .lrange(Self::QUEUE_KEY, 0, -1)
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
                .rpush(Self::QUEUE_KEY, item.as_str())
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
            CompilationRequest::new("tenant-1".to_string(), "scenario-123".to_string(), 42);

        let key = request.unique_key();
        assert_eq!(key, "tenant-1:scenario-123:42");

        let parsed = CompilationRequest::from_unique_key(&key).unwrap();
        assert_eq!(parsed.tenant_id, "tenant-1");
        assert_eq!(parsed.scenario_id, "scenario-123");
        assert_eq!(parsed.version, 42);
    }

    #[test]
    fn test_unique_key_with_special_chars() {
        // Scenario IDs are UUIDs, so no special chars, but test tenant_id edge cases
        let request = CompilationRequest::new(
            "org_abc123".to_string(),
            "d93b2a2f-d4a9-427f-ad1a-3dae942ffb9a".to_string(),
            1,
        );

        let key = request.unique_key();
        let parsed = CompilationRequest::from_unique_key(&key).unwrap();
        assert_eq!(parsed.tenant_id, "org_abc123");
        assert_eq!(parsed.scenario_id, "d93b2a2f-d4a9-427f-ad1a-3dae942ffb9a");
        assert_eq!(parsed.version, 1);
    }

    // =========================================================================
    // CompilationQueue::new Result handling tests
    // =========================================================================

    #[test]
    fn test_compilation_queue_new_with_valid_url() {
        // Valid Redis URL should succeed in creating the client
        // Note: This doesn't actually connect - it just validates URL parsing
        let result = CompilationQueue::new("redis://localhost:6379".to_string());
        assert!(result.is_ok());
    }

    #[test]
    fn test_compilation_queue_new_with_password() {
        // URL with authentication should parse correctly
        let result = CompilationQueue::new("redis://:password@localhost:6379".to_string());
        assert!(result.is_ok());
    }

    #[test]
    fn test_compilation_queue_new_with_user_password() {
        // URL with user and password should parse correctly
        let result = CompilationQueue::new("redis://user:password@localhost:6379".to_string());
        assert!(result.is_ok());
    }

    #[test]
    fn test_compilation_queue_new_with_database() {
        // URL with database selection
        let result = CompilationQueue::new("redis://localhost:6379/1".to_string());
        assert!(result.is_ok());
    }

    #[test]
    fn test_compilation_queue_new_with_invalid_url() {
        // Invalid URL should return ConnectionError
        let result = CompilationQueue::new("not-a-valid-url".to_string());
        assert!(result.is_err());

        if let Err(CompilationQueueError::ConnectionError(msg)) = result {
            // Should contain some error message about invalid URL
            assert!(!msg.is_empty());
        } else {
            panic!("Expected ConnectionError variant");
        }
    }

    #[test]
    fn test_compilation_queue_new_with_empty_url() {
        // Empty URL should fail
        let result = CompilationQueue::new("".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn test_compilation_queue_new_with_wrong_scheme() {
        // Wrong scheme (http instead of redis) should fail
        let result = CompilationQueue::new("http://localhost:6379".to_string());
        assert!(result.is_err());
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
        let result = CompilationRequest::from_unique_key("tenant:scenario");
        assert!(result.is_none());
    }

    #[test]
    fn test_from_unique_key_invalid_version() {
        // Version is not a number
        let result = CompilationRequest::from_unique_key("tenant:scenario:abc");
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
    fn test_from_unique_key_with_colons_in_scenario_id() {
        // splitn(3, ':') means third part captures everything after second colon
        // So "tenant:scenario:with:colons:1" would parse scenario_id as "with:colons:1"
        // This tests the actual behavior of splitn
        let result = CompilationRequest::from_unique_key("tenant:scenario:1:extra:parts");
        // With splitn(3), this becomes ["tenant", "scenario", "1:extra:parts"]
        // "1:extra:parts" won't parse as i32, so should be None
        assert!(result.is_none());
    }

    #[test]
    fn test_compilation_request_equality() {
        let req1 = CompilationRequest::new("t1".to_string(), "s1".to_string(), 1);
        let req2 = CompilationRequest::new("t1".to_string(), "s1".to_string(), 1);
        let req3 = CompilationRequest::new("t1".to_string(), "s1".to_string(), 2);

        assert_eq!(req1, req2);
        assert_ne!(req1, req3);
    }

    #[test]
    fn test_compilation_request_clone() {
        let original = CompilationRequest::new("tenant".to_string(), "scenario".to_string(), 5);
        let cloned = original.clone();

        assert_eq!(original.tenant_id, cloned.tenant_id);
        assert_eq!(original.scenario_id, cloned.scenario_id);
        assert_eq!(original.version, cloned.version);
    }

    #[test]
    fn test_compilation_request_debug() {
        let request = CompilationRequest::new("t1".to_string(), "s1".to_string(), 1);
        let debug_str = format!("{:?}", request);

        assert!(debug_str.contains("CompilationRequest"));
        assert!(debug_str.contains("tenant_id"));
        assert!(debug_str.contains("scenario_id"));
        assert!(debug_str.contains("version"));
    }
}
