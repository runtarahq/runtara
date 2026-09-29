//! Queryable run state (`instance_state`, with its write log
//! `instance_state_writes`; both deleted with their instance).
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use runtara_core::error::CoreError;
use runtara_core::persistence::run_state::*;
use serde_json::{Map, Value};

use crate::PostgresPersistence;
use crate::rows::DbResult;

#[async_trait]
impl RunState for PostgresPersistence {
    async fn apply_state(
        &self,
        tenant_id: &str,
        instance_id: &str,
        operation_id: &str,
        patch: &StatePatch,
    ) -> Result<StateWrite, CoreError> {
        validate_state_write(tenant_id, instance_id, operation_id)?;
        let mut tx = self.pool.begin().await.db()?;
        // The instance row lock serialises writes to one run's state and
        // holds off any status change until the write commits.
        let status: Option<String> = sqlx::query_scalar(
            "SELECT status::text FROM instances \
             WHERE instance_id = $1 AND tenant_id = $2 FOR UPDATE",
        )
        .bind(instance_id)
        .bind(tenant_id)
        .fetch_optional(&mut *tx)
        .await
        .db()?;
        let Some(status) = status else {
            return Err(CoreError::InstanceNotFound {
                instance_id: instance_id.to_owned(),
            });
        };
        let logged: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM instance_state_writes WHERE instance_id = $1 AND operation_id = $2",
        )
        .bind(instance_id)
        .bind(operation_id)
        .fetch_optional(&mut *tx)
        .await
        .db()?;
        if logged.is_some() {
            return Ok(StateWrite::Replayed);
        }
        if status != "running" {
            return Err(CoreError::InvalidInstanceState {
                instance_id: instance_id.to_owned(),
                expected: "running".to_owned(),
                actual: status,
            });
        }

        let mut logged_patch = patch.set.clone();
        for field in &patch.clear {
            logged_patch.insert(field.clone(), Value::Null);
        }
        sqlx::query(
            "INSERT INTO instance_state_writes (instance_id, operation_id, patch) \
             VALUES ($1, $2, $3)",
        )
        .bind(instance_id)
        .bind(operation_id)
        .bind(Value::Object(logged_patch))
        .execute(&mut *tx)
        .await
        .db()?;
        // `-` removes only top-level keys, so nested nulls stay values.
        let merged = sqlx::query(
            "INSERT INTO instance_state (instance_id, state) VALUES ($1, $2::jsonb - $3::text[]) \
             ON CONFLICT (instance_id) DO UPDATE \
             SET state = (instance_state.state || $2::jsonb) - $3::text[], \
                 state_updated_at = clock_timestamp()",
        )
        .bind(instance_id)
        .bind(Value::Object(patch.set.clone()))
        .bind(&patch.clear)
        .execute(&mut *tx)
        .await;
        match merged {
            Err(sqlx::Error::Database(error)) if error.is_check_violation() => {
                return Err(state_too_large());
            }
            other => {
                other.db()?;
            }
        }
        tx.commit().await.db()?;
        Ok(StateWrite::Applied)
    }

    async fn get_state(
        &self,
        tenant_id: &str,
        instance_id: &str,
    ) -> Result<Option<RunStateRecord>, CoreError> {
        let row: Option<(Option<Value>, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT s.state, s.state_updated_at FROM instances i \
             LEFT JOIN instance_state s ON s.instance_id = i.instance_id \
             WHERE i.instance_id = $1 AND i.tenant_id = $2",
        )
        .bind(instance_id)
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await
        .db()?;
        let Some((state, updated_at)) = row else {
            return Err(CoreError::InstanceNotFound {
                instance_id: instance_id.to_owned(),
            });
        };
        Ok(match (state, updated_at) {
            (Some(state), Some(updated_at)) => Some(RunStateRecord {
                state: match state {
                    Value::Object(state) => state,
                    _ => Map::new(),
                },
                updated_at,
            }),
            _ => None,
        })
    }
}
