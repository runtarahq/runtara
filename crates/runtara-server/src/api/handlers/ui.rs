use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, OnceLock};

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderValue, StatusCode, Uri, header},
    response::Response,
    routing::get,
};
use bytes::Bytes;
use regex::Regex;
#[cfg(feature = "embed-ui")]
use rust_embed::RustEmbed;

use crate::api::dto::entitlements::EntitlementsDto;
use crate::entitlements::EntitlementSnapshot;

#[cfg(feature = "embed-ui")]
#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct UiAssets;

/// Where the served UI bytes come from.
///
/// The two sources exist so that "which cargo profile am I running" and "can I
/// see my frontend changes" are independent questions. Embedding is a
/// compile-time snapshot: in a `--release` build `npm run build:watch` output is
/// invisible until the crate is recompiled and the ~113MB binary relinked. That
/// relink is also what makes a running watcher poison every cargo build, since
/// `build.rs` declares `rerun-if-changed=frontend/dist` under `embed-ui`.
///
/// `Disk` breaks that coupling: point a server of any profile at a dist
/// directory and it reads assets per request, so the frontend loop costs a
/// browser reload and no cargo work at all.
#[derive(Clone)]
pub enum AssetSource {
    /// Baked in at compile time via `embed-ui`. Release builds embed the bytes;
    /// debug builds have rust-embed read the compile-time dist path from disk.
    #[cfg(feature = "embed-ui")]
    Embedded,
    /// Read from `RUNTARA_UI_DIST_DIR` on every request.
    Disk(Arc<Path>),
}

/// Resolve the UI asset source for this process, or `None` when the server has
/// no UI to serve (built without `embed-ui` and no `RUNTARA_UI_DIST_DIR`).
///
/// `RUNTARA_UI_DIST_DIR` wins over the embedded copy: an operator who points at
/// a dist directory means "serve that", and silently preferring stale embedded
/// bytes is exactly the confusion this exists to remove.
pub fn resolve_source() -> Option<AssetSource> {
    let dist_dir = std::env::var("RUNTARA_UI_DIST_DIR")
        .ok()
        .map(|raw| raw.trim().to_string())
        .filter(|raw| !raw.is_empty());

    if let Some(dir) = dist_dir {
        let root = PathBuf::from(dir);
        if !root.join("index.html").is_file() {
            tracing::warn!(
                dist_dir = %root.display(),
                "RUNTARA_UI_DIST_DIR is set but index.html is not there yet — \
                 serving 503 until `npm run build` (or `build:watch`) writes it"
            );
        }
        return Some(AssetSource::Disk(Arc::from(root.into_boxed_path())));
    }

    #[cfg(feature = "embed-ui")]
    {
        Some(AssetSource::Embedded)
    }
    #[cfg(not(feature = "embed-ui"))]
    {
        None
    }
}

impl AssetSource {
    /// Whether the bytes behind this source can change while the process runs.
    /// Only an immutable source may have its rewritten index.html cached, and
    /// only its assets may be served `immutable`.
    fn is_mutable(&self) -> bool {
        match self {
            // rust-embed reads the compile-time dist path from disk in debug
            // builds (no `debug-embed` feature) and embeds in release.
            #[cfg(feature = "embed-ui")]
            Self::Embedded => cfg!(debug_assertions),
            Self::Disk(_) => true,
        }
    }

    pub fn describe(&self) -> String {
        match self {
            #[cfg(feature = "embed-ui")]
            Self::Embedded => {
                if cfg!(debug_assertions) {
                    "embedded (debug: read from the compile-time frontend/dist)".to_string()
                } else {
                    "embedded (compile-time snapshot)".to_string()
                }
            }
            Self::Disk(root) => format!("disk ({})", root.display()),
        }
    }
}

/// A UI asset's bytes plus the content type to serve them as.
struct Asset {
    data: Bytes,
    mime: HeaderValue,
}

#[derive(Clone)]
pub struct UiState {
    /// Where to read asset bytes from.
    source: AssetSource,
    /// index.html with `<base href>` rewritten to match the deployed mount
    /// prefix. Populated only for an immutable source; a mutable one rebuilds
    /// per request so `npm run build:watch` output is picked up without a
    /// restart. Asset hashes in index.html change on every frontend build, so a
    /// cache over a mutable source would go stale within seconds.
    index_html: Option<Bytes>,
    /// `<base href>` value to inject into index.html. Needed on every request
    /// when the source is mutable.
    base_href: Arc<str>,
    /// The exact inline config script body to splice into index.html, snapshotted
    /// at startup. Storing the string the CSP hash was taken over — rather than
    /// re-deriving it per request — makes the two impossible to disagree.
    inline_script: Arc<str>,
    /// Mount prefix (e.g. `/ui`), stripped from the request URI before looking
    /// up the asset. Lets multi-segment mounts like `/ui/foo` work correctly.
    mount: Arc<str>,
    /// CSP header for HTML responses. Contains a SHA-256 hash of the inline
    /// `window.__RUNTARA_CONFIG__` script so the browser lets it execute.
    /// Computed once at startup from the script body we inject, which is derived
    /// from env vars and entitlements that don't change during process lifetime
    /// — so the hash stays valid however often index.html is rewritten.
    html_csp: Arc<str>,
}

/// Build a router that serves the embedded UI under `mount` (e.g. `/ui`).
/// `base_href` is what the server injects into index.html's `<base href>` tag
/// (e.g. `/ui/` or `/ui/tenant-abc/`).
///
/// Avoids `Router::nest` — axum 0.8's nest interacts poorly with trailing
/// slashes (`/ui` matches but `/ui/` 404s). Registering explicit routes at the
/// outer level dodges the quirk.
pub fn router(mount: &str, base_href: &str, source: AssetSource) -> Router {
    // Snapshot the inline config script once. The CSP hash covers this exact
    // string and `build_index_html` splices in this exact string, so the two
    // cannot drift. It also means boot doesn't read index.html at all — a `Disk`
    // source may legitimately be mid-rebuild when the server starts.
    let inline_script = inline_config_script(crate::config::entitlements());
    let inline_script_hash = inline_config_script_sha256_b64(&inline_script);

    // Cache the rewritten HTML only when the bytes behind it cannot change.
    let index_html =
        if source.is_mutable() {
            None
        } else {
            Some(build_index_html(&source, base_href, &inline_script).expect(
                "embed-ui: failed to build index.html from the embedded frontend at startup",
            ))
        };

    let state = UiState {
        source,
        index_html,
        base_href: Arc::from(base_href),
        inline_script: Arc::from(inline_script.as_str()),
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

/// `<base href="...">`, tolerating attribute spacing and a self-closing `/>`.
fn base_tag_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"<base\s+href="[^"]*"\s*/?>"#).expect("valid base-tag regex"))
}

/// The runtime-config script element; capture group 1 is its body.
fn config_script_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?s)<script\s+id="runtara-runtime-config"[^>]*>(.*?)</script>"#)
            .expect("valid config-script regex")
    })
}

/// The inline `window.__RUNTARA_CONFIG__` script body that gets spliced into
/// index.html. Derived purely from env vars and the entitlement snapshot, so it
/// is stable for the process lifetime and can be hashed for the CSP without
/// touching dist.
///
/// `entitlements` is threaded through so the inlined snapshot matches the
/// process-wide `crate::config::entitlements()`. Taking it as an argument
/// (rather than reading the `OnceLock` here) lets tests pass a fixture snapshot
/// without initialising the global config.
fn inline_config_script(entitlements: &EntitlementSnapshot) -> String {
    format!(
        "window.__RUNTARA_CONFIG__={};",
        runtime_config_json(entitlements)
    )
}

/// Base64-encoded SHA-256 of the inline config script body. Goes into the
/// `script-src` CSP directive so the browser allows the inline script to run.
fn inline_config_script_sha256_b64(inline_script: &str) -> String {
    use base64::Engine;
    use sha2::Digest;

    let digest = sha2::Sha256::digest(inline_script.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(digest)
}

/// Rewrite index.html for the deployed mount prefix, or `None` when it can't be
/// read or parsed.
///
/// Fallible rather than panicking because a `Disk` source is read per request
/// and `vite build --watch` legitimately leaves dist absent or half-written for
/// a moment on every rebuild. Callers turn `None` into a 503; a panic here would
/// take down the request handler on a routine frontend save.
fn build_index_html(source: &AssetSource, base_href: &str, inline_script: &str) -> Option<Bytes> {
    let raw = match read_asset(source, "index.html") {
        Some(asset) => asset,
        None => {
            tracing::error!(
                source = %source.describe(),
                "index.html not found — run `npm run build` in ./frontend"
            );
            return None;
        }
    };
    let html = match std::str::from_utf8(&raw.data) {
        Ok(html) => html,
        Err(_) => {
            tracing::error!(source = %source.describe(), "index.html is not valid utf-8");
            return None;
        }
    };

    // 1. Rewrite `<base href>` so the SPA resolves asset URLs and computes its
    //    React Router basename against the deployed mount prefix.
    //
    //    Match the tag rather than an exact byte string: index.html is
    //    Prettier-owned, and a reflow to `<base href="/" />` must not panic the
    //    server at startup (it did — see commit bc87364d).
    let base_tag = match base_tag_regex().find(html) {
        Some(found) => found,
        None => {
            tracing::error!(
                source = %source.describe(),
                "expected a `<base href=\"...\">` tag in index.html. Check frontend/index.html."
            );
            return None;
        }
    };
    let mut step_one = String::with_capacity(html.len() + base_href.len());
    step_one.push_str(&html[..base_tag.start()]);
    step_one.push_str(&format!(r#"<base href="{base_href}">"#));
    step_one.push_str(&html[base_tag.end()..]);

    // 2. Populate `window.__RUNTARA_CONFIG__` so the SPA can read tenant-specific
    //    OIDC/API/analytics/entitlement values at runtime without per-tenant rebuilds.
    //
    //    Replace the script element's whole body, not just the assignment, so the
    //    body the CSP hash covers is byte-for-byte what the browser receives.
    //    Prettier indents the placeholder across three lines; replacing only the
    //    assignment would leave that whitespace in the served body and break the
    //    hash computed by `inline_config_script_sha256_b64`.
    let config_body = match config_script_regex()
        .captures(&step_one)
        .and_then(|caps| caps.get(1))
    {
        Some(body) => body,
        None => {
            tracing::error!(
                source = %source.describe(),
                "expected a `<script id=\"runtara-runtime-config\">` element in index.html. \
                 Check frontend/index.html."
            );
            return None;
        }
    };
    if !config_body.as_str().contains("__RUNTARA_CONFIG__") {
        tracing::error!(
            source = %source.describe(),
            "`<script id=\"runtara-runtime-config\">` in index.html no longer assigns \
             `window.__RUNTARA_CONFIG__`. Check frontend/index.html."
        );
        return None;
    }
    let mut rewritten = String::with_capacity(step_one.len() + inline_script.len());
    rewritten.push_str(&step_one[..config_body.start()]);
    rewritten.push_str(inline_script);
    rewritten.push_str(&step_one[config_body.end()..]);

    // The CSP `script-src 'sha256-...'` set at startup hashes exactly this body —
    // between the <script> tags, no surrounding whitespace, no tags — so what we
    // splice in here must stay byte-identical to `inline_config_script`.
    Some(Bytes::from(rewritten.into_bytes()))
}

/// Read one asset from the configured source.
///
/// `path` is the request path with the mount prefix already stripped, e.g.
/// `assets/index-D2f0poI5.js`.
fn read_asset(source: &AssetSource, path: &str) -> Option<Asset> {
    match source {
        #[cfg(feature = "embed-ui")]
        AssetSource::Embedded => {
            let file = UiAssets::get(path)?;
            let mime = file
                .metadata
                .mimetype()
                .parse::<HeaderValue>()
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
            Some(Asset {
                data: Bytes::from(file.data.into_owned()),
                mime,
            })
        }
        AssetSource::Disk(root) => {
            let relative = safe_relative_path(path)?;
            let full = root.join(relative);
            let data = std::fs::read(&full).ok()?;
            let mime = mime_guess::from_path(&full)
                .first_or_octet_stream()
                .as_ref()
                .parse::<HeaderValue>()
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
            Some(Asset {
                data: Bytes::from(data),
                mime,
            })
        }
    }
}

/// Reject anything that could escape the dist root before joining a
/// request-supplied path onto it.
///
/// Keeps only plain path segments: a `..`, an absolute path, or a Windows drive
/// prefix returns `None` rather than being normalised away, so no request can
/// address a file outside the directory the operator pointed at. Symlinks
/// *inside* the root are still followed — dist is generated by vite, which does
/// not create them, and the root is operator-supplied.
fn safe_relative_path(path: &str) -> Option<PathBuf> {
    let candidate = Path::new(path);
    let mut safe = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Normal(segment) => safe.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if safe.as_os_str().is_empty() {
        return None;
    }
    Some(safe)
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
/// before any network request completes — see `docs/entitlements.md`.
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
        return index_response(&state);
    }

    if let Some(asset) = read_asset(&state.source, after_mount) {
        return asset_response(after_mount, asset, &state);
    }

    // SPA fallback: unknown paths hand back index.html so React Router can route.
    index_response(&state)
}

/// Return the index.html body to serve for this request.
///
/// An immutable source reuses the bytes built at startup. A mutable one — a
/// `Disk` source, or a debug build reading the compile-time dist path — rebuilds
/// per request so `npm run build:watch` output is picked up without a restart.
/// Asset hashes in index.html change on every frontend build, so caching over a
/// mutable source would go stale within seconds.
fn index_response(state: &UiState) -> Response {
    let html = match &state.index_html {
        Some(cached) => Some(cached.clone()),
        None => build_index_html(
            &state.source,
            state.base_href.as_ref(),
            state.inline_script.as_ref(),
        ),
    };

    match html {
        Some(body) => html_response(body, &state.html_csp),
        // Reachable when a `Disk` source is mid-rebuild: `vite build --watch`
        // rewrites dist on every save. Say so plainly instead of 500ing, and
        // never cache it — the next reload should get the real page.
        None => frontend_unavailable_response(&state.source),
    }
}

/// 503 for "the frontend bundle isn't readable right now".
fn frontend_unavailable_response(source: &AssetSource) -> Response {
    let body = format!(
        "Frontend bundle unavailable.\n\nAsset source: {}\n\n\
         If a build is in flight this resolves on the next reload. Otherwise run \
         `npm run build` in crates/runtara-server/frontend.\n",
        source.describe()
    );
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .header(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; charset=utf-8"),
        )
        .header(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))
        .body(Body::from(body))
        .unwrap()
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

fn asset_response(path: &str, asset: Asset, state: &UiState) -> Response {
    // `immutable` is only true of a compile-time snapshot. Behind a mutable
    // source the same URL can hand back different bytes — vite reuses unhashed
    // names like `logo-icon.png` across rebuilds — so a year-long cache would
    // pin whatever the browser saw first.
    let cache_control =
        if state.source.is_mutable() || path.ends_with(".webmanifest") || path == "sw.js" {
            HeaderValue::from_static("no-cache")
        } else {
            HeaderValue::from_static("public, max-age=31536000, immutable")
        };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, asset.mime)
        .header(header::CACHE_CONTROL, cache_control)
        .header(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_str(&state.html_csp).expect("CSP header must be ASCII"),
        )
        .body(Body::from(asset.data))
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

    /// index.html is Prettier-owned: it reflowed `<base href="/">` to
    /// `<base href="/" />` (bc87364d) and panicked every `embed-ui` server at
    /// startup. Assert against the real embedded asset so a future reflow fails
    /// here rather than in production boot.
    #[cfg(feature = "embed-ui")]
    #[test]
    fn base_href_is_rewritten_to_the_mount_prefix() {
        let snap = fixture_snapshot("tenant-base", None);
        let html_bytes = build_index_html(
            &AssetSource::Embedded,
            "/ui/tenant-base/",
            &inline_config_script(&snap),
        )
        .expect("embedded index.html builds");
        let html = std::str::from_utf8(&html_bytes).expect("utf-8");

        assert!(
            html.contains(r#"<base href="/ui/tenant-base/">"#),
            "base href must be rewritten to the mount prefix; got:\n{}",
            &html[..html.len().min(400)]
        );
        assert!(
            !html.contains(r#"<base href="/">"#) && !html.contains(r#"<base href="/" />"#),
            "the placeholder base tag must not survive the rewrite"
        );
    }

    #[cfg(feature = "embed-ui")]
    #[test]
    fn inlined_script_contains_entitlements_payload() {
        let snap = fixture_snapshot("tenant-html", None);
        let html_bytes =
            build_index_html(&AssetSource::Embedded, "/ui/", &inline_config_script(&snap))
                .expect("embedded index.html builds");
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

    /// The CSP hash is computed at startup from `inline_config_script`, while the
    /// body the browser receives is spliced in by `build_index_html`. Nothing in
    /// the type system ties those two together, so assert they agree: drift means
    /// the browser refuses to run the config script and the SPA boots blank.
    #[cfg(feature = "embed-ui")]
    #[test]
    fn csp_hash_covers_entitlements_payload() {
        use base64::Engine;
        use sha2::Digest;

        let snap = fixture_snapshot("tenant-csp", None);
        let hash_b64 = inline_config_script_sha256_b64(&inline_config_script(&snap));
        let html_bytes =
            build_index_html(&AssetSource::Embedded, "/ui/", &inline_config_script(&snap))
                .expect("embedded index.html builds");
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

    /// Minimal stand-in for a vite `dist/`: enough of index.html for both
    /// rewrites to apply.
    fn write_fake_dist(root: &Path) {
        std::fs::write(
            root.join("index.html"),
            "<!doctype html><html><head><base href=\"/\">\
             <script id=\"runtara-runtime-config\">\n      window.__RUNTARA_CONFIG__={};\n    \
             </script>\
             <script type=\"module\" src=\"./assets/index-abc123.js\"></script>\
             </head><body></body></html>",
        )
        .expect("write index.html");
        std::fs::create_dir_all(root.join("assets")).expect("create assets dir");
        std::fs::write(root.join("assets/index-abc123.js"), b"console.log(1)")
            .expect("write asset");
    }

    #[test]
    fn disk_source_rewrites_index_html_from_the_dist_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_dist(dir.path());
        let source = AssetSource::Disk(Arc::from(dir.path().to_path_buf().into_boxed_path()));

        let snap = fixture_snapshot("tenant-disk", None);
        let html_bytes =
            build_index_html(&source, "/ui/tenant-disk/", &inline_config_script(&snap))
                .expect("disk index.html builds");
        let html = std::str::from_utf8(&html_bytes).expect("utf-8");

        assert!(html.contains(r#"<base href="/ui/tenant-disk/">"#));
        assert!(html.contains("\"tenantId\":\"tenant-disk\""));
    }

    /// `vite build --watch` rewrites dist on every save. A read landing in that
    /// window must degrade to a 503, not panic the request handler.
    #[test]
    fn disk_source_reports_missing_index_instead_of_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = AssetSource::Disk(Arc::from(dir.path().to_path_buf().into_boxed_path()));

        let snap = fixture_snapshot("tenant-empty", None);
        assert!(build_index_html(&source, "/ui/", &inline_config_script(&snap)).is_none());
    }

    #[test]
    fn disk_source_serves_assets_with_a_guessed_content_type() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_dist(dir.path());
        let source = AssetSource::Disk(Arc::from(dir.path().to_path_buf().into_boxed_path()));

        let asset = read_asset(&source, "assets/index-abc123.js").expect("asset found");
        assert_eq!(asset.data.as_ref(), b"console.log(1)");
        assert!(
            asset.mime.to_str().unwrap().contains("javascript"),
            "expected a javascript content type, got {:?}",
            asset.mime
        );
    }

    /// The dist root is operator-supplied but the path is request-supplied, so a
    /// traversal attempt must be refused rather than normalised.
    #[test]
    fn disk_source_refuses_paths_that_escape_the_dist_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_dist(dir.path());
        let outside = dir.path().join("outside-secret.txt");
        std::fs::write(&outside, b"secret").expect("write outside file");
        let nested = dir.path().join("dist");
        std::fs::create_dir_all(&nested).expect("create nested dist");
        write_fake_dist(&nested);
        let source = AssetSource::Disk(Arc::from(nested.clone().into_boxed_path()));

        for path in [
            "../outside-secret.txt",
            "assets/../../outside-secret.txt",
            "/etc/passwd",
        ] {
            assert!(
                read_asset(&source, path).is_none(),
                "path {path:?} must not resolve outside the dist root"
            );
        }

        assert!(safe_relative_path("..").is_none());
        assert!(safe_relative_path("").is_none());
        assert_eq!(
            safe_relative_path("assets/index-abc123.js"),
            Some(PathBuf::from("assets/index-abc123.js"))
        );
    }

    fn disk_state(root: &Path, snap: &EntitlementSnapshot) -> UiState {
        let inline_script = inline_config_script(snap);
        UiState {
            source: AssetSource::Disk(Arc::from(root.to_path_buf().into_boxed_path())),
            // Mutable source: no cached index.html, exactly as `router` builds it.
            index_html: None,
            base_href: Arc::from("/ui/"),
            inline_script: Arc::from(inline_script.as_str()),
            mount: Arc::from("/ui"),
            html_csp: Arc::from(
                build_html_csp(&inline_config_script_sha256_b64(&inline_script)).as_str(),
            ),
        }
    }

    async fn get(state: &UiState, path: &str) -> (StatusCode, String, String) {
        let response = serve(
            path.parse::<Uri>().expect("valid uri"),
            State(state.clone()),
        )
        .await;
        let status = response.status();
        let cache_control = response
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            String::from_utf8_lossy(&body).to_string(),
            cache_control,
        )
    }

    /// The regression this whole asset-source split exists for: a running server
    /// must serve what `npm run build:watch` just wrote, with no rebuild and no
    /// restart. Rewrite dist under the live state and assert the new asset hash
    /// comes back.
    #[tokio::test]
    async fn disk_source_picks_up_a_rebuild_without_a_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_dist(dir.path());
        let snap = fixture_snapshot("tenant-live", None);
        let state = disk_state(dir.path(), &snap);

        let (status, html, cache_control) = get(&state, "/ui/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains(r#"<base href="/ui/">"#));
        assert!(
            html.contains("assets/index-abc123.js"),
            "first build's asset hash should be served; got:\n{html}"
        );
        assert_eq!(
            cache_control, "no-cache",
            "a mutable source must not be cached by the browser"
        );

        // Simulate the next `vite build --watch` output: new hashed chunk, and
        // index.html pointing at it.
        std::fs::write(dir.path().join("assets/index-def456.js"), b"console.log(2)")
            .expect("write new asset");
        let bumped = std::fs::read_to_string(dir.path().join("index.html"))
            .expect("read index.html")
            .replace("index-abc123.js", "index-def456.js");
        std::fs::write(dir.path().join("index.html"), bumped).expect("rewrite index.html");

        let (status, html, _) = get(&state, "/ui/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            html.contains("assets/index-def456.js") && !html.contains("index-abc123.js"),
            "the rebuilt index.html must be served without a restart; got:\n{html}"
        );

        let (status, body, cache_control) = get(&state, "/ui/assets/index-def456.js").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "console.log(2)");
        assert_eq!(cache_control, "no-cache");
    }

    /// Mid-rebuild reads must degrade to a 503 the next reload clears, not a
    /// panicking handler.
    #[tokio::test]
    async fn disk_source_serves_503_while_dist_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let snap = fixture_snapshot("tenant-gap", None);
        let state = disk_state(dir.path(), &snap);

        let (status, body, cache_control) = get(&state, "/ui/").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.contains("Frontend bundle unavailable"));
        assert_eq!(cache_control, "no-store");

        // …and recovers once the build lands, same process.
        write_fake_dist(dir.path());
        let (status, html, _) = get(&state, "/ui/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains(r#"<base href="/ui/">"#));
    }

    /// Unknown paths are SPA routes, not asset misses: React Router needs
    /// index.html back so it can resolve the route client-side.
    #[tokio::test]
    async fn unknown_paths_fall_back_to_index_html() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_fake_dist(dir.path());
        let snap = fixture_snapshot("tenant-spa", None);
        let state = disk_state(dir.path(), &snap);

        let (status, html, _) = get(&state, "/ui/workflows/some-id").await;
        assert_eq!(status, StatusCode::OK);
        assert!(html.contains(r#"<base href="/ui/">"#));
    }

    /// A mutable source must not be cached anywhere: not the rewritten HTML, not
    /// the assets. Caching either is what made `npm run build:watch` invisible.
    #[test]
    fn disk_source_is_always_mutable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = AssetSource::Disk(Arc::from(dir.path().to_path_buf().into_boxed_path()));
        assert!(
            source.is_mutable(),
            "a disk source can change under a running server in any profile"
        );
    }
}
