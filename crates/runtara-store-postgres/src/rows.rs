// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Row decoding for Core's record types.
//!
//! Core's records carry no sqlx derives — it does not depend on sqlx, and a
//! record that named a driver's traits would be describing one storage engine
//! rather than the data. `FromRow` and the record types are both foreign to
//! this crate, so the impl cannot be written directly; each record gets a local
//! wrapper that decodes it instead.
//!
//! These are public because Environment runs its own Postgres queries against
//! Core's tables (events, step summaries) and needs the same decoding.
//!

use runtara_core::persistence::{
    CheckpointRecord, CustomSignalRecord, EventRecord, InstanceRecord, SignalRecord,
};
use sqlx::{FromRow, Row, postgres::PgRow};

/// Decodes an [`InstanceRecord`].
///
/// `input` is optional in the projection: the meta reads deliberately leave it
/// out, so a missing column decodes as `None` rather than failing.
pub struct InstanceRow(pub InstanceRecord);

impl<'r> FromRow<'r, PgRow> for InstanceRow {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self(InstanceRecord {
            instance_id: row.try_get("instance_id")?,
            tenant_id: row.try_get("tenant_id")?,
            definition_version: row.try_get("definition_version")?,
            status: crate::encoding::status_from_str(&row.try_get::<String, _>("status")?)?,
            checkpoint_id: row.try_get("checkpoint_id")?,
            attempt: row.try_get("attempt")?,
            max_attempts: row.try_get("max_attempts")?,
            created_at: row.try_get("created_at")?,
            started_at: row.try_get("started_at")?,
            finished_at: row.try_get("finished_at")?,
            input: row.try_get("input").unwrap_or_default(),
            output: row.try_get("output")?,
            error: row.try_get("error")?,
            sleep_until: row.try_get("sleep_until")?,
            termination_reason: row.try_get("termination_reason").unwrap_or_default(),
            exit_code: row.try_get("exit_code").unwrap_or_default(),
            recovery_attempts: row.try_get("recovery_attempts").unwrap_or_default(),
            recovery_marker: row.try_get("recovery_marker").unwrap_or_default(),
        }))
    }
}

/// Decodes a [`CheckpointRecord`].
pub struct CheckpointRow(pub CheckpointRecord);

impl<'r> FromRow<'r, PgRow> for CheckpointRow {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self(CheckpointRecord {
            instance_id: row.try_get("instance_id")?,
            checkpoint_id: row.try_get("checkpoint_id")?,
            state: row.try_get("state")?,
            created_at: row.try_get("created_at")?,
        }))
    }
}

/// Decodes an [`EventRecord`].
pub struct EventRow(pub EventRecord);

impl<'r> FromRow<'r, PgRow> for EventRow {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self(EventRecord {
            id: row.try_get("id").unwrap_or_default(),
            instance_id: row.try_get("instance_id")?,
            event_type: crate::encoding::event_type_from_str(
                &row.try_get::<String, _>("event_type")?,
            )?,
            checkpoint_id: row.try_get("checkpoint_id")?,
            payload: row.try_get("payload")?,
            created_at: row.try_get("created_at")?,
            subtype: row.try_get("subtype")?,
        }))
    }
}

/// Decodes a [`SignalRecord`].
pub struct SignalRow(pub SignalRecord);

impl<'r> FromRow<'r, PgRow> for SignalRow {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self(SignalRecord {
            instance_id: row.try_get("instance_id")?,
            signal_type: crate::encoding::signal_type_from_str(
                &row.try_get::<String, _>("signal_type")?,
            )?,
            payload: row.try_get("payload")?,
            created_at: row.try_get("created_at")?,
            acknowledged_at: row.try_get("acknowledged_at")?,
        }))
    }
}

/// Decodes a [`CustomSignalRecord`].
pub struct CustomSignalRow(pub CustomSignalRecord);

impl<'r> FromRow<'r, PgRow> for CustomSignalRow {
    fn from_row(row: &'r PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self(CustomSignalRecord {
            instance_id: row.try_get("instance_id")?,
            checkpoint_id: row.try_get("checkpoint_id")?,
            payload: row.try_get("payload")?,
            created_at: row.try_get("created_at")?,
        }))
    }
}

/// Maps a driver error into Core's vocabulary.
///
/// Core does not depend on sqlx, so it cannot own this conversion; and neither
/// `sqlx::Error` nor `CoreError` is local here, so it cannot be a `From` impl
/// either. An extension trait on the driver's own `Result` is what is left, and
/// it has the advantage of being visible at each call site.
pub(crate) trait DbResult<T> {
    /// Convert a driver failure into [`CoreError::DatabaseError`].
    fn db(self) -> Result<T, runtara_core::error::CoreError>;
}

impl<T> DbResult<T> for Result<T, sqlx::Error> {
    fn db(self) -> Result<T, runtara_core::error::CoreError> {
        self.map_err(|e| runtara_core::error::CoreError::DatabaseError {
            operation: "postgres".to_string(),
            details: e.to_string(),
        })
    }
}
