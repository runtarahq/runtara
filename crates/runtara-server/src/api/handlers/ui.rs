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
    let (_built_index_html, inline_script_hash) = build_index_html(base_href);
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
fn build_index_html(base_href: &str) -> (Bytes, String) {
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
    //    OIDC/API/analytics values at runtime without per-tenant rebuilds.
    let config_needle = "window.__RUNTARA_CONFIG__={};";
    assert!(
        step_one.contains(config_needle),
        "embed-ui: expected `{config_needle}` placeholder in index.html. Check frontend/index.html."
    );
    let inline_script = format!("window.__RUNTARA_CONFIG__={};", runtime_config_json());
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
    format!(
        "default-src 'self'; \
         script-src 'self' https://plausible.io 'wasm-unsafe-eval' 'sha256-{inline_script_sha256_b64}'; \
         style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob:; \
         font-src 'self' data:; \
         connect-src 'self' https: wss: http://localhost:* ws://localhost:*; \
         manifest-src 'self'; \
         worker-src 'self' blob:; \
         frame-ancestors 'none'; \
         object-src 'none'; \
         base-uri 'self'"
    )
}

/// Serialize the runtime config as a JSON object literal. Only keys with a
/// non-empty env value are emitted, so absent values stay `undefined` in JS
/// (the frontend then falls back to the build-time VITE_* default, if any).
///
/// `authMode` and `tenantId` are always emitted: the SPA needs them to decide
/// whether to initiate an OIDC redirect and how to prefix tenant-scoped URLs.
fn runtime_config_json() -> String {
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
    out.push('}');
    out
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
        let (html, _hash) = build_index_html(state.base_href.as_ref());
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
}
