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
        owner: runtara_core::persistence::ExecutionWriter<'_>,
    ) -> Result<(), CoreError> {
        validate_continuation(instance_id, op_hash, attempt, Some(state))?;
        // The ownership fence and the write are one statement. An owned
        // write holds its exact active lease row `FOR SHARE`, which leaving
        // `running` must update, so a continuation is never stored for an
        // execution that stopped running or was superseded. An unowned one
        // holds the running instance row the same way.
        let sql = format!(
            "WITH fence AS ({}) \
             INSERT INTO instance_agent_continuations (instance_id, op_hash, attempt, state) \
             SELECT instance_id, $2, $3, $4 FROM fence \
             ON CONFLICT (instance_id, op_hash) DO UPDATE \
             SET attempt = EXCLUDED.attempt, state = EXCLUDED.state, \
                 updated_at = clock_timestamp()",
            crate::root_owner::fence(owner, 5, true)
        );
        let query = sqlx::query(&sql)
            .bind(instance_id)
            .bind(op_hash)
            .bind(attempt as i32)
            .bind(state);
        let written = crate::root_owner::bind_owner(query, owner)
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
            // Running, but this writer does not own it (or it was not
            // running when written, which the lease fence cannot tell apart).
            Some(_) => Err(CoreError::Superseded {
                instance_id: instance_id.to_owned(),
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
