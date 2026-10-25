// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The `wasi:cli/run` entry of a compiled workflow on the workflow API ABI.
//!
//! The compiler composes this component behind the workflow's logic: `run`
//! reads the input over the API, calls the logic's entry
//! (`runtara:agent-workflow-agent/capabilities#invoke`) and posts what it
//! returned as the run's outcome. A workflow keeps its state in checkpoints,
//! so a suspension posts only its wakes.

/// The JSON a WIT `option<string>` field of `error-info` carries, as a value
/// (a string when it is not JSON).
#[cfg(any(target_arch = "wasm32", test))]
fn json_field(field: Option<String>) -> Option<serde_json::Value> {
    field.map(|text| serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text)))
}

#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({
        path: [
            "../runtara-wit/wit-p3",
            "../runtara-wit/wit/agent",
            "../runtara-wit/wit/workflow",
            "../runtara-wit/wit-p3/entry",
            "../runtara-wit/wit-p3/adapter",
        ],
        world: "runtara:workflow-adapter/run-shim",
        generate_all,
    });
}

#[cfg(target_arch = "wasm32")]
mod run {
    use runtara_workflow_api::{ErrorInfo, PostedOutcome, SignalWait, Suspension, Wake};

    use super::json_field;
    use crate::bindings::exports::wasi::cli::run::Guest;
    use crate::bindings::runtara::agent::types::{self, Outcome};
    use crate::bindings::runtara::agent_workflow_agent::capabilities::invoke;

    fn wake(wake: types::Wake) -> Wake {
        match wake {
            types::Wake::At(at) => Wake::At(at),
            types::Wake::OnSignal(wait) => Wake::OnSignal(SignalWait {
                checkpoint_id: wait.checkpoint_id,
                deadline_ms: wait.deadline_ms,
            }),
            types::Wake::OnResume => Wake::OnResume,
            types::Wake::Instances(id) => Wake::Instances(id),
        }
    }

    struct Run;

    impl Guest for Run {
        async fn run() -> Result<(), ()> {
            let input = runtara_workflow_sdk::input().await.map_err(|_| ())?;
            // The entry's one capability (`runtara_wit::workflow::ENTRY_CAPABILITY`).
            let outcome = match invoke("run".into(), input).await {
                Ok(Outcome::Completed(output)) => PostedOutcome::Completed(output),
                Ok(Outcome::Suspended(suspension)) => PostedOutcome::Suspended(Suspension {
                    wakes: suspension.wakes.into_iter().map(wake).collect(),
                }),
                Err(error) => PostedOutcome::Failed(ErrorInfo {
                    code: error.code,
                    message: error.message,
                    category: error.category,
                    severity: error.severity,
                    retryable: error.retryable,
                    retry_after_ms: error.retry_after_ms,
                    attributes: json_field(error.attributes),
                    details: json_field(error.details),
                }),
            };
            runtara_workflow_sdk::post_outcome(outcome)
                .await
                .map_err(|_| ())
        }
    }

    crate::bindings::export!(Run with_types_in crate::bindings);
}

#[cfg(test)]
mod tests {
    #[test]
    fn error_fields_are_json_when_they_hold_json() {
        assert_eq!(
            super::json_field(Some(r#"{"x":1}"#.into())),
            Some(serde_json::json!({"x": 1}))
        );
        assert_eq!(
            super::json_field(Some("text".into())),
            Some(serde_json::json!("text"))
        );
        assert_eq!(super::json_field(None), None);
    }
}
