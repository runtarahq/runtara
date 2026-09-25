//! Recover queued session delivery even with no attached browser or channel task.
use crate::{
    api::services::session_queue::{
        delivery::deliver_session,
        managed::{has_unresolved, prune_completed, scan_scopes},
    },
    runtime_client::RuntimeClient,
    shutdown::ShutdownSignal,
};
use redis::aio::ConnectionManager;
use std::{collections::VecDeque, sync::Arc, time::Duration};
use tracing::warn;

pub async fn run(
    mut conn: ConnectionManager,
    client: Arc<RuntimeClient>,
    shutdown: ShutdownSignal,
) {
    let mut cursor = 0;
    let mut scopes = VecDeque::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            _=shutdown.clone().wait()=>return,
            _=tick.tick()=>{},
        }
        let work = async {
            // One slow scope must not multiply the per-cycle deadline by the
            // batch size. Unprocessed scopes stay queued for the next tick.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            if scopes.is_empty() {
                match tokio::time::timeout_at(deadline, scan_scopes(&mut conn, cursor, 64)).await {
                    Ok(Ok(page)) => {
                        cursor = page.cursor;
                        for key in page.corrupt_keys {
                            warn!(queue_key=%key,"Queue owner metadata requires repair");
                        }
                        scopes.extend(page.scopes);
                    }
                    Ok(Err(error)) => {
                        warn!(error=%error,"Session queue discovery unavailable");
                        return;
                    }
                    Err(_) => {
                        warn!("Session queue discovery timed out; retaining its cursor for retry");
                        return;
                    }
                }
            }
            // Preserve the remaining SCAN page instead of dropping excess scopes.
            // Only deliveries are capped per tick: idle sessions cost one cheap
            // check, so a sweep's length tracks pending work, not session count.
            let mut deliveries = 0;
            while deliveries < 16 && tokio::time::Instant::now() < deadline {
                let Some(scope) = scopes.pop_front() else {
                    break;
                };
                let delivery = async {
                    prune_completed(&mut conn, &scope, 100).await?;
                    if !has_unresolved(&mut conn, &scope).await? {
                        return Ok(false);
                    }
                    deliver_session(&mut conn, &scope, &client)
                        .await
                        .map(|_| true)
                };
                match tokio::time::timeout_at(deadline, delivery).await {
                    Ok(Ok(delivered)) => deliveries += usize::from(delivered),
                    Ok(Err(error)) => {
                        warn!(tenant_id=%scope.tenant_id(),session_id=%scope.session_id(),error=%error,"Session delivery deferred")
                    }
                    Err(_) => {
                        warn!(tenant_id=%scope.tenant_id(),session_id=%scope.session_id(),"Session delivery timed out; its lease will be recovered")
                    }
                }
            }
        };
        tokio::select! { biased; _=shutdown.clone().wait()=>return, _=work=>{} }
    }
}
