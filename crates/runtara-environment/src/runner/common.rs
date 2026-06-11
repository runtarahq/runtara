// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Helpers shared by the workflow runners (CLI process and embedded).
//!
//! Both runners speak the exact same contract to the guest (env vars) and to
//! the rest of the platform (output read from runtara-core persistence,
//! stderr read from the per-run log file) — extracting these keeps the two
//! implementations bit-for-bit compatible where it matters.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::fs;
use tracing::debug;

use runtara_core::persistence::Persistence;

use super::traits::{Result, RunnerError};
use super::wasm::WasmRunnerConfig;

/// Build the environment variables every workflow instance receives.
pub(crate) fn build_env(
    config: &WasmRunnerConfig,
    instance_id: &str,
    tenant_id: &str,
    runtara_core_addr: &str,
    checkpoint_id: Option<&str>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("RUNTARA_INSTANCE_ID".to_string(), instance_id.to_string());
    env.insert("RUNTARA_TENANT_ID".to_string(), tenant_id.to_string());
    // Suppress verbose tracing in WASM workflows to reduce stderr output.
    env.insert("RUST_LOG".to_string(), "warn".to_string());
    env.insert(
        "RUNTARA_HTTP_URL".to_string(),
        format!("http://{}", runtara_core_addr),
    );
    env.insert(
        "RUNTARA_SERVER_ADDR".to_string(),
        runtara_core_addr.to_string(),
    );

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

    // Forward SDK backend selection and HTTP URL if set in host environment.
    if let Ok(backend) = std::env::var("RUNTARA_SDK_BACKEND") {
        env.insert("RUNTARA_SDK_BACKEND".to_string(), backend);
    }
    if let Ok(url) = std::env::var("RUNTARA_HTTP_URL") {
        env.insert("RUNTARA_HTTP_URL".to_string(), url);
    }
    if let Ok(port) = std::env::var("RUNTARA_CORE_HTTP_PORT") {
        env.insert("RUNTARA_CORE_HTTP_PORT".to_string(), port);
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

/// Create the run directory for stderr capture.
pub(crate) async fn ensure_run_dir(
    data_dir: &Path,
    tenant_id: &str,
    instance_id: &str,
) -> Result<()> {
    let dir = run_dir(data_dir, tenant_id, instance_id);
    fs::create_dir_all(&dir).await?;
    debug!(instance_id = %instance_id, "Run directory created");
    Ok(())
}

/// Load output from runtara-core persistence.
///
/// The SDK reports completion/failure to runtara-core via HTTP during
/// execution. By the time the guest exits, the instance record is already
/// persisted.
pub(crate) async fn load_output(persistence: &dyn Persistence, instance_id: &str) -> Result<Value> {
    match persistence.get_instance(instance_id).await {
        Ok(Some(inst)) => match inst.status.as_str() {
            "completed" => {
                if let Some(output_bytes) = inst.output {
                    serde_json::from_slice(&output_bytes)
                        .map_err(|e| RunnerError::Other(format!("Failed to parse output: {}", e)))
                } else {
                    Ok(Value::Null)
                }
            }
            "failed" => {
                let error = inst.error.unwrap_or_else(|| "Unknown error".to_string());
                Err(RunnerError::Other(error))
            }
            "cancelled" => Err(RunnerError::Cancelled),
            status => Err(RunnerError::Other(format!(
                "Unexpected instance status after exit: {}",
                status
            ))),
        },
        Ok(None) => Err(RunnerError::OutputNotFound(instance_id.to_string())),
        Err(e) => Err(RunnerError::Other(format!(
            "Failed to query instance status: {}",
            e
        ))),
    }
}

/// Load stderr from the per-run log file for diagnostics.
pub(crate) async fn load_stderr(
    data_dir: &Path,
    tenant_id: &str,
    instance_id: &str,
) -> Option<String> {
    let stderr_path = run_dir(data_dir, tenant_id, instance_id).join("stderr.log");
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
                let preview = lines.join("\n");
                let truncated = if preview.len() > 2000 {
                    format!("{}...", &preview[..2000])
                } else {
                    preview
                };
                return Some(truncated);
            }
        }
    }

    None
}
