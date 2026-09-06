// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Workflow runner configuration and contract helpers.
//!
//! The guest-facing contract (env vars) and the platform-facing one (stderr in
//! the per-run log file) live here, separate from the execution engine, so any
//! future runner inherits them unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::fs;

/// How much of a stderr preview is kept for diagnostics.
const STDERR_PREVIEW_CHARS: usize = 2000;

/// Cap a stderr preview at `max` characters, marking it when truncated.
///
/// Counted in characters, not bytes. Guest stderr is arbitrary text, and
/// slicing a `String` at a fixed byte offset panics whenever that offset lands
/// inside a multi-byte character — in the detached monitor task, where the
/// panic is furthest from anyone who could interpret it.
fn truncate_preview(preview: String, max: usize) -> String {
    match preview.char_indices().nth(max) {
        Some((cut, _)) => format!("{}...", &preview[..cut]),
        None => preview,
    }
}

/// Parse a boolean env var accepting the common opt-in forms: `true/false`,
/// `1/0`, `yes/no`, `on/off` (case-insensitive). Unknown values are `false`.
///
/// Deliberately the inverse of `runtara_core::config::parse_enabled_env`, which
/// backs the `*_ENABLED` opt-outs: this one guards a setting that must stay off
/// unless a deployment explicitly asks for it.
fn parse_bool_lenient(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

/// Configuration shared by workflow runners.
#[derive(Clone, Debug)]
pub struct WorkflowRunnerConfig {
    /// Data directory for per-run state (stderr capture).
    pub data_dir: PathBuf,
    /// Default execution timeout.
    pub default_timeout: Duration,
    /// Skip TLS certificate verification (passed to instances).
    pub skip_cert_verification: bool,
    /// Connection service URL for fetching credentials at runtime (passed to instances).
    pub connection_service_url: Option<String>,
}

impl WorkflowRunnerConfig {
    /// Create configuration from environment variables.
    ///
    /// - `DATA_DIR`: data directory for instance I/O (default: `.data`).
    /// - `EXECUTION_TIMEOUT_SECS`: default execution timeout in seconds (default: 300).
    /// - `RUNTARA_SKIP_CERT_VERIFICATION`: skip TLS cert verification (default: false).
    /// - `RUNTARA_CONNECTION_SERVICE_URL`: connection service URL (optional).
    pub fn from_env() -> Self {
        let data_dir_raw =
            PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string()));
        let data_dir = if data_dir_raw.is_absolute() {
            data_dir_raw
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(&data_dir_raw))
                .unwrap_or(data_dir_raw)
        };

        Self {
            data_dir,
            default_timeout: Duration::from_secs(
                std::env::var("EXECUTION_TIMEOUT_SECS")
                    .ok()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(300),
            ),
            skip_cert_verification: std::env::var("RUNTARA_SKIP_CERT_VERIFICATION")
                .ok()
                .map(|v| parse_bool_lenient(&v))
                .unwrap_or(false),
            // `RUNTARA_CONNECTION_SERVICE_URL` is the runner's own setting and
            // wins; `CONNECTION_SERVICE_URL` is the general name the rest of the
            // stack uses, accepted as a fallback so a deployment that sets only
            // that one still points guests at the right host.
            connection_service_url: std::env::var("RUNTARA_CONNECTION_SERVICE_URL")
                .or_else(|_| std::env::var("CONNECTION_SERVICE_URL"))
                .ok(),
        }
    }
}

/// Build the environment variables every workflow instance receives.
///
/// Modern composed artifacts import `runtara:workflow-runtime/runtime` as a
/// host import, satisfied in-process by [`crate::runtime_host`]. Legacy
/// composed artifacts use `wasi:http` to reach the same core API, so callers
/// that support them supply `core_http_url` explicitly. The normal runner API
/// leaves it unset when no embedded core is available.
pub(crate) fn build_env(
    config: &WorkflowRunnerConfig,
    instance_id: &str,
    tenant_id: &str,
    checkpoint_id: Option<&str>,
    core_http_url: Option<&str>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("RUNTARA_INSTANCE_ID".to_string(), instance_id.to_string());
    env.insert("RUNTARA_TENANT_ID".to_string(), tenant_id.to_string());
    // Suppress verbose tracing in WASM workflows to reduce stderr output.
    env.insert("RUST_LOG".to_string(), "warn".to_string());
    if config.skip_cert_verification {
        env.insert(
            "RUNTARA_SKIP_CERT_VERIFICATION".to_string(),
            "true".to_string(),
        );
    }
    if let Some(cp_id) = checkpoint_id {
        env.insert("RUNTARA_CHECKPOINT_ID".to_string(), cp_id.to_string());
    }
    if let Some(ref url) = config.connection_service_url {
        env.insert("CONNECTION_SERVICE_URL".to_string(), url.clone());
    }
    if let Some(core_http_url) = core_http_url {
        env.insert("RUNTARA_HTTP_URL".to_string(), core_http_url.to_string());
    }

    // Forward SDK backend selection if set in the host environment.
    if let Ok(backend) = std::env::var("RUNTARA_SDK_BACKEND") {
        env.insert("RUNTARA_SDK_BACKEND".to_string(), backend);
    }

    // RUNTARA_HTTP_PROXY_URL, RUNTARA_OBJECT_MODEL_URL,
    // RUNTARA_AGENT_SERVICE_URL and RUNTARA_TENANT_ID overrides arrive via
    // LaunchOptions.env (populated by the caller from its typed config) and
    // are merged into `env` by the caller of build_env.

    env
}

/// The per-instance run directory (stderr capture lives here).
pub(crate) fn run_dir(data_dir: &Path, tenant_id: &str, instance_id: &str) -> PathBuf {
    data_dir.join(tenant_id).join("runs").join(instance_id)
}

/// The per-launch run directory.
///
/// A durable instance can have a new physical run while stale cleanup for its
/// predecessor is still unwinding. Keeping stderr under the launch generation
/// prevents the old task from overwriting diagnostics for the new one.
pub(crate) fn launch_run_dir(
    data_dir: &Path,
    tenant_id: &str,
    instance_id: &str,
    launch_id: &str,
) -> PathBuf {
    run_dir(data_dir, tenant_id, instance_id).join(launch_id)
}

/// Load stderr from the per-run log file for diagnostics.
pub(crate) async fn load_stderr(
    data_dir: &Path,
    tenant_id: &str,
    instance_id: &str,
    launch_id: &str,
) -> Option<String> {
    let stderr_path =
        launch_run_dir(data_dir, tenant_id, instance_id, launch_id).join("stderr.log");
    if let Ok(stderr_content) = fs::read_to_string(&stderr_path).await {
        let stderr_trimmed = stderr_content.trim();
        if !stderr_trimmed.is_empty() {
            let lines: Vec<&str> = stderr_trimmed
                .lines()
                .filter(|line| {
                    let line_lower = line.to_lowercase();
                    !line_lower.contains("warning:")
                        && !line_lower.starts_with("at ")
                        && !line.trim().is_empty()
                })
                .take(10)
                .collect();

            if !lines.is_empty() {
                return Some(truncate_preview(lines.join("\n"), STDERR_PREVIEW_CHARS));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{
        STDERR_PREVIEW_CHARS, WorkflowRunnerConfig, build_env, parse_bool_lenient, truncate_preview,
    };
    use std::{path::PathBuf, time::Duration};

    fn config() -> WorkflowRunnerConfig {
        WorkflowRunnerConfig {
            data_dir: PathBuf::from("/tmp/runtara-runner-test"),
            default_timeout: Duration::from_secs(30),
            skip_cert_verification: false,
            connection_service_url: None,
        }
    }

    #[test]
    fn opt_in_booleans_default_to_off_for_anything_unrecognised() {
        for on in ["true", "1", "yes", "on", " TRUE ", "On"] {
            assert!(parse_bool_lenient(on), "{on:?} must read as enabled");
        }
        for off in ["false", "0", "no", "off", "", "disabled", "yep"] {
            assert!(!parse_bool_lenient(off), "{off:?} must read as disabled");
        }
    }

    /// Guest stderr is arbitrary text. Truncating it by byte offset panics the
    /// monitor task whenever the cut lands inside a multi-byte character, so
    /// the cap is counted in characters.
    #[test]
    fn a_stderr_preview_is_truncated_on_a_character_boundary() {
        // An em dash is 3 bytes, so byte 2000 falls *inside* a character:
        // this is the input the old byte slice `&preview[..2000]` panicked on.
        let dashes = "—".repeat(STDERR_PREVIEW_CHARS + 50);
        assert!(
            !dashes.is_char_boundary(STDERR_PREVIEW_CHARS),
            "the fixture must put a character across the old byte cut"
        );

        let truncated = truncate_preview(dashes.clone(), STDERR_PREVIEW_CHARS);
        assert!(truncated.ends_with("..."), "truncation must be marked");
        assert_eq!(
            truncated.chars().count(),
            STDERR_PREVIEW_CHARS + 3,
            "the cap counts characters, not bytes"
        );
        assert!(dashes.starts_with(truncated.trim_end_matches('.')));

        // A multi-byte string under the cap survives whole and unmarked.
        let short = "héllo wörld — ünicode".to_string();
        assert_eq!(truncate_preview(short.clone(), STDERR_PREVIEW_CHARS), short);

        // Exactly at the cap is not truncated; one past it is.
        let exact = "é".repeat(STDERR_PREVIEW_CHARS);
        assert_eq!(truncate_preview(exact.clone(), STDERR_PREVIEW_CHARS), exact);
        assert!(
            truncate_preview("é".repeat(STDERR_PREVIEW_CHARS + 1), STDERR_PREVIEW_CHARS)
                .ends_with("...")
        );
    }

    #[test]
    fn legacy_http_composed_guests_receive_the_configured_core_url() {
        let env = build_env(
            &config(),
            "instance-1",
            "tenant-1",
            None,
            Some("http://127.0.0.1:49123"),
        );

        assert_eq!(
            env.get("RUNTARA_HTTP_URL").map(String::as_str),
            Some("http://127.0.0.1:49123")
        );

        let env_without_core = build_env(&config(), "instance-1", "tenant-1", None, None);
        assert!(!env_without_core.contains_key("RUNTARA_HTTP_URL"));
    }
}
