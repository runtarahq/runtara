//! One immutable operator review snapshot for compilation, cache identity and execution.
use super::ConfigError;
use runtara_environment::runner::ScopedAgentRunnerConfig;
use runtara_workflows::direct_wasm::compile::DIRECT_WORKFLOW_INVOKE_ABI_VERSION;
use runtara_workflows::direct_wasm::{AgentIsolationPolicy, AgentIsolationReview};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};

const ENV: &str = "RUNTARA_EXPERIMENTAL_ISOLATION_POLICY";

#[derive(Debug, Clone, Deserialize, Serialize)]
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

/// Explicit experiment configuration. Disabling new isolated compilation keeps
/// retained runtime approvals active so pinned artifacts can still resume.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IsolationPolicy {
    version: u32,
    compile_enabled: bool,
    reviews: BTreeMap<String, Review>,
    #[serde(default)]
    retained_reviews: BTreeMap<String, Vec<Review>>,
    max_child_tasks: usize,
    max_result_bytes: usize,
    max_handles: usize,
    #[serde(skip)]
    runtime_binding: String,
}

impl IsolationPolicy {
    pub(super) fn from_env() -> Result<Option<Arc<Self>>, ConfigError> {
        let Some(path) = std::env::var_os(ENV).filter(|p| !p.is_empty()) else {
            return Ok(None);
        };
        let bytes = std::fs::read(Path::new(&path))
            .map_err(|_| ConfigError::Invalid(ENV, "cannot read policy file"))?;
        let binding = if std::env::var("RUNTARA_DIRECT_RUNTIME_BINDING")
            .ok()
            .as_deref()
            == Some("composed")
        {
            "composed"
        } else {
            "host-import"
        };
        Self::parse(&bytes, binding).map(|policy| Some(Arc::new(policy)))
    }

    fn parse(bytes: &[u8], runtime_binding: &str) -> Result<Self, ConfigError> {
        // Avoid logging arbitrary file contents through a deserialization error.
        let mut policy: Self = serde_json::from_slice(bytes)
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
        policy.runtime_binding = runtime_binding.into();
        Ok(policy)
    }

    /// Options for the normal server compile path. No policy means unchanged legacy emission.
    pub fn compiler_options(
        &self,
    ) -> Option<(
        AgentIsolationPolicy,
        runtara_workflow_wit::isolation_package::PackageLimits,
    )> {
        self.compile_enabled.then(|| {
            let limit = runtara_component_host::precompile::MAX_PRECOMPILE_COMPONENT_BYTES;
            (
                AgentIsolationPolicy {
                    enabled: true,
                    runtime_supports_inventory_v4: true,
                    reviews: self
                        .reviews
                        .iter()
                        .map(|(id, review)| {
                            (
                                id.clone(),
                                AgentIsolationReview {
                                    sha256: review.sha256.clone(),
                                    reset_safe: review.reset_safe,
                                    compiler_checkpoint_contract: review
                                        .compiler_checkpoint_contract,
                                },
                            )
                        })
                        .collect(),
                },
                runtara_workflow_wit::isolation_package::PackageLimits {
                    total_bytes: limit,
                    manifest_bytes: limit,
                    artifacts: limit / 8,
                    bindings: limit / 8,
                },
            )
        })
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

    /// Existing SQL lowering-mode provenance also covers image reuse, deploy
    /// freshness, queue claims and immutable image names. No new SQL column is needed.
    pub fn lowering_tag(&self, base: &str) -> String {
        if !self.compile_enabled {
            return base.into();
        }
        // Runtime-only retention and quotas do not alter compiler output.
        let bytes = serde_json::to_vec(&(
            1,
            DIRECT_WORKFLOW_INVOKE_ABI_VERSION,
            4,
            &self.runtime_binding,
            &self.reviews,
        ))
        .expect("review identity serializes");
        format!("{base},isolation=v1-{:x}", Sha256::digest(bytes))
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
        IsolationPolicy::parse(&serde_json::to_vec(value).unwrap(), "host-import").unwrap()
    }
    #[test]
    fn shared_policy_separates_current_compilation_from_retained_runtime_reviews() {
        let mut value = config();
        value["retainedReviews"] = json!({"utils":[{"sha256":"b".repeat(64),"resetSafe":true,"compilerCheckpointContract":true}]});
        let policy = parsed(&value);
        let compiler = policy.compiler_options().unwrap().0;
        assert_eq!(compiler.reviews["utils"].sha256, "a".repeat(64));
        let runner = policy.runner_config();
        assert_eq!(runner.reviewed_agents["utils"], "a".repeat(64));
        assert!(runner.retained_agents["utils"].contains(&"b".repeat(64)));
        value["compileEnabled"] = false.into();
        let rollback = parsed(&value);
        assert!(rollback.compiler_options().is_none());
        assert_eq!(rollback.lowering_tag("legacy"), "legacy");
        assert_eq!(
            rollback.runner_config().retained_agents,
            runner.retained_agents
        );
        value["reviews"]["utils"]["resetSafe"] = false.into();
        assert!(parsed(&value).runner_config().reviewed_agents.is_empty());
    }
    #[test]
    fn cache_identity_changes_with_review_and_runtime_contract_but_not_runtime_only_controls() {
        let value = config();
        let original = parsed(&value).lowering_tag("base");
        for field in ["sha256", "resetSafe", "compilerCheckpointContract"] {
            let mut changed = value.clone();
            changed["reviews"]["utils"][field] = if field == "sha256" {
                "b".repeat(64).into()
            } else {
                false.into()
            };
            assert_ne!(parsed(&changed).lowering_tag("base"), original);
        }
        assert_ne!(
            IsolationPolicy::parse(&serde_json::to_vec(&value).unwrap(), "composed")
                .unwrap()
                .lowering_tag("base"),
            original
        );
        let mut operational = value;
        operational["maxChildTasks"] = 4.into();
        operational["retainedReviews"] = json!({"utils":[]});
        assert_eq!(parsed(&operational).lowering_tag("base"), original);
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
        assert!(
            IsolationPolicy::parse(&serde_json::to_vec(&value).unwrap(), "host-import").is_err()
        );
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
            assert!(
                IsolationPolicy::parse(&serde_json::to_vec(&invalid).unwrap(), "host-import")
                    .is_err()
            );
        }
        for digest in ["bad".into(), "A".repeat(64), "g".repeat(64)] {
            let mut invalid = config();
            invalid["reviews"]["utils"]["sha256"] = digest.into();
            assert!(
                IsolationPolicy::parse(&serde_json::to_vec(&invalid).unwrap(), "host-import")
                    .is_err()
            );
        }
    }
}
