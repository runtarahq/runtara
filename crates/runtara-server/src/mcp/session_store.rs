use redis::{AsyncCommands, aio::ConnectionManager};
use rmcp::transport::streamable_http_server::session::{
    SessionState, SessionStore, SessionStoreError,
};

/// Valkey-backed MCP session recovery store.
///
/// `rmcp` keeps live session workers in-process. This store persists only the
/// initialize state needed for another process to restore the session after a
/// restart or cross-instance route.
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
        session_key(&self.tenant_id, session_id)
    }
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
}
