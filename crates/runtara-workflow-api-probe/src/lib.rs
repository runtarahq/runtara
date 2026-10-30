// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A workflow on the workflow API ABI, for tests.
//!
//! It reads its run and input over the API and posts an outcome chosen by
//! `data.mode`: `complete` (default) echoes `data` with the run's identity,
//! `fail` posts a failure, `suspend` posts a timed wake, `durable` exercises
//! checkpoints, time, heartbeat and events, `state` published state, `control`
//! the control calls, `trusted` a refused trusted call, and `spin` and `trap`
//! never post one (they loop forever or abort).

#[cfg(target_arch = "wasm32")]
mod bindings {
    wit_bindgen::generate!({
        path: "../runtara-wit/wit-p3",
        world: "runtara:workflow-guest/run",
        generate_all,
    });
}

#[cfg(target_arch = "wasm32")]
mod probe {
    use crate::bindings::exports::wasi::cli::run::Guest;
    use runtara_workflow_api::{ErrorInfo, PostedOutcome, Suspension, Wake};
    use serde_json::{Value, json};

    struct Probe;

    impl Guest for Probe {
        async fn run() -> Result<(), ()> {
            let info = runtara_workflow_sdk::run_info().await.map_err(|_| ())?;
            let input = runtara_workflow_sdk::input().await.map_err(|_| ())?;
            let input: Value = serde_json::from_slice(&input).unwrap_or(Value::Null);
            let data = input.get("data").cloned().unwrap_or(Value::Null);
            match data.get("mode").and_then(Value::as_str) {
                Some("deadline") => {
                    // Shorten the deadline, then spin past it.
                    runtara_workflow_sdk::set_deadline(200)
                        .await
                        .map_err(|_| ())?;
                    loop {
                        std::hint::spin_loop();
                    }
                }
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
                Some("connections") => {
                    use runtara_workflow_api::{
                        ConnectionTarget, OutboundDestination, OutboundMeta, SqlOperation,
                    };
                    use runtara_workflow_sdk::Outbound;
                    let describe = runtara_workflow_sdk::describe_connection("conn-1")
                        .await
                        .map_err(|_| ())?;
                    let resource =
                        runtara_workflow_sdk::resolve_resource("conn-1", &json!({"kind": "list"}))
                            .await
                            .map_err(|_| ())?;
                    let rows = runtara_workflow_sdk::sql(
                        "conn-1",
                        SqlOperation::Query,
                        &json!({"sql": "select 1"}),
                    )
                    .await
                    .map_err(|_| ())?;
                    let refused =
                        runtara_workflow_sdk::sql("conn-1", SqlOperation::Query, &json!({}))
                            .await
                            .err()
                            .and_then(|error| error.code().map(|code| code.as_str().to_string()));
                    let call = |destination| OutboundMeta {
                        method: "POST".into(),
                        destination,
                        headers: vec![("authorization".into(), "Bearer guest".into())],
                        timeout_ms: Some(5000),
                        max_response_bytes: None,
                        has_body: false,
                    };
                    let reply = match runtara_workflow_sdk::http(
                        &call(OutboundDestination::Connection(ConnectionTarget {
                            connection_id: "conn-1".into(),
                            url: "https://api.example.com/items".into(),
                            endpoint: None,
                            endpoint_ref: None,
                            ai_provider: None,
                            aws_service: None,
                        })),
                        Some(b"ping".to_vec()),
                    )
                    .await
                    .map_err(|_| ())?
                    {
                        Outbound::Reply {
                            status,
                            headers,
                            body,
                        } => json!({
                            "status": status,
                            "headers": headers,
                            "body": String::from_utf8_lossy(&body),
                        }),
                        Outbound::Failed { failure, .. } => json!({"unexpected": failure}),
                    };
                    let failed = match runtara_workflow_sdk::http(
                        &call(OutboundDestination::Public(
                            "https://blocked.example".into(),
                        )),
                        None,
                    )
                    .await
                    .map_err(|_| ())?
                    {
                        Outbound::Failed { failure, body } => json!({
                            "code": failure.code,
                            "status": failure.status,
                            "body": String::from_utf8_lossy(&body),
                        }),
                        Outbound::Reply { .. } => json!({"unexpected": "reply"}),
                    };
                    PostedOutcome::Completed(
                        serde_json::to_vec(&json!({
                            "describe": describe,
                            "resource": resource,
                            "rows": rows,
                            "refused": refused,
                            "reply": reply,
                            "failed": failed,
                        }))
                        .map_err(|_| ())?,
                    )
                }
                Some("operation") => {
                    let key =
                        r#"runtara:v2:["agent","probe",[],[],["probe-agent","wait","step-1"]]"#;
                    let (id, first) = runtara_workflow_sdk::enter_operation(key, 1, true)
                        .await
                        .map_err(|_| ())?;
                    runtara_workflow_sdk::suspend_operation(
                        &id,
                        b"saved".to_vec(),
                        &[Wake::At(1_900_000_000_000)],
                    )
                    .await
                    .map_err(|_| ())?;
                    let (again, loaded) = runtara_workflow_sdk::enter_operation(key, 1, true)
                        .await
                        .map_err(|_| ())?;
                    let refused = runtara_workflow_sdk::suspend_operation(
                        &again,
                        b"x".to_vec(),
                        &[Wake::OnResume],
                    )
                    .await
                    .err()
                    .and_then(|error| error.code().map(|code| code.as_str().to_string()));
                    runtara_workflow_sdk::exit_operation(&again, false)
                        .await
                        .map_err(|_| ())?;
                    runtara_workflow_sdk::release_operation(&again)
                        .await
                        .map_err(|_| ())?;
                    PostedOutcome::Completed(
                        serde_json::to_vec(&json!({
                            "sameId": id == again,
                            "first": first.map(|state| String::from_utf8_lossy(&state).into_owned()),
                            "loaded": loaded.map(|state| String::from_utf8_lossy(&state).into_owned()),
                            "refused": refused,
                        }))
                        .map_err(|_| ())?,
                    )
                }
                Some("control") => {
                    use runtara_workflow_api::ControlCall;
                    let code = |error: runtara_workflow_sdk::Error| {
                        error.code().map(|code| code.as_str().to_string())
                    };
                    let key =
                        r#"runtara:v2:["agent","probe",[],[],["probe-agent","start","step-1"]]"#;
                    let (op, _) = runtara_workflow_sdk::enter_operation(key, 1, false)
                        .await
                        .map_err(|_| ())?;
                    let started = runtara_workflow_sdk::control(
                        &ControlCall::Start(json!({
                            "workflowId": "child-wf",
                            "input": {"data": {"n": 1}, "variables": {}},
                            "parentClosePolicy": "cancel",
                        })),
                        Some(&op),
                    )
                    .await
                    .map_err(|_| ())?;
                    let child = started["instanceId"].as_str().ok_or(())?.to_string();
                    let detail =
                        runtara_workflow_sdk::control(&ControlCall::Get(child.clone()), None)
                            .await
                            .map_err(|_| ())?;
                    let page = runtara_workflow_sdk::control(
                        &ControlCall::Query(json!({"parent": "caller", "statuses": ["running"]})),
                        None,
                    )
                    .await
                    .map_err(|_| ())?;
                    let outside =
                        runtara_workflow_sdk::control(&ControlCall::Pause(child.clone()), None)
                            .await
                            .err()
                            .and_then(code);
                    let stranger = runtara_workflow_sdk::control(
                        &ControlCall::Pause(child.clone()),
                        Some(&"0".repeat(64)),
                    )
                    .await
                    .err()
                    .and_then(code);
                    let not_child = runtara_workflow_sdk::control(
                        &ControlCall::Cancel("someone-else".into(), Value::Null),
                        Some(&op),
                    )
                    .await
                    .err()
                    .and_then(code);
                    runtara_workflow_sdk::exit_operation(&op, false)
                        .await
                        .map_err(|_| ())?;
                    PostedOutcome::Completed(
                        serde_json::to_vec(&json!({
                            "started": started,
                            "detail": detail,
                            "page": page,
                            "outside": outside,
                            "stranger": stranger,
                            "notChild": not_child,
                        }))
                        .map_err(|_| ())?,
                    )
                }
                Some("trusted") => {
                    // This probe pins no trusted artifact, so every call is
                    // refused before any credential lookup.
                    let refusal = match runtara_workflow_sdk::trusted(
                        "s3-storage",
                        "storage-generate-presigned-url",
                        "s3",
                        &json!({"bucket": "b", "key": "k"}),
                        None,
                    )
                    .await
                    {
                        Ok(answer) => json!({"unexpected": answer}),
                        Err(runtara_workflow_sdk::Error::Api(error)) => json!({
                            "code": error.code.as_str(),
                            "envelope": error.details.and_then(|d| d.get("code").cloned()),
                        }),
                        Err(error) => json!({"transport": error.to_string()}),
                    };
                    PostedOutcome::Completed(
                        serde_json::to_vec(&json!({"refusal": refusal})).map_err(|_| ())?,
                    )
                }
                Some("wait") => {
                    // Wait for `data.children`; park until they finish, then
                    // complete with the settled read.
                    let key = r#"runtara:v2:["wait-instances","probe",[],[],["wait-1"]]"#;
                    let children: Vec<String> = data
                        .get("children")
                        .and_then(|children| serde_json::from_value(children.clone()).ok())
                        .unwrap_or_default();
                    let read = runtara_workflow_sdk::register_wait(key, &children, "all", None)
                        .await
                        .map_err(|_| ())?;
                    if read.get("resolution").is_none_or(Value::is_null) {
                        let wait_id = read
                            .get("waitId")
                            .and_then(Value::as_str)
                            .ok_or(())?
                            .to_string();
                        PostedOutcome::Suspended(Suspension {
                            wakes: vec![Wake::Instances(wait_id)],
                        })
                    } else {
                        runtara_workflow_sdk::release_wait(key)
                            .await
                            .map_err(|_| ())?;
                        PostedOutcome::Completed(
                            serde_json::to_vec(&json!({"result": read})).map_err(|_| ())?,
                        )
                    }
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

    crate::bindings::export!(Probe with_types_in crate::bindings);
}
