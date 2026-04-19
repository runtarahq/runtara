/// Triggers repository - handles all database operations for invocation triggers
use sqlx::PgPool;

use crate::api::dto::triggers::*;

/// Repository for invocation trigger data access
pub struct TriggerRepository {
    pool: PgPool,
}

impl TriggerRepository {
    /// Create a new TriggerRepository
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create a new invocation trigger
    pub async fn create(
        &self,
        request: &CreateInvocationTriggerRequest,
        tenant_id: Option<&str>,
    ) -> Result<InvocationTrigger, sqlx::Error> {
        let trigger = sqlx::query_as::<_, InvocationTrigger>(
            r#"
            INSERT INTO public.invocation_trigger
                (tenant_id, workflow_id, trigger_type, active, configuration, remote_tenant_id, single_instance)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            RETURNING id, tenant_id, workflow_id, trigger_type, active, configuration,
                      created_at, last_run, updated_at, remote_tenant_id, single_instance
            "#,
        )
        .bind(tenant_id)
        .bind(&request.workflow_id)
        .bind(&request.trigger_type)
        .bind(request.active)
        .bind(&request.configuration)
        .bind(&request.remote_tenant_id)
        .bind(request.single_instance)
        .fetch_one(&self.pool)
        .await?;

        Ok(trigger)
    }

    /// List all invocation triggers with optional tenant filtering
    pub async fn list(
        &self,
        tenant_id: Option<&str>,
    ) -> Result<Vec<InvocationTrigger>, sqlx::Error> {
        let triggers = if let Some(tid) = tenant_id {
            sqlx::query_as::<_, InvocationTrigger>(
                r#"
                SELECT id, tenant_id, workflow_id, trigger_type, active, configuration,
                       created_at, last_run, updated_at, remote_tenant_id, single_instance
                FROM public.invocation_trigger
                WHERE tenant_id = $1 OR tenant_id IS NULL
                ORDER BY created_at DESC
                "#,
            )
            .bind(tid)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, InvocationTrigger>(
                r#"
                SELECT id, tenant_id, workflow_id, trigger_type, active, configuration,
                       created_at, last_run, updated_at, remote_tenant_id, single_instance
                FROM public.invocation_trigger
                ORDER BY created_at DESC
                "#,
            )
            .fetch_all(&self.pool)
            .await?
        };

        Ok(triggers)
    }

    /// Get a single invocation trigger by ID with optional tenant filtering
    pub async fn get_by_id(
        &self,
        id: &str,
        tenant_id: Option<&str>,
    ) -> Result<Option<InvocationTrigger>, sqlx::Error> {
        let trigger = if let Some(tid) = tenant_id {
            sqlx::query_as::<_, InvocationTrigger>(
                r#"
                SELECT id, tenant_id, workflow_id, trigger_type, active, configuration,
                       created_at, last_run, updated_at, remote_tenant_id, single_instance
                FROM public.invocation_trigger
                WHERE id = $1 AND (tenant_id = $2 OR tenant_id IS NULL)
                "#,
            )
            .bind(id)
            .bind(tid)
            .fetch_optional(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, InvocationTrigger>(
                r#"
                SELECT id, tenant_id, workflow_id, trigger_type, active, configuration,
                       created_at, last_run, updated_at, remote_tenant_id, single_instance
                FROM public.invocation_trigger
                WHERE id = $1
                "#,
            )
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
        };

        Ok(trigger)
    }

    /// Update an invocation trigger by ID with optional tenant filtering
    pub async fn update(
        &self,
        id: &str,
        request: &UpdateInvocationTriggerRequest,
        tenant_id: Option<&str>,
    ) -> Result<Option<InvocationTrigger>, sqlx::Error> {
        let trigger = if let Some(tid) = tenant_id {
            sqlx::query_as::<_, InvocationTrigger>(
                r#"
                UPDATE public.invocation_trigger
                SET workflow_id = $2,
                    trigger_type = $3,
                    active = $4,
                    configuration = $5,
                    remote_tenant_id = $6,
                    single_instance = $7
                WHERE id = $1 AND (tenant_id = $8 OR tenant_id IS NULL)
                RETURNING id, tenant_id, workflow_id, trigger_type, active, configuration,
                          created_at, last_run, updated_at, remote_tenant_id, single_instance
                "#,
            )
            .bind(id)
            .bind(&request.workflow_id)
            .bind(&request.trigger_type)
            .bind(request.active)
            .bind(&request.configuration)
            .bind(&request.remote_tenant_id)
            .bind(request.single_instance)
            .bind(tid)
            .fetch_optional(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, InvocationTrigger>(
                r#"
                UPDATE public.invocation_trigger
                SET workflow_id = $2,
                    trigger_type = $3,
                    active = $4,
                    configuration = $5,
                    remote_tenant_id = $6,
                    single_instance = $7
                WHERE id = $1
                RETURNING id, tenant_id, workflow_id, trigger_type, active, configuration,
                          created_at, last_run, updated_at, remote_tenant_id, single_instance
                "#,
            )
            .bind(id)
            .bind(&request.workflow_id)
            .bind(&request.trigger_type)
            .bind(request.active)
            .bind(&request.configuration)
            .bind(&request.remote_tenant_id)
            .bind(request.single_instance)
            .fetch_optional(&self.pool)
            .await?
        };

        Ok(trigger)
    }

    /// Delete an invocation trigger by ID with optional tenant filtering
    pub async fn delete(&self, id: &str, tenant_id: Option<&str>) -> Result<bool, sqlx::Error> {
        let result = if let Some(tid) = tenant_id {
            sqlx::query(
                r#"
                DELETE FROM public.invocation_trigger
                WHERE id = $1 AND (tenant_id = $2 OR tenant_id IS NULL)
                "#,
            )
            .bind(id)
            .bind(tid)
            .execute(&self.pool)
            .await?
        } else {
            sqlx::query(
                r#"
                DELETE FROM public.invocation_trigger
                WHERE id = $1
                "#,
            )
            .bind(id)
            .execute(&self.pool)
            .await?
        };

        Ok(result.rows_affected() > 0)
    }

    /// Update only the configuration field of a trigger.
    /// Used to store webhook secrets after registration.
    pub async fn update_configuration(
        &self,
        id: &str,
        configuration: &serde_json::Value,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE public.invocation_trigger
            SET configuration = $2, updated_at = NOW()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(configuration)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
