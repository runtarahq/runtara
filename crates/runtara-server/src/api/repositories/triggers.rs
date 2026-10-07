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
        created_by: Option<&str>,
    ) -> Result<InvocationTrigger, sqlx::Error> {
        let trigger = sqlx::query_as::<_, InvocationTrigger>(
            r#"
            INSERT INTO public.invocation_trigger
                (tenant_id, workflow_id, trigger_type, active, configuration, remote_tenant_id, single_instance, created_by)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
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
        .bind(created_by)
        .fetch_one(&self.pool)
        .await?;

        Ok(trigger)
    }

    /// The `created_by` (owner) of a trigger, for `Own`-scoped authorization. `None` when the
    /// trigger does not exist or predates ownership tracking (NULL `created_by`).
    pub async fn owner(&self, id: &str) -> Result<Option<String>, sqlx::Error> {
        let owner: Option<Option<String>> =
            sqlx::query_scalar("SELECT created_by FROM invocation_trigger WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(owner.flatten())
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

    /// Whether any active Channel trigger visible to `tenant_id` is still
    /// bound to `connection_id`. Channel webhooks are registered per
    /// connection, so one must stay registered while any trigger still uses it.
    pub async fn connection_has_active_channel_trigger(
        &self,
        connection_id: &str,
        tenant_id: &str,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM public.invocation_trigger
                WHERE trigger_type = 'CHANNEL'
                  AND active = true
                  AND configuration->>'connection_id' = $1
                  AND (tenant_id = $2 OR tenant_id IS NULL)
            )
            "#,
        )
        .bind(connection_id)
        .bind(tenant_id)
        .fetch_one(&self.pool)
        .await
    }

    /// Deactivate every active trigger visible to `tenant_id` whose workflow
    /// is deleted or missing, returning the deactivated rows.
    ///
    /// Triggers have no foreign key to `workflows`, so a trigger whose
    /// workflow was deleted without deactivating it would otherwise keep
    /// firing indefinitely. The workflow lookup is deliberately not
    /// tenant-scoped: a trigger is only treated as orphaned when no tenant has
    /// a live workflow with that id, so a global (NULL-tenant) trigger is
    /// never switched off from one tenant's point of view.
    pub async fn deactivate_orphaned(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<InvocationTrigger>, sqlx::Error> {
        sqlx::query_as::<_, InvocationTrigger>(
            r#"
            UPDATE public.invocation_trigger t
            SET active = false, updated_at = NOW()
            WHERE t.active = true
              AND (t.tenant_id = $1 OR t.tenant_id IS NULL)
              AND NOT EXISTS (
                  SELECT 1 FROM workflows w
                  WHERE w.workflow_id = t.workflow_id AND w.deleted_at IS NULL
              )
            RETURNING t.id, t.tenant_id, t.workflow_id, t.trigger_type, t.active, t.configuration,
                      t.created_at, t.last_run, t.updated_at, t.remote_tenant_id, t.single_instance
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
    }
}
