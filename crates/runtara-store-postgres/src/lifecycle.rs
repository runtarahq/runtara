// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Transactional encoding and application of core lifecycle decisions.
use crate::rows::DbResult;
use runtara_core::{
    domain::{InstanceStatus, SignalType},
    error::CoreError,
    lifecycle::{Change, Command, SuspensionReason, Transition, WakeDeadline},
};
use sqlx::{Postgres, Transaction};

pub(crate) struct LockedCommand {
    pub instance_id: String,
    id: String,
    kind: SignalType,
    acknowledged: bool,
}
impl LockedCommand {
    pub fn command(&self) -> Command<'_> {
        Command {
            id: &self.id,
            kind: self.kind,
            acknowledged: self.acknowledged,
        }
    }
}

pub(crate) async fn lock_instance(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<InstanceStatus, CoreError> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status::text FROM instances WHERE instance_id = $1 FOR UPDATE")
            .bind(id)
            .fetch_optional(&mut **tx)
            .await
            .db()?;
    let status = status.ok_or_else(|| CoreError::InstanceNotFound {
        instance_id: id.into(),
    })?;
    crate::encoding::status_from_str(&status).db()
}

pub(crate) async fn lock_commands(
    tx: &mut Transaction<'_, Postgres>,
    ids: &[String],
) -> Result<Vec<LockedCommand>, CoreError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<(String, String, String, bool)> = sqlx::query_as("SELECT instance_id, command_id::text, signal_type::text, acknowledged_at IS NOT NULL FROM pending_signals WHERE instance_id = ANY($1) AND signal_type <> 'resume' ORDER BY instance_id FOR UPDATE")
        .bind(ids).fetch_all(&mut **tx).await.db()?;
    rows.into_iter()
        .map(|(instance_id, id, kind, acknowledged)| {
            Ok(LockedCommand {
                instance_id,
                id,
                kind: crate::encoding::signal_type_from_str(&kind).db()?,
                acknowledged,
            })
        })
        .collect()
}

fn reason_label(reason: SuspensionReason) -> &'static str {
    match reason {
        SuspensionReason::Shutdown => "shutdown_requested",
        SuspensionReason::Sleeping => "sleeping",
        SuspensionReason::WaitingSignal => "waiting_signal",
        SuspensionReason::WaitingInstances => "waiting_instances",
    }
}

/// All instances and their commands must already be locked. Equal effects are
/// applied in batches, keeping recovery independent of per-instance round trips.
pub(crate) async fn apply_transition(
    tx: &mut Transaction<'_, Postgres>,
    ids: &[String],
    effects: Transition,
) -> Result<(), CoreError> {
    if ids.is_empty() {
        return Ok(());
    }
    let reason = match effects.reason {
        Change::Set(value) => Some(reason_label(value)),
        _ => None,
    };
    let wake_at = match effects.wake {
        Change::Set(WakeDeadline::At(deadline)) => Some(deadline),
        _ => None,
    };
    let wake_reason = match effects.wake_reason {
        Change::Set(reason) => Some(crate::encoding::wake_reason_to_str(reason)),
        _ => None,
    };
    let wake_now = matches!(effects.wake, Change::Set(WakeDeadline::Now));
    sqlx::query(r#"
        UPDATE instances SET
            status = COALESCE($2::instance_status, status),
            finished_at = CASE WHEN $3 THEN NOW() ELSE finished_at END,
            termination_reason = CASE WHEN $4 THEN termination_reason ELSE $5::termination_reason END,
            sleep_until = CASE WHEN $6 THEN sleep_until WHEN $7 THEN NOW() ELSE $8 END,
            output = CASE WHEN $9 THEN NULL ELSE output END,
            error = CASE WHEN $9 THEN NULL ELSE error END,
            wake_reason = CASE WHEN $10 THEN wake_reason ELSE $11 END
        WHERE instance_id = ANY($1)
    "#).bind(ids).bind(effects.status.map(crate::encoding::status_to_str)).bind(effects.finish_now)
        .bind(matches!(effects.reason, Change::Keep)).bind(reason)
        .bind(matches!(effects.wake, Change::Keep)).bind(wake_now).bind(wake_at).bind(effects.clear_result).bind(matches!(effects.wake_reason, Change::Keep)).bind(wake_reason)
        .execute(&mut **tx).await.db()?;
    if !matches!(effects.reason, Change::Keep)
        || effects.status.is_some_and(InstanceStatus::is_terminal)
    {
        sqlx::query("DELETE FROM instance_input_parks WHERE instance_id=ANY($1)")
            .bind(ids)
            .execute(&mut **tx)
            .await
            .db()?;
    }
    if effects.status.is_some_and(InstanceStatus::is_terminal) {
        crate::inputs::close_roots(tx, ids).await.db()?;
    }
    if let Some(event) = effects.event {
        sqlx::query("INSERT INTO instance_events (instance_id, event_type, created_at) SELECT unnest($1::text[]), $2::instance_event_type, NOW()")
            .bind(ids).bind(crate::encoding::event_type_to_str(event)).execute(&mut **tx).await.db()?;
    }
    if effects.acknowledge {
        sqlx::query(
            "UPDATE pending_signals SET acknowledged_at = NOW() WHERE instance_id = ANY($1)",
        )
        .bind(ids)
        .execute(&mut **tx)
        .await
        .db()?;
    }
    Ok(())
}

/// Evaluate the park policy against the locked `status` and apply it with
/// its targets. The instance row must already be locked.
pub(crate) async fn park(
    tx: &mut Transaction<'_, Postgres>,
    instance_id: &str,
    status: InstanceStatus,
    request: runtara_core::lifecycle::ParkRequest,
    targets: runtara_core::persistence::ParkTargets<'_>,
) -> Result<runtara_core::lifecycle::Decision, CoreError> {
    let decision = runtara_core::lifecycle::park(status, request);
    if let runtara_core::lifecycle::Decision::Applied(effects) = decision {
        apply_transition(tx, &[instance_id.to_owned()], effects).await?;
        let signals = request.reason == runtara_core::lifecycle::ParkReason::Signal
            && !targets.signal_ids.is_empty();
        if signals || !targets.wait_ids.is_empty() {
            sqlx::query(
                "INSERT INTO instance_input_parks (instance_id, signal_ids, wait_ids) VALUES ($1,$2,$3)",
            )
            .bind(instance_id)
            .bind(targets.signal_ids)
            .bind(targets.wait_ids)
            .execute(&mut **tx)
            .await
            .db()?;
        }
        if signals {
            crate::inputs::schedule_accepted(tx, instance_id, true)
                .await
                .db()?;
        }
        if !targets.wait_ids.is_empty() {
            crate::waits::park_on(tx, instance_id, targets.wait_ids)
                .await
                .map_err(|error| CoreError::PersistenceError {
                    operation: "park_instance_on_targets".into(),
                    details: error.to_string(),
                })?;
        }
    }
    Ok(decision)
}
