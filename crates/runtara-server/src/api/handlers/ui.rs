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
    let needle = r#"<base href="/">"#;
    assert!(
        html.contains(needle),
        "embed-ui: expected `{needle}` placeholder in index.html. Check frontend/index.html."
    );
    let replacement = format!(r#"<base href="{base_href}">"#);
    Bytes::from(html.replacen(needle, &replacement, 1).into_bytes())
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

fn html_response(body: Bytes) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        )
        .header(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"))
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
        .body(Body::from(file.data.into_owned()))
        .unwrap()
}
