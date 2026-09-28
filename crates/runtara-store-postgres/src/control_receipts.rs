//! Intent-first, success-only receipts of control mutations
//! (`instance_control_receipts`, deleted with their caller).
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use runtara_core::error::CoreError;
use runtara_core::persistence::control_receipts::*;
use serde_json::Value;

use crate::PostgresPersistence;
use crate::rows::DbResult;

const COLUMNS: &str = "caller_instance_id, operation_id, command, target_instance_id, \
     fingerprint, detail, state, result, created_at, completed_at";

type Row = (
    String,
    String,
    String,
    String,
    String,
    Value,
    String,
    Option<Value>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

fn receipt(row: Row) -> Result<ControlReceipt, CoreError> {
    let (caller, operation, command, target, fingerprint, detail, state, result, created, done) =
        row;
    Ok(ControlReceipt {
        caller_instance_id: caller,
        operation_id: operation,
        intent: ControlIntent {
            command,
            target_instance_id: target,
            fingerprint,
            detail,
        },
        state: ControlReceiptState::parse(&state).ok_or_else(|| CoreError::PersistenceError {
            operation: "control_receipt".into(),
            details: format!("unknown receipt state {state}"),
        })?,
        result,
        created_at: created,
        completed_at: done,
    })
}

#[async_trait]
impl ControlReceipts for PostgresPersistence {
    async fn receipt_by_operation(
        &self,
        caller: &str,
        operation: &str,
    ) -> Result<Option<ControlReceipt>, CoreError> {
        validate_receipt_key(caller, operation)?;
        sqlx::query_as::<_, Row>(&format!(
            "SELECT {COLUMNS} FROM instance_control_receipts \
             WHERE caller_instance_id = $1 AND operation_id = $2"
        ))
        .bind(caller)
        .bind(operation)
        .fetch_optional(&self.pool)
        .await
        .db()?
        .map(receipt)
        .transpose()
    }

    async fn begin(
        &self,
        caller: &str,
        operation: &str,
        intent: &ControlIntent,
    ) -> Result<BeginReceipt, CoreError> {
        validate_receipt_key(caller, operation)?;
        let inserted = sqlx::query_as::<_, Row>(&format!(
            "INSERT INTO instance_control_receipts \
                 (caller_instance_id, operation_id, command, target_instance_id, fingerprint, \
                  detail, state) \
             VALUES ($1, $2, $3, $4, $5, $6, 'pending') \
             ON CONFLICT (caller_instance_id, operation_id) DO NOTHING \
             RETURNING {COLUMNS}"
        ))
        .bind(caller)
        .bind(operation)
        .bind(&intent.command)
        .bind(&intent.target_instance_id)
        .bind(&intent.fingerprint)
        .bind(&intent.detail)
        .fetch_optional(&self.pool)
        .await;
        let inserted = match inserted {
            Err(sqlx::Error::Database(error)) if error.is_foreign_key_violation() => {
                return Err(CoreError::InstanceNotFound {
                    instance_id: caller.to_owned(),
                });
            }
            other => other.db()?,
        };
        if let Some(row) = inserted {
            return Ok(BeginReceipt::Started(receipt(row)?));
        }
        // The conflicting row committed before our insert looked, and receipts
        // are only deleted with their caller or while pending by their owner.
        self.receipt_by_operation(caller, operation)
            .await?
            .map(BeginReceipt::Existing)
            .ok_or_else(|| CoreError::PersistenceError {
                operation: "begin_control_receipt".into(),
                details: "the conflicting receipt disappeared; retry".into(),
            })
    }

    async fn complete(
        &self,
        caller: &str,
        operation: &str,
        result: &Value,
    ) -> Result<ControlReceipt, CoreError> {
        validate_receipt_key(caller, operation)?;
        // The first result wins: a completed row is returned unchanged.
        let row = sqlx::query_as::<_, Row>(&format!(
            "UPDATE instance_control_receipts \
             SET state = 'completed', \
                 result = CASE WHEN state = 'pending' THEN $3 ELSE result END, \
                 completed_at = COALESCE(completed_at, clock_timestamp()) \
             WHERE caller_instance_id = $1 AND operation_id = $2 \
             RETURNING {COLUMNS}"
        ))
        .bind(caller)
        .bind(operation)
        .bind(result)
        .fetch_optional(&self.pool)
        .await
        .db()?
        .ok_or_else(|| CoreError::PersistenceError {
            operation: "complete_control_receipt".into(),
            details: format!("no control receipt for {caller}/{operation}"),
        })?;
        receipt(row)
    }

    async fn discard(&self, caller: &str, operation: &str) -> Result<bool, CoreError> {
        validate_receipt_key(caller, operation)?;
        let deleted = sqlx::query(
            "DELETE FROM instance_control_receipts \
             WHERE caller_instance_id = $1 AND operation_id = $2 AND state = 'pending'",
        )
        .bind(caller)
        .bind(operation)
        .execute(&self.pool)
        .await
        .db()?;
        Ok(deleted.rows_affected() > 0)
    }
}
