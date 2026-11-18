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

    /// Gives every trigger that names no tenant to `tenant_id` (the primary
    /// tenant, which they ran as), returning how many. Runs on the system
    /// path: row-level security hides such rows from every tenant.
    pub async fn claim_untenanted(&self, tenant_id: &str) -> Result<u64, sqlx::Error> {
        sqlx::query("UPDATE public.invocation_trigger SET tenant_id = $1 WHERE tenant_id IS NULL")
            .bind(tenant_id)
            .execute(&self.pool)
            .await
            .map(|done| done.rows_affected())
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
                WHERE tenant_id = $1
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
                WHERE id = $1 AND tenant_id = $2
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

    /// Update an invocation trigger by ID with optional tenant filtering.
    ///
    /// The configuration is replaced, except for the keys only webhook
    /// registration writes (requests cannot set them):
    /// - a stored `webhook_secret` is kept: it is never sent to clients, so
    ///   their configuration cannot carry it;
    /// - a stored `platform` is kept while the configuration names the same
    ///   connection. A trigger moved to another connection gets the platform
    ///   of that one when it is registered there.
    ///
    /// Doing this in the statement itself means an update never drops a key
    /// that a concurrent registration has just stored.
    pub async fn update(
        &self,
        id: &str,
        request: &UpdateInvocationTriggerRequest,
        tenant_id: Option<&str>,
    ) -> Result<Option<InvocationTrigger>, sqlx::Error> {
        sqlx::query_as::<_, InvocationTrigger>(
            r#"
            UPDATE public.invocation_trigger
            SET workflow_id = $2,
                trigger_type = $3,
                active = $4,
                configuration = CASE
                    WHEN jsonb_typeof($5) = 'object' AND jsonb_typeof(configuration) = 'object'
                        THEN $5
                            || CASE
                                WHEN configuration ? 'platform'
                                    AND configuration->'connection_id' IS NOT DISTINCT FROM $5->'connection_id'
                                    THEN jsonb_build_object('platform', configuration->'platform')
                                ELSE '{}'::jsonb
                            END
                            || CASE
                                WHEN configuration ? 'webhook_secret'
                                    THEN jsonb_build_object('webhook_secret', configuration->'webhook_secret')
                                ELSE '{}'::jsonb
                            END
                    ELSE $5
                END,
                remote_tenant_id = $6,
                single_instance = $7
            WHERE id = $1 AND ($8::text IS NULL OR tenant_id = $8)
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
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await
    }

    /// Delete an invocation trigger by ID with optional tenant filtering
    pub async fn delete(&self, id: &str, tenant_id: Option<&str>) -> Result<bool, sqlx::Error> {
        let result = if let Some(tid) = tenant_id {
            sqlx::query(
                r#"
                DELETE FROM public.invocation_trigger
                WHERE id = $1 AND tenant_id = $2
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

    /// What checking `id`'s webhook needs, as the engine last wrote it from
    /// the trigger's connection. See
    /// [`crate::api::services::webhook_verification`].
    pub async fn webhook_verification(
        &self,
        id: &str,
    ) -> Result<Option<serde_json::Value>, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT webhook_verification FROM public.invocation_trigger WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map(Option::flatten)
    }

    /// The triggers of `tenant_id` that are bound to a connection: each one's
    /// id, the connection, and what is kept for checking its webhook.
    pub async fn bound_to_connections(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<(String, String, Option<serde_json::Value>)>, sqlx::Error> {
        sqlx::query_as(
            r#"
            SELECT id, configuration->>'connection_id', webhook_verification
            FROM public.invocation_trigger
            WHERE tenant_id = $1 AND configuration->>'connection_id' IS NOT NULL
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Keeps `material` for checking `id`'s webhook, while the trigger is
    /// still bound to `connection_id`.
    pub async fn set_webhook_verification(
        &self,
        id: &str,
        connection_id: &str,
        material: &serde_json::Value,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE public.invocation_trigger
            SET webhook_verification = $3
            WHERE id = $1 AND configuration->>'connection_id' = $2
            "#,
        )
        .bind(id)
        .bind(connection_id)
        .bind(material)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Deactivate every active trigger of `tenant_id` whose workflow is
    /// deleted or missing, returning the deactivated rows.
    ///
    /// Triggers have no foreign key to `workflows`, so a trigger whose
    /// workflow was deleted without deactivating it would otherwise keep
    /// firing indefinitely. Workflow ids are only unique per tenant, so a
    /// trigger needs a live workflow in its own tenant.
    pub async fn deactivate_orphaned(
        &self,
        tenant_id: &str,
    ) -> Result<Vec<InvocationTrigger>, sqlx::Error> {
        sqlx::query_as::<_, InvocationTrigger>(
            r#"
            UPDATE public.invocation_trigger t
            SET active = false, updated_at = NOW()
            WHERE t.active = true
              AND t.tenant_id = $1
              AND NOT EXISTS (
                  SELECT 1 FROM workflows w
                  WHERE w.workflow_id = t.workflow_id
                    AND w.deleted_at IS NULL
                    AND w.tenant_id = t.tenant_id
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
