// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Control agent: coordinate child runs from a workflow.
//!
//! The composed copy of this component, inside a workflow, never runs a
//! capability body. Its `capabilities` and `suspendable` exports forward to
//! `runtara:control/executor`; the host then runs `runtara:control/execution`
//! on its own approved copy of these bytes, in a fresh store where
//! `runtara:control/api` is real and the caller's tenant, instance and
//! operation come from the host. Everywhere else `api` is linked `denied`.
//!
//! Spike S0.2 skeleton: one `wait` capability. It registers a host-owned wait
//! once, keeps the wait id as its continuation, and polls it on every
//! re-invocation until the wait settles.

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use runtara_agent_suspension::{SuspendContext, Suspendable, Wake};
use runtara_control_contract::ErrorCode;
use serde::{Deserialize, Serialize};

/// Version tag of the `wait` continuation.
const CONTINUATION_VERSION: u32 = runtara_control_contract::CONTROL_CONTINUATION_V1;

#[derive(Debug, Deserialize, CapabilityInput)]
#[serde(rename_all = "camelCase")]
#[capability_input(display_name = "Wait Input")]
pub struct WaitInput {
    #[field(
        display_name = "Instance IDs",
        description = "Child runs of this workflow to wait for"
    )]
    pub instance_ids: Vec<String>,

    #[field(
        display_name = "Mode",
        description = "`all` waits for every run to finish, `any` for the first",
        example = "all",
        default = "all"
    )]
    #[serde(default)]
    pub mode: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[serde(rename_all = "camelCase")]
#[capability_output(display_name = "Wait Output")]
pub struct WaitOutput {
    #[field(display_name = "Mode", description = "The wait mode")]
    pub mode: String,

    #[field(
        display_name = "Resolution",
        description = "`satisfied` when the mode was met, `deadline` when time ran out, `empty` for no runs"
    )]
    pub resolution: String,

    #[field(display_name = "Finished", description = "Runs that finished")]
    pub finished: Vec<String>,

    #[field(display_name = "Remaining", description = "Runs still going")]
    pub remaining: Vec<String>,
}

/// The `wait` continuation: the id of the wait registered on first call.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct WaitContinuation {
    v: u32,
    wait_id: String,
}

/// Wait-mode spelling accepted in the input. `true` means all.
fn wait_all(mode: Option<&str>) -> Result<bool, String> {
    match mode.unwrap_or("all") {
        "all" => Ok(true),
        "any" => Ok(false),
        other => Err(control_error(
            ErrorCode::Invalid,
            &format!("mode must be `all` or `any`, not `{other}`"),
            None,
        )),
    }
}

fn error(code: &str, message: &str) -> String {
    serde_json::json!({
        "code": code,
        "message": message,
        "category": "permanent",
        "severity": "error",
    })
    .to_string()
}

/// A control failure as the `#[capability]` JSON error envelope.
fn control_error(code: ErrorCode, message: &str, retry_after_ms: Option<u64>) -> String {
    runtara_control_contract::agent_error(code, message, retry_after_ms).to_string()
}

fn encode_continuation(wait_id: &str) -> Vec<u8> {
    serde_json::to_vec(&WaitContinuation {
        v: CONTINUATION_VERSION,
        wait_id: wait_id.to_owned(),
    })
    .expect("a continuation always serializes")
}

fn decode_continuation(bytes: &[u8]) -> Result<String, String> {
    serde_json::from_slice::<WaitContinuation>(bytes)
        .ok()
        .filter(|continuation| continuation.v == CONTINUATION_VERSION)
        .map(|continuation| continuation.wait_id)
        .ok_or_else(|| {
            error(
                "AGENT_CONTINUATION_REJECTED",
                "the saved wait continuation is not a version this agent reads",
            )
        })
}

/// One read of a registered wait. Natively only the host stub exists.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
enum Poll {
    Pending,
    Settled {
        resolution: &'static str,
        finished: Vec<String>,
        remaining: Vec<String>,
    },
}

#[capability(
    module = "control",
    id = "wait",
    display_name = "Wait For Runs",
    description = "Park the workflow without holding a runner until child runs finish.",
    side_effects = false,
    idempotent = true,
    suspends = true
)]
pub async fn wait(
    input: WaitInput,
    context: &SuspendContext,
) -> Result<Suspendable<WaitOutput>, String> {
    let all = wait_all(input.mode.as_deref())?;
    let mode = if all { "all" } else { "any" }.to_string();
    if input.instance_ids.is_empty() {
        return Ok(Suspendable::Completed(WaitOutput {
            mode,
            resolution: "empty".into(),
            finished: vec![],
            remaining: vec![],
        }));
    }
    // Register once; every re-invocation only polls the wait it registered.
    let wait_id = match context.continuation() {
        Some(continuation) => decode_continuation(continuation)?,
        None => host::register_wait(input.instance_ids, all).await?,
    };
    Ok(match host::poll_wait(&wait_id).await? {
        Poll::Pending => Suspendable::Suspended {
            state: encode_continuation(&wait_id),
            wakes: vec![Wake::Instances(wait_id)],
        },
        Poll::Settled {
            resolution,
            finished,
            remaining,
        } => Suspendable::Completed(WaitOutput {
            mode,
            resolution: resolution.into(),
            finished,
            remaining,
        }),
    })
}

/// Host control calls. Real only in the host executor's store.
#[cfg(target_arch = "wasm32")]
mod host {
    use super::{ErrorCode, Poll};
    use crate::bindings::runtara::control::{api, types};

    fn control_error(error: types::ControlError) -> String {
        let code = match error.code {
            types::ErrorCode::Denied => ErrorCode::Denied,
            types::ErrorCode::Invalid => ErrorCode::Invalid,
            types::ErrorCode::NotFound => ErrorCode::NotFound,
            types::ErrorCode::NotRunnable => ErrorCode::NotRunnable,
            types::ErrorCode::NotChild => ErrorCode::NotChild,
            types::ErrorCode::RequiresInstance => ErrorCode::RequiresInstance,
            types::ErrorCode::RequiresOperation => ErrorCode::RequiresOperation,
            types::ErrorCode::Capacity => ErrorCode::Capacity,
            types::ErrorCode::ReplayConflict => ErrorCode::ReplayConflict,
            types::ErrorCode::LabelConflict => ErrorCode::LabelConflict,
            types::ErrorCode::TooLarge => ErrorCode::TooLarge,
            types::ErrorCode::Unavailable => ErrorCode::Unavailable,
            types::ErrorCode::Unsupported => ErrorCode::Unsupported,
            types::ErrorCode::NotWaiting => ErrorCode::NotWaiting,
            types::ErrorCode::Ambiguous => ErrorCode::Ambiguous,
            types::ErrorCode::AlreadyAnswered => ErrorCode::AlreadyAnswered,
            types::ErrorCode::NotPausable => ErrorCode::NotPausable,
            types::ErrorCode::NotPaused => ErrorCode::NotPaused,
            types::ErrorCode::WaitClosed => ErrorCode::WaitClosed,
        };
        super::control_error(code, &error.message, error.retry_after_ms)
    }

    pub(super) async fn register_wait(ids: Vec<String>, all: bool) -> Result<String, String> {
        let mode = if all {
            types::WaitMode::All
        } else {
            types::WaitMode::Any
        };
        api::wait(types::WaitRequest {
            instance_ids: ids,
            mode,
            deadline_ms: None,
        })
        .await
        .map_err(control_error)
    }

    pub(super) async fn poll_wait(wait_id: &str) -> Result<Poll, String> {
        Ok(
            match api::poll_wait(wait_id.to_owned())
                .await
                .map_err(control_error)?
            {
                types::WaitPoll::Pending(_) => Poll::Pending,
                types::WaitPoll::Settled(settled) => Poll::Settled {
                    resolution: match settled.resolution {
                        types::WaitResolution::Satisfied => "satisfied",
                        types::WaitResolution::Deadline => "deadline",
                        types::WaitResolution::Empty => "empty",
                    },
                    finished: settled
                        .progress
                        .finished
                        .into_iter()
                        .map(|target| target.instance_id)
                        .collect(),
                    remaining: settled.progress.remaining,
                },
            },
        )
    }
}

/// Natively there is no control host: the capability runs only as a
/// component under the host executor.
#[cfg(not(target_arch = "wasm32"))]
mod host {
    use super::Poll;

    fn unavailable() -> String {
        super::control_error(
            super::ErrorCode::Unavailable,
            "control capabilities require the component host",
            None,
        )
    }

    pub(super) async fn register_wait(_ids: Vec<String>, _all: bool) -> Result<String, String> {
        Err(unavailable())
    }

    pub(super) async fn poll_wait(_wait_id: &str) -> Result<Poll, String> {
        Err(unavailable())
    }
}

/// Canonical `AgentInfo` for the sidecar meta.json (host-only).
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{AgentInfo, capability_to_api_with_types};
    use std::collections::HashMap;

    let output_types = HashMap::from([("WaitOutput", &__OUTPUT_META_WaitOutput)]);
    AgentInfo {
        id: runtara_dsl::agent_meta::CONTROL_AGENT_ID.into(),
        name: "Control".into(),
        description: "Coordinate child runs of a workflow.".into(),
        has_side_effects: false,
        supports_connections: false,
        integration_ids: vec![],
        capabilities: vec![capability_to_api_with_types(
            &__CAPABILITY_META_WAIT,
            Some(&__INPUT_META_WaitInput),
            Some(&__OUTPUT_META_WaitOutput),
            &output_types,
        )],
    }
}

runtara_agent_macro::agent_component!(
    agent = "control",
    control_executor = true,
    capabilities = [wait],
    suspending = [wait],
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_declares_wait_as_suspending() {
        let info = agent_info();
        assert_eq!(info.id, "control");
        let wait = &info.capabilities[0];
        assert_eq!(wait.id, "wait");
        assert!(wait.suspends);
        assert!(!wait.trusted);
    }

    #[test]
    fn the_continuation_round_trips_and_rejects_other_versions() {
        let bytes = encode_continuation("wait-1");
        assert_eq!(decode_continuation(&bytes).unwrap(), "wait-1");
        let other = serde_json::to_vec(&serde_json::json!({"v": 2, "waitId": "w"})).unwrap();
        assert!(
            decode_continuation(&other)
                .unwrap_err()
                .contains("AGENT_CONTINUATION_REJECTED")
        );
        assert!(decode_continuation(b"not json").is_err());
    }

    #[test]
    fn modes_are_all_or_any() {
        assert!(wait_all(None).unwrap());
        assert!(wait_all(Some("all")).unwrap());
        assert!(!wait_all(Some("any")).unwrap());
        assert!(wait_all(Some("some")).is_err());
    }

    #[test]
    fn plain_invoke_refuses_the_suspending_capability() {
        let error = futures_lite_block_on(__invoke_wait(serde_json::json!({})));
        assert!(
            error
                .unwrap_err()
                .contains(runtara_agent_suspension::SUSPENSION_UNSUPPORTED)
        );
    }

    #[test]
    fn an_empty_wait_completes_without_the_host() {
        let result = futures_lite_block_on(__suspend_wait(
            serde_json::json!({"instanceIds": []}),
            &SuspendContext::default(),
        ))
        .unwrap();
        assert_eq!(
            result,
            Suspendable::Completed(serde_json::json!({
                "mode": "all", "resolution": "empty", "finished": [], "remaining": []
            }))
        );
    }

    /// The capability futures never pend natively; poll once.
    fn futures_lite_block_on<F: std::future::Future>(future: F) -> F::Output {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(output) => output,
            std::task::Poll::Pending => panic!("native control futures complete immediately"),
        }
    }
}
