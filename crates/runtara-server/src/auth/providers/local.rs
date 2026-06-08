use async_trait::async_trait;
use axum::http::HeaderMap;

use crate::auth::{
    AuthContext, AuthMethod,
    provider::{AuthError, AuthProvider, AuthProviderKind},
};
use crate::authz::Role;

/// `AUTH_PROVIDER=local` — no per-request auth; every request inherits the configured
/// tenant with a static "local" user. Safe only behind a loopback bind (enforced at
/// startup by `bind::enforce_loopback_for_unauthenticated`).
pub struct LocalProvider {
    tenant_id: String,
    /// Dev-only role override from `RUNTARA_DEV_ROLE` (owner/admin/member/viewer). Lets local
    /// runs exercise role-based behavior — `/me` reports it and, with
    /// `RUNTARA_AUTH_MEMBERSHIP_POLICY=required`, the authz middleware enforces it. Inert in
    /// production, which never uses this provider (it runs `oidc`).
    dev_role: Option<Role>,
}

impl LocalProvider {
    pub fn new(tenant_id: String) -> Self {
        let dev_role = std::env::var("RUNTARA_DEV_ROLE")
            .ok()
            .and_then(|s| Role::from_wire(s.trim()));
        Self {
            tenant_id,
            dev_role,
        }
    }
}

#[async_trait]
impl AuthProvider for LocalProvider {
    async fn authenticate(&self, _headers: &HeaderMap) -> Result<AuthContext, AuthError> {
        let mut ctx = AuthContext::new(
            self.tenant_id.clone(),
            "local".to_string(),
            AuthMethod::Unauthenticated,
        );
        ctx.role = self.dev_role;
        Ok(ctx)
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
