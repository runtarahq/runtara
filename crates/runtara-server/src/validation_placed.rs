// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Save-time validation, placed where workflows run
//! (docs/compiler-on-an-executor.md).
//!
//! With executors as the runner, a workflow's closure is validated by a
//! guest placed on one: the validator as a WASI command, in a `runtara-run`
//! that can open no file and make no socket, on a host with no database. It
//! is given the closure and the agent catalog, and answers its report. This
//! process does not validate a tenant's graph itself, and does not load the
//! validator.
//!
//! There is no validating anywhere else instead: a validation no executor
//! takes is refused, as a graph that could not be validated, and so is its
//! save.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use runtara_environment::runner::{CompileElsewhere, Runner};
use runtara_workflows::{ClosureValidationReport, ClosureValidationRequest};

use crate::compile_placed::Places;

/// The validator placed on an executor, beside the components.
pub const VALIDATE_GUEST: &str = "runtara-validate-placed.wasm";
/// The setting that names it, for a deployment that keeps it elsewhere.
pub const VALIDATE_GUEST_ENV: &str = "RUNTARA_VALIDATE_GUEST_WASM";

/// The validator's memory: as the sandbox in this process gives it.
const MEMORY_BYTES: u64 = 256 * 1024 * 1024;
/// How long a validation may run. The sandbox in this process allows ten
/// seconds of validating; here the guest's start is counted too.
const TIMEOUT: Duration = Duration::from_secs(20);
/// How many validations are placed at once: each takes a run's slot on an
/// executor, so one tenant's many saves cannot take them all.
const CONCURRENT: usize = 4;

/// Where the validator to place is: what [`VALIDATE_GUEST_ENV`] names, or
/// [`VALIDATE_GUEST`] in the components directory.
pub fn configured() -> Option<PathBuf> {
    let named = std::env::var(VALIDATE_GUEST_ENV)
        .ok()
        .map(|path| path.trim().to_owned())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    named.or_else(|| {
        std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
            .map(|dir| Path::new(&dir).join(VALIDATE_GUEST))
    })
}

struct Placement {
    places: Arc<dyn Places>,
    /// The validator in the artifact store, which a validation's runner
    /// fetches it from.
    validator: runtara_artifact_store::Digest,
}

/// The validator as a guest placed with the runner.
pub(crate) struct PlacedValidator {
    /// Set when the runner is up: where validation is done is chosen before.
    placement: OnceLock<Placement>,
    turns: tokio::sync::Semaphore,
}

impl Default for PlacedValidator {
    fn default() -> Self {
        Self {
            placement: OnceLock::new(),
            turns: tokio::sync::Semaphore::new(CONCURRENT),
        }
    }
}

static PLACED: OnceLock<PlacedValidator> = OnceLock::new();

/// Validate workflows by placing the validator with the runner, from now
/// on. The runner is given later ([`runner_is_up`]).
pub fn install() {
    PLACED.get_or_init(PlacedValidator::default);
}

/// Whether validations are placed with the runner.
pub fn installed() -> bool {
    PLACED.get().is_some()
}

/// The runner is up: validations are placed with it. `validator` is the WASI
/// command to place, which is put in the artifact store for its runner to
/// fetch. A no-op unless [`install`] was called.
pub async fn runner_is_up(
    runner: Arc<dyn Runner>,
    artifacts: &dyn runtara_artifact_store::ArtifactStore,
    validator: &[u8],
) -> Result<(), String> {
    let Some(placed) = PLACED.get() else {
        return Ok(());
    };
    let validator = artifacts
        .put(validator)
        .await
        .map_err(|error| format!("keeping the validator in the artifact store: {error}"))?;
    let _ = placed.placement.set(Placement {
        places: Arc::new(runner),
        validator,
    });
    Ok(())
}

/// Validates `request`, `tenant`'s, on an executor. `Err` says why no report
/// came back: the closure was not validated, and is not validated here
/// instead.
pub async fn validate(
    tenant: &str,
    request: &ClosureValidationRequest,
) -> Result<ClosureValidationReport, String> {
    let placed = PLACED
        .get()
        .ok_or("validations are not placed with the runner")?;
    placed.validate(tenant, request).await
}

impl PlacedValidator {
    async fn validate(
        &self,
        tenant: &str,
        request: &ClosureValidationRequest,
    ) -> Result<ClosureValidationReport, String> {
        let placement = self
            .placement
            .get()
            .ok_or("the runner that places the validator is not up yet")?;
        let request = serde_json::to_vec(request).map_err(|error| error.to_string())?;
        let _turn = self
            .turns
            .acquire()
            .await
            .map_err(|_| "validations are no longer placed".to_owned())?;
        let answered = placement
            .places
            .place(CompileElsewhere {
                tenant_id: tenant.to_owned(),
                compiler: placement.validator.clone(),
                request,
                components: Default::default(),
                memory_bytes: MEMORY_BYTES,
                timeout: TIMEOUT,
            })
            .await
            .ok_or("this runner places no guests")??;
        serde_json::from_slice(&answered.outcome)
            .map_err(|error| format!("the validator's report is not one: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use runtara_environment::runner::CompiledElsewhere;
    use std::sync::Mutex;

    struct StandIn {
        answer: Mutex<Option<Option<Result<CompiledElsewhere, String>>>>,
        asked: Mutex<Vec<(String, usize, u64)>>,
    }

    #[async_trait]
    impl Places for StandIn {
        async fn place(
            &self,
            compile: CompileElsewhere,
        ) -> Option<Result<CompiledElsewhere, String>> {
            self.asked.lock().unwrap().push((
                compile.tenant_id,
                compile.components.len(),
                compile.memory_bytes,
            ));
            self.answer.lock().unwrap().take().expect("one answer")
        }
    }

    fn placed(
        answer: Option<Result<CompiledElsewhere, String>>,
    ) -> (PlacedValidator, Arc<StandIn>) {
        let runner = Arc::new(StandIn {
            answer: Mutex::new(Some(answer)),
            asked: Mutex::default(),
        });
        let validator = PlacedValidator::default();
        let _ = validator.placement.set(Placement {
            places: runner.clone(),
            validator: runtara_artifact_store::Digest::of(b"the validator"),
        });
        (validator, runner)
    }

    fn request() -> ClosureValidationRequest {
        ClosureValidationRequest {
            root_workflow_id: "wf".into(),
            root: serde_json::from_value(serde_json::json!({
                "entryPoint": "finish",
                "steps": {"finish": {"id": "finish", "stepType": "Finish"}},
                "executionPlan": []
            }))
            .unwrap(),
            catalog: Default::default(),
            children: vec![],
        }
    }

    fn answered(outcome: &[u8]) -> Option<Result<CompiledElsewhere, String>> {
        Some(Ok(CompiledElsewhere {
            outcome: outcome.to_vec(),
            made: None,
        }))
    }

    #[tokio::test]
    async fn a_validation_is_placed_for_its_tenant_and_given_no_files() {
        let report = serde_json::to_vec(&ClosureValidationReport::default()).unwrap();
        let (validator, runner) = placed(answered(&report));
        let got = validator.validate("acme", &request()).await.unwrap();
        assert!(got.is_ok());
        assert_eq!(
            *runner.asked.lock().unwrap(),
            [("acme".to_owned(), 0, MEMORY_BYTES)]
        );
    }

    #[tokio::test]
    async fn a_validation_no_executor_takes_is_not_done_here() {
        let (validator, _) = placed(Some(Err("executor did not take it: no room".into())));
        let error = validator.validate("acme", &request()).await.unwrap_err();
        assert!(error.contains("did not take it"), "{error}");
        let (validator, _) = placed(None);
        let error = validator.validate("acme", &request()).await.unwrap_err();
        assert!(error.contains("places no guests"), "{error}");
    }

    #[tokio::test]
    async fn what_a_validator_posted_is_refused_when_it_is_not_a_report() {
        let (validator, _) = placed(answered(b"not json"));
        let error = validator.validate("acme", &request()).await.unwrap_err();
        assert!(error.contains("is not one"), "{error}");
    }

    #[tokio::test]
    async fn a_validation_before_the_runner_is_up_fails() {
        let error = PlacedValidator::default()
            .validate("acme", &request())
            .await
            .unwrap_err();
        assert!(error.contains("not up yet"), "{error}");
    }
}
