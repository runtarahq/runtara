use std::path::PathBuf;

/// Test the same declared component set that is shipped in release bundles.
pub fn bundle_dir() -> PathBuf {
    std::env::var_os("RUNTARA_AGENT_COMPONENTS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("target/agent-components")
        })
}
