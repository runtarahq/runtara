use serde::{Deserialize, Serialize};

/// The run a workflow API request belongs to.
///
/// Attached by the host outside the guest's control: by the embedded runner's
/// `send_request` hook, or by runtara-cloud's binding gateway from a node's
/// verified execution identity. It never appears in a path, query or body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Binding {
    pub tenant_id: String,
    pub instance_id: String,
    /// One launch of the instance; idempotency receipts and the outcome are
    /// scoped to it.
    pub launch_id: String,
    /// Root lease epoch the launch holds. A write presenting an older epoch
    /// than the run's current lease is `fenced`.
    pub root_lease_epoch: u64,
}
