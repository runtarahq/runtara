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
//! Reads: `get`, `query` and `list-pending-signals` cover the caller's
//! tenant; identity and caller-relative filters need a calling instance.
//! `wait` registers a host-owned wait once, keeps the wait id as its
//! continuation, and polls it on every re-invocation until the wait settles.

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

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// Instance states as the agent spells them.
const STATUSES: [&str; 8] = [
    "queued",
    "pending",
    "running",
    "suspended",
    "completed",
    "failed",
    "cancelled",
    "not_started",
];

fn invalid(message: String) -> String {
    control_error(ErrorCode::Invalid, &message, None)
}

/// Page size of `query` and `list-pending-signals`: 20 unless given.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn page_size(requested: Option<u32>) -> u32 {
    requested.unwrap_or(20)
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[serde(rename_all = "camelCase")]
#[capability_input(display_name = "Get Run Input")]
pub struct GetInput {
    #[field(
        display_name = "Instance ID",
        description = "The run to read, in this tenant"
    )]
    pub instance_id: String,
}

/// One run as control reports it.
#[derive(Debug, Serialize, Deserialize, CapabilityOutput, PartialEq)]
#[serde(rename_all = "camelCase")]
#[capability_output(display_name = "Run")]
pub struct RunSummary {
    #[field(display_name = "Instance ID", description = "The run's id")]
    pub instance_id: String,
    #[field(
        display_name = "Workflow ID",
        description = "The workflow the run executes"
    )]
    pub workflow_id: String,
    #[field(display_name = "Version", description = "The workflow version")]
    pub version: Option<u32>,
    #[field(display_name = "Run Label", description = "The run's label, if any")]
    pub run_label: Option<String>,
    #[field(
        display_name = "Parent Instance ID",
        description = "The run that started this one, if any"
    )]
    pub parent_instance_id: Option<String>,
    #[field(
        display_name = "Status",
        description = "queued, pending, running, suspended, completed, failed, cancelled or not_started"
    )]
    pub status: String,
    #[field(
        display_name = "Suspension Reason",
        description = "Why a suspended run is not running: paused, waiting_signal, waiting_instances, sleeping or shutdown"
    )]
    pub suspension_reason: Option<String>,
    #[field(
        display_name = "Termination Reason",
        description = "Why a finished run ended"
    )]
    pub termination_reason: Option<String>,
    #[field(
        display_name = "Created At",
        description = "Milliseconds since the Unix epoch"
    )]
    pub created_at_ms: u64,
    #[field(
        display_name = "Started At",
        description = "Milliseconds since the Unix epoch"
    )]
    pub started_at_ms: Option<u64>,
    #[field(
        display_name = "Finished At",
        description = "Milliseconds since the Unix epoch"
    )]
    pub finished_at_ms: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput, PartialEq)]
#[serde(rename_all = "camelCase")]
#[capability_output(display_name = "Get Run Output")]
pub struct GetOutput {
    #[field(display_name = "Run", description = "The run's identity and state")]
    pub instance: RunSummary,
    #[field(
        display_name = "Output",
        description = "The finished run's output, when inlined (up to 1 MiB)"
    )]
    pub output: Option<serde_json::Value>,
    #[field(
        display_name = "Output Bytes",
        description = "Size of the full output, also when omitted"
    )]
    pub output_bytes: Option<u64>,
    #[field(
        display_name = "Output Omitted",
        description = "The output was over 1 MiB and is not inlined"
    )]
    pub output_omitted: bool,
    #[field(
        display_name = "Error",
        description = "The failed run's error, when inlined (up to 64 KiB)"
    )]
    pub error: Option<serde_json::Value>,
    #[field(
        display_name = "Error Omitted",
        description = "The error was over 64 KiB and is not inlined"
    )]
    pub error_omitted: bool,
}

#[capability(
    module = "control",
    id = "get",
    display_name = "Get Run",
    description = "Read one run of this tenant, with its output or error once it finished.",
    side_effects = false,
    idempotent = true,
    errors(
        permanent("CONTROL_NOT_FOUND", "No such run in this tenant"),
        permanent("CONTROL_INVALID", "The instance id is malformed"),
        permanent("CONTROL_DENIED", "Control is not available to this call"),
        permanent("CONTROL_TOO_LARGE", "The response is over its cap"),
        transient(
            "CONTROL_UNAVAILABLE",
            "The control service is temporarily unavailable"
        ),
        permanent("CONTROL_UNSUPPORTED", "The operation is not available in this build"),
        permanent("CONTROL_TIMEOUT", "The control call ran past its deadline"),
    )
)]
pub async fn get(input: GetInput) -> Result<GetOutput, String> {
    if input.instance_id.trim().is_empty() {
        return Err(invalid("instanceId must not be empty".into()));
    }
    host::get(input.instance_id).await
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[serde(rename_all = "camelCase")]
#[capability_input(display_name = "Query Runs Input")]
pub struct QueryInput {
    #[field(
        display_name = "Workflow ID",
        description = "Only runs of this workflow"
    )]
    #[serde(default)]
    pub workflow_id: Option<String>,
    #[field(
        display_name = "Run Label",
        description = "Only runs with this exact label"
    )]
    #[serde(default)]
    pub run_label: Option<String>,
    #[field(
        display_name = "Statuses",
        description = "Only runs in these states; empty means every state"
    )]
    #[serde(default)]
    pub statuses: Vec<String>,
    #[field(
        display_name = "Parent Instance ID",
        description = "Only children of this run"
    )]
    #[serde(default)]
    pub parent_instance_id: Option<String>,
    #[field(
        display_name = "Children Of This Run",
        description = "Only children of the calling run (needs a calling run)"
    )]
    #[serde(default)]
    pub caller_children: bool,
    #[field(display_name = "Created After", description = "Epoch ms, inclusive")]
    #[serde(default)]
    pub created_after_ms: Option<u64>,
    #[field(display_name = "Created Before", description = "Epoch ms, exclusive")]
    #[serde(default)]
    pub created_before_ms: Option<u64>,
    #[field(display_name = "Finished After", description = "Epoch ms, inclusive")]
    #[serde(default)]
    pub finished_after_ms: Option<u64>,
    #[field(display_name = "Finished Before", description = "Epoch ms, exclusive")]
    #[serde(default)]
    pub finished_before_ms: Option<u64>,
    #[field(
        display_name = "Sort By",
        description = "created_at or finished_at",
        example = "created_at",
        default = "created_at"
    )]
    #[serde(default)]
    pub sort_by: Option<String>,
    #[field(
        display_name = "Order",
        description = "asc or desc",
        example = "desc",
        default = "desc"
    )]
    #[serde(default)]
    pub order: Option<String>,
    #[field(display_name = "Page Size", description = "1-100", default = "20")]
    #[serde(default)]
    pub page_size: Option<u32>,
    #[field(
        display_name = "Page Token",
        description = "nextPageToken of the previous page"
    )]
    #[serde(default)]
    pub page_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput, PartialEq)]
#[serde(rename_all = "camelCase")]
#[capability_output(display_name = "Query Runs Output")]
pub struct QueryOutput {
    #[field(display_name = "Items", description = "The runs on this page")]
    pub items: Vec<RunSummary>,
    #[field(display_name = "Total", description = "Runs matching the filters")]
    pub total: u64,
    #[field(
        display_name = "Next Page Token",
        description = "Absent on the last page"
    )]
    pub next_page_token: Option<String>,
}

/// Validated `query` arguments, before they become the WIT request.
#[derive(Debug)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct QueryArgs {
    statuses: Vec<usize>,
    by_finished: bool,
    ascending: bool,
}

fn query_args(input: &QueryInput) -> Result<QueryArgs, String> {
    let statuses = input
        .statuses
        .iter()
        .map(|status| {
            STATUSES
                .iter()
                .position(|known| known == status)
                .ok_or_else(|| invalid(format!("unknown status `{status}`")))
        })
        .collect::<Result<_, _>>()?;
    let by_finished = match input.sort_by.as_deref().unwrap_or("created_at") {
        "created_at" => false,
        "finished_at" => true,
        other => {
            return Err(invalid(format!(
                "sortBy must be `created_at` or `finished_at`, not `{other}`"
            )));
        }
    };
    let ascending = match input.order.as_deref().unwrap_or("desc") {
        "asc" => true,
        "desc" => false,
        other => {
            return Err(invalid(format!(
                "order must be `asc` or `desc`, not `{other}`"
            )));
        }
    };
    if input.caller_children && input.parent_instance_id.is_some() {
        return Err(invalid(
            "pass either parentInstanceId or callerChildren, not both".into(),
        ));
    }
    Ok(QueryArgs {
        statuses,
        by_finished,
        ascending,
    })
}

#[capability(
    module = "control",
    id = "query",
    display_name = "Query Runs",
    description = "Find runs of this tenant, sorted by creation or finish time, a page at a time.",
    side_effects = false,
    idempotent = true,
    errors(
        permanent("CONTROL_INVALID", "A filter, sort or page size is out of range"),
        permanent("CONTROL_REQUIRES_INSTANCE", "Children of this run need a calling run"),
        permanent("CONTROL_DENIED", "Control is not available to this call"),
        permanent("CONTROL_TOO_LARGE", "The response is over its cap"),
        transient(
            "CONTROL_UNAVAILABLE",
            "The control service is temporarily unavailable"
        ),
        permanent("CONTROL_UNSUPPORTED", "The filter is not available in this build"),
        permanent("CONTROL_TIMEOUT", "The control call ran past its deadline"),
    )
)]
pub async fn query(input: QueryInput) -> Result<QueryOutput, String> {
    let args = query_args(&input)?;
    host::query(input, args).await
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[serde(rename_all = "camelCase")]
#[capability_input(display_name = "List Pending Signals Input")]
pub struct ListPendingSignalsInput {
    #[field(
        display_name = "Instance ID",
        description = "Open requests of this run (set exactly one scope)"
    )]
    #[serde(default)]
    pub instance_id: Option<String>,
    #[field(
        display_name = "Workflow ID",
        description = "Open requests of this workflow's runs (set exactly one scope)"
    )]
    #[serde(default)]
    pub workflow_id: Option<String>,
    #[field(
        display_name = "Children Of This Run",
        description = "Open requests of the calling run's children (set exactly one scope)"
    )]
    #[serde(default)]
    pub children: bool,
    #[field(
        display_name = "Signal ID",
        description = "Only requests of this WaitForSignal step id"
    )]
    #[serde(default)]
    pub signal_id: Option<String>,
    #[field(
        display_name = "Action Key",
        description = "Only requests with this action.key"
    )]
    #[serde(default)]
    pub action_key: Option<String>,
    #[field(display_name = "Page Size", description = "1-100", default = "20")]
    #[serde(default)]
    pub page_size: Option<u32>,
    #[field(
        display_name = "Page Token",
        description = "nextPageToken of the previous page"
    )]
    #[serde(default)]
    pub page_token: Option<String>,
}

/// One open `WaitForSignal` request.
#[derive(Debug, Serialize, Deserialize, CapabilityOutput, PartialEq)]
#[serde(rename_all = "camelCase")]
#[capability_output(display_name = "Pending Signal")]
pub struct PendingSignal {
    #[field(display_name = "Instance ID", description = "The waiting run")]
    pub instance_id: String,
    #[field(
        display_name = "Workflow ID",
        description = "The waiting run's workflow"
    )]
    pub workflow_id: String,
    #[field(
        display_name = "Signal ID",
        description = "The waiting WaitForSignal step id"
    )]
    pub signal_id: String,
    #[field(display_name = "Request ID", description = "The open request's id")]
    pub request_id: String,
    #[field(
        display_name = "Action Key",
        description = "The request's action.key, if any"
    )]
    pub action_key: Option<String>,
    #[field(
        display_name = "Response Schema",
        description = "JSON Schema the response must match"
    )]
    pub response_schema: Option<serde_json::Value>,
    #[field(
        display_name = "Context",
        description = "Prompt and correlation context"
    )]
    pub context: Option<serde_json::Value>,
    #[field(
        display_name = "Requested At",
        description = "Milliseconds since the Unix epoch"
    )]
    pub requested_at_ms: u64,
    #[field(
        display_name = "Deadline",
        description = "Milliseconds since the Unix epoch"
    )]
    pub deadline_ms: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput, PartialEq)]
#[serde(rename_all = "camelCase")]
#[capability_output(display_name = "List Pending Signals Output")]
pub struct ListPendingSignalsOutput {
    #[field(display_name = "Items", description = "Open requests on this page")]
    pub items: Vec<PendingSignal>,
    #[field(
        display_name = "Next Page Token",
        description = "Absent on the last page"
    )]
    pub next_page_token: Option<String>,
}

/// Where `list-pending-signals` looks.
#[derive(Debug)]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
enum SignalScope {
    Instance(String),
    Workflow(String),
    Children,
}

fn signal_scope(input: &ListPendingSignalsInput) -> Result<SignalScope, String> {
    match (
        input.instance_id.clone(),
        input.workflow_id.clone(),
        input.children,
    ) {
        (Some(id), None, false) => Ok(SignalScope::Instance(id)),
        (None, Some(id), false) => Ok(SignalScope::Workflow(id)),
        (None, None, true) => Ok(SignalScope::Children),
        _ => Err(invalid(
            "set exactly one of instanceId, workflowId or children".into(),
        )),
    }
}

#[capability(
    module = "control",
    id = "list-pending-signals",
    display_name = "List Pending Signals",
    description = "List open WaitForSignal requests of a run, a workflow or this run's children.",
    side_effects = false,
    idempotent = true,
    errors(
        permanent("CONTROL_INVALID", "The scope or page size is out of range"),
        permanent("CONTROL_NOT_FOUND", "No such run in this tenant"),
        permanent("CONTROL_REQUIRES_INSTANCE", "Children of this run need a calling run"),
        permanent("CONTROL_DENIED", "Control is not available to this call"),
        permanent("CONTROL_TOO_LARGE", "The page is over its cap"),
        transient(
            "CONTROL_UNAVAILABLE",
            "The control service is temporarily unavailable"
        ),
        permanent("CONTROL_UNSUPPORTED", "The scope is not available in this build"),
        permanent("CONTROL_TIMEOUT", "The control call ran past its deadline"),
    )
)]
pub async fn list_pending_signals(
    input: ListPendingSignalsInput,
) -> Result<ListPendingSignalsOutput, String> {
    let scope = signal_scope(&input)?;
    host::list_pending_signals(input, scope).await
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

    fn json(bytes: Option<Vec<u8>>) -> Option<serde_json::Value> {
        bytes.and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    fn status_name(status: types::InstanceStatus) -> &'static str {
        match status {
            types::InstanceStatus::Queued => "queued",
            types::InstanceStatus::Pending => "pending",
            types::InstanceStatus::Running => "running",
            types::InstanceStatus::Suspended => "suspended",
            types::InstanceStatus::Completed => "completed",
            types::InstanceStatus::Failed => "failed",
            types::InstanceStatus::Cancelled => "cancelled",
            types::InstanceStatus::NotStarted => "not_started",
        }
    }

    /// In [`super::STATUSES`] order.
    const WIT_STATUSES: [types::InstanceStatus; 8] = [
        types::InstanceStatus::Queued,
        types::InstanceStatus::Pending,
        types::InstanceStatus::Running,
        types::InstanceStatus::Suspended,
        types::InstanceStatus::Completed,
        types::InstanceStatus::Failed,
        types::InstanceStatus::Cancelled,
        types::InstanceStatus::NotStarted,
    ];

    fn summary(instance: types::InstanceSummary) -> super::RunSummary {
        super::RunSummary {
            instance_id: instance.instance_id,
            workflow_id: instance.workflow_id,
            version: instance.version,
            run_label: instance.run_label,
            parent_instance_id: instance.parent_instance_id,
            status: status_name(instance.status).into(),
            suspension_reason: instance.suspension_reason.map(|reason| {
                match reason {
                    types::SuspensionReason::Paused => "paused",
                    types::SuspensionReason::WaitingSignal => "waiting_signal",
                    types::SuspensionReason::WaitingInstances => "waiting_instances",
                    types::SuspensionReason::Sleeping => "sleeping",
                    types::SuspensionReason::Shutdown => "shutdown",
                }
                .into()
            }),
            termination_reason: instance.termination_reason,
            created_at_ms: instance.created_at_ms,
            started_at_ms: instance.started_at_ms,
            finished_at_ms: instance.finished_at_ms,
        }
    }

    pub(super) async fn get(instance_id: String) -> Result<super::GetOutput, String> {
        let detail = api::get(instance_id).await.map_err(control_error)?;
        let terminal = detail.terminal;
        Ok(super::GetOutput {
            instance: summary(detail.instance),
            output: json(terminal.output),
            output_bytes: terminal.output_bytes,
            output_omitted: terminal.output_omitted,
            error: json(terminal.error),
            error_omitted: terminal.error_omitted,
        })
    }

    pub(super) async fn query(
        input: super::QueryInput,
        args: super::QueryArgs,
    ) -> Result<super::QueryOutput, String> {
        let parent = if input.caller_children {
            Some(types::ParentFilter::Caller)
        } else {
            input.parent_instance_id.map(types::ParentFilter::Instance)
        };
        let page = api::query(types::QueryRequest {
            workflow_id: input.workflow_id,
            run_label: input.run_label,
            statuses: args.statuses.into_iter().map(|i| WIT_STATUSES[i]).collect(),
            parent,
            created_after_ms: input.created_after_ms,
            created_before_ms: input.created_before_ms,
            finished_after_ms: input.finished_after_ms,
            finished_before_ms: input.finished_before_ms,
            sort_by: if args.by_finished {
                types::SortField::FinishedAt
            } else {
                types::SortField::CreatedAt
            },
            order: if args.ascending {
                types::SortOrder::Ascending
            } else {
                types::SortOrder::Descending
            },
            page_size: super::page_size(input.page_size),
            page_token: input.page_token,
        })
        .await
        .map_err(control_error)?;
        Ok(super::QueryOutput {
            items: page.items.into_iter().map(summary).collect(),
            total: page.total,
            next_page_token: page.next_page_token,
        })
    }

    pub(super) async fn list_pending_signals(
        input: super::ListPendingSignalsInput,
        scope: super::SignalScope,
    ) -> Result<super::ListPendingSignalsOutput, String> {
        let page = api::list_pending_signals(types::PendingSignalsRequest {
            scope: match scope {
                super::SignalScope::Instance(id) => types::SignalScope::Instance(id),
                super::SignalScope::Workflow(id) => types::SignalScope::Workflow(id),
                super::SignalScope::Children => types::SignalScope::Children,
            },
            signal_id: input.signal_id,
            action_key: input.action_key,
            page_size: super::page_size(input.page_size),
            page_token: input.page_token,
        })
        .await
        .map_err(control_error)?;
        Ok(super::ListPendingSignalsOutput {
            items: page
                .items
                .into_iter()
                .map(|signal| super::PendingSignal {
                    instance_id: signal.instance_id,
                    workflow_id: signal.workflow_id,
                    signal_id: signal.signal_id,
                    request_id: signal.request_id,
                    action_key: signal.action_key,
                    response_schema: json(signal.response_schema),
                    context: json(signal.context),
                    requested_at_ms: signal.requested_at_ms,
                    deadline_ms: signal.deadline_ms,
                })
                .collect(),
            next_page_token: page.next_page_token,
        })
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

    pub(super) async fn get(_instance_id: String) -> Result<super::GetOutput, String> {
        Err(unavailable())
    }

    pub(super) async fn query(
        _input: super::QueryInput,
        _args: super::QueryArgs,
    ) -> Result<super::QueryOutput, String> {
        Err(unavailable())
    }

    pub(super) async fn list_pending_signals(
        _input: super::ListPendingSignalsInput,
        _scope: super::SignalScope,
    ) -> Result<super::ListPendingSignalsOutput, String> {
        Err(unavailable())
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

    let output_types = HashMap::from([
        ("WaitOutput", &__OUTPUT_META_WaitOutput),
        ("RunSummary", &__OUTPUT_META_RunSummary),
        ("GetOutput", &__OUTPUT_META_GetOutput),
        ("QueryOutput", &__OUTPUT_META_QueryOutput),
        ("PendingSignal", &__OUTPUT_META_PendingSignal),
        (
            "ListPendingSignalsOutput",
            &__OUTPUT_META_ListPendingSignalsOutput,
        ),
    ]);
    AgentInfo {
        id: runtara_dsl::agent_meta::CONTROL_AGENT_ID.into(),
        name: "Control".into(),
        description: "Read and coordinate runs of this tenant from a workflow.".into(),
        has_side_effects: false,
        supports_connections: false,
        integration_ids: vec![],
        capabilities: vec![
            capability_to_api_with_types(
                &__CAPABILITY_META_GET,
                Some(&__INPUT_META_GetInput),
                Some(&__OUTPUT_META_GetOutput),
                &output_types,
            ),
            capability_to_api_with_types(
                &__CAPABILITY_META_QUERY,
                Some(&__INPUT_META_QueryInput),
                Some(&__OUTPUT_META_QueryOutput),
                &output_types,
            ),
            capability_to_api_with_types(
                &__CAPABILITY_META_LIST_PENDING_SIGNALS,
                Some(&__INPUT_META_ListPendingSignalsInput),
                Some(&__OUTPUT_META_ListPendingSignalsOutput),
                &output_types,
            ),
            capability_to_api_with_types(
                &__CAPABILITY_META_WAIT,
                Some(&__INPUT_META_WaitInput),
                Some(&__OUTPUT_META_WaitOutput),
                &output_types,
            ),
        ],
    }
}

runtara_agent_macro::agent_component!(
    agent = "control",
    control_executor = true,
    capabilities = [get, query, list_pending_signals, wait],
    suspending = [wait],
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_declares_wait_as_suspending() {
        let info = agent_info();
        assert_eq!(info.id, "control");
        let ids: Vec<_> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["get", "query", "list-pending-signals", "wait"]);
        let wait = &info.capabilities[3];
        assert!(wait.suspends);
        assert!(!wait.trusted);
    }

    #[test]
    fn reads_are_idempotent_and_declare_control_errors() {
        let info = agent_info();
        for read in &info.capabilities[..3] {
            assert!(!read.suspends && !read.has_side_effects, "{}", read.id);
            assert!(read.is_idempotent, "{}", read.id);
            let codes: Vec<_> = read.known_errors.iter().map(|e| e.code.as_str()).collect();
            assert!(codes.contains(&"CONTROL_UNAVAILABLE"), "{}", read.id);
            assert!(
                codes.iter().all(|code| code.starts_with("CONTROL_")),
                "{codes:?}"
            );
            let known = runtara_control_contract::ErrorCode::all_agent_codes();
            assert!(codes.iter().all(|code| known.contains(code)), "{codes:?}");
        }
    }

    #[test]
    fn query_and_signal_arguments_are_validated_before_the_host() {
        let query: QueryInput = serde_json::from_value(serde_json::json!({
            "statuses": ["running", "not_started"], "sortBy": "finished_at", "order": "asc"
        }))
        .unwrap();
        let args = query_args(&query).unwrap();
        assert_eq!(args.statuses, [2, 7]);
        assert!(args.by_finished && args.ascending);
        for bad in [
            serde_json::json!({"statuses": ["done"]}),
            serde_json::json!({"sortBy": "name"}),
            serde_json::json!({"order": "up"}),
            serde_json::json!({"parentInstanceId": "p", "callerChildren": true}),
        ] {
            let input: QueryInput = serde_json::from_value(bad).unwrap();
            assert!(query_args(&input).unwrap_err().contains("CONTROL_INVALID"));
        }
        for (scope, ok) in [
            (serde_json::json!({"instanceId": "i"}), true),
            (serde_json::json!({"workflowId": "w"}), true),
            (serde_json::json!({"children": true}), true),
            (serde_json::json!({}), false),
            (
                serde_json::json!({"instanceId": "i", "workflowId": "w"}),
                false,
            ),
        ] {
            let input: ListPendingSignalsInput = serde_json::from_value(scope).unwrap();
            assert_eq!(signal_scope(&input).is_ok(), ok);
        }
        assert_eq!(page_size(None), 20);
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
