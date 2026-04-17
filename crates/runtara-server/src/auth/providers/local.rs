use async_trait::async_trait;
use axum::http::HeaderMap;

use crate::auth::{
    AuthContext, AuthMethod,
    provider::{AuthError, AuthProvider, AuthProviderKind},
};

/// `AUTH_PROVIDER=local` — no per-request auth; every request inherits the configured
/// tenant with a static "local" user. Safe only behind a loopback bind (enforced at
/// startup by `bind::enforce_loopback_for_unauthenticated`).
pub struct LocalProvider {
    tenant_id: String,
}

impl LocalProvider {
    pub fn new(tenant_id: String) -> Self {
        Self { tenant_id }
    }
}

#[async_trait]
impl AuthProvider for LocalProvider {
    async fn authenticate(&self, _headers: &HeaderMap) -> Result<AuthContext, AuthError> {
        Ok(AuthContext {
            org_id: self.tenant_id.clone(),
            user_id: "local".to_string(),
            auth_method: AuthMethod::Unauthenticated,
        })
    }

    fn kind(&self) -> AuthProviderKind {
        AuthProviderKind::Local
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_local_user_for_empty_headers() {
        let provider = LocalProvider::new("org_123".into());
        let ctx = provider.authenticate(&HeaderMap::new()).await.unwrap();
        assert_eq!(ctx.org_id, "org_123");
        assert_eq!(ctx.user_id, "local");
        assert_eq!(ctx.auth_method, AuthMethod::Unauthenticated);
    }

    #[tokio::test]
    async fn ignores_any_authorization_header() {
        let provider = LocalProvider::new("org_123".into());
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer garbage".parse().unwrap());
        headers.insert("X-Forwarded-User", "attacker".parse().unwrap());
        let ctx = provider.authenticate(&headers).await.unwrap();
        assert_eq!(ctx.user_id, "local", "local mode must not trust any header");
        assert_eq!(ctx.org_id, "org_123");
    }

    #[test]
    fn kind_is_local() {
        assert_eq!(
            LocalProvider::new("x".into()).kind(),
            AuthProviderKind::Local
        );
    }
}
