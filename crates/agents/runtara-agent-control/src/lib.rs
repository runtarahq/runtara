// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Control agent: coordinate child runs from a workflow.
//!
//! An ordinary composed agent: each capability validates its input, makes
//! exactly one `runtara:control/api` call and shapes the result. The host
//! makes `api` real in the run's own store, with the caller's tenant, instance
//! and operation from the host, and links it `denied` everywhere else. The
//! compiler grants `runtara:control` to this agent alone.
//!
//! Reads: `get`, `get-state`, `query` and `list-pending-signals` cover the
//! caller's tenant; identity and caller-relative filters need a calling
//! instance. `get-state` reads a run's published state and `query` filters
//! by it, without waking the run.
//! Mutations: `start` durably admits a child of the calling run and returns
//! once it is accepted, without waiting for it to run; `send-signal`
//! answers an open `WaitForSignal` request of a child, an ancestor, or a
//! request that opted in with `action.key`; `cancel`, `pause` and `resume`
//! reach direct children only; none may target the calling run. Each is
//! replay-safe under the step's operation identity,
//! which the compiler emits and the host keeps, so a retried or replayed step
//! never applies twice. Waiting on children is the `WaitForInstances` step,
//! not a control call.

use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use runtara_control_contract::ErrorCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// A control failure as the `#[capability]` JSON error envelope.
fn control_error(code: ErrorCode, message: &str, retry_after_ms: Option<u64>) -> String {
    runtara_control_contract::agent_error(code, message, retry_after_ms).to_string()
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

#[derive(Debug, Serialize, Deserialize, CapabilityOutput, PartialEq)]
#[serde(rename_all = "camelCase")]
#[capability_output(display_name = "Get Run State Output")]
pub struct GetStateOutput {
    #[field(
        display_name = "Run",
        description = "The run's identity, status and workflow version"
    )]
    pub instance: RunSummary,
    #[field(
        display_name = "State",
        description = "The run's published state, as declared by its workflow's stateSchema; absent when it published none"
    )]
    pub state: Option<Value>,
    #[field(
        display_name = "State Updated At",
        description = "When the state last changed, milliseconds since the Unix epoch"
    )]
    pub state_updated_at_ms: Option<u64>,
}

#[capability(
    module = "control",
    id = "get-state",
    display_name = "Get Run State",
    description = "Read one run's published state (what its SetState steps wrote), without waking the run.",
    side_effects = false,
    idempotent = true,
    errors(
        permanent("CONTROL_NOT_FOUND", "No such run in this tenant"),
        permanent("CONTROL_INVALID", "The instance id is malformed"),
        permanent("CONTROL_DENIED", "Control is not available to this call"),
        transient(
            "CONTROL_UNAVAILABLE",
            "The control service is temporarily unavailable"
        ),
        permanent("CONTROL_UNSUPPORTED", "The operation is not available in this build"),
        permanent("CONTROL_TIMEOUT", "The control call ran past its deadline"),
    )
)]
pub async fn get_state(input: GetInput) -> Result<GetStateOutput, String> {
    if input.instance_id.trim().is_empty() {
        return Err(invalid("instanceId must not be empty".into()));
    }
    host::get_state(input.instance_id).await
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
        display_name = "State Filters",
        description = "Only runs whose published state matches every filter: [{field, op, value}] with op eq, ne, in, lt, lte, gt, gte or exists. A run without the field does not match; date-times compare in UTC.",
        example = r#"[{"field": "stage", "op": "eq", "value": "approval"}]"#
    )]
    #[serde(default)]
    pub state: Option<Value>,
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

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, CapabilityInput)]
#[serde(rename_all = "camelCase")]
#[capability_input(display_name = "Send Signal Input")]
pub struct SendSignalInput {
    #[field(
        display_name = "Instance ID",
        description = "The waiting run: a child or ancestor of this run, or any run whose request opted in with action.key"
    )]
    pub instance_id: String,
    #[field(
        display_name = "Signal ID",
        description = "The waiting WaitForSignal step id"
    )]
    pub signal_id: String,
    #[field(
        display_name = "Action Key",
        description = "The request's action.key; required to answer a run outside this run's lineage"
    )]
    #[serde(default)]
    pub action_key: Option<String>,
    #[field(
        display_name = "Request ID",
        description = "Pick one of several open requests (from List Pending Signals)"
    )]
    #[serde(default)]
    pub request_id: Option<String>,
    #[field(
        display_name = "Payload",
        description = "The response; it must match the request's response schema"
    )]
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput, PartialEq)]
#[serde(rename_all = "camelCase")]
#[capability_output(display_name = "Send Signal Output")]
pub struct SendSignalOutput {
    #[field(display_name = "Request ID", description = "The answered request")]
    pub request_id: String,
    #[field(
        display_name = "Replayed",
        description = "This step had already answered it; nothing new was sent"
    )]
    pub replayed: bool,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[serde(rename_all = "camelCase")]
#[capability_input(display_name = "Cancel Run Input")]
pub struct CancelInput {
    #[field(
        display_name = "Instance ID",
        description = "A direct child of this run"
    )]
    pub instance_id: String,
    #[field(
        display_name = "Reason",
        description = "Why it is cancelled (at most 1024 bytes)"
    )]
    #[serde(default)]
    pub reason: Option<String>,
    #[field(
        display_name = "Grace (ms)",
        description = "Cooperative grace before the stop is forced, 0-3600000",
        default = "5000"
    )]
    #[serde(default)]
    pub grace_ms: Option<u64>,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[serde(rename_all = "camelCase")]
#[capability_input(display_name = "Run Input")]
pub struct RunInput {
    #[field(
        display_name = "Instance ID",
        description = "A direct child of this run"
    )]
    pub instance_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput, PartialEq)]
#[serde(rename_all = "camelCase")]
#[capability_output(display_name = "Command Output")]
pub struct CommandOutput {
    #[field(display_name = "Instance ID", description = "The target run")]
    pub instance_id: String,
    #[field(
        display_name = "Outcome",
        description = "requested (applies at the run's next checkpoint), applied (took effect at once), unchanged or already_terminal"
    )]
    pub outcome: String,
    #[field(
        display_name = "Replayed",
        description = "This step had already issued the command; nothing new happened"
    )]
    pub replayed: bool,
}

fn require_id(field: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(invalid(format!("{field} must not be empty")));
    }
    Ok(())
}

/// The `parentClosePolicy` values, in [`runtara_control_contract::ParentClosePolicy::ALL`]
/// order; the editor preselects the first.
const PARENT_CLOSE_POLICIES: [&str; 2] = ["cancel", "leave_running"];

/// Allowed values of `parentClosePolicy` for the input metadata.
pub struct ParentClosePolicyNames;

impl runtara_dsl::agent_meta::EnumVariants for ParentClosePolicyNames {
    fn variant_names() -> &'static [&'static str] {
        &PARENT_CLOSE_POLICIES
    }
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[serde(rename_all = "camelCase")]
#[capability_input(display_name = "Start Child Run Input")]
pub struct StartInput {
    #[field(
        display_name = "Workflow ID",
        description = "Id of the workflow to start (not its slug)"
    )]
    pub workflow_id: String,
    #[field(
        display_name = "Version",
        description = "Workflow version (at least 1); defaults to the current version, fixed at admission"
    )]
    #[serde(default)]
    pub version: Option<u32>,
    #[field(
        display_name = "Inputs",
        description = "The child's {data, variables} input envelope"
    )]
    #[serde(default)]
    pub inputs: Option<BTreeMap<String, Value>>,
    #[field(
        display_name = "Run Label",
        description = "Business identity of the child, unique per parent for the parent's lifetime (1-1024 bytes)"
    )]
    #[serde(default)]
    pub run_label: Option<String>,
    // Required and without a default: the author must choose (E022 while
    // unmapped); the editor preselects the first value, `cancel`.
    #[field(
        display_name = "Parent Close Policy",
        description = "What happens to the child if this run ends first: cancel it (after a 5 s grace) or leave it running",
        enum_type = "ParentClosePolicyNames"
    )]
    pub parent_close_policy: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput, PartialEq)]
#[serde(rename_all = "camelCase")]
#[capability_output(display_name = "Start Child Run Output")]
pub struct StartOutput {
    #[field(display_name = "Instance ID", description = "The child run")]
    pub instance_id: String,
    #[field(
        display_name = "Workflow ID",
        description = "The workflow the child executes"
    )]
    pub workflow_id: String,
    #[field(
        display_name = "Version",
        description = "The workflow version fixed at admission"
    )]
    pub version: u32,
    #[field(display_name = "Run Label", description = "The child's label, if any")]
    pub run_label: Option<String>,
    #[field(
        display_name = "Replayed",
        description = "This step had already started the child; nothing new ran"
    )]
    pub replayed: bool,
}

/// Validated `start` arguments, before they become the WIT request.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
struct StartArgs {
    input: Vec<u8>,
    policy: runtara_control_contract::ParentClosePolicy,
}

fn start_args(input: &StartInput) -> Result<StartArgs, String> {
    require_id("workflowId", &input.workflow_id)?;
    if input.version == Some(0) {
        return Err(invalid("version must be at least 1".into()));
    }
    if let Some(label) = &input.run_label {
        let max = runtara_control_contract::MAX_RUN_LABEL_BYTES;
        if label.is_empty() || label.len() > max {
            return Err(invalid(format!("runLabel must be 1-{max} bytes")));
        }
    }
    let policy = runtara_control_contract::ParentClosePolicy::ALL
        .into_iter()
        .find(|policy| policy.input_name() == input.parent_close_policy)
        .ok_or_else(|| {
            invalid(format!(
                "parentClosePolicy must be `cancel` or `leave_running`, not `{}`",
                input.parent_close_policy
            ))
        })?;
    let input = match &input.inputs {
        Some(inputs) => serde_json::to_vec(inputs),
        None => serde_json::to_vec(&serde_json::json!({"data": {}, "variables": {}})),
    }
    .map_err(|_| invalid("inputs must be JSON".into()))?;
    Ok(StartArgs { input, policy })
}

#[capability(
    module = "control",
    id = "send-signal",
    display_name = "Send Signal",
    description = "Answer an open WaitForSignal request of a child, an ancestor, or a run whose request opted in with action.key. Replay-safe: a retried step answers once.",
    side_effects = true,
    tags = "runtime:requires-run",
    errors(
        permanent(
            "CONTROL_INVALID",
            "Malformed arguments, the calling run as target, or a payload that does not match the response schema"
        ),
        permanent("CONTROL_NOT_FOUND", "No such run in this tenant"),
        permanent(
            "CONTROL_DENIED",
            "The target is outside this run's lineage and its request did not opt in with this action.key"
        ),
        permanent("CONTROL_NOT_WAITING", "No open request waits on that signal"),
        permanent(
            "CONTROL_AMBIGUOUS",
            "Several open requests wait on that signal; pass a requestId"
        ),
        permanent(
            "CONTROL_ALREADY_ANSWERED",
            "Another operation already answered the request"
        ),
        permanent(
            "CONTROL_REPLAY_CONFLICT",
            "This step already ran with different arguments"
        ),
        permanent("CONTROL_REQUIRES_INSTANCE", "Only works inside a run"),
        permanent("CONTROL_REQUIRES_OPERATION", "Only works in a compiled workflow step"),
        transient(
            "CONTROL_UNAVAILABLE",
            "The control service is temporarily unavailable"
        ),
        permanent("CONTROL_TIMEOUT", "The control call ran past its deadline"),
    )
)]
pub async fn send_signal(input: SendSignalInput) -> Result<SendSignalOutput, String> {
    require_id("instanceId", &input.instance_id)?;
    require_id("signalId", &input.signal_id)?;
    host::send_signal(input).await
}

#[capability(
    module = "control",
    id = "cancel",
    display_name = "Cancel Run",
    description = "Cancel a direct child: cooperatively, then forced after the grace period. A parked or queued child ends at once.",
    side_effects = true,
    tags = "runtime:requires-run",
    errors(
        permanent("CONTROL_INVALID", "Malformed arguments or the calling run as target"),
        permanent("CONTROL_NOT_FOUND", "No such run in this tenant"),
        permanent("CONTROL_NOT_CHILD", "The target is not a direct child of this run"),
        permanent("CONTROL_DENIED", "The target is an ancestor of this run"),
        permanent(
            "CONTROL_REPLAY_CONFLICT",
            "This step already ran with different arguments"
        ),
        permanent("CONTROL_REQUIRES_INSTANCE", "Only works inside a run"),
        permanent("CONTROL_REQUIRES_OPERATION", "Only works in a compiled workflow step"),
        transient(
            "CONTROL_UNAVAILABLE",
            "The control service is temporarily unavailable"
        ),
        permanent("CONTROL_TIMEOUT", "The control call ran past its deadline"),
    )
)]
pub async fn cancel(input: CancelInput) -> Result<CommandOutput, String> {
    require_id("instanceId", &input.instance_id)?;
    if runtara_control_contract::cancel_grace_ms(input.grace_ms).is_none() {
        return Err(invalid(format!(
            "graceMs must be 0-{}",
            runtara_control_contract::MAX_CANCEL_GRACE_MS
        )));
    }
    host::cancel(input).await
}

#[capability(
    module = "control",
    id = "pause",
    display_name = "Pause Run",
    description = "Pause a direct child. A waiting child pauses at once; a running one at its next checkpoint.",
    side_effects = true,
    tags = "runtime:requires-run",
    errors(
        permanent("CONTROL_INVALID", "Malformed arguments or the calling run as target"),
        permanent("CONTROL_NOT_FOUND", "No such run in this tenant"),
        permanent("CONTROL_NOT_CHILD", "The target is not a direct child of this run"),
        permanent("CONTROL_DENIED", "The target is an ancestor of this run"),
        permanent("CONTROL_NOT_PAUSABLE", "The run has not started or has finished"),
        permanent(
            "CONTROL_REPLAY_CONFLICT",
            "This step already ran with different arguments"
        ),
        permanent("CONTROL_REQUIRES_INSTANCE", "Only works inside a run"),
        permanent("CONTROL_REQUIRES_OPERATION", "Only works in a compiled workflow step"),
        transient(
            "CONTROL_UNAVAILABLE",
            "The control service is temporarily unavailable"
        ),
        permanent("CONTROL_TIMEOUT", "The control call ran past its deadline"),
    )
)]
pub async fn pause(input: RunInput) -> Result<CommandOutput, String> {
    require_id("instanceId", &input.instance_id)?;
    host::pause(input.instance_id).await
}

#[capability(
    module = "control",
    id = "resume",
    display_name = "Resume Run",
    description = "Resume an explicitly paused direct child. It never answers a WaitForSignal request.",
    side_effects = true,
    tags = "runtime:requires-run",
    errors(
        permanent("CONTROL_INVALID", "Malformed arguments or the calling run as target"),
        permanent("CONTROL_NOT_FOUND", "No such run in this tenant"),
        permanent("CONTROL_NOT_CHILD", "The target is not a direct child of this run"),
        permanent("CONTROL_DENIED", "The target is an ancestor of this run"),
        permanent("CONTROL_NOT_PAUSED", "The run is not explicitly paused"),
        permanent(
            "CONTROL_REPLAY_CONFLICT",
            "This step already ran with different arguments"
        ),
        permanent("CONTROL_REQUIRES_INSTANCE", "Only works inside a run"),
        permanent("CONTROL_REQUIRES_OPERATION", "Only works in a compiled workflow step"),
        transient(
            "CONTROL_UNAVAILABLE",
            "The control service is temporarily unavailable"
        ),
        permanent("CONTROL_TIMEOUT", "The control call ran past its deadline"),
    )
)]
pub async fn resume(input: RunInput) -> Result<CommandOutput, String> {
    require_id("instanceId", &input.instance_id)?;
    host::resume(input.instance_id).await
}

#[capability(
    module = "control",
    id = "start",
    display_name = "Start Child Run",
    description = "Durably admit a child run of this run and return once it is accepted, without waiting for it to run. Replay-safe: a retried step returns the same child (replayed: true). The run label is unique per parent. Children count against the tenant concurrency limit and may hold at most max(1, floor(0.8 x limit)) slots.",
    side_effects = true,
    tags = "runtime:requires-run",
    errors(
        permanent(
            "CONTROL_INVALID",
            "Malformed arguments, lineage deeper than 16, or inputs that do not match the child's input schema"
        ),
        permanent("CONTROL_NOT_FOUND", "No such workflow or version in this tenant"),
        permanent(
            "CONTROL_NOT_RUNNABLE",
            "The workflow's compilation failed permanently; a workflow not compiled yet is accepted"
        ),
        permanent(
            "CONTROL_LABEL_CONFLICT",
            "This run already gave the label to another child"
        ),
        permanent(
            "CONTROL_REPLAY_CONFLICT",
            "This step already ran with different arguments"
        ),
        transient(
            "CONTROL_CAPACITY_RATE_LIMITED",
            "The concurrency limit or the control share of it is full; retry after the hint"
        ),
        permanent(
            "CONTROL_CAPACITY_UNSATISFIABLE",
            "A concurrency limit of at most 1 can never admit a child"
        ),
        permanent("CONTROL_DENIED", "Control is not available to this call"),
        permanent("CONTROL_REQUIRES_INSTANCE", "Only works inside a run"),
        permanent("CONTROL_REQUIRES_OPERATION", "Only works in a compiled workflow step"),
        transient(
            "CONTROL_UNAVAILABLE",
            "The control service is temporarily unavailable"
        ),
        permanent("CONTROL_TIMEOUT", "The control call ran past its deadline"),
    )
)]
pub async fn start(input: StartInput) -> Result<StartOutput, String> {
    let args = start_args(&input)?;
    host::start(input, args).await
}

/// Host control calls. Real only for a run's own entry.
#[cfg(target_arch = "wasm32")]
mod host {
    use super::ErrorCode;
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
            types::ErrorCode::Timeout => ErrorCode::Timeout,
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

    pub(super) async fn get_state(instance_id: String) -> Result<super::GetStateOutput, String> {
        let read = api::get_state(instance_id).await.map_err(control_error)?;
        Ok(super::GetStateOutput {
            instance: summary(read.instance),
            state: json(read.state),
            state_updated_at_ms: read.state_updated_at_ms,
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
            state: input
                .state
                .map(|filters| serde_json::to_vec(&filters).expect("JSON values serialize")),
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

    fn command(result: types::CommandResult) -> super::CommandOutput {
        super::CommandOutput {
            instance_id: result.instance_id,
            outcome: match result.outcome {
                types::CommandOutcome::Requested => "requested",
                types::CommandOutcome::Applied => "applied",
                types::CommandOutcome::Unchanged => "unchanged",
                types::CommandOutcome::AlreadyTerminal => "already_terminal",
            }
            .into(),
            replayed: result.replayed,
        }
    }

    pub(super) async fn send_signal(
        input: super::SendSignalInput,
    ) -> Result<super::SendSignalOutput, String> {
        let payload = serde_json::to_vec(&input.payload)
            .map_err(|_| super::invalid("payload must be JSON".into()))?;
        let result = api::send_signal(types::SendSignalRequest {
            instance_id: input.instance_id,
            signal_id: input.signal_id,
            action_key: input.action_key,
            request_id: input.request_id,
            payload,
        })
        .await
        .map_err(control_error)?;
        Ok(super::SendSignalOutput {
            request_id: result.request_id,
            replayed: result.replayed,
        })
    }

    pub(super) async fn cancel(input: super::CancelInput) -> Result<super::CommandOutput, String> {
        api::cancel(types::CancelRequest {
            instance_id: input.instance_id,
            reason: input.reason,
            grace_ms: input.grace_ms,
        })
        .await
        .map(command)
        .map_err(control_error)
    }

    pub(super) async fn pause(instance_id: String) -> Result<super::CommandOutput, String> {
        api::pause(instance_id)
            .await
            .map(command)
            .map_err(control_error)
    }

    pub(super) async fn resume(instance_id: String) -> Result<super::CommandOutput, String> {
        api::resume(instance_id)
            .await
            .map(command)
            .map_err(control_error)
    }

    pub(super) async fn start(
        input: super::StartInput,
        args: super::StartArgs,
    ) -> Result<super::StartOutput, String> {
        use runtara_control_contract::ParentClosePolicy;
        let result = api::start(types::StartRequest {
            workflow_id: input.workflow_id,
            version: input.version,
            input: args.input,
            run_label: input.run_label,
            parent_close_policy: match args.policy {
                ParentClosePolicy::Cancel => types::ParentClosePolicy::Cancel,
                ParentClosePolicy::LeaveRunning => types::ParentClosePolicy::LeaveRunning,
            },
        })
        .await
        .map_err(control_error)?;
        Ok(super::StartOutput {
            instance_id: result.instance_id,
            workflow_id: result.workflow_id,
            version: result.version,
            run_label: result.run_label,
            replayed: result.replayed,
        })
    }
}

/// Natively there is no control host: the capability runs only as a
/// component.
#[cfg(not(target_arch = "wasm32"))]
mod host {
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

    pub(super) async fn get_state(_instance_id: String) -> Result<super::GetStateOutput, String> {
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

    pub(super) async fn send_signal(
        _input: super::SendSignalInput,
    ) -> Result<super::SendSignalOutput, String> {
        Err(unavailable())
    }

    pub(super) async fn cancel(_input: super::CancelInput) -> Result<super::CommandOutput, String> {
        Err(unavailable())
    }

    pub(super) async fn pause(_instance_id: String) -> Result<super::CommandOutput, String> {
        Err(unavailable())
    }

    pub(super) async fn resume(_instance_id: String) -> Result<super::CommandOutput, String> {
        Err(unavailable())
    }

    pub(super) async fn start(
        _input: super::StartInput,
        _args: super::StartArgs,
    ) -> Result<super::StartOutput, String> {
        Err(unavailable())
    }
}

/// Canonical `AgentInfo` for the sidecar meta.json (host-only).
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{AgentInfo, capability_to_api_with_types};
    use std::collections::HashMap;

    let output_types = HashMap::from([
        ("RunSummary", &__OUTPUT_META_RunSummary),
        ("GetOutput", &__OUTPUT_META_GetOutput),
        ("QueryOutput", &__OUTPUT_META_QueryOutput),
        ("PendingSignal", &__OUTPUT_META_PendingSignal),
        (
            "ListPendingSignalsOutput",
            &__OUTPUT_META_ListPendingSignalsOutput,
        ),
        ("StartOutput", &__OUTPUT_META_StartOutput),
        ("SendSignalOutput", &__OUTPUT_META_SendSignalOutput),
        ("CommandOutput", &__OUTPUT_META_CommandOutput),
        ("GetStateOutput", &__OUTPUT_META_GetStateOutput),
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
                &__CAPABILITY_META_START,
                Some(&__INPUT_META_StartInput),
                Some(&__OUTPUT_META_StartOutput),
                &output_types,
            ),
            capability_to_api_with_types(
                &__CAPABILITY_META_SEND_SIGNAL,
                Some(&__INPUT_META_SendSignalInput),
                Some(&__OUTPUT_META_SendSignalOutput),
                &output_types,
            ),
            capability_to_api_with_types(
                &__CAPABILITY_META_CANCEL,
                Some(&__INPUT_META_CancelInput),
                Some(&__OUTPUT_META_CommandOutput),
                &output_types,
            ),
            capability_to_api_with_types(
                &__CAPABILITY_META_PAUSE,
                Some(&__INPUT_META_RunInput),
                Some(&__OUTPUT_META_CommandOutput),
                &output_types,
            ),
            capability_to_api_with_types(
                &__CAPABILITY_META_RESUME,
                Some(&__INPUT_META_RunInput),
                Some(&__OUTPUT_META_CommandOutput),
                &output_types,
            ),
            capability_to_api_with_types(
                &__CAPABILITY_META_GET_STATE,
                Some(&__INPUT_META_GetInput),
                Some(&__OUTPUT_META_GetStateOutput),
                &output_types,
            ),
        ],
    }
}

runtara_agent_macro::agent_component!(
    agent = "control",
    control = true,
    capabilities = [
        get,
        query,
        list_pending_signals,
        start,
        send_signal,
        cancel,
        pause,
        resume,
        get_state
    ],
);

#[cfg(test)]
mod tests {
    use super::*;

    /// Every capability makes exactly one `runtara:control/api` call, so the
    /// host's per-call bound and the operation it reads at the call are the
    /// capability's own. A second call would get its own 90 s and reuse the
    /// capability's operation for a second mutation.
    #[test]
    fn every_capability_makes_exactly_one_api_call() {
        let source = include_str!("lib.rs");
        let host_start = source
            .find("#[cfg(target_arch = \"wasm32\")]\nmod host {")
            .expect("the wasm host module");
        let host_end = host_start + source[host_start..].find("\n}\n").expect("its end");
        let host = &source[host_start..host_end];
        let host_fns: Vec<&str> = host.split("pub(super) async fn ").skip(1).collect();
        assert_eq!(host_fns.len(), agent_info().capabilities.len());
        for body in host_fns {
            let name = &body[..body.find('(').unwrap()];
            assert_eq!(body.matches("api::").count(), 1, "host::{name}");
        }
        let capabilities: Vec<&str> = source[..host_start]
            .split("pub async fn ")
            .skip(1)
            .collect();
        assert_eq!(capabilities.len(), agent_info().capabilities.len());
        for body in capabilities {
            let name = &body[..body.find('(').unwrap()];
            assert_eq!(body.matches("host::").count(), 1, "{name}");
        }
    }

    #[test]
    fn metadata_lists_every_capability_and_none_suspends() {
        let info = agent_info();
        assert_eq!(info.id, "control");
        let ids: Vec<_> = info.capabilities.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "get",
                "query",
                "list-pending-signals",
                "start",
                "send-signal",
                "cancel",
                "pause",
                "resume",
                "get-state"
            ]
        );
        assert!(
            info.capabilities
                .iter()
                .all(|capability| !capability.suspends && !capability.trusted)
        );
    }

    #[test]
    fn mutations_have_side_effects_and_need_a_run() {
        let info = agent_info();
        for mutation in &info.capabilities[3..8] {
            assert!(mutation.has_side_effects, "{}", mutation.id);
            assert!(!mutation.suspends, "{}", mutation.id);
            assert!(
                mutation
                    .tags
                    .iter()
                    .any(|tag| tag == runtara_control_contract::REQUIRES_RUN_TAG),
                "{}: {:?}",
                mutation.id,
                mutation.tags
            );
            let codes: Vec<_> = mutation
                .known_errors
                .iter()
                .map(|e| e.code.as_str())
                .collect();
            for code in [
                "CONTROL_REQUIRES_INSTANCE",
                "CONTROL_REQUIRES_OPERATION",
                "CONTROL_REPLAY_CONFLICT",
                "CONTROL_UNAVAILABLE",
            ] {
                assert!(codes.contains(&code), "{}: {codes:?}", mutation.id);
            }
            let known = runtara_control_contract::ErrorCode::all_agent_codes();
            assert!(codes.iter().all(|code| known.contains(code)), "{codes:?}");
        }
    }

    #[test]
    fn a_signal_payload_is_any_json() {
        let info = serde_json::to_value(agent_info()).unwrap();
        let send = info["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|capability| capability["id"] == "send-signal")
            .unwrap();
        let payload = send["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["name"] == "payload")
            .unwrap();
        assert_eq!(payload["type"], "any", "{payload}");
    }

    #[test]
    fn start_requires_a_parent_close_policy_without_a_default() {
        let info = agent_info();
        let start = info
            .capabilities
            .iter()
            .find(|capability| capability.id == "start")
            .unwrap();
        assert!(start.has_side_effects && !start.suspends);
        assert!(
            start
                .tags
                .iter()
                .any(|tag| tag == runtara_control_contract::REQUIRES_RUN_TAG)
        );
        let codes: Vec<_> = start.known_errors.iter().map(|e| e.code.as_str()).collect();
        for code in [
            runtara_control_contract::CONTROL_CAPACITY_RATE_LIMITED,
            runtara_control_contract::CONTROL_CAPACITY_UNSATISFIABLE,
            "CONTROL_LABEL_CONFLICT",
            "CONTROL_NOT_RUNNABLE",
        ] {
            assert!(codes.contains(&code), "{codes:?}");
        }

        let info = serde_json::to_value(agent_info()).unwrap();
        let start = info["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .find(|capability| capability["id"] == "start")
            .unwrap();
        let field = |name: &str| {
            start["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|field| field["name"] == name)
                .unwrap_or_else(|| panic!("no {name} in {start}"))
                .clone()
        };
        let schema = runtara_control_contract::start_input_schema();
        let policy = field(runtara_control_contract::PARENT_CLOSE_POLICY_FIELD);
        assert_eq!(policy["required"], true, "{policy}");
        assert_eq!(policy["type"], "string", "{policy}");
        assert_eq!(
            policy["enum"],
            serde_json::json!(["cancel", "leave_running"]),
            "{policy}"
        );
        assert_eq!(
            policy["enum"],
            schema["properties"][runtara_control_contract::PARENT_CLOSE_POLICY_FIELD]["enum"]
        );
        assert!(policy.get("default").is_none(), "{policy}");
        let required: Vec<_> = start["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|field| field["required"] == true)
            .map(|field| field["name"].clone())
            .collect();
        assert_eq!(serde_json::Value::from(required), schema["required"]);
        for (name, kind) in [
            ("workflowId", "string"),
            ("version", "integer"),
            ("inputs", "object"),
            ("runLabel", "string"),
        ] {
            let field = field(name);
            assert_eq!(field["type"], kind, "{field}");
            assert_eq!(field["type"], schema["properties"][name]["type"], "{field}");
        }
    }

    #[test]
    fn start_arguments_are_validated_before_the_host() {
        let start_with = |input: serde_json::Value| {
            let input: StartInput = serde_json::from_value(input).unwrap();
            futures_lite_block_on(start(input)).unwrap_err()
        };
        let long = "x".repeat(runtara_control_contract::MAX_RUN_LABEL_BYTES + 1);
        for bad in [
            serde_json::json!({"workflowId": " ", "parentClosePolicy": "cancel"}),
            serde_json::json!({"workflowId": "wf", "version": 0, "parentClosePolicy": "cancel"}),
            serde_json::json!({"workflowId": "wf", "parentClosePolicy": "leave-running"}),
            serde_json::json!({"workflowId": "wf", "parentClosePolicy": ""}),
            serde_json::json!({"workflowId": "wf", "runLabel": "", "parentClosePolicy": "cancel"}),
            serde_json::json!({"workflowId": "wf", "runLabel": long, "parentClosePolicy": "cancel"}),
        ] {
            assert!(start_with(bad.clone()).contains("CONTROL_INVALID"), "{bad}");
        }
        // Valid arguments reach the (native, absent) host.
        let label = "x".repeat(runtara_control_contract::MAX_RUN_LABEL_BYTES);
        assert!(
            start_with(serde_json::json!({
                "workflowId": "wf", "version": 1, "runLabel": label,
                "parentClosePolicy": "leave_running"
            }))
            .contains("CONTROL_UNAVAILABLE")
        );
        // Absent inputs become the empty envelope; given inputs pass through.
        let input: StartInput = serde_json::from_value(serde_json::json!({
            "workflowId": "wf", "parentClosePolicy": "cancel"
        }))
        .unwrap();
        let args = start_args(&input).unwrap();
        assert_eq!(
            args.policy,
            runtara_control_contract::ParentClosePolicy::Cancel
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&args.input).unwrap(),
            serde_json::json!({"data": {}, "variables": {}})
        );
        let input: StartInput = serde_json::from_value(serde_json::json!({
            "workflowId": "wf", "parentClosePolicy": "leave_running",
            "inputs": {"data": {"n": 1}, "variables": {}}
        }))
        .unwrap();
        let args = start_args(&input).unwrap();
        assert_eq!(
            args.policy,
            runtara_control_contract::ParentClosePolicy::LeaveRunning
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&args.input).unwrap(),
            serde_json::json!({"data": {"n": 1}, "variables": {}})
        );
        assert_eq!(
            PARENT_CLOSE_POLICIES,
            runtara_control_contract::ParentClosePolicy::ALL
                .map(runtara_control_contract::ParentClosePolicy::input_name)
        );
    }

    #[test]
    fn mutation_arguments_are_validated_before_the_host() {
        let send = |input: serde_json::Value| {
            futures_lite_block_on(send_signal(serde_json::from_value(input).unwrap()))
        };
        assert!(
            send(serde_json::json!({"instanceId": " ", "signalId": "approve"}))
                .unwrap_err()
                .contains("CONTROL_INVALID")
        );
        assert!(
            send(serde_json::json!({"instanceId": "child", "signalId": ""}))
                .unwrap_err()
                .contains("CONTROL_INVALID")
        );
        let cancel_with = |grace: u64| {
            futures_lite_block_on(cancel(CancelInput {
                instance_id: "child".into(),
                reason: None,
                grace_ms: Some(grace),
            }))
        };
        assert!(
            cancel_with(3_600_001)
                .unwrap_err()
                .contains("CONTROL_INVALID")
        );
        // In range, the call reaches the (native, absent) host.
        assert!(cancel_with(0).unwrap_err().contains("CONTROL_UNAVAILABLE"));
        let empty = || RunInput {
            instance_id: String::new(),
        };
        for error in [
            futures_lite_block_on(pause(empty())).unwrap_err(),
            futures_lite_block_on(resume(empty())).unwrap_err(),
        ] {
            assert!(error.contains("CONTROL_INVALID"));
        }
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
