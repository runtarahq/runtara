//! Deliver managed responses to an existing session execution. Never launch a run.
use super::managed::*;
use crate::{
    runtime_client::{RuntimeClient, RuntimeError},
    workers::execution_engine::{ExecutionEngine, ExecutionError, SessionLaunchState},
};
use redis::aio::ConnectionManager;

pub async fn deliver_session(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    client: &RuntimeClient,
    engine: &ExecutionEngine,
) -> QueueResult<DeliveryOutcome> {
    let lease = match claim(conn, scope, 30_000).await? {
        ClaimOutcome::Empty => return Ok(DeliveryOutcome::Idle),
        ClaimOutcome::Busy => return Ok(DeliveryOutcome::Busy),
        ClaimOutcome::Deferred(e) => return Ok(DeliveryOutcome::Deferred(e)),
        ClaimOutcome::Blocked(e) => return Ok(DeliveryOutcome::Blocked(e)),
        ClaimOutcome::Claimed(e) => e,
    };
    // Receipt replay precedes current route/liveness checks, even after completion.
    if lease.target.is_some() {
        return deliver_claimed(conn, scope, client, lease, None).await;
    }
    let route = match session_route(conn, scope).await {
        Ok(route) => route,
        Err(QueueError::NotFound) => {
            return blocked(conn, scope, &lease, DeliveryReason::StaleTarget).await;
        }
        Err(error) => return Err(error),
    };
    match client.get_instance_info(&route.instance_id).await {
        Ok(info) if info.tenant_id != scope.tenant_id() || info.status.is_terminal() => {
            blocked(conn, scope, &lease, DeliveryReason::StaleTarget).await
        }
        Ok(_) => deliver_claimed(conn, scope, client, lease, Some(&route.instance_id)).await,
        Err(RuntimeError::InstanceNotFound(_)) => {
            missing_route(conn, scope, &lease, engine, &route.instance_id).await
        }
        Err(_) => {
            super::managed::defer(conn, scope, &lease, DeliveryReason::BackendUnavailable).await
        }
    }
}
async fn missing_route(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    lease: &Envelope,
    engine: &ExecutionEngine,
    instance: &str,
) -> QueueResult<DeliveryOutcome> {
    match engine
        .session_launch_state(scope.tenant_id(), &format!("session:{instance}"), instance)
        .await
    {
        Ok(SessionLaunchState::Pending) => {
            super::managed::defer(conn, scope, lease, DeliveryReason::NoTarget).await
        }
        Ok(SessionLaunchState::Rejected) | Err(ExecutionError::ValidationError(_)) => {
            blocked(conn, scope, lease, DeliveryReason::LaunchRejected).await
        }
        Ok(SessionLaunchState::Missing | SessionLaunchState::HandedOff) => {
            blocked(conn, scope, lease, DeliveryReason::StaleTarget).await
        }
        Err(_) => {
            super::managed::defer(conn, scope, lease, DeliveryReason::BackendUnavailable).await
        }
    }
}

async fn blocked(
    conn: &mut ConnectionManager,
    scope: &QueueScope,
    lease: &Envelope,
    reason: DeliveryReason,
) -> QueueResult<DeliveryOutcome> {
    Ok(DeliveryOutcome::Blocked(
        block(conn, scope, lease, reason).await?,
    ))
}
