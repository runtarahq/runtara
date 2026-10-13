// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Automatic recovery of guest instances killed by an Environment restart.
//!
//! When the Environment restarts — a graceful drain OR an abrupt kill — an
//! in-process guest dies with it. The graceful drain already suspends the
//! instance (`shutdown_requested` + `sleep_until=now`) so the wake scheduler
//! relaunches it. The abrupt path leaves the instance `running` in Core, and on
//! the next startup the orphan scans ([`crate::runtime`] startup recovery and
//! [`crate::heartbeat_monitor`]) would otherwise mark it terminally `failed`
//! with "Process terminated during Environment restart" — never relaunching it.
//!
//! [`recover_registered`] and [`recover_orphaned`] route those would-be
//! failures into the SAME
//! suspend → wake → relaunch path the drain uses. The engine is
//! replay-from-start with checkpoints as a result cache, so a relaunched
//! instance replays from the entry point and completed durable steps are served
//! from cache — i.e. "resume from checkpoint" and "auto re-queue" in one move.
//!
//! A crash-loop cap (`RUNTARA_MAX_AUTO_RESTARTS`) bounds instances that crash
//! before making progress. The cap counts only CONSECUTIVE no-progress
//! restarts: the counter resets whenever the instance's checkpoint progress
//! (`Persistence::checkpoint_progress`) advances between recoveries, so a
//! genuinely long-running workflow survives any number of restarts.

use crate::config::{ProcessEnv, Vars, parse_enabled, positive};
use runtara_core::persistence::{CompleteInstanceParams, Persistence};
use tracing::{info, warn};

use crate::error::Result;

/// Default maximum number of CONSECUTIVE no-progress auto-restarts before an
/// instance is failed terminally. Override with `RUNTARA_MAX_AUTO_RESTARTS`.
pub const DEFAULT_MAX_AUTO_RESTARTS: i32 = 5;

/// Read the configured crash-loop cap (`RUNTARA_MAX_AUTO_RESTARTS`, default
/// [`DEFAULT_MAX_AUTO_RESTARTS`]). Values below 1 fall back to the default.
pub fn max_auto_restarts() -> i32 {
    max_auto_restarts_from(&ProcessEnv)
}

/// [`max_auto_restarts`] against a supplied set of values.
fn max_auto_restarts_from(vars: &dyn Vars) -> i32 {
    positive(vars, "RUNTARA_MAX_AUTO_RESTARTS", DEFAULT_MAX_AUTO_RESTARTS)
}

/// Operator-level kill switch for automatic restart recovery. Set
/// `RUNTARA_AUTO_RECOVER` to any of `false`/`0`/`no`/`off`/`disabled` to turn
/// auto-recovery off for the whole Environment — instances killed by a restart
/// then fail terminally with the `environment_restart` reason instead of being
/// relaunched. Defaults to on, matching the always-on graceful-drain recovery.
///
/// Shares [`parse_enabled`](crate::config::parse_enabled) with
/// the `*_CLEANUP_ENABLED` opt-outs so every switch in the crate answers to the
/// same spellings; a hand-rolled parser here used to ignore `off`.
pub fn auto_recover_enabled() -> bool {
    auto_recover_enabled_from(&ProcessEnv)
}

/// [`auto_recover_enabled`] against a supplied set of values.
fn auto_recover_enabled_from(vars: &dyn Vars) -> bool {
    parse_enabled(vars.get("RUNTARA_AUTO_RECOVER").as_deref())
}

/// The Environment-wide restart-recovery settings, applied identically to
/// every orphaned instance. There is no per-workflow override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryPolicy {
    /// See [`auto_recover_enabled`].
    pub auto_recover: bool,
    /// See [`max_auto_restarts`].
    pub max_auto_restarts: i32,
}

impl RecoveryPolicy {
    /// Read `RUNTARA_AUTO_RECOVER` and `RUNTARA_MAX_AUTO_RESTARTS`.
    pub fn from_env() -> Self {
        Self::from_vars(&ProcessEnv)
    }

    /// [`RecoveryPolicy::from_env`] against a supplied set of values.
    fn from_vars(vars: &dyn Vars) -> Self {
        Self {
            auto_recover: auto_recover_enabled_from(vars),
            max_auto_restarts: max_auto_restarts_from(vars),
        }
    }
}

/// What an Environment with neither variable set runs with.
impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self {
            auto_recover: true,
            max_auto_restarts: DEFAULT_MAX_AUTO_RESTARTS,
        }
    }
}

/// Outcome of a recovery decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryOutcome {
    /// Instance was marked for recovery (suspended + due to wake).
    Recovered,
    /// Crash-loop cap exceeded, or auto-recovery disabled: instance failed
    /// terminally with `termination_reason = environment_restart`.
    Failed,
    /// Another lifecycle transition won before the recovery write. No recovery
    /// state was applied and the caller must not claim a restart or failure.
    Unchanged,
}

/// Recover one observed physical registration only after locking its durable
/// launch. Holding that lock across the Core write prevents queue reconciliation
/// and a new start from replacing the generation midway through recovery.
/// None means a live claim or a replacement owns the observation now.
///
/// Whether a recovered instance is relaunched or failed follows the
/// Environment-wide [`RecoveryPolicy`].
pub async fn recover_registered(
    pool: &sqlx::PgPool,
    persistence: &dyn Persistence,
    container: &crate::container_registry::ContainerInfo,
) -> Result<Option<RecoveryOutcome>> {
    recover_registered_with(pool, persistence, container, RecoveryPolicy::from_env()).await
}

/// [`recover_registered`] with the policy supplied rather than read from the
/// process environment.
pub async fn recover_registered_with(
    pool: &sqlx::PgPool,
    persistence: &dyn Persistence,
    container: &crate::container_registry::ContainerInfo,
    policy: RecoveryPolicy,
) -> Result<Option<RecoveryOutcome>> {
    let mut guard = pool.begin().await?;
    let launch_state: Option<String> =
        sqlx::query_scalar("SELECT state FROM instance_launches WHERE launch_id = $1 FOR UPDATE")
            .bind(&container.launch_id)
            .fetch_optional(&mut *guard)
            .await?;
    if let Some(state) = launch_state {
        // Evaluate time in a fresh statement after acquiring the row lock.
        let live: bool = sqlx::query_scalar(
            "SELECT lease_owner IS NOT NULL AND \
             COALESCE(lease_expires_at > clock_timestamp(), false) \
             FROM instance_launches WHERE launch_id = $1",
        )
        .bind(&container.launch_id)
        .fetch_one(&mut *guard)
        .await?;
        if live || !matches!(state.as_str(), "running" | "starting") {
            return Ok(None);
        }
    }
    // Legacy registrations may predate the launch queue. Their prior recovery
    // behavior remains; they do not establish cross-version owner liveness.
    let registered: Option<(Option<sqlx::types::Json<crate::observed_exit::ObservedExit>>,)> =
        sqlx::query_as(
            "SELECT observed_exit FROM container_registry \
         WHERE instance_id = $1 AND launch_id = $2 AND container_id = $3 FOR UPDATE",
        )
        .bind(&container.instance_id)
        .bind(&container.launch_id)
        .bind(&container.container_id)
        .fetch_optional(&mut *guard)
        .await?;
    let Some((observed_exit,)) = registered else {
        return Ok(None);
    };
    // An observed physical failure is not a lost Environment process. Complete
    // its retained lifecycle transition before considering normal auto-resume.
    let outcome = if let Some(sqlx::types::Json(intent)) = observed_exit {
        if intent
            .apply(pool, persistence, &container.instance_id)
            .await?
        {
            if intent.is_drain() {
                RecoveryOutcome::Recovered
            } else {
                RecoveryOutcome::Failed
            }
        } else {
            RecoveryOutcome::Unchanged
        }
    } else {
        match persistence
            .get_instance_meta(&container.instance_id)
            .await?
        {
            Some(instance) if instance.status == runtara_core::domain::InstanceStatus::Running => {
                let outcome =
                    recover_or_fail_with(pool, persistence, &container.instance_id, policy).await?;
                if outcome == RecoveryOutcome::Unchanged {
                    return Ok(Some(outcome));
                }
                outcome
            }
            Some(instance) if instance.status == runtara_core::domain::InstanceStatus::Pending => {
                // The durable start-gate/queue expiry path owns an unopened run.
                return Ok(None);
            }
            _ => RecoveryOutcome::Unchanged,
        }
    };
    sqlx::query(
        "DELETE FROM container_registry \
         WHERE instance_id = $1 AND launch_id = $2 AND container_id = $3",
    )
    .bind(&container.instance_id)
    .bind(&container.launch_id)
    .bind(&container.container_id)
    .execute(&mut *guard)
    .await?;
    guard.commit().await?;
    Ok(Some(outcome))
}

/// Recover an instance Core shows `running` that no registration tracks, only
/// after re-checking under locks that it is still orphaned.
///
/// The orphan scan's snapshot is not enough to act on. Core keeps an instance's
/// first `started_at` across generations, so the scan's age cutoff says nothing
/// about how fresh the current generation is, and the `if_running` guard on the
/// recovery write cannot tell the orphaned run from a newer one. Between the
/// scan and the write, the instance can park and be relaunched, or another
/// scanner sharing the database can recover it and the wake scheduler relaunch
/// it. Writing then would suspend or fail a generation that a runner owns.
///
/// So, inside one transaction held across the Core write:
/// - a per-instance advisory lock serializes orphan recovery, so a second
///   scanner re-checks only after the first one's write has committed;
/// - an active launch with a live lease is left to its owner, as in
///   [`recover_registered`];
/// - Core must still be `running`, and only then must no registration exist.
///   That order matters: a generation registers before it is promoted to
///   `running`, so one promoted between the two reads is always seen.
///
/// None means the instance is no longer an orphan and nothing was written.
pub async fn recover_orphaned(
    pool: &sqlx::PgPool,
    persistence: &dyn Persistence,
    instance_id: &str,
) -> Result<Option<RecoveryOutcome>> {
    recover_orphaned_with(pool, persistence, instance_id, RecoveryPolicy::from_env()).await
}

/// [`recover_orphaned`] with the policy supplied rather than read from the
/// process environment.
pub async fn recover_orphaned_with(
    pool: &sqlx::PgPool,
    persistence: &dyn Persistence,
    instance_id: &str,
    policy: RecoveryPolicy,
) -> Result<Option<RecoveryOutcome>> {
    let mut guard = pool.begin().await?;
    sqlx::query(ORPHAN_RECOVERY_LOCK)
        .bind(instance_id)
        .execute(&mut *guard)
        .await?;
    let active_states =
        crate::launch_queue::LaunchState::sql_list(crate::launch_queue::LaunchState::ACTIVE);
    let active_launch: Option<String> = sqlx::query_scalar(&format!(
        "SELECT launch_id FROM instance_launches \
         WHERE instance_id = $1 AND state IN ({active_states}) FOR UPDATE"
    ))
    .bind(instance_id)
    .fetch_optional(&mut *guard)
    .await?;
    if let Some(launch_id) = active_launch {
        // Evaluate time in a fresh statement after acquiring the row lock.
        let live: bool = sqlx::query_scalar(
            "SELECT lease_owner IS NOT NULL AND \
             COALESCE(lease_expires_at > clock_timestamp(), false) \
             FROM instance_launches WHERE launch_id = $1",
        )
        .bind(&launch_id)
        .fetch_one(&mut *guard)
        .await?;
        if live {
            return Ok(None);
        }
    }
    match persistence.get_instance_meta(instance_id).await? {
        Some(instance) if instance.status == runtara_core::domain::InstanceStatus::Running => {}
        _ => return Ok(None),
    }
    let registered: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM container_registry WHERE instance_id = $1)",
    )
    .bind(instance_id)
    .fetch_one(&mut *guard)
    .await?;
    if registered {
        return Ok(None);
    }
    let outcome = recover_or_fail_with(pool, persistence, instance_id, policy).await?;
    guard.commit().await?;
    Ok(Some(outcome))
}

/// The per-instance lock [`recover_orphaned_with`] holds for its transaction.
const ORPHAN_RECOVERY_LOCK: &str =
    "SELECT pg_advisory_xact_lock(hashtextextended('runtara:orphan-recovery' || chr(31) || $1, 0))";

/// Pure crash-loop decision, separated from any I/O so it can be unit-tested.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Decision {
    /// Recover, recording this 1-based consecutive-no-progress attempt number.
    Recover { attempt: i32 },
    /// Fail terminally with this operator-facing error string.
    Fail { error: String },
}

/// Decide whether to recover, given the prior counters, the current progress
/// fingerprint (checkpoint count), the cap, and the Environment-wide
/// auto-recovery switch.
///
/// The counter resets to 1 when progress advanced since the last recovery (or
/// there was no prior recovery); otherwise it increments. Once it would exceed
/// `cap`, or auto-recovery is disabled, the instance fails.
fn decide(
    prev_attempts: i32,
    prev_marker: Option<&str>,
    progress: i64,
    cap: i32,
    auto_recover: bool,
) -> Decision {
    let made_progress = prev_marker
        .and_then(|m| m.parse::<i64>().ok())
        .map(|p| progress > p)
        .unwrap_or(true);
    let attempt = if made_progress { 1 } else { prev_attempts + 1 };

    if !auto_recover {
        return Decision::Fail {
            error: "Killed by Environment restart; automatic recovery is disabled for this \
                    Environment (RUNTARA_AUTO_RECOVER)"
                .to_string(),
        };
    }
    if attempt > cap {
        return Decision::Fail {
            error: format!(
                "Killed by Environment restart; exceeded automatic restart limit ({cap})"
            ),
        };
    }
    Decision::Recover { attempt }
}

/// Why a still-`running` instance is being handed to recovery. It names the
/// suspension (or failure) recovery records, and nothing else differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryCause {
    /// The Environment restarted or lost the execution.
    EnvironmentRestart,
    /// The run exited to suspend, but its park could not be committed.
    ParkFailed,
}

impl RecoveryCause {
    /// The `termination_reason` recovery records.
    pub fn termination_reason(self) -> &'static str {
        match self {
            Self::EnvironmentRestart => "environment_restart",
            Self::ParkFailed => "park_failed",
        }
    }
}

/// The restart recovery decision for a given cause, under the Environment's
/// policy. The crash-loop cap applies to every cause.
///
/// Like every recovery write, it applies only while Core shows the instance
/// `running`, which does not identify a generation. The caller must hold the
/// instance's launch and registration locks, as `observed_exit` does.
pub async fn recover_or_fail_because(
    pool: &sqlx::PgPool,
    persistence: &dyn Persistence,
    instance_id: &str,
    cause: RecoveryCause,
) -> Result<RecoveryOutcome> {
    recover_or_fail_for(
        pool,
        persistence,
        instance_id,
        RecoveryPolicy::from_env(),
        cause,
    )
    .await
}

/// Decide whether to auto-recover an instance that was killed by an Environment
/// restart, or fail it terminally, under the supplied policy.
///
/// On `Recovered`, the wake scheduler relaunches the instance on its next poll.
/// On `Failed`, the instance is left in a terminal state with a clear
/// operator-facing reason. Recovery is governed by the Environment-wide
/// [`RecoveryPolicy`]; there is no per-workflow override, so every orphaned
/// instance in this Environment gets the same answer.
///
/// The write applies only while Core shows the instance `running`, and that
/// guard cannot tell the orphaned run from a newer generation. So this is not
/// an entry point: callers establish that the instance is still orphaned, under
/// locks held across this call, as [`recover_registered`] and
/// [`recover_orphaned`] do. A concurrent lifecycle transition returns
/// `Unchanged`; a failed write returns an error, so callers do not retire
/// tracking as though it succeeded.
async fn recover_or_fail_with(
    pool: &sqlx::PgPool,
    persistence: &dyn Persistence,
    instance_id: &str,
    policy: RecoveryPolicy,
) -> Result<RecoveryOutcome> {
    recover_or_fail_for(
        pool,
        persistence,
        instance_id,
        policy,
        RecoveryCause::EnvironmentRestart,
    )
    .await
}

pub(crate) async fn recover_or_fail_for(
    pool: &sqlx::PgPool,
    persistence: &dyn Persistence,
    instance_id: &str,
    policy: RecoveryPolicy,
    cause: RecoveryCause,
) -> Result<RecoveryOutcome> {
    let RecoveryPolicy {
        auto_recover,
        max_auto_restarts: cap,
    } = policy;
    // Progress fingerprint: rises with every checkpoint the instance stores,
    // and does not fall when a composite step's result deletes the rows inside
    // it, as a row count would. A higher value than at the last recovery means
    // the instance made forward progress across the restart.
    let progress = persistence
        .checkpoint_progress(instance_id)
        .await
        .unwrap_or(0);
    let marker = progress.to_string();

    // Prior crash-loop counters (best-effort; treat read failure as a fresh
    // instance so we err toward recovering rather than failing).
    let (prev_attempts, prev_marker) = match persistence.get_instance_meta(instance_id).await {
        Ok(Some(inst)) => (inst.recovery_attempts, inst.recovery_marker),
        _ => (0, None),
    };

    match decide(
        prev_attempts,
        prev_marker.as_deref(),
        progress,
        cap,
        auto_recover,
    ) {
        Decision::Fail { error: err } => {
            let applied = persistence
                .complete_instance(
                    CompleteInstanceParams::new(
                        instance_id,
                        runtara_core::domain::InstanceStatus::Failed,
                    )
                    .if_running()
                    .with_termination(cause.termination_reason(), None)
                    .with_error(&err),
                )
                .await?;
            if !applied {
                return Ok(RecoveryOutcome::Unchanged);
            }
            warn!(
                instance_id = %instance_id,
                cap,
                auto_recover,
                "Instance NOT auto-recovered after Environment restart"
            );
            Ok(RecoveryOutcome::Failed)
        }
        Decision::Recover { attempt } => {
            let applied = crate::instance_repository::InstanceRepository::new(pool.clone())
                .mark_for_recovery(instance_id, attempt, Some(&marker), cause)
                .await?;
            if !applied {
                return Ok(RecoveryOutcome::Unchanged);
            }
            info!(
                instance_id = %instance_id,
                attempt,
                cap,
                progress,
                "Marked instance for automatic recovery after Environment restart (wake scheduler will relaunch)"
            );
            Ok(RecoveryOutcome::Recovered)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Decision, RecoveryPolicy, auto_recover_enabled_from, decide, max_auto_restarts_from,
    };
    use crate::config::FixedVars;

    #[test]
    fn first_recovery_starts_at_one() {
        // No prior marker → treated as forward progress → attempt 1.
        assert_eq!(
            decide(0, None, 0, 5, true),
            Decision::Recover { attempt: 1 }
        );
    }

    #[test]
    fn no_progress_increments_toward_cap() {
        // Same checkpoint count as last recovery → no progress → increment.
        assert_eq!(
            decide(1, Some("0"), 0, 5, true),
            Decision::Recover { attempt: 2 }
        );
        assert_eq!(
            decide(4, Some("0"), 0, 5, true),
            Decision::Recover { attempt: 5 }
        );
    }

    #[test]
    fn exceeding_cap_fails() {
        // attempt would be 6 > cap 5 → fail terminally.
        match decide(5, Some("0"), 0, 5, true) {
            Decision::Fail { error } => assert!(error.contains("restart limit (5)")),
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    #[test]
    fn forward_progress_resets_counter() {
        // Checkpoint count advanced (3 > 0) since last recovery → reset to 1
        // even though prev_attempts was at the cap. A long-running workflow
        // that keeps making progress recovers unboundedly.
        assert_eq!(
            decide(5, Some("0"), 3, 5, true),
            Decision::Recover { attempt: 1 }
        );
    }

    #[test]
    fn disabled_auto_recover_always_fails() {
        match decide(0, None, 10, 5, false) {
            Decision::Fail { error } => assert_eq!(
                error,
                "Killed by Environment restart; automatic recovery is disabled for this \
                 Environment (RUNTARA_AUTO_RECOVER)"
            ),
            other => panic!("expected Fail, got {other:?}"),
        }
    }

    /// The cap rejects anything that would disable crash-loop protection or
    /// make it nonsensical, rather than honouring it.
    ///
    /// A cap of zero fails every restarted instance on its first attempt, which
    /// reads as "recovery is broken" rather than as a configured policy; the
    /// switch for turning recovery off is RUNTARA_AUTO_RECOVER.
    #[test]
    fn the_restart_cap_falls_back_on_anything_non_positive() {
        for value in ["0", "-1", "", "  ", "lots"] {
            let vars = FixedVars::new([("RUNTARA_MAX_AUTO_RESTARTS", value)]);
            assert_eq!(max_auto_restarts_from(&vars), 5, "{value:?}");
        }

        assert_eq!(max_auto_restarts_from(&FixedVars::empty()), 5);
        assert_eq!(
            max_auto_restarts_from(&FixedVars::new([("RUNTARA_MAX_AUTO_RESTARTS", "12")])),
            12
        );
    }

    /// The kill switch answers to the same spellings as every other switch in
    /// the crate; a hand-rolled parser here used to ignore `off`.
    #[test]
    fn auto_recovery_is_on_unless_explicitly_disabled() {
        assert!(auto_recover_enabled_from(&FixedVars::empty()));

        for value in ["false", "0", "no", "off", "disabled", "Off", "  FALSE  "] {
            let vars = FixedVars::new([("RUNTARA_AUTO_RECOVER", value)]);
            assert!(!auto_recover_enabled_from(&vars), "{value:?}");
        }

        for value in ["true", "1", "yes", "on", "typo"] {
            let vars = FixedVars::new([("RUNTARA_AUTO_RECOVER", value)]);
            assert!(auto_recover_enabled_from(&vars), "{value:?}");
        }
    }

    /// The policy reads both variables, and an unset environment yields the
    /// same policy as `Default`.
    #[test]
    fn policy_reads_both_variables_and_defaults_match() {
        assert_eq!(
            RecoveryPolicy::from_vars(&FixedVars::empty()),
            RecoveryPolicy::default()
        );
        assert_eq!(
            RecoveryPolicy::from_vars(&FixedVars::new([
                ("RUNTARA_AUTO_RECOVER", "off"),
                ("RUNTARA_MAX_AUTO_RESTARTS", "9"),
            ])),
            RecoveryPolicy {
                auto_recover: false,
                max_auto_restarts: 9,
            }
        );
    }
}

#[cfg(all(test, feature = "db-integration-tests"))]
mod persistence_tests {
    use super::{
        ORPHAN_RECOVERY_LOCK, RecoveryCause, RecoveryOutcome, RecoveryPolicy, recover_or_fail_for,
        recover_or_fail_with, recover_orphaned_with,
    };
    use crate::instance_repository::InstanceRepository;
    use runtara_store_postgres::PostgresPersistence;

    fn policy(auto_recover: bool) -> RecoveryPolicy {
        RecoveryPolicy {
            auto_recover,
            ..RecoveryPolicy::default()
        }
    }

    async fn snapshot(pool: &sqlx::PgPool, id: &str) -> serde_json::Value {
        sqlx::query_scalar(
            "SELECT jsonb_build_object(\
             'status', status, 'termination_reason', termination_reason, \
             'sleep_until', sleep_until, 'wake_reason', wake_reason, \
             'finished_at', finished_at, 'output', output, 'error', error, \
             'recovery_attempts', recovery_attempts, 'recovery_marker', recovery_marker) \
             FROM instances WHERE instance_id = $1",
        )
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("read lifecycle snapshot")
    }

    #[tokio::test]
    async fn recovery_write_preserves_states_that_changed_after_scan() {
        let pool = crate::test_support::pool().await;
        let repository = InstanceRepository::new(pool.clone());
        for status in ["cancelled", "completed", "failed", "suspended", "pending"] {
            let id = crate::test_support::unique_id("recovery-stale-scan");
            sqlx::query(
                "INSERT INTO instances (instance_id, tenant_id, status) \
                 VALUES ($1, 'recovery-test', 'running')",
            )
            .bind(&id)
            .execute(&pool)
            .await
            .expect("create running instance");
            // A recovery scan observed Running, but another lifecycle writer
            // settles/parks/requeues it before the recovery UPDATE executes.
            assert_eq!(snapshot(&pool, &id).await["status"], "running");
            sqlx::query(
                "UPDATE instances SET status = $2::instance_status, \
                 termination_reason = 'cancelled', finished_at = NOW(), \
                 sleep_until = NOW() + INTERVAL '1 hour', \
                 recovery_attempts = 3, recovery_marker = '7', \
                 output = $3, error = 'accepted outcome' WHERE instance_id = $1",
            )
            .bind(&id)
            .bind(status)
            .bind(b"accepted result".as_slice())
            .execute(&pool)
            .await
            .expect("settle after scan");
            let before = snapshot(&pool, &id).await;
            assert!(
                !repository
                    .mark_for_recovery(&id, 4, Some("8"), RecoveryCause::EnvironmentRestart)
                    .await
                    .expect("attempt stale recovery")
            );
            assert_eq!(snapshot(&pool, &id).await, before, "state {status}");
            let persistence = PostgresPersistence::new(pool.clone());
            for auto_recover in [true, false] {
                assert_eq!(
                    recover_or_fail_with(&pool, &persistence, &id, policy(auto_recover))
                        .await
                        .expect("resolve stale recovery"),
                    RecoveryOutcome::Unchanged
                );
                assert_eq!(snapshot(&pool, &id).await, before, "state {status}");
            }
        }
    }

    /// Recording a composite step's result deletes the rows inside it, so a
    /// run can hold fewer checkpoints after moving forward. A restart after
    /// that is progress, not a step toward the crash-loop cap.
    #[tokio::test]
    async fn rows_collapsed_between_restarts_still_count_as_progress() {
        use runtara_core::persistence::Persistence;
        let pool = crate::test_support::pool().await;
        let persistence = PostgresPersistence::new(pool.clone());
        let id = crate::test_support::unique_id("recovery-collapse");
        sqlx::query(
            "INSERT INTO instances (instance_id, tenant_id, status) \
             VALUES ($1, 'recovery-test', 'running')",
        )
        .bind(&id)
        .execute(&pool)
        .await
        .expect("create running instance");
        let key = |kind: &str, loops: serde_json::Value, parts: serde_json::Value| {
            format!(
                "runtara:v2:{}",
                serde_json::json!([kind, "wf", [], loops, parts])
            )
        };
        // Killed mid-Split, with every finished iteration checkpointed.
        for i in 0..20 {
            let item = key(
                "agent",
                serde_json::json!([["Split", "s", i]]),
                serde_json::json!(["u", "c", "x"]),
            );
            persistence.save_checkpoint(&id, &item, b"x").await.unwrap();
        }
        assert_eq!(
            recover_or_fail_with(&pool, &persistence, &id, policy(true))
                .await
                .expect("first recovery"),
            RecoveryOutcome::Recovered
        );

        // The relaunch finishes the Split: its result is recorded and the
        // rows inside it are deleted. Then it is killed again.
        sqlx::query(
            "UPDATE instances SET status = 'running', sleep_until = NULL, wake_reason = NULL \
             WHERE instance_id = $1",
        )
        .bind(&id)
        .execute(&pool)
        .await
        .expect("relaunch");
        let split = key("split", serde_json::json!([]), serde_json::json!(["s"]));
        persistence
            .save_checkpoint(&id, &split, b"all")
            .await
            .unwrap();
        sqlx::query("DELETE FROM checkpoints WHERE instance_id = $1 AND checkpoint_id <> $2")
            .bind(&id)
            .bind(&split)
            .execute(&pool)
            .await
            .expect("collapse the Split's iterations");

        assert_eq!(
            recover_or_fail_with(&pool, &persistence, &id, policy(true))
                .await
                .expect("second recovery"),
            RecoveryOutcome::Recovered
        );
        assert_eq!(
            snapshot(&pool, &id).await["recovery_attempts"],
            1,
            "progress across the restart resets the crash-loop counter"
        );
    }

    #[tokio::test]
    async fn recovery_reports_applied_outcome_once() {
        let pool = crate::test_support::pool().await;
        let persistence = PostgresPersistence::new(pool.clone());
        for auto_recover in [true, false] {
            let id = crate::test_support::unique_id("recovery-applied");
            sqlx::query(
                "INSERT INTO instances (instance_id, tenant_id, status) \
                 VALUES ($1, 'recovery-test', 'running')",
            )
            .bind(&id)
            .execute(&pool)
            .await
            .expect("create running instance");
            assert_eq!(
                recover_or_fail_with(&pool, &persistence, &id, policy(auto_recover))
                    .await
                    .expect("apply recovery decision"),
                if auto_recover {
                    RecoveryOutcome::Recovered
                } else {
                    RecoveryOutcome::Failed
                }
            );
            let after = snapshot(&pool, &id).await;
            assert_eq!(
                after["status"],
                if auto_recover { "suspended" } else { "failed" }
            );
            assert_eq!(after["termination_reason"], "environment_restart");
            if auto_recover {
                assert!(!after["sleep_until"].is_null());
                // The wake scheduler refuses a wake launch without a reason.
                assert_eq!(after["wake_reason"], "recovery");
                assert_eq!(after["recovery_attempts"], 1);
            }
            assert_eq!(
                recover_or_fail_with(&pool, &persistence, &id, policy(auto_recover))
                    .await
                    .expect("repeat recovery decision"),
                RecoveryOutcome::Unchanged
            );
            assert_eq!(snapshot(&pool, &id).await, after);
        }
    }

    /// A run whose park never committed is suspended for recovery under its
    /// own cause, and the crash-loop cap still bounds it: a store that keeps
    /// refusing the park ends in a terminal failure, not an endless loop.
    #[tokio::test]
    async fn an_unparked_suspension_recovers_under_its_cause_until_the_cap() {
        let pool = crate::test_support::pool().await;
        let persistence = PostgresPersistence::new(pool.clone());
        let id = crate::test_support::unique_id("recovery-park-failed");
        sqlx::query(
            "INSERT INTO instances (instance_id, tenant_id, status) \
             VALUES ($1, 'recovery-test', 'running')",
        )
        .bind(&id)
        .execute(&pool)
        .await
        .expect("create running instance");
        let policy = RecoveryPolicy {
            auto_recover: true,
            max_auto_restarts: 2,
        };
        for attempt in 1..=2 {
            assert_eq!(
                recover_or_fail_for(&pool, &persistence, &id, policy, RecoveryCause::ParkFailed)
                    .await
                    .expect("recover"),
                RecoveryOutcome::Recovered
            );
            let after = snapshot(&pool, &id).await;
            assert_eq!(after["status"], "suspended");
            assert_eq!(after["termination_reason"], "park_failed");
            assert!(!after["sleep_until"].is_null(), "recovery wakes it at once");
            assert_eq!(after["wake_reason"], "recovery");
            assert_eq!(after["recovery_attempts"], attempt);
            // The relaunch made no progress and exited the same way.
            sqlx::query("UPDATE instances SET status = 'running' WHERE instance_id = $1")
                .bind(&id)
                .execute(&pool)
                .await
                .unwrap();
        }
        assert_eq!(
            recover_or_fail_for(&pool, &persistence, &id, policy, RecoveryCause::ParkFailed)
                .await
                .expect("cap"),
            RecoveryOutcome::Failed
        );
        let after = snapshot(&pool, &id).await;
        assert_eq!(after["status"], "failed");
        assert_eq!(after["termination_reason"], "park_failed");
    }

    /// A `running` instance with no registration: what the orphan scan selects.
    async fn running_orphan(pool: &sqlx::PgPool, prefix: &str) -> String {
        let id = crate::test_support::unique_id(prefix);
        sqlx::query(
            "INSERT INTO instances (instance_id, tenant_id, status) \
             VALUES ($1, 'recovery-test', 'running')",
        )
        .bind(&id)
        .execute(pool)
        .await
        .expect("create running instance");
        id
    }

    /// A running launch generation whose owner's lease ends `lease_secs` from
    /// now (negative: it already has).
    async fn running_launch(pool: &sqlx::PgPool, id: &str, lease_secs: i64) {
        let image_id = crate::test_support::unique_id("recovery-image");
        sqlx::query(
            "INSERT INTO images (image_id, tenant_id, name, binary_path) \
             VALUES ($1, 'recovery-test', $1, '/unused')",
        )
        .bind(&image_id)
        .execute(pool)
        .await
        .expect("create image");
        sqlx::query(
            "INSERT INTO instance_launches (launch_id, instance_id, tenant_id, image_id, kind, \
             state, deadline_at, lease_owner, lease_expires_at, attempt_count) \
             VALUES ($1, $2, 'recovery-test', $3, 'wake', 'running', NOW() + INTERVAL '1 hour', \
             'owner', NOW() + $4 * INTERVAL '1 second', 1)",
        )
        .bind(crate::test_support::unique_id("recovery-launch"))
        .bind(id)
        .bind(&image_id)
        .bind(lease_secs)
        .execute(pool)
        .await
        .expect("create running launch");
    }

    /// The registration a relaunched generation writes before Core promotes it.
    async fn register(pool: &sqlx::PgPool, id: &str) {
        sqlx::query(
            "INSERT INTO container_registry \
             (container_id, launch_id, instance_id, tenant_id, binary_path, started_at) \
             VALUES ($1, $1, $2, 'recovery-test', '/unused', NOW())",
        )
        .bind(crate::test_support::unique_id("recovery-container"))
        .bind(id)
        .execute(pool)
        .await
        .expect("register relaunched generation");
    }

    #[tokio::test]
    async fn orphan_recovery_recovers_a_still_orphaned_instance() {
        let pool = crate::test_support::pool().await;
        let persistence = PostgresPersistence::new(pool.clone());
        // No launch generation at all, and one whose owner stopped renewing.
        for expired_launch in [false, true] {
            let id = running_orphan(&pool, "orphan-recover").await;
            if expired_launch {
                running_launch(&pool, &id, -60).await;
            }
            assert_eq!(
                recover_orphaned_with(&pool, &persistence, &id, policy(true))
                    .await
                    .expect("recover orphan"),
                Some(RecoveryOutcome::Recovered),
                "expired launch: {expired_launch}"
            );
            let after = snapshot(&pool, &id).await;
            assert_eq!(after["status"], "suspended");
            assert_eq!(after["termination_reason"], "environment_restart");
            assert_eq!(after["wake_reason"], "recovery");
        }
    }

    /// The scan selected the instance, then it moved on before recovery wrote.
    #[tokio::test]
    async fn orphan_recovery_leaves_an_instance_that_is_no_longer_orphaned() {
        let pool = crate::test_support::pool().await;
        let persistence = PostgresPersistence::new(pool.clone());
        for case in ["relaunched", "leased", "parked"] {
            let id = running_orphan(&pool, "orphan-moved-on").await;
            match case {
                "relaunched" => register(&pool, &id).await,
                "leased" => running_launch(&pool, &id, 3600).await,
                _ => {
                    sqlx::query("UPDATE instances SET status = 'suspended' WHERE instance_id = $1")
                        .bind(&id)
                        .execute(&pool)
                        .await
                        .expect("park");
                }
            }
            let before = snapshot(&pool, &id).await;
            for auto_recover in [true, false] {
                assert_eq!(
                    recover_orphaned_with(&pool, &persistence, &id, policy(auto_recover))
                        .await
                        .expect("re-check orphan"),
                    None,
                    "{case}"
                );
                assert_eq!(snapshot(&pool, &id).await, before, "{case}");
            }
        }
    }

    /// Two scanners can select the same orphan. The second must re-check only
    /// once the first has written, or it recovers the generation that the
    /// first one's recovery relaunched.
    #[tokio::test]
    async fn concurrent_orphan_recoveries_serialize() {
        let pool = crate::test_support::pool().await;
        let id = running_orphan(&pool, "orphan-serialize").await;
        let mut first = pool.begin().await.expect("begin first scanner");
        sqlx::query(ORPHAN_RECOVERY_LOCK)
            .bind(&id)
            .execute(&mut *first)
            .await
            .expect("first scanner takes the lock");
        let second = tokio::spawn({
            let pool = pool.clone();
            let id = id.clone();
            async move {
                let persistence = PostgresPersistence::new(pool.clone());
                recover_orphaned_with(&pool, &persistence, &id, policy(true)).await
            }
        });
        // Wait until the second scanner is blocked on the lock, not merely
        // spawned. Relaunching before it got there would leave the registration
        // check alone to stop it, and this would pass without the lock. The key
        // repeats ORPHAN_RECOVERY_LOCK's; if they drift apart, this times out.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM pg_locks \
                 WHERE locktype = 'advisory' AND NOT granted AND objsubid = 1 \
                   AND database = (SELECT oid FROM pg_database WHERE datname = current_database()) \
                   AND ((classid::bigint << 32) | objid::bigint) \
                       = hashtextextended('runtara:orphan-recovery' || chr(31) || $1, 0))",
            )
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("read lock waits");
            if waiting {
                break;
            }
            assert!(
                !second.is_finished(),
                "the second scanner must wait for the first"
            );
            assert!(
                tokio::time::Instant::now() < deadline,
                "the second scanner never reached the lock"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        // Meanwhile the first recovered it and the wake relaunched it.
        register(&pool, &id).await;
        let before = snapshot(&pool, &id).await;
        first.commit().await.expect("first scanner commits");
        assert_eq!(
            second
                .await
                .expect("second scanner task")
                .expect("second re-check"),
            None
        );
        assert_eq!(snapshot(&pool, &id).await, before);
    }

    #[tokio::test]
    async fn recovery_write_failure_is_not_reported_as_terminal_failure() {
        let pool = crate::test_support::pool().await;
        let persistence = PostgresPersistence::new(pool.clone());
        pool.close().await;
        for auto_recover in [true, false] {
            assert!(
                recover_or_fail_with(&pool, &persistence, "unavailable", policy(auto_recover))
                    .await
                    .is_err()
            );
        }
    }
}
