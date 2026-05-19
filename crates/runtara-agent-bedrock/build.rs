// Generate the per-agent `wit/agent.wit` from the crate's `CARGO_PKG_NAME`.
// Same role as the macro-derived `meta.json` (which describes the agent's
// capabilities): never hand-edited, always derived from the agent's identity.
//
// The package version is pinned at 0.3.0 — the WIT *contract* version, which
// only changes when the invoke signature changes. Independent of the agent
// crate's version.

use std::env;
use std::fs;
use std::path::Path;

const WIT_TEMPLATE: &str = include_str!("../runtara-agent-wit/templates/agent.wit.in");

fn main() {
    let pkg_name = env::var("CARGO_PKG_NAME").expect("CARGO_PKG_NAME unset");
    let agent_id = pkg_name
        .strip_prefix("runtara-agent-")
        .unwrap_or_else(|| panic!("crate name `{pkg_name}` must start with `runtara-agent-`"));

    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR unset");
    let wit_dir = Path::new(&manifest_dir).join("wit");
    fs::create_dir_all(&wit_dir).expect("create wit dir");

    let wit = WIT_TEMPLATE.replace("{AGENT_ID}", agent_id);
    let wit_path = wit_dir.join("agent.wit");

    // Only write if the contents differ — keeps mtimes stable for incremental
    // builds when nothing changed.
    let needs_write = match fs::read_to_string(&wit_path) {
        Ok(existing) => existing != wit,
        Err(_) => true,
    };
    if needs_write {
        fs::write(&wit_path, &wit).expect("write wit/agent.wit");
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../runtara-agent-wit/templates/agent.wit.in");
    println!("cargo:rerun-if-env-changed=CARGO_PKG_NAME");
}
