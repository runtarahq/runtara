//! Deliver managed responses to an existing session execution. Never launch a run.
use super::managed::*;
use crate::runtime_client::RuntimeClient;
use redis::aio::ConnectionManager;

/// Every response is bound to its request when retained, and receipt replay
/// precedes liveness checks even after completion. An unbound legacy message is
/// blocked for explicit resolution rather than routed to the current run.
pub async fn deliver_session(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    client: &RuntimeClient,
) -> QueueResult<DeliveryOutcome> {
    deliver_to_instance(conn, scope, client).await
}
