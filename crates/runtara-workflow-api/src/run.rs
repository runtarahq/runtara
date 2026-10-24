use serde::{Deserialize, Serialize};

use crate::Wake;

/// `GET /v1/run`: what a launched artifact needs to know about its run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInfo {
    pub instance_id: String,
    pub tenant_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    pub attempt: u32,
    /// How this launch started (`fresh`, `resume`, `recovery`, ...).
    pub launch_kind: String,
    pub debug: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
    pub api_version: String,
    /// Set only for an agent launched as a child run: the capability its run
    /// adapter calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_id: Option<String>,
}

/// The kind of a posted run outcome, carried in [`crate::headers::OUTCOME`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeKind {
    /// Body: the raw output bytes.
    Completed,
    /// Body: JSON [`ErrorInfo`].
    Failed,
    /// Body: JSON [`Suspension`].
    Suspended,
}

impl OutcomeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Suspended => "suspended",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "completed" => Some(Self::Completed),
            "failed" => Some(Self::Failed),
            "suspended" => Some(Self::Suspended),
            _ => None,
        }
    }
}

/// A failed run's error, the JSON form of `runtara:agent/types.error-info`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorInfo {
    pub code: String,
    pub message: String,
    /// `transient`, `permanent`, `timeout`, ...
    pub category: String,
    /// `warning`, `error` or `critical`.
    pub severity: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after_ms: Option<u64>,
    /// JSON object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attributes: Option<serde_json::Value>,
    /// The producer's full structured error, persisted verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// A suspended outcome: re-invoke when any wake fires.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suspension {
    pub wakes: Vec<Wake>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn outcome_kinds_round_trip() {
        for kind in [
            OutcomeKind::Completed,
            OutcomeKind::Failed,
            OutcomeKind::Suspended,
        ] {
            assert_eq!(OutcomeKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(OutcomeKind::parse("cancelled"), None);
    }

    #[test]
    fn run_info_omits_unset_fields() {
        let info = RunInfo {
            instance_id: "i-1".into(),
            tenant_id: "t-1".into(),
            workflow_id: None,
            version: None,
            attempt: 1,
            launch_kind: "fresh".into(),
            debug: false,
            deadline_ms: None,
            api_version: crate::api_version(),
            capability_id: None,
        };
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            json!({
                "instanceId": "i-1",
                "tenantId": "t-1",
                "attempt": 1,
                "launchKind": "fresh",
                "debug": false,
                "apiVersion": "1.0"
            })
        );
    }

    #[test]
    fn suspension_body() {
        let body: Suspension =
            serde_json::from_value(json!({"wakes": [{"at": 5}, {"onResume": true}]})).unwrap();
        assert_eq!(body.wakes, vec![Wake::At(5), Wake::OnResume]);
    }
}
