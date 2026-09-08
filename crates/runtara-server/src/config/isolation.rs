//! Runtime approvals for older isolated artifacts. New compilation always uses
//! standard composed components; this file cannot select a compilation backend.
use super::ConfigError;
use runtara_environment::runner::ScopedAgentRunnerConfig;
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

const ENV: &str = "RUNTARA_EXPERIMENTAL_ISOLATION_POLICY";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Review {
    sha256: String,
    reset_safe: bool,
    compiler_checkpoint_contract: bool,
}
impl Review {
    fn approved(&self) -> bool {
        self.reset_safe && self.compiler_checkpoint_contract
    }
}

/// Retained experiment approvals so pinned artifacts can still resume. The old
/// `compileEnabled` field is accepted for configuration compatibility but has no
/// effect: no value can enable new isolated compilation.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IsolationPolicy {
    version: u32,
    #[serde(rename = "compileEnabled", default)]
    _retired_compile_enabled: bool,
    reviews: BTreeMap<String, Review>,
    #[serde(default)]
    retained_reviews: BTreeMap<String, Vec<Review>>,
    max_child_tasks: usize,
    max_result_bytes: usize,
    max_handles: usize,
}

impl IsolationPolicy {
    pub(super) fn from_env() -> Result<Option<Arc<Self>>, ConfigError> {
        let Some(path) = std::env::var_os(ENV).filter(|p| !p.is_empty()) else {
            return Ok(None);
        };
        let bytes = std::fs::read(Path::new(&path))
            .map_err(|_| ConfigError::Invalid(ENV, "cannot read policy file"))?;
        Self::parse(&bytes).map(|policy| Some(Arc::new(policy)))
    }

    fn parse(bytes: &[u8]) -> Result<Self, ConfigError> {
        // Avoid logging arbitrary file contents through a deserialization error.
        let policy: Self = serde_json::from_slice(bytes)
            .map_err(|_| ConfigError::Invalid(ENV, "invalid policy schema"))?;
        if policy.version != 1 {
            return Err(ConfigError::Invalid(ENV, "unsupported policy version"));
        }
        if policy.max_child_tasks == 0
            || policy.max_child_tasks > tokio::sync::Semaphore::MAX_PERMITS
            || policy.max_handles == 0
            || policy.max_handles > tokio::sync::Semaphore::MAX_PERMITS
            || policy.max_result_bytes == 0
        {
            return Err(ConfigError::Invalid(ENV, "invalid resource bounds"));
        }
        for (id, review) in policy.reviews.iter().chain(
            policy
                .retained_reviews
                .iter()
                .flat_map(|(id, reviews)| reviews.iter().map(move |review| (id, review))),
        ) {
            if id.is_empty()
                || !id
                    .bytes()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
                || review.sha256.len() != 64
                || !review
                    .sha256
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            {
                return Err(ConfigError::Invalid(ENV, "invalid review identity"));
            }
        }
        Ok(policy)
    }

    /// Runtime retains exact historical reviews independently of new compilation.
    pub fn runner_config(&self) -> ScopedAgentRunnerConfig {
        ScopedAgentRunnerConfig {
            reviewed_agents: self
                .reviews
                .iter()
                .filter(|(_, review)| review.approved())
                .map(|(id, review)| (id.clone(), review.sha256.clone()))
                .collect(),
            retained_agents: self
                .retained_reviews
                .iter()
                .map(|(id, reviews)| {
                    (
                        id.clone(),
                        reviews
                            .iter()
                            .filter(|r| r.approved())
                            .map(|r| r.sha256.clone())
                            .collect::<BTreeSet<_>>(),
                    )
                })
                .collect(),
            max_child_tasks: self.max_child_tasks,
            max_result_bytes: self.max_result_bytes,
            max_handles: self.max_handles,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    fn config() -> Value {
        json!({"version":1,"compileEnabled":true,"reviews":{"utils":{"sha256":"a".repeat(64),"resetSafe":true,"compilerCheckpointContract":true}},"maxChildTasks":8,"maxResultBytes":8388608,"maxHandles":32})
    }
    fn parsed(value: &Value) -> IsolationPolicy {
        IsolationPolicy::parse(&serde_json::to_vec(value).unwrap()).unwrap()
    }
    #[test]
    fn retired_compile_switch_preserves_only_runtime_approvals() {
        let mut value = config();
        value["retainedReviews"] = json!({"utils":[{"sha256":"b".repeat(64),"resetSafe":true,"compilerCheckpointContract":true}]});
        let policy = parsed(&value);
        let runner = policy.runner_config();
        assert_eq!(runner.reviewed_agents["utils"], "a".repeat(64));
        assert!(runner.retained_agents["utils"].contains(&"b".repeat(64)));
        value["compileEnabled"] = false.into();
        let rollback = parsed(&value);
        assert_eq!(
            rollback.runner_config().retained_agents,
            runner.retained_agents
        );
        value["reviews"]["utils"]["resetSafe"] = false.into();
        assert!(parsed(&value).runner_config().reviewed_agents.is_empty());
    }
    #[test]
    fn obsolete_compile_field_is_optional_and_does_not_change_runtime_admission() {
        let mut value = config();
        let enabled = parsed(&value).runner_config();
        for setting in [Some(false), None] {
            if let Some(setting) = setting {
                value["compileEnabled"] = setting.into();
            } else {
                value.as_object_mut().unwrap().remove("compileEnabled");
            }
            let runner = parsed(&value).runner_config();
            assert_eq!(runner.reviewed_agents, enabled.reviewed_agents);
            assert_eq!(runner.retained_agents, enabled.retained_agents);
            assert_eq!(runner.max_child_tasks, enabled.max_child_tasks);
            assert_eq!(runner.max_result_bytes, enabled.max_result_bytes);
            assert_eq!(runner.max_handles, enabled.max_handles);
        }
    }
    #[test]
    fn retained_reviews_require_valid_exact_identity_and_complete_approval() {
        let mut value = config();
        for field in ["resetSafe", "compilerCheckpointContract"] {
            let mut historical = value["reviews"]["utils"].clone();
            historical[field] = false.into();
            value["retainedReviews"] = json!({"utils":[historical]});
            assert!(parsed(&value).runner_config().retained_agents["utils"].is_empty());
        }
        value["retainedReviews"]["utils"][0]["sha256"] = "invalid".into();
        assert!(IsolationPolicy::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }

    #[test]
    fn policy_rejects_unknown_fields_versions_bounds_and_invalid_review_digests() {
        for (field, value) in [
            ("version", json!(2)),
            ("unexpected", json!(true)),
            ("maxHandles", json!(0)),
            ("maxChildTasks", json!(0)),
            ("maxResultBytes", json!(0)),
        ] {
            let mut invalid = config();
            invalid[field] = value;
            assert!(IsolationPolicy::parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        for digest in ["bad".into(), "A".repeat(64), "g".repeat(64)] {
            let mut invalid = config();
            invalid["reviews"]["utils"]["sha256"] = digest.into();
            assert!(IsolationPolicy::parse(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
    }
}
