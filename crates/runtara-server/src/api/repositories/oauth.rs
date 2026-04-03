//! OAuth2 state repository — temporary storage for CSRF state tokens

use sqlx::PgPool;

/// A row from the oauth_state table.
#[allow(dead_code)]
pub struct OAuthStateRow {
    pub state: String,
    pub tenant_id: String,
    pub connection_id: String,
    pub integration_id: String,
    pub redirect_uri: String,
}

pub struct OAuthRepository {
    pool: PgPool,
}

impl OAuthRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert a new OAuth state token. Expires after 10 minutes (DB default).
    pub async fn create_state(
        &self,
        state: &str,
        tenant_id: &str,
        connection_id: &str,
        integration_id: &str,
        redirect_uri: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO oauth_state (state, tenant_id, connection_id, integration_id, redirect_uri)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(state)
        .bind(tenant_id)
        .bind(connection_id)
        .bind(integration_id)
        .bind(redirect_uri)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Atomically consume a state token: delete it and return the row if it exists
    /// and hasn't expired. Returns None if not found or expired.
    pub async fn get_and_delete_state(
        &self,
        state: &str,
    ) -> Result<Option<OAuthStateRow>, sqlx::Error> {
        let row = sqlx::query_as::<_, (String, String, String, String, String)>(
            r#"
            DELETE FROM oauth_state
            WHERE state = $1 AND expires_at > NOW()
            RETURNING state, tenant_id, connection_id, integration_id, redirect_uri
            "#,
        )
        .bind(state)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(
            |(state, tenant_id, connection_id, integration_id, redirect_uri)| OAuthStateRow {
                state,
                tenant_id,
                connection_id,
                integration_id,
                redirect_uri,
            },
        ))
    }

    /// Delete expired state tokens (housekeeping).
    #[allow(dead_code)]
    pub async fn cleanup_expired(&self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM oauth_state WHERE expires_at < NOW()")
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }
}
