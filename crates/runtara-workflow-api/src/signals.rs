//! Wire shapes of run services shared by the host and the guest.

use serde::{Deserialize, Serialize};

/// The JSON announcement of a lifecycle signal: everything but its payload.
/// Sent in `Runtara-Signal` (with the payload as the body) and
/// `Runtara-Pending-Signal` (on checkpoint writes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleNotice {
    pub signal_type: String,
    pub command_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
}

/// A custom signal waiting at a checkpoint, announced without its payload in
/// `Runtara-Custom-Signal`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomSignalNotice {
    pub signal_id: String,
    pub checkpoint_id: String,
}

/// One line of `POST /v1/events`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub kind: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}
