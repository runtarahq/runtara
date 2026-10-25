// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A workflow on the workflow API ABI, for tests.
//!
//! It reads its run and input over the API and posts an outcome chosen by
//! `data.mode`: `complete` (default) echoes `data` with the run's identity,
//! `fail` posts a failure, `suspend` posts a timed wake, `durable` exercises
//! checkpoints, time, heartbeat and events, `state` published state, and `spin` and
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
                Some("durable") => {
                    // Checkpoints keep the first committed state, time and
                    // heartbeat answer, and events are appended.
                    let first = runtara_workflow_sdk::checkpoint("probe/step:1", b"first".to_vec())
                        .await
                        .map_err(|_| ())?;
                    let second =
                        runtara_workflow_sdk::checkpoint("probe/step:1", b"second".to_vec())
                            .await
                            .map_err(|_| ())?;
                    let read = runtara_workflow_sdk::get_checkpoint("probe/step:1")
                        .await
                        .map_err(|_| ())?;
                    let missing = runtara_workflow_sdk::get_checkpoint("probe/never")
                        .await
                        .map_err(|_| ())?;
                    let now_ms = runtara_workflow_sdk::now_ms().await.map_err(|_| ())?;
                    runtara_workflow_sdk::heartbeat().await.map_err(|_| ())?;
                    runtara_workflow_sdk::emit_events(&[runtara_workflow_api::Event {
                        kind: "probe".into(),
                        payload: json!({"step": 1}),
                    }])
                    .await
                    .map_err(|_| ())?;
                    PostedOutcome::Completed(
                        serde_json::to_vec(&json!({
                            "firstStored": first.stored,
                            "secondStored": second.stored,
                            "secondState": String::from_utf8_lossy(&second.state),
                            "read": read.map(|state| String::from_utf8_lossy(&state).into_owned()),
                            "missing": missing.is_none(),
                            "nowMs": now_ms,
                        }))
                        .map_err(|_| ())?,
                    )
                }
                Some("input") => {
                    let mut descriptor = serde_json::Map::new();
                    descriptor.insert("signal_id".into(), json!("probe-input"));
                    descriptor.insert("prompt".into(), json!("approve?"));
                    runtara_workflow_sdk::register_input(&descriptor, None)
                        .await
                        .map_err(|_| ())?;
                    // A replay of the same registration finds it again.
                    runtara_workflow_sdk::register_input(&descriptor, None)
                        .await
                        .map_err(|_| ())?;
                    let open = runtara_workflow_sdk::poll_input("probe-input", 0)
                        .await
                        .map_err(|_| ())?;
                    let closed = runtara_workflow_sdk::close_input("probe-input")
                        .await
                        .map_err(|_| ())?;
                    PostedOutcome::Completed(
                        serde_json::to_vec(&json!({"open": open, "closed": closed}))
                            .map_err(|_| ())?,
                    )
                }
                Some("state") => {
                    let set_key = r#"runtara:v2:["state","probe",[],[],["set-1"]]"#;
                    let read_key = r#"runtara:v2:["state-read","probe",[],[],["get-1"]]"#;
                    let mut patch = serde_json::Map::new();
                    patch.insert("stage".into(), json!("probed"));
                    runtara_workflow_sdk::set_state(set_key, &patch)
                        .await
                        .map_err(|_| ())?;
                    let state = runtara_workflow_sdk::get_state(read_key)
                        .await
                        .map_err(|_| ())?;
                    let refused = runtara_workflow_sdk::set_state("not-a-key", &patch)
                        .await
                        .err()
                        .and_then(|error| error.code().map(|code| code.as_str().to_string()));
                    PostedOutcome::Completed(
                        serde_json::to_vec(&json!({"state": state, "refused": refused}))
                            .map_err(|_| ())?,
                    )
                }
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
