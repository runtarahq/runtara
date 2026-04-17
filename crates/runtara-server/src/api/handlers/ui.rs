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
    /// Built once at startup; cloned (cheap) per request.
    index_html: Bytes,
    /// Mount prefix (e.g. `/ui`), stripped from the request URI before looking
    /// up the asset. Lets multi-segment mounts like `/ui/foo` work correctly.
    mount: Arc<str>,
}

/// Build a router that serves the embedded UI under `mount` (e.g. `/ui`).
/// `base_href` is what the server injects into index.html's `<base href>` tag
/// (e.g. `/ui/` or `/ui/tenant-abc/`).
///
/// Avoids `Router::nest` — axum 0.8's nest interacts poorly with trailing
/// slashes (`/ui` matches but `/ui/` 404s). Registering explicit routes at the
/// outer level dodges the quirk.
pub fn router(mount: &str, base_href: &str) -> Router {
    let state = UiState {
        index_html: build_index_html(base_href),
        mount: Arc::from(mount),
    };
    let wild = format!("{mount}/{{*path}}");
    let with_slash = format!("{mount}/");
    Router::new()
        .route(mount, get(serve))
        .route(&with_slash, get(serve))
        .route(&wild, get(serve))
        .with_state(state)
}

fn build_index_html(base_href: &str) -> Bytes {
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
    let config_replacement = format!("window.__RUNTARA_CONFIG__={};", runtime_config_json());
    let rewritten = step_one.replacen(config_needle, &config_replacement, 1);

    Bytes::from(rewritten.into_bytes())
}

/// Serialize the runtime config as a JSON object literal. Only keys with a
/// non-empty env value are emitted, so absent values stay `undefined` in JS
/// (the frontend then falls back to the build-time VITE_* default, if any).
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
        return html_response(state.index_html.clone());
    }

    if let Some(file) = UiAssets::get(after_mount) {
        return asset_response(after_mount, file);
    }

    // SPA fallback: unknown paths hand back index.html so React Router can route.
    html_response(state.index_html.clone())
}

/// CSP that allows the SPA to actually run. The default middleware CSP is
/// `default-src 'none'`, which would block all assets, inline styles, OIDC,
/// and analytics. Operators tightening for production should front the server
/// with a reverse proxy that overrides this header.
const UI_CSP: &str = concat!(
    "default-src 'self'; ",
    "script-src 'self' https://plausible.io; ",
    "style-src 'self' 'unsafe-inline'; ",
    "img-src 'self' data: blob:; ",
    "font-src 'self' data:; ",
    "connect-src 'self' https: wss: http://localhost:* ws://localhost:*; ",
    "manifest-src 'self'; ",
    "worker-src 'self' blob:; ",
    "frame-ancestors 'none'; ",
    "object-src 'none'; ",
    "base-uri 'self'",
);

fn html_response(body: Bytes) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )
        .header(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))
        .header(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(UI_CSP),
        )
        .body(Body::from(body))
        .unwrap()
}

fn asset_response(path: &str, file: EmbeddedFile) -> Response {
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
            HeaderValue::from_static(UI_CSP),
        )
        .body(Body::from(file.data.into_owned()))
        .unwrap()
}
