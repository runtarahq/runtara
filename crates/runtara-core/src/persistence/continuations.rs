//! Per-operation continuations of suspending agent capabilities.
//!
//! A suspending capability returns `suspended {wakes, state}`; the host keeps
//! `state` (the continuation) per `(instance, op_hash)`, tagged with the
//! attempt that stored it, and hands it back when the same attempt re-invokes
//! the operation. A retry (a new attempt) starts afresh: it never sees an
//! earlier attempt's continuation.
//!
//! A continuation is not a checkpoint: it lives beside them, is replaced in
//! place, is capped at [`MAX_CONTINUATION_BYTES`], and goes with its instance.
//! Writes are fenced like checkpoints: only a `running` instance may store one.

use async_trait::async_trait;

use crate::error::CoreError;

/// Largest continuation a store keeps (bytes). Equal to
/// `runtara_agent_suspension::MAX_CONTINUATION_BYTES`.
pub const MAX_CONTINUATION_BYTES: usize = 64 * 1024;

/// Longest `op_hash` a store accepts.
pub const MAX_OPERATION_HASH_BYTES: usize = 128;

/// Validate a continuation's key, attempt and (when given) state.
///
/// Shared by every backend so they refuse the same inputs the same way.
pub fn validate_continuation(
    instance_id: &str,
    op_hash: &str,
    attempt: u32,
    state: Option<&[u8]>,
) -> Result<(), CoreError> {
    let bad = |field: &str, message: String| CoreError::ValidationError {
        field: field.into(),
        message,
    };
    if instance_id.is_empty() || instance_id.contains('\0') {
        return Err(bad(
            "instance_id",
            "invalid continuation instance id".into(),
        ));
    }
    if op_hash.is_empty()
        || op_hash.len() > MAX_OPERATION_HASH_BYTES
        || op_hash.chars().any(char::is_control)
    {
        return Err(bad("op_hash", "invalid continuation op_hash".into()));
    }
    if attempt == 0 || attempt > i32::MAX as u32 {
        return Err(bad(
            "attempt",
            format!("continuation attempt must be between 1 and {}", i32::MAX),
        ));
    }
    if let Some(state) = state
        && state.len() > MAX_CONTINUATION_BYTES
    {
        return Err(bad(
            "state",
            format!(
                "continuation of {} bytes exceeds the {MAX_CONTINUATION_BYTES}-byte limit",
                state.len()
            ),
        ));
    }
    Ok(())
}

/// The fence error for an instance that exists but is not `running`: the
/// same error core's checkpoint path returns.
pub fn not_running(instance_id: &str, status: crate::domain::InstanceStatus) -> CoreError {
    CoreError::InvalidInstanceState {
        instance_id: instance_id.to_string(),
        expected: "running".to_string(),
        actual: format!("{status:?}"),
    }
}

/// Durable per-operation continuations of suspending agent capabilities.
#[async_trait]
pub trait AgentContinuations: Send + Sync {
    /// The continuation `op_hash` of `instance_id` saved for exactly
    /// `attempt`, or `None` (none saved, or saved by another attempt:
    /// attempt-matched).
    async fn get(
        &self,
        instance_id: &str,
        op_hash: &str,
        attempt: u32,
    ) -> Result<Option<Vec<u8>>, CoreError>;

    /// Save `state` as the operation's continuation for `attempt`, replacing
    /// any earlier one of the operation (whatever its attempt).
    ///
    /// Rejects state over [`MAX_CONTINUATION_BYTES`], attempt 0 and an
    /// empty, oversized or control-character `op_hash`
    /// ([`CoreError::ValidationError`]). Fenced like a checkpoint, atomically
    /// with the write: a missing instance is [`CoreError::InstanceNotFound`]
    /// and one that is not `running` is [`CoreError::InvalidInstanceState`].
    async fn put(
        &self,
        instance_id: &str,
        op_hash: &str,
        attempt: u32,
        state: &[u8],
    ) -> Result<(), CoreError>;

    /// Drop the operation's continuation. Idempotent; returns whether one was
    /// deleted.
    async fn delete(&self, instance_id: &str, op_hash: &str) -> Result<bool, CoreError>;
}
