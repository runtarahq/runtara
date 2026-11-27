//! The control family (`/v1/instances`): the control calls with their JSON
//! bodies. The host enforces the control rules; the API only names
//! the call and carries the operation it runs inside.

use serde_json::Value;

/// One control call. Bodies are the JSON forms of the WIT requests.
#[derive(Clone, Debug, PartialEq)]
pub enum ControlCall {
    /// `POST /v1/instances`
    Start(Value),
    /// `GET /v1/instances/{id}`
    Get(String),
    /// `GET /v1/instances/{id}/state`
    GetState(String),
    /// `POST /v1/instances/query`
    Query(Value),
    /// `POST /v1/instances/signals/pending`
    PendingSignals(Value),
    /// `POST /v1/instances/{id}/signals`
    SendSignal(String, Value),
    /// `POST /v1/instances/{id}/cancel` (the body may be empty)
    Cancel(String, Value),
    /// `POST /v1/instances/{id}/pause`
    Pause(String),
    /// `POST /v1/instances/{id}/resume`
    Resume(String),
    /// `POST /v1/conversations/send`: a message into a conversation with a
    /// workflow of the caller's tenant.
    ConversationSend(Value),
    /// `POST /v1/conversations/history`: what was said in the conversation
    /// the calling run carries.
    ConversationHistory(Value),
    /// `POST /v1/conversations/delta`: a piece of a reply the calling run is
    /// writing, for whoever reads its conversation now. Not kept.
    ConversationDelta(Value),
    /// `POST /v1/runs/query`: a query over the runs of a workflow of the
    /// caller's tenant (`runtara_environment::run_query::RunQuery`).
    RunsQuery(Value),
    /// `POST /v1/runs/latest`: the latest run of one label of a workflow of
    /// the caller's tenant.
    RunsLatest(Value),
}

impl ControlCall {
    /// The WIT name of the call, for audit logs.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Start(_) => "start",
            Self::Get(_) => "get",
            Self::GetState(_) => "get-state",
            Self::Query(_) => "query",
            Self::PendingSignals(_) => "list-pending-signals",
            Self::SendSignal(..) => "send-signal",
            Self::Cancel(..) => "cancel",
            Self::Pause(_) => "pause",
            Self::Resume(_) => "resume",
            Self::ConversationSend(_) => "conversation-send",
            Self::ConversationHistory(_) => "conversation-history",
            Self::ConversationDelta(_) => "conversation-delta",
            Self::RunsQuery(_) => "query-workflow-runs",
            Self::RunsLatest(_) => "get-latest-run",
        }
    }

    /// Whether the call changes another run, and so needs an operation.
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::Start(_)
                | Self::SendSignal(..)
                | Self::Cancel(..)
                | Self::Pause(_)
                | Self::Resume(_)
        )
    }
}
