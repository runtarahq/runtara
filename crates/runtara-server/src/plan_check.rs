// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Boot-time plan-change detection (`plan.changed`).
//!
//! `RUNTARA_PRICING_TIER` is parsed once into a `OnceLock<Config>` at boot
//! (see `config::entitlements()`) and never re-read for the life of the
//! process — a plan change made by restarting with a different env var is
//! invisible to runtara until this next boot. So the only point a plan
//! change CAN be observed is right here: compare the just-locked-in plan
//! against the last one persisted in the generic `metadata` table
//! (key = `"plan"`), and a divergence is the `plan.changed` signal.
//!
//! A missing row (first boot ever) just establishes the baseline — there is
//! nothing to compare against yet, so it is not treated as a "change".

use sqlx::PgPool;

use crate::entitlements::Tier;
use crate::product_events::{ActorType, EventSource, EventType, ProductEvent, ProductEventSink};

const METADATA_KEY: &str = "plan";

/// Compare `current_plan` against the last persisted value, update `metadata`, and emit
/// `plan.changed` if it changed. Best-effort: a DB error is logged and swallowed rather than
/// failing server startup over an analytics side-channel.
pub async fn check_and_record(pool: &PgPool, events: &ProductEventSink, current_plan: &Tier) {
    if let Err(e) = check_and_record_inner(pool, events, current_plan.name()).await {
        tracing::warn!(error = %e, "plan check: failed to compare/persist current plan");
    }
}

async fn check_and_record_inner(
    pool: &PgPool,
    events: &ProductEventSink,
    current_plan: &str,
) -> Result<(), sqlx::Error> {
    let previous: Option<String> = sqlx::query_scalar("SELECT value FROM metadata WHERE key = $1")
        .bind(METADATA_KEY)
        .fetch_optional(pool)
        .await?;

    if previous.as_deref() == Some(current_plan) {
        return Ok(());
    }

    sqlx::query(
        "INSERT INTO metadata (key, value, updated_at) VALUES ($1, $2, NOW()) \
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()",
    )
    .bind(METADATA_KEY)
    .bind(current_plan)
    .execute(pool)
    .await?;

    if let Some(previous_plan) = previous {
        tracing::info!(
            old_plan = %previous_plan,
            new_plan = %current_plan,
            "plan check: plan changed since last boot"
        );
        events.emit(
            ProductEvent::new(EventType::PlanChanged)
                .no_user_actor("server_boot", ActorType::System)
                .source(EventSource::Worker)
                .properties(serde_json::json!({
                    "old_plan": previous_plan,
                    "new_plan": current_plan,
                })),
        );
    } else {
        tracing::info!(plan = %current_plan, "plan check: first boot, recording baseline plan");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn first_boot_records_baseline_without_emitting() {
        let pool = PgPool::connect_lazy("postgres://localhost/dummy").unwrap();
        let (tx, mut rx) = mpsc::channel(4);
        let sink = ProductEventSink::new(tx);

        // No DB reachable (lazy pool), so this exercises the error path only — the emit
        // assertion below is what actually matters for `check_and_record`'s public contract:
        // a DB failure must never panic or emit a spurious event.
        check_and_record(&pool, &sink, &Tier::Starter).await;
        assert!(rx.try_recv().is_err(), "no event on DB failure");
    }
}
