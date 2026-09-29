//! A run's queryable state: the typed values its `SetState` steps wrote.
//!
//! State is one JSON object per instance. A write merges a patch into it
//! (shallow: a given field replaces its value, a `null` field is cleared) and
//! is keyed by an `operation_id`, the hash of the writing step's durable key.
//! Each key applies at most once, so a replayed step changes nothing, even
//! when its value came from a non-durable step. Readers get the stored object
//! and never wake the run.
//!
//! State is capped at [`MAX_STATE_BYTES`] and goes with its instance. Writes
//! are fenced like checkpoints: only a `running` instance of the given tenant
//! may write.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use crate::error::CoreError;

/// Largest state document a store keeps (bytes of compact JSON).
pub const MAX_STATE_BYTES: usize = 64 * 1024;

/// Length of an `operation_id`: a lowercase hex SHA-256.
pub const OPERATION_ID_LEN: usize = 64;

/// A validated state patch, split into the fields it sets and the fields it
/// clears.
#[derive(Debug, Clone, PartialEq)]
pub struct StatePatch {
    /// Fields whose value is replaced.
    pub set: Map<String, Value>,
    /// Fields that are removed (given as `null`).
    pub clear: Vec<String>,
}

impl StatePatch {
    /// Split a patch object into set and cleared fields.
    pub fn from_object(patch: &Map<String, Value>) -> Self {
        let mut set = Map::new();
        let mut clear = Vec::new();
        for (field, value) in patch {
            if value.is_null() {
                clear.push(field.clone());
            } else {
                set.insert(field.clone(), value.clone());
            }
        }
        Self { set, clear }
    }

    /// Apply the patch to `state` in place.
    pub fn apply_to(&self, state: &mut Map<String, Value>) {
        for (field, value) in &self.set {
            state.insert(field.clone(), value.clone());
        }
        for field in &self.clear {
            state.remove(field);
        }
    }
}

/// Whether a write changed the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateWrite {
    /// The first write under this operation id; merged.
    Applied,
    /// The operation id was already written; nothing changed.
    Replayed,
}

/// A run's stored state.
#[derive(Debug, Clone, PartialEq)]
pub struct RunStateRecord {
    /// The state object.
    pub state: Map<String, Value>,
    /// When the state last changed.
    pub updated_at: DateTime<Utc>,
}

/// Validate a write's identity and patch. Shared by every backend so they
/// refuse the same inputs the same way.
pub fn validate_state_write(
    tenant_id: &str,
    instance_id: &str,
    operation_id: &str,
) -> Result<(), CoreError> {
    let bad = |field: &str, message: &str| CoreError::ValidationError {
        field: field.into(),
        message: message.into(),
    };
    if tenant_id.is_empty() || tenant_id.contains('\0') {
        return Err(bad("tenant_id", "invalid state tenant id"));
    }
    if instance_id.is_empty() || instance_id.contains('\0') {
        return Err(bad("instance_id", "invalid state instance id"));
    }
    if operation_id.len() != OPERATION_ID_LEN
        || !operation_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(bad(
            "operation_id",
            "state operation id must be 64 lowercase hex characters",
        ));
    }
    Ok(())
}

/// The error for a merged state over [`MAX_STATE_BYTES`].
pub fn state_too_large() -> CoreError {
    CoreError::ValidationError {
        field: "state".into(),
        message: format!("state exceeds the {MAX_STATE_BYTES}-byte limit"),
    }
}

/// Size of `state` as compact JSON.
pub fn state_size(state: &Map<String, Value>) -> usize {
    serde_json::to_vec(state)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

/// Durable per-run state.
#[async_trait]
pub trait RunState: Send + Sync {
    /// Merge `patch` into the state of `instance_id` once per `operation_id`.
    ///
    /// The write-log insert and the merge are one transaction: a second write
    /// under the same `operation_id` returns [`StateWrite::Replayed`] and
    /// changes nothing. A missing instance, or one of another tenant, is
    /// [`CoreError::InstanceNotFound`]; one that is not `running` is
    /// [`CoreError::InvalidInstanceState`]; a merged state over
    /// [`MAX_STATE_BYTES`] is a [`CoreError::ValidationError`] on `state` and
    /// writes nothing.
    async fn apply_state(
        &self,
        tenant_id: &str,
        instance_id: &str,
        operation_id: &str,
        patch: &StatePatch,
    ) -> Result<StateWrite, CoreError>;

    /// The stored state of `instance_id`, or `None` when it has none. A
    /// missing instance, or one of another tenant, is
    /// [`CoreError::InstanceNotFound`].
    async fn get_state(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<Option<RunStateRecord>, CoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_patch_splits_nulls_into_clears() {
        let patch = json!({"stage": "approval", "note": null, "tags": ["a"]});
        let patch = StatePatch::from_object(patch.as_object().unwrap());
        assert_eq!(patch.clear, vec!["note".to_string()]);
        assert_eq!(patch.set.len(), 2);

        let mut state = json!({"note": "x", "stage": "received", "keep": 1})
            .as_object()
            .unwrap()
            .clone();
        patch.apply_to(&mut state);
        assert_eq!(
            Value::Object(state),
            json!({"stage": "approval", "tags": ["a"], "keep": 1})
        );
    }

    #[test]
    fn nested_nulls_are_values_not_clears() {
        let patch = json!({"meta": {"a": null}});
        let patch = StatePatch::from_object(patch.as_object().unwrap());
        assert!(patch.clear.is_empty());
        assert_eq!(patch.set["meta"], json!({"a": null}));
    }

    #[test]
    fn operation_ids_are_lowercase_sha256_hex() {
        let good = "a".repeat(64);
        assert!(validate_state_write("t", "i", &good).is_ok());
        assert!(validate_state_write("t", "i", &"A".repeat(64)).is_err());
        assert!(validate_state_write("t", "i", &"a".repeat(63)).is_err());
        assert!(validate_state_write("", "i", &good).is_err());
        assert!(validate_state_write("t", "", &good).is_err());
    }
}
