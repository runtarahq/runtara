//! Recover queued session delivery even with no attached browser or channel task.
use crate::{
    api::services::session_queue::{
        delivery::deliver_session,
        managed::{DeliveryOutcome, has_unresolved, prune_completed, scan_scopes},
    },
    runtime_client::RuntimeClient,
    shutdown::ShutdownSignal,
};
use redis::aio::ConnectionManager;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::time::Instant;
use tracing::{debug, warn};

/// A problem that persists, such as a corrupt queue, is met on every scan pass.
/// Its warning repeats at most this often; the passes in between log at debug.
const REPEAT_WARNING_AFTER: Duration = Duration::from_secs(60 * 60);
/// Problems last warned about before the window are forgotten once this many
/// are remembered.
const REMEMBERED_PROBLEMS: usize = 1024;

/// One queue's recurring problem. Another error on the same session is a
/// different problem and warns on its own.
#[derive(Debug, PartialEq, Eq, Hash)]
enum Problem {
    CorruptOwner { queue_key: String },
    Deferred { session_id: String, error: String },
    TimedOut { session_id: String },
}

struct WarningLimiter {
    warned: HashMap<Problem, Instant>,
}

impl WarningLimiter {
    fn new() -> Self {
        Self {
            warned: HashMap::new(),
        }
    }

    /// Whether `problem` has gone a whole window without a warning, recording
    /// one now if so.
    fn due(&mut self, problem: Problem) -> bool {
        let now = Instant::now();
        if self
            .warned
            .get(&problem)
            .is_some_and(|warned| now.duration_since(*warned) < REPEAT_WARNING_AFTER)
        {
            return false;
        }
        if self.warned.len() >= REMEMBERED_PROBLEMS {
            self.warned
                .retain(|_, warned| now.duration_since(*warned) < REPEAT_WARNING_AFTER);
        }
        self.warned.insert(problem, now);
        true
    }

    /// Record a sweep of `session_id` that returned `outcome`, or `None` when
    /// nothing was unresolved. A queue found idle or a response accepted shows
    /// the session has recovered, so its next problem is a new incident and
    /// warns again. Busy, deferred and blocked outcomes show nothing: a lease
    /// that a timed-out attempt still holds reads as busy, and forgetting then
    /// would rearm the warning for every lease the attempt leaves behind.
    fn record_sweep(&mut self, session_id: &str, outcome: Option<&DeliveryOutcome>) {
        if matches!(
            outcome,
            Some(
                DeliveryOutcome::Busy | DeliveryOutcome::Deferred(_) | DeliveryOutcome::Blocked(_)
            )
        ) {
            return;
        }
        self.warned.retain(|problem, _| match problem {
            Problem::Deferred { session_id: id, .. } | Problem::TimedOut { session_id: id } => {
                id != session_id
            }
            Problem::CorruptOwner { .. } => true,
        });
    }
}

/// Warn when the limiter says the problem is due, otherwise log at debug.
macro_rules! warn_if_due {
    ($limiter:expr, $problem:expr, $($event:tt)+) => {
        if $limiter.due($problem) {
            warn!($($event)+)
        } else {
            debug!($($event)+)
        }
    };
}

pub async fn run(
    mut conn: ConnectionManager,
    client: Arc<RuntimeClient>,
    pool: sqlx::PgPool,
    tenant_id: String,
    shutdown: ShutdownSignal,
) {
    let mut cursor = 0;
    let mut scopes = VecDeque::new();
    let mut warnings = WarningLimiter::new();
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
                match tokio::time::timeout_at(
                    deadline,
                    scan_scopes(&mut conn, &tenant_id, cursor, 64),
                )
                .await
                {
                    Ok(Ok(page)) => {
                        cursor = page.cursor;
                        for key in page.corrupt_keys {
                            warn_if_due!(
                                warnings,
                                Problem::CorruptOwner {
                                    queue_key: key.clone()
                                },
                                queue_key=%key,"Queue owner metadata requires repair"
                            );
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
                        return Ok(None);
                    }
                    let outcome = deliver_session(&mut conn, &scope, &client).await?;
                    if let DeliveryOutcome::Accepted(envelope) = &outcome
                        && let (Some(actor), Some(target), Some(receipt)) =
                            (&envelope.actor_id, &envelope.target, &envelope.receipt_id)
                    {
                        crate::audit::emit(&pool,scope.tenant_id(),Some(actor),crate::audit::AuditEvent::new("input.answer").resource("instance",&target.instance_id).payload(serde_json::json!({"requestId":target.request_id,"receiptId":receipt}))).await;
                    }
                    Ok::<_, crate::api::services::session_queue::managed::QueueError>(Some(outcome))
                };
                match tokio::time::timeout_at(deadline, delivery).await {
                    Ok(Ok(outcome)) => {
                        warnings.record_sweep(scope.session_id(), outcome.as_ref());
                        deliveries += usize::from(outcome.is_some())
                    }
                    Ok(Err(error)) => warn_if_due!(
                        warnings,
                        Problem::Deferred {
                            session_id: scope.session_id().into(),
                            error: error.to_string()
                        },
                        tenant_id=%scope.tenant_id(),session_id=%scope.session_id(),error=%error,"Session delivery deferred"
                    ),
                    Err(_) => warn_if_due!(
                        warnings,
                        Problem::TimedOut {
                            session_id: scope.session_id().into()
                        },
                        tenant_id=%scope.tenant_id(),session_id=%scope.session_id(),"Session delivery timed out; its lease will be recovered"
                    ),
                }
            }
        };
        tokio::select! { biased; _=shutdown.clone().wait()=>return, _=work=>{} }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::services::session_queue::managed::Envelope;

    fn corrupt(key: &str) -> Problem {
        Problem::CorruptOwner {
            queue_key: key.into(),
        }
    }
    fn envelope() -> Envelope {
        serde_json::from_value(serde_json::json!({
            "message_id": "message",
            "operation_id": "operation",
            "payload_json": "{}",
            "enqueued_at_ms": 0,
            "state": "queued",
            "attempts": 1,
        }))
        .unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn a_recurring_problem_warns_once_per_window() {
        let mut limiter = WarningLimiter::new();
        assert!(limiter.due(corrupt("a")));
        assert!(!limiter.due(corrupt("a")));
        tokio::time::advance(REPEAT_WARNING_AFTER - Duration::from_secs(1)).await;
        assert!(!limiter.due(corrupt("a")));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(limiter.due(corrupt("a")));
        assert!(!limiter.due(corrupt("a")));
    }

    #[tokio::test(start_paused = true)]
    async fn each_queue_and_error_warns_on_its_own() {
        let mut limiter = WarningLimiter::new();
        let deferred = |error: &str| Problem::Deferred {
            session_id: "session".into(),
            error: error.into(),
        };
        assert!(limiter.due(corrupt("a")));
        assert!(limiter.due(corrupt("b")));
        assert!(limiter.due(deferred("queue contains corrupt state")));
        assert!(limiter.due(deferred("queue unavailable")));
        assert!(limiter.due(Problem::TimedOut {
            session_id: "session".into()
        }));
        assert!(!limiter.due(deferred("queue contains corrupt state")));
    }

    #[tokio::test(start_paused = true)]
    async fn a_session_that_recovers_warns_again_on_its_next_problem() {
        let deferred = |session: &str| Problem::Deferred {
            session_id: session.into(),
            error: "queue unavailable".into(),
        };
        let timed_out = |session: &str| Problem::TimedOut {
            session_id: session.into(),
        };
        // Nothing unresolved, an idle queue, and an accepted response.
        for recovery in [
            None,
            Some(DeliveryOutcome::Idle),
            Some(DeliveryOutcome::Accepted(envelope())),
        ] {
            let mut limiter = WarningLimiter::new();
            for problem in [
                deferred("recovered"),
                timed_out("recovered"),
                deferred("other"),
                corrupt("a"),
            ] {
                assert!(limiter.due(problem));
            }
            limiter.record_sweep("recovered", recovery.as_ref());
            assert!(limiter.due(deferred("recovered")));
            assert!(limiter.due(timed_out("recovered")));
            assert!(!limiter.due(deferred("other")));
            assert!(!limiter.due(corrupt("a")));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_lease_held_by_a_timed_out_attempt_does_not_rearm_its_warning() {
        let timed_out = || Problem::TimedOut {
            session_id: "session".into(),
        };
        for inconclusive in [
            DeliveryOutcome::Busy,
            DeliveryOutcome::Deferred(envelope()),
            DeliveryOutcome::Blocked(envelope()),
        ] {
            let mut limiter = WarningLimiter::new();
            assert!(limiter.due(timed_out()));
            // The next sweep cannot tell whether the cause is gone; the
            // timed-out attempt's lease, for one, still reads as busy.
            limiter.record_sweep("session", Some(&inconclusive));
            // The lease expires, and the next attempt times out as well.
            tokio::time::advance(Duration::from_secs(30)).await;
            assert!(!limiter.due(timed_out()));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn expired_problems_are_forgotten_once_enough_are_remembered() {
        let mut limiter = WarningLimiter::new();
        for key in 0..REMEMBERED_PROBLEMS {
            assert!(limiter.due(corrupt(&key.to_string())));
        }
        tokio::time::advance(REPEAT_WARNING_AFTER).await;
        assert!(limiter.due(corrupt("new")));
        assert_eq!(limiter.warned.len(), 1);
    }
}
