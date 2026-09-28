//! Per-operation continuations of suspending agent capabilities
//! (`instance_agent_continuations`, deleted with their instance).
use async_trait::async_trait;
use runtara_core::domain::InstanceStatus;
use runtara_core::error::CoreError;
use runtara_core::persistence::Persistence;
use runtara_core::persistence::continuations::*;

use crate::PostgresPersistence;
use crate::rows::DbResult;

#[async_trait]
impl AgentContinuations for PostgresPersistence {
    async fn get(
        &self,
        instance_id: &str,
        op_hash: &str,
        attempt: u32,
    ) -> Result<Option<Vec<u8>>, CoreError> {
        validate_continuation(instance_id, op_hash, attempt, None)?;
        sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT state FROM instance_agent_continuations \
             WHERE instance_id = $1 AND op_hash = $2 AND attempt = $3",
        )
        .bind(instance_id)
        .bind(op_hash)
        .bind(attempt as i32)
        .fetch_optional(&self.pool)
        .await
        .db()
    }

    async fn put(
        &self,
        instance_id: &str,
        op_hash: &str,
        attempt: u32,
        state: &[u8],
    ) -> Result<(), CoreError> {
        validate_continuation(instance_id, op_hash, attempt, Some(state))?;
        // The running fence and the write are one statement. `FOR SHARE`
        // re-checks the status against the latest committed row version and
        // holds off any status change until the write commits, so a
        // continuation is never stored for an instance that stopped running.
        let written = sqlx::query(
            "WITH fence AS ( \
                 SELECT instance_id FROM instances \
                 WHERE instance_id = $1 AND status = 'running' \
                 FOR SHARE \
             ) \
             INSERT INTO instance_agent_continuations (instance_id, op_hash, attempt, state) \
             SELECT instance_id, $2, $3, $4 FROM fence \
             ON CONFLICT (instance_id, op_hash) DO UPDATE \
             SET attempt = EXCLUDED.attempt, state = EXCLUDED.state, \
                 updated_at = clock_timestamp()",
        )
        .bind(instance_id)
        .bind(op_hash)
        .bind(attempt as i32)
        .bind(state)
        .execute(&self.pool)
        .await;
        let written = match written {
            // Defensive: the fence's row lock keeps the instance in place.
            Err(sqlx::Error::Database(error)) if error.is_foreign_key_violation() => {
                return Err(CoreError::InstanceNotFound {
                    instance_id: instance_id.to_owned(),
                });
            }
            other => other.db()?,
        };
        if written.rows_affected() > 0 {
            return Ok(());
        }
        // Nothing written: tell a missing instance from a non-running one.
        match self.get_instance_meta(instance_id).await? {
            None => Err(CoreError::InstanceNotFound {
                instance_id: instance_id.to_owned(),
            }),
            Some(meta) if meta.status != InstanceStatus::Running => {
                Err(not_running(instance_id, meta.status))
            }
            // It was running when re-read but not when written: report the
            // state the fence saw rather than retrying the write.
            Some(_) => Err(CoreError::InvalidInstanceState {
                instance_id: instance_id.to_owned(),
                expected: "running".to_owned(),
                actual: "not running at write".to_owned(),
            }),
        }
    }

    async fn delete(&self, instance_id: &str, op_hash: &str) -> Result<bool, CoreError> {
        validate_continuation(instance_id, op_hash, 1, None)?;
        let deleted = sqlx::query(
            "DELETE FROM instance_agent_continuations WHERE instance_id = $1 AND op_hash = $2",
        )
        .bind(instance_id)
        .bind(op_hash)
        .execute(&self.pool)
        .await
        .db()?;
        Ok(deleted.rows_affected() > 0)
    }
}
