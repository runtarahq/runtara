use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderValue, StatusCode, Uri, header},
    response::Response,
    routing::get,
};
use bytes::Bytes;
use rust_embed::{EmbeddedFile, RustEmbed};

use crate::api::dto::entitlements::EntitlementsDto;
use crate::entitlements::EntitlementSnapshot;

#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct UiAssets;

#[derive(Clone)]
pub struct UiState {
    /// index.html with <base href> rewritten to match the deployed mount prefix.
    /// Release builds cache this at startup; debug builds rebuild per request
    /// so `npm run build:watch` changes are picked up without a server restart.
    #[cfg(not(debug_assertions))]
    index_html: Bytes,
    /// `<base href>` value to inject into index.html. Debug-only: rebuild_index_html
    /// needs it on every request.
    #[cfg(debug_assertions)]
    base_href: Arc<str>,
    /// Mount prefix (e.g. `/ui`), stripped from the request URI before looking
    /// up the asset. Lets multi-segment mounts like `/ui/foo` work correctly.
    mount: Arc<str>,
    /// CSP header for HTML responses. Contains a SHA-256 hash of the inline
    /// `window.__RUNTARA_CONFIG__` script so the browser lets it execute.
    /// Computed once at startup — the inline script body is derived from env
    /// vars that don't change during process lifetime, so the hash is stable
    /// even when debug builds rewrite index.html per request.
    html_csp: Arc<str>,
}

/// Build a router that serves the embedded UI under `mount` (e.g. `/ui`).
/// `base_href` is what the server injects into index.html's `<base href>` tag
/// (e.g. `/ui/` or `/ui/tenant-abc/`).
///
/// Avoids `Router::nest` — axum 0.8's nest interacts poorly with trailing
/// slashes (`/ui` matches but `/ui/` 404s). Registering explicit routes at the
/// outer level dodges the quirk.
pub fn router(mount: &str, base_href: &str) -> Router {
    // Always build once at startup so we can derive a stable CSP (whose hash is
    // pinned to the inline script body). In release the rewritten HTML is
    // reused for every request; in debug we throw it away and rebuild per
    // request so `npm run build:watch` output is picked up without a restart.
    let (_built_index_html, inline_script_hash) =
        build_index_html(base_href, crate::config::entitlements());
    let state = UiState {
        #[cfg(not(debug_assertions))]
        index_html: _built_index_html,
        #[cfg(debug_assertions)]
        base_href: Arc::from(base_href),
        mount: Arc::from(mount),
        html_csp: Arc::from(build_html_csp(&inline_script_hash).as_str()),
    };
    let wild = format!("{mount}/{{*path}}");
    let with_slash = format!("{mount}/");
    Router::new()
        .route(mount, get(serve))
        .route(&with_slash, get(serve))
        .route(&wild, get(serve))
        .with_state(state)
}

/// Returns the rewritten HTML and the base64-encoded SHA-256 of the inline
/// `window.__RUNTARA_CONFIG__` script body. The hash goes into the
/// `script-src` CSP directive so the browser allows the inline script to run.
///
/// `entitlements` is threaded through so the inlined snapshot in the
/// `window.__RUNTARA_CONFIG__` body matches the process-wide
/// `crate::config::entitlements()`. Taking it as an argument (rather than
/// reading the `OnceLock` here) lets tests pass a fixture snapshot without
/// initialising the global config.
fn build_index_html(base_href: &str, entitlements: &EntitlementSnapshot) -> (Bytes, String) {
    use base64::Engine;
    use sha2::Digest;

    let raw = UiAssets::get("index.html")
        .expect("frontend/dist/index.html missing — run `npm run build` in ./frontend");
    let html = std::str::from_utf8(&raw.data).expect("index.html is not valid utf-8");

    // 1. Rewrite `<base href>` so the SPA resolves asset URLs and computes its
    //    React Router basename against the deployed mount prefix.
    let base_needle = r#"<base href="/">"#;
    assert!(
        html.contains(base_needle),
        "embed-ui: expected `{base_needle}` placeholder in index.html. Check frontend/index.html."
    );
    let base_replacement = format!(r#"<base href="{base_href}">"#);
    let step_one = html.replacen(base_needle, &base_replacement, 1);

    // 2. Populate `window.__RUNTARA_CONFIG__` so the SPA can read tenant-specific
    //    OIDC/API/analytics/entitlement values at runtime without per-tenant rebuilds.
    let config_needle = "window.__RUNTARA_CONFIG__={};";
    assert!(
        step_one.contains(config_needle),
        "embed-ui: expected `{config_needle}` placeholder in index.html. Check frontend/index.html."
    );
    let inline_script = format!(
        "window.__RUNTARA_CONFIG__={};",
        runtime_config_json(entitlements)
    );
    let rewritten = step_one.replacen(config_needle, &inline_script, 1);

    // CSP `script-src 'sha256-...'` matches the hash of the inline script body
    // between the <script> tags (no surrounding whitespace, no tags). The
    // content we splice in IS that body verbatim.
    let digest = sha2::Sha256::digest(inline_script.as_bytes());
    let hash_b64 = base64::engine::general_purpose::STANDARD.encode(digest);

    (Bytes::from(rewritten.into_bytes()), hash_b64)
}

/// CSP for HTML responses. Parameterized by the base64 SHA-256 hash of the
/// inline config script we just injected so the browser allows it to run.
/// Operators tightening for production should front the server with a reverse
/// proxy that overrides this header.
fn build_html_csp(inline_script_sha256_b64: &str) -> String {
    build_html_csp_with_plausible_source(
        inline_script_sha256_b64,
        plausible_script_src_from_env().as_deref(),
    )
}

fn build_html_csp_with_plausible_source(
    inline_script_sha256_b64: &str,
    plausible_script_src: Option<&str>,
) -> String {
    let mut script_sources = vec!["'self'".to_string(), "https://plausible.io".to_string()];
    if let Some(source) = plausible_script_src
        && !script_sources.iter().any(|existing| existing == source)
    {
        script_sources.push(source.to_string());
    }
    script_sources.push("'wasm-unsafe-eval'".to_string());
    script_sources.push(format!("'sha256-{inline_script_sha256_b64}'"));

    format!(
        "default-src 'self'; \
         script-src {}; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob:; \
         font-src 'self' data:; \
         connect-src 'self' https: wss: http://localhost:* ws://localhost:*; \
         manifest-src 'self'; \
         worker-src 'self' blob:; \
         frame-ancestors 'none'; \
         object-src 'none'; \
         base-uri 'self'",
        script_sources.join(" ")
    )
}

fn plausible_script_src_from_env() -> Option<String> {
    std::env::var("RUNTARA_UI_PLAUSIBLE_HOST")
        .ok()
        .and_then(|host| normalize_plausible_script_src(&host))
}

fn normalize_plausible_script_src(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let url = if trimmed.starts_with("//") {
        format!("https:{trimmed}")
    } else if trimmed.contains("://") {
        trimmed.to_string()
    } else if trimmed.starts_with('/') {
        return None;
    } else {
        format!("https://{trimmed}")
    };

    let parsed = url::Url::parse(&url).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }

    Some(parsed.origin().ascii_serialization())
}

/// Serialize the runtime config as a JSON object literal. Only keys with a
/// non-empty env value are emitted, so absent values stay `undefined` in JS
/// (the frontend then falls back to the build-time VITE_* default, if any).
///
/// `authMode` and `tenantId` are always emitted: the SPA needs them to decide
/// whether to initiate an OIDC redirect and how to prefix tenant-scoped URLs.
///
/// `entitlements` is the resolved per-process snapshot, inlined as a nested
/// JSON object (not a stringified blob) so the SPA can branch on features
/// before any network request completes — see Phase 4.1 in
/// `docs/entitlements.md`.
fn runtime_config_json(entitlements: &EntitlementSnapshot) -> String {
    use std::fmt::Write;

    let pairs = [
        ("oidcAuthority", "RUNTARA_UI_OIDC_AUTHORITY"),
        ("oidcClientId", "RUNTARA_UI_OIDC_CLIENT_ID"),
        ("oidcAudience", "RUNTARA_UI_OIDC_AUDIENCE"),
        ("apiBaseUrl", "RUNTARA_UI_API_BASE_URL"),
        ("plausibleDomain", "RUNTARA_UI_PLAUSIBLE_DOMAIN"),
        ("plausibleHost", "RUNTARA_UI_PLAUSIBLE_HOST"),
    ];
    let mut entries: Vec<(String, String)> = Vec::new();
    for (key, env) in pairs {
        if let Ok(val) = std::env::var(env)
            && !val.trim().is_empty()
        {
            entries.push((key.to_string(), val));
        }
    }

    entries.push(("version".to_string(), env!("BUILD_VERSION").to_string()));
    entries.push(("commit".to_string(), env!("BUILD_COMMIT").to_string()));
    let build_number = env!("BUILD_NUMBER");
    if !build_number.is_empty() {
        entries.push(("buildNumber".to_string(), build_number.to_string()));
    }

    // Normalize the provider name to the three values the SPA branches on.
    // Anything unrecognized degrades to "oidc" so the SPA behaves like before.
    let auth_mode = match std::env::var("AUTH_PROVIDER")
        .unwrap_or_else(|_| "oidc".to_string())
        .as_str()
    {
        "local" => "local",
        "trust_proxy" | "trust-proxy" => "trust_proxy",
        _ => "oidc",
    };
    entries.push(("authMode".to_string(), auth_mode.to_string()));

    if let Ok(tenant) = std::env::var("TENANT_ID")
        && !tenant.trim().is_empty()
    {
        entries.push(("tenantId".to_string(), tenant));
    }

    // Operator switch: when set, the SPA stops prefixing /api/runtime/ with the
    // org_id. Use this for single-tenant deployments where the server already
    // resolves the tenant from auth context. Accepts truthy "1"/"true"/"yes".
    if let Ok(raw) = std::env::var("RUNTARA_UI_STRIP_ORG_ID")
        && matches!(
            raw.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes"
        )
    {
        entries.push(("stripOrgId".to_string(), "true".to_string()));
    }

    let mut out = String::from("{");
    for (i, (key, val)) in entries.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        // Keys are fixed identifiers, values are untrusted env content — JSON-escape.
        let _ = write!(out, "\"{}\":{}", key, json_string(val));
    }
    // `version`, `commit`, and `authMode` are pushed unconditionally above, so
    // `entries` is never empty in practice — but guard the leading comma anyway
    // to keep the function safe if the unconditional pushes ever change.
    if !entries.is_empty() {
        out.push(',');
    }
    out.push_str("\"entitlements\":");
    out.push_str(&entitlements_inline_json(entitlements));
    out.push('}');
    out
}

/// Serialise the entitlement snapshot as a JSON object literal suitable for
/// inlining inside a `<script>` tag. Defangs `<`/`>` in any string value so a
/// `</script>` token inside e.g. `tenantId` can't break out of the inline
/// script. JSON syntax doesn't use `<` or `>` outside string literals, so a
/// blanket replace is safe.
fn entitlements_inline_json(entitlements: &EntitlementSnapshot) -> String {
    let dto = EntitlementsDto::from(entitlements);
    serde_json::to_string(&dto)
        .expect("EntitlementsDto serialises to JSON")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
}

/// Minimal JSON string encoder — escapes the small set of chars that are illegal
/// inside a JSON string literal. Sufficient for env var values that we embed
/// directly in a `<script>` tag (we also escape `<` and `>` to defang any
/// accidental `</script>` tokens).
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

async fn serve(uri: Uri, State(state): State<UiState>) -> Response {
    // Routes are registered with the full mount path (e.g. `/ui`, `/ui/`,
    // `/ui/{*path}`) so uri.path() here includes the mount. Strip it to get
    // the asset-relative path.
    let path = uri.path();
    let after_mount = path
        .strip_prefix(state.mount.as_ref())
        .unwrap_or(path)
        .trim_start_matches('/');

    if after_mount.is_empty() || after_mount == "index.html" {
        return html_response(current_index_html(&state), &state.html_csp);
    }

    if let Some(file) = UiAssets::get(after_mount) {
        return asset_response(after_mount, file, &state.html_csp);
    }

    // SPA fallback: unknown paths hand back index.html so React Router can route.
    html_response(current_index_html(&state), &state.html_csp)
}

/// Return the index.html body to serve for this request.
///
/// Release builds reuse the startup-cached bytes; debug builds rebuild from
/// disk every request so `npm run build:watch` output is picked up without a
/// server restart. Asset hashes in index.html change on every frontend build,
/// so the release cache would quickly go stale without this split.
fn current_index_html(state: &UiState) -> Bytes {
    #[cfg(debug_assertions)]
    {
        let (html, _hash) =
            build_index_html(state.base_href.as_ref(), crate::config::entitlements());
        html
    }
    #[cfg(not(debug_assertions))]
    {
        state.index_html.clone()
    }
}

fn html_response(body: Bytes, csp: &str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )
        .header(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))
        .header(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_str(csp).expect("CSP header must be ASCII"),
        )
        .body(Body::from(body))
        .unwrap()
}

fn asset_response(path: &str, file: EmbeddedFile, csp: &str) -> Response {
    let mime = file
        .metadata
        .mimetype()
        .parse::<HeaderValue>()
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let cache_control = if path.ends_with(".webmanifest") || path == "sw.js" {
        HeaderValue::from_static("no-cache")
    } else {
        HeaderValue::from_static("public, max-age=31536000, immutable")
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, cache_control)
        .header(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_str(csp).expect("CSP header must be ASCII"),
        )
        .body(Body::from(file.data.into_owned()))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csp_allows_wasm_validation_without_general_eval() {
        let csp = build_html_csp("inline-config-hash");

        assert!(csp.contains("'wasm-unsafe-eval'"));
        assert!(!csp.contains("'unsafe-eval'"));
    }

    #[test]
    fn csp_allows_custom_plausible_host() {
        let csp = build_html_csp_with_plausible_source(
            "inline-config-hash",
            Some("https://metrics.syncmyorders.com"),
        );

        assert!(csp.contains("https://metrics.syncmyorders.com"));
    }

    #[test]
    fn plausible_script_source_normalizes_scheme_less_host() {
        assert_eq!(
            normalize_plausible_script_src("metrics.syncmyorders.com"),
            Some("https://metrics.syncmyorders.com".to_string())
        );
    }

    #[test]
    fn plausible_script_source_uses_origin_only() {
        assert_eq!(
            normalize_plausible_script_src("https://metrics.syncmyorders.com/proxy/"),
            Some("https://metrics.syncmyorders.com".to_string())
        );
    }

    #[test]
    fn plausible_script_source_handles_protocol_relative_host() {
        assert_eq!(
            normalize_plausible_script_src("//metrics.syncmyorders.com/"),
            Some("https://metrics.syncmyorders.com".to_string())
        );
    }

    #[test]
    fn plausible_script_source_ignores_same_origin_path() {
        assert_eq!(normalize_plausible_script_src("/plausible/"), None);
    }

    use crate::entitlements::{EntitlementSnapshot, parse_agents};
    use std::collections::BTreeSet;

    fn registered_agents() -> BTreeSet<String> {
        parse_agents(&["http", "csv", "xml", "openai", "anthropic"])
    }

    fn fixture_snapshot(tenant_id: &str, entitlements_json: Option<&str>) -> EntitlementSnapshot {
        EntitlementSnapshot::parse_entitlements(
            tenant_id,
            None,
            entitlements_json,
            None,
            &registered_agents(),
        )
        .expect("fixture snapshot parses")
    }

    #[test]
    fn entitlements_inline_json_is_a_camel_case_object() {
        let snap = fixture_snapshot("tenant-abc", None);
        let json = entitlements_inline_json(&snap);

        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON object");
        let obj = value.as_object().expect("object");
        for key in ["tenantId", "pricingTier", "features", "agents", "limits"] {
            assert!(obj.contains_key(key), "missing key {key}: {json}");
        }
        // Sanity-check nested camelCase from EntitlementsDto.
        assert_eq!(obj["tenantId"], serde_json::json!("tenant-abc"));
        assert_eq!(obj["features"]["reports"], serde_json::json!(true));
    }

    #[test]
    fn entitlements_inline_json_defangs_script_breakouts() {
        // Tenant id contains `</script>` — must be escaped so the inline script
        // body can't be terminated early in the HTML.
        let snap = fixture_snapshot("</script>evil", None);
        let json = entitlements_inline_json(&snap);

        assert!(
            !json.contains("</script>"),
            "raw </script> must not appear in inline JSON: {json}"
        );
        assert!(
            json.contains("\\u003c/script\\u003e"),
            "expected defanged </script> token: {json}"
        );
        // Still parses as JSON — defanging is inside a string literal, so
        // round-trips back to the original value.
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(value["tenantId"], serde_json::json!("</script>evil"));
    }

    #[test]
    fn runtime_config_json_embeds_entitlements_as_nested_object() {
        let snap = fixture_snapshot("tenant-xyz", Some(r#"{"features":{"reports":false}}"#));
        let raw = runtime_config_json(&snap);

        let value: serde_json::Value = serde_json::from_str(&raw).expect("valid JSON object");
        let ents = value
            .get("entitlements")
            .expect("entitlements key present in runtime config");
        assert!(
            ents.is_object(),
            "entitlements must be an object, not a string"
        );
        assert_eq!(ents["tenantId"], serde_json::json!("tenant-xyz"));
        assert_eq!(ents["features"]["reports"], serde_json::json!(false));
        assert_eq!(ents["features"]["database"], serde_json::json!(true));
    }

    #[test]
    fn inlined_script_contains_entitlements_payload() {
        let snap = fixture_snapshot("tenant-html", None);
        let (html_bytes, _hash) = build_index_html("/ui/", &snap);
        let html = std::str::from_utf8(&html_bytes).expect("utf-8");

        // The inline script is `window.__RUNTARA_CONFIG__={...};` — locate the body
        // between the assignment and the trailing semicolon.
        let prefix = "window.__RUNTARA_CONFIG__=";
        let start = html.find(prefix).expect("inline config script present");
        let after = &html[start + prefix.len()..];
        let end = after.find(";</script>").or_else(|| after.find(";")).expect(
            "inline script terminator present (expected `;</script>` or `;` from build output)",
        );
        let body = &after[..end];

        let value: serde_json::Value = serde_json::from_str(body)
            .unwrap_or_else(|e| panic!("inline body should be JSON: {e}\nbody: {body}"));
        let ents = value
            .get("entitlements")
            .expect("entitlements key inlined into window.__RUNTARA_CONFIG__");
        assert_eq!(ents["tenantId"], serde_json::json!("tenant-html"));
        assert!(ents["features"].is_object());
        assert!(ents["agents"].is_array());
        assert!(ents["limits"].is_object());
    }

    #[test]
    fn csp_hash_covers_entitlements_payload() {
        use base64::Engine;
        use sha2::Digest;

        let snap = fixture_snapshot("tenant-csp", None);
        let (html_bytes, hash_b64) = build_index_html("/ui/", &snap);
        let html = std::str::from_utf8(&html_bytes).expect("utf-8");

        // Locate the exact inline script body — everything between
        // `window.__RUNTARA_CONFIG__=` and the terminating `;` that build_index_html
        // splices in. The hash in the CSP must equal SHA-256 of THIS string,
        // including the trailing semicolon (see build_index_html's `inline_script`).
        let needle_start = "window.__RUNTARA_CONFIG__=";
        let start_idx = html.find(needle_start).expect("config assignment present");
        // The script body in build_index_html ends at the `;` that closes the
        // statement — find the first `;` after the start.
        let after = &html[start_idx..];
        let semi = after.find(';').expect("semicolon terminator present");
        let inline_script = &after[..=semi];

        let expected = base64::engine::general_purpose::STANDARD
            .encode(sha2::Sha256::digest(inline_script.as_bytes()));
        assert_eq!(
            hash_b64, expected,
            "CSP hash must match SHA-256 of the inlined script body — drift would break CSP"
        );
    }
}
