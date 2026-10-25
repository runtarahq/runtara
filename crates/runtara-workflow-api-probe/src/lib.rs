// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A workflow on the workflow API ABI, for tests.
//!
//! It reads its run and input over the API and posts an outcome chosen by
//! `data.mode`: `complete` (default) echoes `data` with the run's identity,
//! `fail` posts a failure, `suspend` posts a timed wake, and `spin` and
//! `trap` never post one (they loop forever or abort).

#[cfg(target_arch = "wasm32")]
mod probe {
    use runtara_workflow_api::{ErrorInfo, PostedOutcome, Suspension, Wake};
    use runtara_workflow_sdk::bindings::exports::wasi::cli::run::Guest;
    use serde_json::{Value, json};

    struct Probe;

    impl Guest for Probe {
        async fn run() -> Result<(), ()> {
            let info = runtara_workflow_sdk::run_info().await.map_err(|_| ())?;
            let input = runtara_workflow_sdk::input().await.map_err(|_| ())?;
            let input: Value = serde_json::from_slice(&input).unwrap_or(Value::Null);
            let data = input.get("data").cloned().unwrap_or(Value::Null);
            match data.get("mode").and_then(Value::as_str) {
                Some("spin") => loop {
                    std::hint::spin_loop();
                },
                Some("trap") => std::process::abort(),
                _ => {}
            }
            let outcome = match data.get("mode").and_then(Value::as_str) {
                Some("fail") => PostedOutcome::Failed(ErrorInfo {
                    code: "PROBE_FAILED".into(),
                    message: "the probe was asked to fail".into(),
                    category: "permanent".into(),
                    severity: "error".into(),
                    retryable: false,
                    retry_after_ms: None,
                    attributes: None,
                    details: Some(json!({"instanceId": info.instance_id})),
                }),
                Some("suspend") => PostedOutcome::Suspended(Suspension {
                    wakes: vec![Wake::At(
                        data.get("wakeAt").and_then(Value::as_u64).unwrap_or(0),
                    )],
                }),
                _ => PostedOutcome::Completed(
                    serde_json::to_vec(&json!({
                        "instanceId": info.instance_id,
                        "tenantId": info.tenant_id,
                        "apiVersion": info.api_version,
                        "echo": data,
                    }))
                    .map_err(|_| ())?,
                ),
            };
            runtara_workflow_sdk::post_outcome(outcome)
                .await
                .map_err(|_| ())
        }
    }

    runtara_workflow_sdk::export_workflow!(Probe);
}
