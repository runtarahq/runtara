//! Staging + catalog overlay for PUBLISHED workflow-agents.
//!
//! A workflow published as an agent produces two files in a per-tenant staging
//! dir, under the exact naming convention native agents use:
//!
//! ```text
//! $DATA_DIR/workflow-agents/<tenant>/runtara_agent_<slug snake>.wasm
//! $DATA_DIR/workflow-agents/<tenant>/runtara_agent_<slug snake>.meta.json
//! ```
//!
//! - the `.wasm` is the composed `AgentCapabilities` artifact (exports
//!   `runtara:agent-<slug>/capabilities`), which a PARENT workflow composes in
//!   like any native agent — the compile pipeline searches this dir after the
//!   primary components dir (`extra_component_dirs`);
//! - the `.meta.json` is the synthesized [`AgentInfo`]
//!   (`workflow_agent_info`), which the catalog overlay merges into the boot
//!   catalog so save-time validation and capability checks see the published
//!   agent.
//!
//! The overlay is read per call (a few small JSON files) rather than cached —
//! publishes take effect immediately with no invalidation machinery.

use std::path::PathBuf;
use std::sync::Arc;

use runtara_dsl::agent_meta::{AgentCatalog, AgentInfo};

/// Per-tenant staging dir for published workflow-agents.
pub fn staging_dir(tenant_id: &str) -> PathBuf {
    data_dir().join("workflow-agents").join(tenant_id)
}

fn data_dir() -> PathBuf {
    let raw = PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| ".data".to_string()));
    if raw.is_absolute() {
        raw
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&raw))
            .unwrap_or(raw)
    }
}

/// Load the tenant's published workflow-agent metadata. Missing dir → empty;
/// an unparseable sidecar is skipped with a warning (one bad publish must not
/// blind validation to the rest).
pub fn load_tenant_agents(tenant_id: &str) -> Vec<AgentInfo> {
    let dir = staging_dir(tenant_id);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut agents = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !(name.starts_with("runtara_agent_") && name.ends_with(".meta.json")) {
            continue;
        }
        match std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                serde_json::from_slice::<AgentInfo>(&bytes).map_err(|e| e.to_string())
            }) {
            Ok(info) => agents.push(info),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping unparseable workflow-agent sidecar");
            }
        }
    }
    // Deterministic order for stable catalogs/diffs.
    agents.sort_by(|a, b| a.id.cmp(&b.id));
    agents
}

/// The boot catalog merged with the tenant's published workflow-agents.
/// Returns the base unchanged (no clone of the agent list) when the tenant has
/// none — the common case stays free.
pub fn catalog_with_workflow_agents(
    base: &Arc<AgentCatalog>,
    tenant_id: &str,
) -> Arc<AgentCatalog> {
    let overlay = load_tenant_agents(tenant_id);
    if overlay.is_empty() {
        return Arc::clone(base);
    }
    let mut agents = base.agents().to_vec();
    agents.extend(overlay);
    Arc::new(AgentCatalog::from_agents(agents))
}

/// Stage a published workflow-agent: copy the composed `.wasm` and write the
/// synthesized `.meta.json` sidecar. Returns `(wasm_path, meta_path)`.
pub fn stage(
    tenant_id: &str,
    slug: &str,
    composed_wasm: &std::path::Path,
    info: &AgentInfo,
) -> std::io::Result<(PathBuf, PathBuf)> {
    let dir = staging_dir(tenant_id);
    std::fs::create_dir_all(&dir)?;
    let snake = slug.replace('-', "_");
    let wasm_path = dir.join(format!("runtara_agent_{snake}.wasm"));
    let meta_path = dir.join(format!("runtara_agent_{snake}.meta.json"));
    std::fs::copy(composed_wasm, &wasm_path)?;
    let meta_json = serde_json::to_vec_pretty(info)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&meta_path, meta_json)?;
    Ok((wasm_path, meta_path))
}
