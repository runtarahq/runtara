use async_trait::async_trait;
use axum::http::HeaderMap;

use crate::auth::{
    AuthContext, AuthMethod,
    provider::{AuthError, AuthProvider, AuthProviderKind},
};

/// `AUTH_PROVIDER=trust_proxy` — a reverse proxy has already authenticated the request
/// and forwards the end-user identity in a configurable header (default
/// `X-Forwarded-User`). The proxy is responsible for stripping any client-supplied copy
/// of that header before it reaches us; we make no attempt to validate it.
///
/// Safe only behind a loopback bind (enforced at startup by
/// `bind::enforce_loopback_for_unauthenticated`).
pub struct TrustProxyProvider {
    tenant_id: String,
    user_header: String,
}

impl TrustProxyProvider {
    pub fn new(tenant_id: String, user_header: String) -> Self {
        Self {
            tenant_id,
            user_header,
        }
    }
}

#[async_trait]
impl AuthProvider for TrustProxyProvider {
    async fn authenticate(&self, headers: &HeaderMap) -> Result<AuthContext, AuthError> {
        let user_id = headers
            .get(&self.user_header)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "proxy".to_string());

        Ok(AuthContext {
            org_id: self.tenant_id.clone(),
            user_id,
            auth_method: AuthMethod::Unauthenticated,
        })
    }

    fn kind(&self) -> AuthProviderKind {
        AuthProviderKind::TrustProxy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_default_user_header() {
        let provider = TrustProxyProvider::new("org_123".into(), "x-forwarded-user".into());
        let mut headers = HeaderMap::new();
        headers.insert("X-Forwarded-User", "alice".parse().unwrap());
        let ctx = provider.authenticate(&headers).await.unwrap();
        assert_eq!(ctx.user_id, "alice");
        assert_eq!(ctx.org_id, "org_123");
        assert_eq!(ctx.auth_method, AuthMethod::Unauthenticated);
    }

    #[tokio::test]
    async fn falls_back_to_proxy_when_header_absent() {
        let provider = TrustProxyProvider::new("org_123".into(), "x-forwarded-user".into());
        let ctx = provider.authenticate(&HeaderMap::new()).await.unwrap();
        assert_eq!(ctx.user_id, "proxy");
    }

    #[tokio::test]
    async fn respects_custom_header_name() {
        let provider = TrustProxyProvider::new("org_123".into(), "x-auth-request-user".into());
        let mut headers = HeaderMap::new();
        headers.insert("x-auth-request-user", "bob".parse().unwrap());
        // Default header name must NOT be consulted when a custom one is configured.
        headers.insert("x-forwarded-user", "attacker".parse().unwrap());
        let ctx = provider.authenticate(&headers).await.unwrap();
        assert_eq!(ctx.user_id, "bob");
    }

    #[test]
    fn kind_is_trust_proxy() {
        assert_eq!(
            TrustProxyProvider::new("x".into(), "x-forwarded-user".into()).kind(),
            AuthProviderKind::TrustProxy,
        );
    }
}
