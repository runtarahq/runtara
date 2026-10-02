use serde_json::Value;
use sqlx::PgPool;

pub struct OperationsRepository {
    pool: PgPool,
}
impl OperationsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Only this tenant's current graphs; no execution or credential data.
    pub async fn current_graphs(&self, tenant: &str) -> Result<Vec<(String, Value)>, sqlx::Error> {
        sqlx::query_as("SELECT w.workflow_id, d.definition FROM workflows w JOIN workflow_definitions d ON d.tenant_id=w.tenant_id AND d.workflow_id=w.workflow_id AND d.version=COALESCE(w.current_version,w.latest_version) WHERE w.tenant_id=$1 ORDER BY w.workflow_id")
            .bind(tenant).fetch_all(&self.pool).await
    }
}

impl OperationsRepository {
    pub async fn list_views(
        &self,
        tenant: &str,
    ) -> Result<Vec<crate::api::dto::operations::SavedOperationView>, sqlx::Error> {
        sqlx::query_as("SELECT id,configuration,revision,updated_at FROM operations_views WHERE tenant_id=$1 ORDER BY configuration->>'name',id")
            .bind(tenant).fetch_all(&self.pool).await
    }
    pub async fn get_view(
        &self,
        tenant: &str,
        id: &str,
    ) -> Result<Option<crate::api::dto::operations::SavedOperationView>, sqlx::Error> {
        sqlx::query_as("SELECT id,configuration,revision,updated_at FROM operations_views WHERE tenant_id=$1 AND id=$2")
            .bind(tenant).bind(id).fetch_optional(&self.pool).await
    }
    pub async fn save_view(
        &self,
        tenant: &str,
        actor: &str,
        id: Option<&str>,
        request: &crate::api::dto::operations::SaveOperationView,
    ) -> Result<Option<crate::api::dto::operations::SavedOperationView>, sqlx::Error> {
        let configuration =
            serde_json::to_value(&request.configuration).expect("view configuration");
        if let Some(id) = id {
            sqlx::query_as("UPDATE operations_views SET configuration=$3,revision=revision+1,updated_at=clock_timestamp() WHERE tenant_id=$1 AND id=$2 AND revision=$4 AND workflow_id=$5 RETURNING id,configuration,revision,updated_at")
                .bind(tenant).bind(id).bind(configuration).bind(request.revision).bind(&request.configuration.workflow).fetch_optional(&self.pool).await
        } else {
            sqlx::query_as("INSERT INTO operations_views(tenant_id,id,workflow_id,configuration,created_by) VALUES ($1,$2,$3,$4,$5) RETURNING id,configuration,revision,updated_at")
                .bind(tenant).bind(uuid::Uuid::new_v4().to_string()).bind(&request.configuration.workflow).bind(configuration).bind(actor).fetch_optional(&self.pool).await
        }
    }
    pub async fn delete_view(
        &self,
        tenant: &str,
        id: &str,
        revision: i32,
    ) -> Result<bool, sqlx::Error> {
        Ok(
            sqlx::query(
                "DELETE FROM operations_views WHERE tenant_id=$1 AND id=$2 AND revision=$3",
            )
            .bind(tenant)
            .bind(id)
            .bind(revision)
            .execute(&self.pool)
            .await?
            .rows_affected()
                == 1,
        )
    }
}
