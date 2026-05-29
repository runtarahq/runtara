//! Concurrent Executions Gate
//!
//! Per-tenant, Valkey-backed counter that enforces
//! `entitlements.limits.maxConcurrentExecutions` at intake time. Mirrors the
//! same wire contract as the other tier-limit gates: denials produce a 403
//! with `code: "ENTITLEMENT_LIMIT_EXCEEDED"` and the `limit` / `maximum`
//! fields. See `docs/entitlements.md` § Error Model.
//!
//! ## Why Valkey and not Postgres
//!
//! The cap is per-tenant, hot path, and high cardinality (every enqueue
//! checks). A `SELECT COUNT(*)` on `workflow_instances` would add 10–50ms
//! per intake plus pressure the DB; Valkey `ZADD`/`ZCARD` round-trips in
//! ~1ms over the shared `ConnectionManager` we already use for the trigger
//! stream.
//!
//! ## Why a sorted set, not `INCR`/`DECR`
//!
//! The runtara-server has **no callback path** for terminal execution
//! state — executions are dispatched to the external `runtara-environment`
//! runtime and status is poll-only via `RuntimeClient`. That means there's
//! no obvious place to fire a `DECR` when an execution finishes. A simple
//! integer counter would drift upward forever as zombie counts accumulate.
//!
//! A sorted set with `score = enqueue_timestamp_ms` and `member =
//! instance_id` gives us a self-healing release path: every intake runs an
//! inline `ZREMRANGEBYSCORE` to evict entries older than the configurable
//! age-out window (default 1h, well past the longest expected real
//! execution). No background reconciler needed; the count converges within
//! one intake of every cap-checking call.
//!
//! ## Failure mode
//!
//! Valkey unavailability fails **open** — log a warning and let the
//! execution proceed. A new gate that breaks every enqueue on infra hiccup
//! would be worse than the gate being temporarily silent. The decision
//! mirrors the existing pattern in `trigger_worker::process_trigger_event`
//! where the `single_instance` check also fails open on RuntimeClient
//! errors.
//!
//! ## Race resilience
//!
//! Two parallel intakes racing through the pipeline can both observe
//! `count == cap` before either increments — same race the other count-style
//! caps tolerate (`maxApiKeys`, `maxObjectSchemas`). The pipeline used here
//! makes the increment atomic from Valkey's perspective (`ZADD` then `ZCARD`
//! in one round-trip), so each caller sees the *post-add* count. A race can
//! still allow `cap + 1` for one tick before the second caller rolls back,
//! which is acceptable for a soft cap. No `pg_advisory_lock`-style strict
//! mutex; consistent with the rest of the entitlement gates.
//!
//! ## Layout
//!
//! ```text
//! KEY:    runtara:ent:in_flight:{tenant_id}
//! TYPE:   SORTED SET
//! SCORE:  enqueue timestamp (ms since UNIX epoch)
//! MEMBER: workflow instance UUID string
//! TTL:    AGE_OUT_TTL_SECS * 2 on the key itself (belt-and-braces; the
//!         inline ZREMRANGEBYSCORE handles the actual eviction)
//! ```

use redis::AsyncCommands;
use redis::aio::ConnectionManager;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

use crate::entitlement_error::EntitlementDenial;

/// Redis key prefix for per-tenant in-flight execution sorted sets.
pub const KEY_PREFIX: &str = "runtara:ent:in_flight";

/// Default age-out window for sorted-set entries (seconds). Executions
/// older than this are presumed dead (the external runtime has its own
/// timeout shorter than this) and their slot is released. Overridable via
/// `RUNTARA_CONCURRENT_EXECUTION_TTL_SECS`.
pub const DEFAULT_AGE_OUT_TTL_SECS: u64 = 3_600;

/// Get the per-tenant key.
pub fn key_for(tenant_id: &str) -> String {
    format!("{}:{}", KEY_PREFIX, tenant_id)
}

/// Read the configurable age-out window, falling back to the default.
/// Wraps [`parse_age_out_ttl_secs`] with an env lookup so the testable
/// parse logic doesn't depend on process-wide env state.
pub fn age_out_ttl_secs_from_env() -> u64 {
    parse_age_out_ttl_secs(
        std::env::var("RUNTARA_CONCURRENT_EXECUTION_TTL_SECS")
            .ok()
            .as_deref(),
    )
}

/// Pure parser for the age-out TTL env var. Lifted out of
/// [`age_out_ttl_secs_from_env`] so tests can exercise it without racing
/// on `std::env::set_var` against other parallel tests.
///
/// - `None` or unparseable → default.
/// - `Some("0")` → default (a zero TTL would freeze every entry forever).
/// - `Some(N)` with `N > 0` → `N`.
pub fn parse_age_out_ttl_secs(raw: Option<&str>) -> u64 {
    raw.and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_AGE_OUT_TTL_SECS)
}

/// Outcome of an intake attempt. `Acquired` means the execution may
/// proceed; the slot is held in Valkey until age-out. `Denied` carries the
/// entitlement denial the caller must surface to the client.
#[derive(Debug)]
pub enum AcquireOutcome {
    Acquired,
    Denied(EntitlementDenial),
}

/// Per-tenant in-flight execution gate.
///
/// Wraps the shared Valkey `ConnectionManager` plus the configured age-out
/// window. Construct one at startup from the same manager used by
/// `TriggerStreamPublisher` and clone freely — the underlying connection
/// is reference-counted.
#[derive(Clone)]
pub struct ConcurrentExecutionGate {
    manager: ConnectionManager,
    /// Age-out window in seconds. Captured at construction so per-intake
    /// hot paths don't re-read env. See [`parse_age_out_ttl_secs`].
    age_out_ttl_secs: u64,
}

impl ConcurrentExecutionGate {
    /// Build a gate from a shared connection manager and explicit TTL.
    /// Tests should use this form; production should use [`Self::from_env`].
    pub fn new(manager: ConnectionManager, age_out_ttl_secs: u64) -> Self {
        Self {
            manager,
            age_out_ttl_secs,
        }
    }

    /// Build a gate with the TTL read from `RUNTARA_CONCURRENT_EXECUTION_TTL_SECS`,
    /// falling back to [`DEFAULT_AGE_OUT_TTL_SECS`].
    pub fn from_env(manager: ConnectionManager) -> Self {
        Self::new(manager, age_out_ttl_secs_from_env())
    }

    /// Atomic intake: age out stale entries, add this execution, count the
    /// post-add total, decide.
    ///
    /// `cap` is the **effective** cap (already composed via
    /// `effective_limit(infra, snapshot.limits.max_concurrent_executions)`).
    ///
    /// Returns:
    /// - `Ok(AcquireOutcome::Acquired)` if `cap == 0` or if the post-add count is within cap. The slot is
    ///   held in Valkey until age-out.
    /// - `Ok(AcquireOutcome::Denied(denial))` if the post-add count would
    ///   exceed `cap`. The speculative `ZADD` is rolled back via `ZREM`
    ///   before returning. Caller surfaces `denial` as `403
    ///   ENTITLEMENT_LIMIT_EXCEEDED`.
    /// - `Err` only on unexpected Valkey errors that the caller should
    ///   audit; the standard behavior is fail-open ([`try_acquire`]
    ///   handles this translation).
    pub async fn try_acquire_strict(
        &self,
        tenant_id: &str,
        instance_id: &str,
        cap: usize,
    ) -> redis::RedisResult<AcquireOutcome> {
        // `cap == 0` is "fully disabled" — no executions permitted at all,
        // consistent with how every other numeric tier limit treats a zero
        // cap (`limit_decision` with `Some(0)` rejects immediately; see its
        // unit tests). Deny without touching Redis: the answer can't depend
        // on the live count when the cap is zero, and a suspended tenant
        // being hammered shouldn't generate ZADD/ZREM churn.
        //
        // Note: the no-cap *default* never reaches here as 0 —
        // `effective_limit(infra, None)` returns the infra cap
        // (`num_cpus * 32`), so cap is only 0 when an entitlement (or the
        // infra knob) explicitly sets it to 0.
        if cap == 0 {
            return Ok(AcquireOutcome::Denied(EntitlementDenial::LimitExceeded {
                limit: "maxConcurrentExecutions",
                maximum: 0,
            }));
        }

        let key = key_for(tenant_id);
        let now_ms = current_unix_ms();
        let cutoff_ms = now_ms.saturating_sub((self.age_out_ttl_secs as i64) * 1_000);
        let key_ttl_secs = (self.age_out_ttl_secs * 2).min(86_400) as i64;

        // Pipeline: age out, add, count, refresh key TTL — one round trip.
        let mut conn = self.manager.clone();
        let (_pruned, _added, new_count, _ttl_set): (i64, i64, i64, bool) = redis::pipe()
            .zrembyscore(&key, "-inf", format!("({}", cutoff_ms))
            .zadd(&key, instance_id, now_ms)
            .zcard(&key)
            .expire(&key, key_ttl_secs)
            .query_async(&mut conn)
            .await?;

        if (new_count as u64) > cap as u64 {
            // Rollback: remove the speculative add so the next caller sees
            // the real count.
            let _: i64 = conn.zrem(&key, instance_id).await.unwrap_or(0);
            return Ok(AcquireOutcome::Denied(EntitlementDenial::LimitExceeded {
                limit: "maxConcurrentExecutions",
                maximum: cap as u64,
            }));
        }

        Ok(AcquireOutcome::Acquired)
    }

    /// Fail-open wrapper around [`try_acquire_strict`]. Valkey errors are
    /// logged at WARN and converted to `Acquired` so a Valkey outage doesn't
    /// silently break every enqueue path. The gate stops enforcing during
    /// the outage; everything else keeps working.
    pub async fn try_acquire(
        &self,
        tenant_id: &str,
        instance_id: &str,
        cap: usize,
    ) -> AcquireOutcome {
        match self.try_acquire_strict(tenant_id, instance_id, cap).await {
            Ok(outcome) => outcome,
            Err(e) => {
                warn!(
                    tenant_id = %tenant_id,
                    instance_id = %instance_id,
                    cap = cap,
                    error = %e,
                    "concurrent_execution_gate: Valkey error, failing open"
                );
                AcquireOutcome::Acquired
            }
        }
    }

    /// Explicit release. The gate's primary release mechanism is age-out
    /// (see module docs), but callers that *do* observe terminal state for
    /// an instance can release the slot eagerly to free it before age-out
    /// would. Best-effort; Valkey errors are logged but not surfaced.
    ///
    /// Currently no caller in runtara-server has a reliable terminal-state
    /// hook for async executions, so this method is provided for future
    /// use (e.g., a sync-execute path that observes completion, or a
    /// callback added later).
    pub async fn release(&self, tenant_id: &str, instance_id: &str) {
        let key = key_for(tenant_id);
        let mut conn = self.manager.clone();
        if let Err(e) = conn.zrem::<_, _, i64>(&key, instance_id).await {
            warn!(
                tenant_id = %tenant_id,
                instance_id = %instance_id,
                error = %e,
                "concurrent_execution_gate: failed to release slot"
            );
        }
    }
}

fn current_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_for_includes_tenant() {
        assert_eq!(key_for("tenant_abc"), "runtara:ent:in_flight:tenant_abc");
    }

    #[test]
    fn key_for_uses_documented_prefix() {
        let k = key_for("x");
        assert!(
            k.starts_with(KEY_PREFIX),
            "key must start with {KEY_PREFIX}"
        );
    }

    // Pure parser tests — no env mutation, safe under parallel test runs.

    #[test]
    fn parse_age_out_ttl_secs_falls_back_when_missing() {
        assert_eq!(parse_age_out_ttl_secs(None), DEFAULT_AGE_OUT_TTL_SECS);
    }

    #[test]
    fn parse_age_out_ttl_secs_falls_back_when_unparseable() {
        assert_eq!(
            parse_age_out_ttl_secs(Some("not-a-number")),
            DEFAULT_AGE_OUT_TTL_SECS
        );
        assert_eq!(parse_age_out_ttl_secs(Some("")), DEFAULT_AGE_OUT_TTL_SECS);
    }

    #[test]
    fn parse_age_out_ttl_secs_rejects_zero() {
        assert_eq!(
            parse_age_out_ttl_secs(Some("0")),
            DEFAULT_AGE_OUT_TTL_SECS,
            "TTL=0 would freeze every entry forever; fall back to the default instead"
        );
    }

    #[test]
    fn parse_age_out_ttl_secs_accepts_positive_value() {
        assert_eq!(parse_age_out_ttl_secs(Some("120")), 120);
        assert_eq!(parse_age_out_ttl_secs(Some("86400")), 86_400);
    }
}
