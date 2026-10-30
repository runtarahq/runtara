use redis::{AsyncCommands, aio::ConnectionManager};
use rmcp::transport::streamable_http_server::session::{
    SessionState, SessionStore, SessionStoreError,
};

tokio::task_local! {
    /// The tenant of the authenticated MCP request being handled. rmcp calls
    /// the store inline while handling a request, so the store reads it there.
    static REQUEST_TENANT: String;
}

/// Bind the authenticated request's tenant around the MCP transport, so its
/// session state is kept, and restored, under that tenant only. Layered
/// inside the auth middleware.
pub async fn scope_session_tenant(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    match request
        .extensions()
        .get::<crate::auth::AuthContext>()
        .map(|auth| auth.org_id.clone())
    {
        Some(tenant) => REQUEST_TENANT.scope(tenant, next.run(request)).await,
        None => next.run(request).await,
    }
}

/// Valkey-backed MCP session recovery store.
///
/// `rmcp` keeps live session workers in-process. This store persists only the
/// initialize state needed for another process to restore the session after a
/// restart or cross-instance route. Each session is kept under the tenant of
/// the request that made it, so a request authenticated for another tenant
/// cannot restore it: under its own tenant the session does not exist.
#[derive(Clone)]
pub struct ValkeyMcpSessionStore {
    manager: ConnectionManager,
    tenant_id: String,
    ttl_seconds: u64,
}

impl ValkeyMcpSessionStore {
    pub fn new(manager: ConnectionManager, tenant_id: String, ttl_seconds: u64) -> Self {
        Self {
            manager,
            tenant_id,
            ttl_seconds,
        }
    }

    fn key(&self, session_id: &str) -> String {
        request_session_key(&self.tenant_id, session_id)
    }
}

/// The key of `session_id` under the request's tenant, else (outside a
/// request) under `fallback_tenant`, the process's primary.
fn request_session_key(fallback_tenant: &str, session_id: &str) -> String {
    REQUEST_TENANT
        .try_with(|tenant| session_key(tenant, session_id))
        .unwrap_or_else(|_| session_key(fallback_tenant, session_id))
}

fn session_key(tenant_id: &str, session_id: &str) -> String {
    format!("runtara:mcp:sessions:{}:{}", tenant_id, session_id)
}

#[async_trait::async_trait]
impl SessionStore for ValkeyMcpSessionStore {
    async fn load(&self, session_id: &str) -> Result<Option<SessionState>, SessionStoreError> {
        let key = self.key(session_id);
        let mut conn = self.manager.clone();
        let payload: Option<String> = conn.get(key).await.map_err(boxed_error)?;

        payload
            .map(|payload| serde_json::from_str(&payload).map_err(boxed_error))
            .transpose()
    }

    async fn store(&self, session_id: &str, state: &SessionState) -> Result<(), SessionStoreError> {
        let key = self.key(session_id);
        let payload = serde_json::to_string(state).map_err(boxed_error)?;
        let mut conn = self.manager.clone();
        conn.set_ex::<_, _, ()>(key, payload, self.ttl_seconds)
            .await
            .map_err(boxed_error)
    }

    async fn delete(&self, session_id: &str) -> Result<(), SessionStoreError> {
        let key = self.key(session_id);
        let mut conn = self.manager.clone();
        conn.del::<_, ()>(key).await.map_err(boxed_error)
    }
}

fn boxed_error<E>(error: E) -> SessionStoreError
where
    E: std::error::Error + Send + Sync + 'static,
{
    Box::new(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_scopes_sessions_by_tenant() {
        assert_eq!(
            session_key("tenant-a", "session-1"),
            "runtara:mcp:sessions:tenant-a:session-1"
        );
    }

    #[tokio::test]
    async fn a_session_is_keyed_by_the_requests_tenant() {
        assert_eq!(
            request_session_key("primary", "s-1"),
            "runtara:mcp:sessions:primary:s-1"
        );
        let scoped = REQUEST_TENANT
            .scope("tenant-b".to_owned(), async {
                request_session_key("primary", "s-1")
            })
            .await;
        assert_eq!(scoped, "runtara:mcp:sessions:tenant-b:s-1");
    }
}
