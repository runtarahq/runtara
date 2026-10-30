//! The tenant a request names in its path.
//!
//! A process may serve several tenants, and the frontend and MCP clients name the one
//! they act for: `/api/runtime/{tenant}/…` and `/mcp/{tenant}`. [`strip_tenant_segment`]
//! runs before routing, removes a served tenant's segment so the routes stay
//! tenant-less, and records it as [`RequestedTenant`] for the auth middleware. A segment
//! that is not a served tenant is left alone, so `/api/runtime/workflows` routes as
//! before and an unknown tenant 404s.

use axum::http::{Request, Uri};

/// The served tenant a request's path named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestedTenant(pub String);

/// Strip a served tenant's segment from the request path, recording it.
pub fn strip_tenant_segment<B>(mut request: Request<B>) -> Request<B> {
    // Never trust a value the caller could have planted before this layer.
    request.extensions_mut().remove::<RequestedTenant>();
    if let Some((tenant, uri)) = split_tenant(request.uri(), crate::config::is_served_tenant) {
        *request.uri_mut() = uri;
        request.extensions_mut().insert(RequestedTenant(tenant));
    }
    request
}

fn split_tenant(uri: &Uri, is_served: impl Fn(&str) -> bool) -> Option<(String, Uri)> {
    let path = uri.path();
    let (prefix, rest) = [("/api/runtime/", "/api/runtime/"), ("/mcp/", "/mcp")]
        .into_iter()
        .find_map(|(prefix, kept)| path.strip_prefix(prefix).map(|rest| (kept, rest)))?;
    let (segment, tail) = rest.split_once('/').unwrap_or((rest, ""));
    if segment.is_empty() || !is_served(segment) {
        return None;
    }
    let path = match (prefix, tail) {
        ("/mcp", "") => "/mcp".to_owned(),
        ("/mcp", tail) => format!("/mcp/{tail}"),
        (prefix, tail) => format!("{prefix}{tail}"),
    };
    let path_and_query = match uri.query() {
        Some(query) => format!("{path}?{query}"),
        None => path,
    };
    let mut parts = uri.clone().into_parts();
    parts.path_and_query = Some(path_and_query.parse().ok()?);
    Some((segment.to_owned(), Uri::from_parts(parts).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(uri: &str) -> Option<(String, String)> {
        split_tenant(&uri.parse().unwrap(), |t| t == "acme" || t == "beta")
            .map(|(tenant, uri)| (tenant, uri.to_string()))
    }

    #[test]
    fn a_served_tenants_api_segment_is_stripped() {
        assert_eq!(
            split("/api/runtime/acme/workflows?page=2"),
            Some(("acme".into(), "/api/runtime/workflows?page=2".into()))
        );
        assert_eq!(
            split("/api/runtime/beta/me"),
            Some(("beta".into(), "/api/runtime/me".into()))
        );
    }

    #[test]
    fn a_served_tenants_mcp_segment_is_stripped() {
        assert_eq!(split("/mcp/acme"), Some(("acme".into(), "/mcp".into())));
        assert_eq!(
            split("/mcp/acme/sse"),
            Some(("acme".into(), "/mcp/sse".into()))
        );
    }

    #[test]
    fn other_paths_are_left_alone() {
        assert_eq!(split("/api/runtime/workflows"), None);
        assert_eq!(split("/api/runtime/other/workflows"), None);
        assert_eq!(split("/api/runtime/"), None);
        assert_eq!(split("/mcp"), None);
        assert_eq!(split("/ui/acme/"), None);
        assert_eq!(split("/api/oauth/acme/callback"), None);
    }
}
