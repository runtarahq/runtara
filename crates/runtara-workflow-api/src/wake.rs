use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A pending WaitForSignal: the deterministic signal id and its persisted
/// absolute timeout deadline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalWait {
    pub checkpoint_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline_ms: Option<u64>,
}

/// When the host should re-invoke a suspended run. A suspension carries a
/// non-empty list with re-invoke-on-any semantics, as
/// `runtara:agent/types.wake` did.
///
/// Wire form: `{"at": ms}`, `{"onSignal": {...}}`, `{"onResume": true}`,
/// `{"instances": "wait-id"}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wake {
    /// Re-invoke at (or after) this wall-clock ms since the Unix epoch.
    At(u64),
    /// Re-invoke when the signal arrives, or at its deadline.
    OnSignal(SignalWait),
    /// Pause or drain: re-invoke on relaunch.
    OnResume,
    /// Re-invoke when the host-owned instance wait with this id settles.
    Instances(String),
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
enum WireWake {
    At(u64),
    OnSignal(SignalWait),
    OnResume(bool),
    Instances(String),
}

impl Serialize for Wake {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let wire = match self {
            Self::At(ms) => WireWake::At(*ms),
            Self::OnSignal(wait) => WireWake::OnSignal(wait.clone()),
            Self::OnResume => WireWake::OnResume(true),
            Self::Instances(id) => WireWake::Instances(id.clone()),
        };
        wire.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Wake {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match WireWake::deserialize(deserializer)? {
            WireWake::At(ms) => Self::At(ms),
            WireWake::OnSignal(wait) => Self::OnSignal(wait),
            WireWake::OnResume(true) => Self::OnResume,
            WireWake::OnResume(false) => {
                return Err(serde::de::Error::custom("onResume must be true"));
            }
            WireWake::Instances(id) => Self::Instances(id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn wire_form_matches_the_design() {
        let wakes = vec![
            Wake::At(1_730_000_000_000),
            Wake::OnSignal(SignalWait {
                checkpoint_id: "wait-1".into(),
                deadline_ms: Some(1_730_000_600_000),
            }),
            Wake::OnResume,
            Wake::Instances("wait-id".into()),
        ];
        let value = serde_json::to_value(&wakes).unwrap();
        assert_eq!(
            value,
            json!([
                {"at": 1_730_000_000_000u64},
                {"onSignal": {"checkpointId": "wait-1", "deadlineMs": 1_730_000_600_000u64}},
                {"onResume": true},
                {"instances": "wait-id"}
            ])
        );
        let back: Vec<Wake> = serde_json::from_value(value).unwrap();
        assert_eq!(back, wakes);
    }

    #[test]
    fn rejects_false_resume_and_unknown_kinds() {
        assert!(serde_json::from_value::<Wake>(json!({"onResume": false})).is_err());
        assert!(serde_json::from_value::<Wake>(json!({"never": 1})).is_err());
        assert!(serde_json::from_value::<Wake>(json!("onResume")).is_err());
    }
}
