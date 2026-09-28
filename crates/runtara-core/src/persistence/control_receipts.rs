//! Receipts of control mutations (`send-signal`, `cancel`, `pause`,
//! `resume`), keyed by the calling instance and its operation.
//!
//! A control call site is identified by the compiler-emitted operation scope:
//! `operation_id` holds its `op_hash` (sha256 of the site's checkpoint key), so
//! every replay and retry of the same site is the same operation. Receipts are
//! intent-first and success-only:
//!
//! 1. [`ControlReceipts::begin`] records the intent (`pending`) before the
//!    command is applied, or returns the receipt the operation already has.
//! 2. On success [`ControlReceipts::complete`] stores the result; a replay
//!    returns it without applying the command again.
//! 3. On failure [`ControlReceipts::discard`] removes the pending intent, so a
//!    retry of the operation applies the command afresh.
//!
//! A `pending` receipt found by `begin` means an earlier attempt crashed
//! between the intent and its outcome; the caller re-applies the command
//! (every control command is idempotent at its target) and completes it.
//! Receipts go with their caller: deleting the calling instance deletes them.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::error::CoreError;

/// Longest `operation_id` a store accepts.
pub const MAX_OPERATION_ID_BYTES: usize = 128;

/// Where a receipt is in its intent-first lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlReceiptState {
    /// The intent is recorded; the outcome is not.
    Pending,
    /// The command succeeded; `result` is final.
    Completed,
}

impl ControlReceiptState {
    /// Storage spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
        }
    }

    /// Parse the storage spelling.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

/// What an operation intends to do, recorded before it does it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlIntent {
    /// The command (`send_signal`, `cancel`, `pause`, `resume`).
    pub command: String,
    /// The instance the command targets.
    pub target_instance_id: String,
    /// Fingerprint of the command's arguments. A replay with a different
    /// fingerprint is a replay conflict.
    pub fingerprint: String,
    /// Command-specific facts resolved before applying it (for `send-signal`,
    /// the request it answers), used to re-apply a pending intent exactly.
    /// Never a payload.
    pub detail: Value,
}

/// One stored receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlReceipt {
    /// The calling instance.
    pub caller_instance_id: String,
    /// The caller's operation (`op_hash`).
    pub operation_id: String,
    /// The recorded intent.
    pub intent: ControlIntent,
    /// Lifecycle state.
    pub state: ControlReceiptState,
    /// The command's result, once completed. Never a payload.
    pub result: Option<Value>,
    /// When the intent was recorded.
    pub created_at: DateTime<Utc>,
    /// When the outcome was recorded.
    pub completed_at: Option<DateTime<Utc>>,
}

/// Result of [`ControlReceipts::begin`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeginReceipt {
    /// This call recorded the intent.
    Started(ControlReceipt),
    /// The operation already had a receipt (pending or completed), returned
    /// as stored; its intent may differ from the one passed.
    Existing(ControlReceipt),
}

/// Validate the key of a receipt.
pub fn validate_receipt_key(caller: &str, operation: &str) -> Result<(), CoreError> {
    let bad = |what: &str| CoreError::ValidationError {
        field: what.into(),
        message: format!("invalid control receipt {what}"),
    };
    if caller.is_empty() || caller.contains('\0') {
        return Err(bad("caller"));
    }
    if operation.is_empty()
        || operation.len() > MAX_OPERATION_ID_BYTES
        || operation.chars().any(char::is_control)
    {
        return Err(bad("operation"));
    }
    Ok(())
}

/// Durable, atomic receipts of control mutations.
#[async_trait]
pub trait ControlReceipts: Send + Sync {
    /// The receipt of `(caller, operation)`, pending or completed.
    async fn receipt_by_operation(
        &self,
        caller: &str,
        operation: &str,
    ) -> Result<Option<ControlReceipt>, CoreError>;

    /// Record `intent` as pending, or return the operation's existing receipt
    /// unchanged. Fails with `InstanceNotFound` for a missing caller.
    async fn begin(
        &self,
        caller: &str,
        operation: &str,
        intent: &ControlIntent,
    ) -> Result<BeginReceipt, CoreError>;

    /// Complete the operation's receipt with `result`. Completing an already
    /// completed receipt returns it unchanged (the first result wins); a
    /// missing receipt is an error.
    async fn complete(
        &self,
        caller: &str,
        operation: &str,
        result: &Value,
    ) -> Result<ControlReceipt, CoreError>;

    /// Remove the operation's pending intent after a failed command. A
    /// completed receipt is kept (`false`), since success is final.
    async fn discard(&self, caller: &str, operation: &str) -> Result<bool, CoreError>;
}
