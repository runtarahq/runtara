//! `emit-meta` — walks every runtara-agent-* crate's macro-emitted statics and
//! writes a sibling `runtara_agent_<id>.meta.json` next to each `.wasm` in the
//! output directory.
//!
//! Usage:
//!     emit-meta <output-dir>
//!
//! Example:
//!     emit-meta target/wasm32-wasip2/release
//!
//! Run by `scripts/build-agent-components.sh` after `cargo component build`.
//! The agent dispatcher loads each pair (`.wasm` + `.meta.json`) at boot.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use runtara_dsl::agent_meta::AgentInfo;

/// One entry per migrated agent. The wasm filename is derived from the agent
/// id (with hyphens converted to underscores by cargo-component, e.g.
/// `azure-blob-storage` → `runtara_agent_azure_blob_storage.wasm`).
fn agents() -> Vec<(&'static str, AgentInfo)> {
    vec![
        ("ai-tools", runtara_agent_ai_tools::agent_info()),
        (
            "azure-blob-storage",
            runtara_agent_azure_blob_storage::agent_info(),
        ),
        ("bedrock", runtara_agent_bedrock::agent_info()),
        ("compression", runtara_agent_compression::agent_info()),
        ("crypto", runtara_agent_crypto::agent_info()),
        ("csv", runtara_agent_csv::agent_info()),
        ("datetime", runtara_agent_datetime::agent_info()),
        ("http", runtara_agent_http::agent_info()),
        ("hubspot", runtara_agent_hubspot::agent_info()),
        ("mailgun", runtara_agent_mailgun::agent_info()),
        ("mcp", runtara_agent_mcp::agent_info()),
        ("object-model", runtara_agent_object_model::agent_info()),
        ("openai", runtara_agent_openai::agent_info()),
        ("s3-storage", runtara_agent_s3_storage::agent_info()),
        ("sftp", runtara_agent_sftp::agent_info()),
        ("sharepoint", runtara_agent_sharepoint::agent_info()),
        ("shopify", runtara_agent_shopify::agent_info()),
        ("slack", runtara_agent_slack::agent_info()),
        ("stripe", runtara_agent_stripe::agent_info()),
        ("text", runtara_agent_text::agent_info()),
        ("transform", runtara_agent_transform::agent_info()),
        ("utils", runtara_agent_utils::agent_info()),
        ("xlsx", runtara_agent_xlsx::agent_info()),
        ("xml", runtara_agent_xml::agent_info()),
        // Subsequent agents are added here as they migrate to the macro-derived
        // metadata path. See docs/wasm-components-migration-plan.md.
    ]
}

fn meta_path_for(out_dir: &Path, agent_id: &str) -> PathBuf {
    let filename = format!("runtara_agent_{}.meta.json", agent_id.replace('-', "_"));
    out_dir.join(filename)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 {
        eprintln!("usage: emit-meta <output-dir>");
        std::process::exit(2);
    }
    let out_dir = PathBuf::from(&args[1]);
    if !out_dir.is_dir() {
        anyhow::bail!("output directory does not exist: {}", out_dir.display());
    }

    let mut written = 0usize;
    for (agent_id, info) in agents() {
        let path = meta_path_for(&out_dir, agent_id);
        let json = serde_json::to_string_pretty(&info)
            .with_context(|| format!("serialize AgentInfo for `{agent_id}`"))?;
        std::fs::write(&path, json)
            .with_context(|| format!("write meta.json for `{agent_id}` to {}", path.display()))?;
        written += 1;
        eprintln!("  wrote {}", path.display());
    }

    eprintln!(
        "✓ Emitted {written} meta.json file(s) into {}",
        out_dir.display()
    );
    Ok(())
}
