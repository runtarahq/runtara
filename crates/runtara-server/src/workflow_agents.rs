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
//! - the `.wasm` is the composed `PublishedAgent` artifact (exports
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
//!
//! The artifact store is the source of truth: each published agent is one
//! record, `tenants/<hex tenant>/workflow-agents/<slug snake>.json`, whose
//! content is the `.wasm` and whose metadata holds the lowering it was built
//! with and the `.meta.json`'s exact bytes. The staging dir is a cache of
//! those records, written after them and rebuilt from them at boot
//! ([`sync_from_store`]), so a lost data dir loses nothing.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use runtara_environment::artifacts::{ArtifactStore, Manifest};

use runtara_dsl::agent_meta::{AgentCatalog, AgentInfo, CONTROL_AGENT_ID, canonical_agent_id};

/// Whether an agent id or workflow slug folds onto a built-in id no tenant
/// workflow-agent may take, whether or not that built-in is installed:
/// `control`, whose imports only the approved control agent receives.
pub fn folds_onto_reserved_agent(id: &str) -> bool {
    canonical_agent_id(id) == CONTROL_AGENT_ID
}

/// Whether the metadata claims a host-enforced execution mode only built-ins
/// may have: `trusted` (host credentials) or `suspends` (an agent component's
/// continuation state under an operation scope). A workflow-agent parks
/// through its `suspended` outcome instead, which its caller forwards.
fn claims_host_execution_mode(info: &AgentInfo) -> bool {
    info.capabilities.iter().any(|c| c.trusted || c.suspends)
}

static STORE: OnceLock<Arc<dyn ArtifactStore>> = OnceLock::new();

/// Keep published workflow-agents in `store`, the server's artifact store.
/// Set once, at boot; until then the default store under `DATA_DIR` is used.
pub fn install_store(store: Arc<dyn ArtifactStore>) {
    let _ = STORE.set(store);
}

fn store() -> Arc<dyn ArtifactStore> {
    STORE.get().cloned().unwrap_or_else(|| {
        Arc::new(runtara_environment::image_registry::default_artifact_store(
            &data_dir(),
        ))
    })
}

/// A published workflow-agent's record metadata.
#[derive(serde::Serialize, serde::Deserialize)]
struct StagedRecord {
    /// The lowering it was built with; empty when unknown.
    lowering: String,
    /// The `.meta.json`, byte for byte.
    meta: Box<serde_json::value::RawValue>,
}

fn tenant_prefix(tenant_id: &str) -> String {
    format!(
        "tenants/{}/workflow-agents/",
        hex::encode(tenant_id.as_bytes())
    )
}

fn record_key(tenant_id: &str, snake: &str) -> std::io::Result<String> {
    let key = format!("{}{snake}.json", tenant_prefix(tenant_id));
    runtara_environment::artifacts::validate_manifest_key(&key).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("workflow-agent slug `{snake}` is not a valid artifact name"),
        )
    })?;
    Ok(key)
}

fn store_error(error: runtara_environment::artifacts::ArtifactError) -> std::io::Error {
    std::io::Error::other(error.to_string())
}

/// Record a staged agent in the store: content first, then its record.
async fn put_record(
    tenant_id: &str,
    snake: &str,
    wasm: &[u8],
    meta: &[u8],
    lowering: &str,
) -> std::io::Result<()> {
    let key = record_key(tenant_id, snake)?;
    let store = store();
    let content = store.put(wasm).await.map_err(store_error)?;
    let record = serde_json::to_vec(&StagedRecord {
        lowering: lowering.to_owned(),
        meta: serde_json::value::RawValue::from_string(
            String::from_utf8(meta.to_vec())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
        )
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
    })
    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let manifest = Manifest::new(content, &record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    store
        .put_manifest(&key, &manifest)
        .await
        .map_err(store_error)
}

/// Write a staged agent's cache files: sidecar first, then the `.wasm`, then
/// its lowering (none when unknown, which makes it stale).
fn write_staged_files(
    tenant_id: &str,
    snake: &str,
    wasm: &[u8],
    meta: &[u8],
    lowering: &str,
) -> std::io::Result<(PathBuf, PathBuf)> {
    let dir = staging_dir(tenant_id);
    std::fs::create_dir_all(&dir)?;
    let wasm_path = dir.join(format!("runtara_agent_{snake}.wasm"));
    let meta_path = dir.join(format!("runtara_agent_{snake}.meta.json"));
    let lowering_path = dir.join(format!("runtara_agent_{snake}.lowering"));
    // Sidecar FIRST: the compose-time stale-artifact gate reasons from the
    // sidecar's tags, so a crash mid-stage must never leave a `.wasm` without
    // its `.meta.json` (a wasm-only stage would hit the gate's anomalous
    // missing-sidecar branch and fail parent compiles until re-published).
    std::fs::write(&meta_path, meta)?;
    std::fs::write(&wasm_path, wasm)?;
    // Last: an artifact with no record of its lowering is restaged.
    if lowering.is_empty() {
        match std::fs::remove_file(&lowering_path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    } else {
        std::fs::write(&lowering_path, lowering)?;
    }
    Ok((wasm_path, meta_path))
}

/// Make every tenant's staging dir hold exactly what the store records: a
/// staged agent with no record (staged before the store) is recorded first,
/// then every record is written to its tenant's dir. Run at boot, before
/// anything reads the staging dirs.
pub async fn sync_from_store() -> std::io::Result<(usize, usize)> {
    let store = store();
    let mut imported = 0;
    // Staged before the store: record them.
    if let Ok(tenants) = std::fs::read_dir(data_dir().join("workflow-agents")) {
        for tenant in tenants.flatten() {
            let Some(tenant_id) = tenant.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Ok(entries) = std::fs::read_dir(tenant.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(snake) = name
                    .to_str()
                    .and_then(|n| n.strip_prefix("runtara_agent_"))
                    .and_then(|n| n.strip_suffix(".meta.json"))
                    .map(str::to_owned)
                else {
                    continue;
                };
                let key = record_key(&tenant_id, &snake)?;
                if store
                    .get_manifest(&key)
                    .await
                    .map_err(store_error)?
                    .is_some()
                {
                    continue;
                }
                let dir = tenant.path();
                let (Ok(meta), Ok(wasm)) = (
                    std::fs::read(dir.join(format!("runtara_agent_{snake}.meta.json"))),
                    std::fs::read(dir.join(format!("runtara_agent_{snake}.wasm"))),
                ) else {
                    tracing::warn!(tenant = %tenant_id, agent = %snake, "staged workflow-agent is missing its .wasm or .meta.json; not recorded");
                    continue;
                };
                let lowering =
                    std::fs::read_to_string(dir.join(format!("runtara_agent_{snake}.lowering")))
                        .unwrap_or_default();
                put_record(&tenant_id, &snake, &wasm, &meta, &lowering).await?;
                imported += 1;
            }
        }
    }
    // The store is the truth: write every record to its staging dir.
    let mut written = 0;
    for key in store
        .list_manifests("tenants/")
        .await
        .map_err(store_error)?
    {
        let Some((tenant_hex, snake)) = key
            .strip_prefix("tenants/")
            .and_then(|rest| rest.split_once("/workflow-agents/"))
            .and_then(|(tenant, file)| Some((tenant, file.strip_suffix(".json")?)))
        else {
            continue;
        };
        let Some(tenant_id) = hex::decode(tenant_hex)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
        else {
            continue;
        };
        let manifest = match store.get_manifest(&key).await {
            Ok(Some(manifest)) => manifest,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(%key, %error, "unreadable workflow-agent record; not staged");
                continue;
            }
        };
        let Ok(record) = serde_json::from_slice::<StagedRecord>(manifest.metadata_bytes()) else {
            tracing::warn!(%key, "malformed workflow-agent record; not staged");
            continue;
        };
        let Some(wasm) = store.get(&manifest.content).await.map_err(store_error)? else {
            continue;
        };
        write_staged_files(
            &tenant_id,
            snake,
            &wasm,
            record.meta.get().as_bytes(),
            &record.lowering,
        )?;
        written += 1;
    }
    Ok((imported, written))
}

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
            Ok(info) if claims_host_execution_mode(&info) => {
                tracing::warn!(
                    "rejecting tenant workflow-agent with trusted or suspending capability metadata"
                );
            }
            Ok(info) if folds_onto_reserved_agent(&info.id) => {
                tracing::warn!(agent = %info.id, "excluding tenant workflow-agent whose id is reserved");
            }
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
    merge_catalog(base, overlay)
}

fn merge_catalog(base: &Arc<AgentCatalog>, overlay: Vec<AgentInfo>) -> Arc<AgentCatalog> {
    let mut agents = base.agents().to_vec();
    agents.extend(admitted(base, overlay));
    Arc::new(AgentCatalog::from_agents(agents))
}

/// The tenant's published workflow-agents as the catalog overlay admits them
/// on top of `base`, for lookups that serve them alongside the built-ins.
pub fn tenant_overlay(base: &AgentCatalog, tenant_id: &str) -> Vec<AgentInfo> {
    admitted(base, load_tenant_agents(tenant_id))
}

/// The overlay entries that may join `base`: a built-in id always wins, and
/// metadata claiming a host-enforced execution mode or the `control` id never
/// gets in.
fn admitted(base: &AgentCatalog, overlay: Vec<AgentInfo>) -> Vec<AgentInfo> {
    let built_in_ids: std::collections::HashSet<_> = base
        .agents()
        .iter()
        .map(|a| canonical_agent_id(&a.id))
        .collect();
    overlay
        .into_iter()
        .filter(|a| {
            !claims_host_execution_mode(a)
                && !folds_onto_reserved_agent(&a.id)
                && !built_in_ids.contains(&canonical_agent_id(&a.id))
        })
        .collect()
}

/// Canonical ids of the tenant's published workflow-agents — the exemption
/// set for the `enabled_agents` entitlement gate (a tenant's own workflows
/// are not licensed integrations). Unlike [`tenant_overlay`], it keeps an id
/// a built-in agent took after the publication; the gates never exempt a
/// registered dispatcher module, so such an id stays under the allowlist.
pub fn published_agent_ids(tenant_id: &str) -> std::collections::HashSet<String> {
    load_tenant_agents(tenant_id)
        .into_iter()
        .map(|info| canonical_agent_id(&info.id))
        .collect()
}

/// Remove a published workflow-agent's staged artifacts (best-effort; absent
/// files are fine). Called when the owning workflow is deleted: the DB row is
/// soft-deleted (which deliberately RESERVES the slug), but the staged agent
/// must stop being composable — otherwise parents keep compiling against a
/// workflow that no longer exists. Parents that already composed it keep
/// their baked copy (composition embeds bytes), exactly like a removed
/// native agent.
pub async fn unstage(tenant_id: &str, slug: &str) -> std::io::Result<()> {
    let dir = staging_dir(tenant_id);
    let snake = slug.replace('-', "_");
    // The record first: a cache file without one is gone at the next boot.
    store()
        .delete_manifest(&record_key(tenant_id, &snake)?)
        .await
        .map_err(store_error)?;
    for name in [
        format!("runtara_agent_{snake}.wasm"),
        format!("runtara_agent_{snake}.meta.json"),
        format!("runtara_agent_{snake}.lowering"),
    ] {
        match std::fs::remove_file(dir.join(&name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Stage a published workflow-agent: record the composed `.wasm` and the
/// synthesized `.meta.json` in the store, then write them to the staging dir.
/// Returns `(wasm_path, meta_path)`.
pub async fn stage(
    tenant_id: &str,
    slug: &str,
    composed_wasm: &std::path::Path,
    info: &AgentInfo,
) -> std::io::Result<(PathBuf, PathBuf)> {
    if claims_host_execution_mode(info) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "tenant workflow-agents cannot declare trusted or suspending capabilities",
        ));
    }
    if folds_onto_reserved_agent(slug) || folds_onto_reserved_agent(&info.id) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("workflow-agent id `{slug}` is reserved for a built-in agent"),
        ));
    }
    let snake = slug.replace('-', "_");
    let meta = serde_json::to_vec_pretty(info)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let wasm = std::fs::read(composed_wasm)?;
    let lowering = crate::config::workflow_lowering_tag();
    put_record(tenant_id, &snake, &wasm, &meta, &lowering).await?;
    write_staged_files(tenant_id, &snake, &wasm, &meta, &lowering)
}

/// A staged workflow-agent that was built by another lowering than this
/// server's, and the version it was published from when it recorded one.
#[derive(Debug, Clone)]
pub struct StaleAgent {
    pub slug: String,
    pub published: Option<(String, u32)>,
}

/// The tenant's staged workflow-agents that must be rebuilt: no record of
/// their lowering, another lowering than `lowering`, or `cancellable` waits
/// the runtime no longer loads.
pub fn stale_agents(tenant_id: &str, lowering: &str) -> Vec<StaleAgent> {
    let dir = staging_dir(tenant_id);
    let mut stale = Vec::new();
    for info in load_tenant_agents(tenant_id) {
        let snake = info.id.replace('-', "_");
        let recorded = std::fs::read_to_string(dir.join(format!("runtara_agent_{snake}.lowering")));
        let cancellable = std::fs::read(dir.join(format!("runtara_agent_{snake}.wasm")))
            .map(|bytes| {
                let marker = b"[cancellable]";
                bytes.windows(marker.len()).any(|window| window == marker)
            })
            .unwrap_or(false);
        if recorded.as_deref().ok() == Some(lowering) && !cancellable {
            continue;
        }
        let published = info
            .capabilities
            .iter()
            .find_map(|capability| capability.workflow_run.as_ref())
            .map(|run| (run.workflow_id.clone(), run.version));
        stale.push(StaleAgent {
            slug: info.id.clone(),
            published,
        });
    }
    stale
}

#[cfg(test)]
mod tests {
    use super::*;
    fn info(id: &str) -> AgentInfo {
        runtara_dsl::agent_meta::workflow_agent_info(
            id,
            id,
            "",
            &Default::default(),
            &Default::default(),
        )
    }
    #[test]
    fn tenant_catalog_cannot_shadow_builtins_or_declare_trust() {
        let mut builtin = info("s3-storage");
        builtin.name = "approved".into();
        let base = Arc::new(AgentCatalog::from_agents(vec![builtin]));
        let mut spoof = info("tenant-signer");
        spoof.capabilities[0].trusted = true;
        let merged = merge_catalog(&base, vec![info("S3_STORAGE"), spoof, info("tenant-safe")]);
        assert_eq!(merged.agents().len(), 2);
        assert!(
            merged
                .agents()
                .iter()
                .any(|a| a.id == "s3-storage" && a.name == "approved")
        );
        assert!(merged.agents().iter().any(|a| a.id == "tenant-safe"));
    }
    #[test]
    fn tenant_catalog_cannot_declare_suspension_or_take_the_control_id() {
        // The control agent need not be installed for `control` to stay reserved.
        let base = Arc::new(AgentCatalog::from_agents(vec![info("s3-storage")]));
        let mut parks = info("tenant-waiter");
        parks.capabilities[0].suspends = true;
        let merged = merge_catalog(
            &base,
            vec![parks, info("control"), info("CONTROL"), info("tenant-safe")],
        );
        let ids: Vec<_> = merged.agents().iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["s3-storage", "tenant-safe"]);
    }

    #[tokio::test]
    async fn the_tenant_overlay_serves_what_the_catalog_merge_admits() {
        // No other test uses this tenant.
        const TENANT: &str = "overlay-lookup-tenant";
        let _env = crate::test_env::ENV_MUTEX.lock().await;
        let data_dir = tempfile::tempdir().unwrap();
        let mut env = crate::test_env::EnvGuard::new();
        env.set("DATA_DIR", data_dir.path().to_str().unwrap());
        let wasm = data_dir.path().join("agent.wasm");
        std::fs::write(&wasm, b"").unwrap();
        // `stage` leaves slug reservation to the publish path, so a stale
        // publication can still carry a built-in's id.
        for id in ["s3-storage", "tenant-safe"] {
            stage(TENANT, id, &wasm, &info(id)).await.unwrap();
        }
        let base = Arc::new(AgentCatalog::from_agents(vec![info("s3-storage")]));

        let overlay = tenant_overlay(&base, TENANT);
        let ids: Vec<_> = overlay.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["tenant-safe"], "a built-in id always wins");
        let merged = catalog_with_workflow_agents(&base, TENANT);
        let ids: Vec<_> = merged.agents().iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["s3-storage", "tenant-safe"]);
    }

    #[tokio::test]
    async fn agents_staged_by_another_lowering_or_with_cancellable_waits_are_stale() {
        // No other test uses this tenant.
        const TENANT: &str = "stale-lowering-tenant";
        let _env = crate::test_env::ENV_MUTEX.lock().await;
        let data_dir = tempfile::tempdir().unwrap();
        let mut env = crate::test_env::EnvGuard::new();
        env.set("DATA_DIR", data_dir.path().to_str().unwrap());
        let lowering = crate::config::workflow_lowering_tag();
        let wasm = data_dir.path().join("agent.wasm");
        std::fs::write(&wasm, b"\0asm[async-lift]").unwrap();
        let waits = data_dir.path().join("waits.wasm");
        std::fs::write(&waits, b"\0asm[cancellable][waitable-set-wait]").unwrap();

        stage(TENANT, "fresh", &wasm, &info("fresh")).await.unwrap();
        let published = runtara_dsl::agent_meta::with_workflow_run(
            info("relowered"),
            runtara_dsl::agent_meta::WorkflowRunTarget {
                workflow_id: "wf-1".into(),
                version: 3,
                parks: false,
            },
        );
        stage(TENANT, "relowered", &wasm, &published).await.unwrap();
        let dir = staging_dir(TENANT);
        std::fs::write(dir.join("runtara_agent_relowered.lowering"), "older").unwrap();
        stage(TENANT, "unrecorded", &wasm, &info("unrecorded"))
            .await
            .unwrap();
        std::fs::remove_file(dir.join("runtara_agent_unrecorded.lowering")).unwrap();
        stage(TENANT, "waits", &waits, &info("waits"))
            .await
            .unwrap();

        let mut stale = stale_agents(TENANT, &lowering);
        stale.sort_by(|a, b| a.slug.cmp(&b.slug));
        let slugs: Vec<_> = stale.iter().map(|agent| agent.slug.as_str()).collect();
        assert_eq!(slugs, ["relowered", "unrecorded", "waits"]);
        assert_eq!(stale[0].published, Some(("wf-1".to_string(), 3)));
        assert_eq!(stale[1].published, None);

        unstage(TENANT, "relowered").await.unwrap();
        assert!(!dir.join("runtara_agent_relowered.lowering").exists());
    }

    /// The store is the truth: a lost staging dir is rebuilt from the records,
    /// and an agent staged before the store is recorded from its files.
    #[tokio::test]
    async fn staging_dirs_are_rebuilt_from_the_store_and_legacy_agents_recorded() {
        // No other test uses these tenants.
        const TENANT: &str = "store-sync-tenant";
        const LEGACY: &str = "Legacy Tenant";
        let _env = crate::test_env::ENV_MUTEX.lock().await;
        let data_dir = tempfile::tempdir().unwrap();
        let mut env = crate::test_env::EnvGuard::new();
        env.set("DATA_DIR", data_dir.path().to_str().unwrap());
        let wasm = data_dir.path().join("agent.wasm");
        std::fs::write(&wasm, b"\0asm published").unwrap();
        stage(TENANT, "kept", &wasm, &info("kept")).await.unwrap();
        let meta = std::fs::read(staging_dir(TENANT).join("runtara_agent_kept.meta.json")).unwrap();
        stage(TENANT, "dropped", &wasm, &info("dropped"))
            .await
            .unwrap();
        unstage(TENANT, "dropped").await.unwrap();

        // Staged before the store: files only, no record.
        let legacy = staging_dir(LEGACY);
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("runtara_agent_old.wasm"), b"\0asm old").unwrap();
        std::fs::write(
            legacy.join("runtara_agent_old.meta.json"),
            serde_json::to_vec(&info("old")).unwrap(),
        )
        .unwrap();

        std::fs::remove_dir_all(staging_dir(TENANT)).unwrap();
        assert_eq!(sync_from_store().await.unwrap(), (1, 2));

        let dir = staging_dir(TENANT);
        assert_eq!(
            std::fs::read(dir.join("runtara_agent_kept.wasm")).unwrap(),
            b"\0asm published"
        );
        assert_eq!(
            std::fs::read(dir.join("runtara_agent_kept.meta.json")).unwrap(),
            meta
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("runtara_agent_kept.lowering")).unwrap(),
            crate::config::workflow_lowering_tag()
        );
        assert!(
            !dir.join("runtara_agent_dropped.wasm").exists(),
            "unstaged stays gone"
        );
        assert_eq!(
            published_agent_ids(LEGACY).into_iter().collect::<Vec<_>>(),
            ["old"]
        );
        assert!(
            !legacy.join("runtara_agent_old.lowering").exists(),
            "an unknown lowering stays unknown, so it is restaged"
        );
        assert_eq!(sync_from_store().await.unwrap(), (0, 2), "recorded once");
    }

    #[tokio::test]
    async fn publication_rejects_suspension_and_reserved_ids_before_writing_any_files() {
        let mut parks = info("tenant-waiter");
        parks.capabilities[0].suspends = true;
        let err = stage(
            "test-tenant",
            "tenant-waiter",
            std::path::Path::new("no-source-needed"),
            &parks,
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("suspending"));

        let err = stage(
            "test-tenant",
            "control",
            std::path::Path::new("no-source-needed"),
            &info("control"),
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("reserved"));
        assert!(
            !staging_dir("test-tenant")
                .join("runtara_agent_control.meta.json")
                .exists()
        );
    }

    #[test]
    fn only_ids_folding_onto_control_are_reserved() {
        for id in ["control", "CONTROL", "Control"] {
            assert!(folds_onto_reserved_agent(id), "{id}");
        }
        for id in ["controls", "control-room", "my-control", "ctrl"] {
            assert!(!folds_onto_reserved_agent(id), "{id}");
        }
    }

    #[tokio::test]
    async fn publication_rejects_trust_before_writing_any_files() {
        let mut spoof = info("tenant-signer");
        spoof.capabilities[0].trusted = true;
        let err = stage(
            "test-tenant",
            "tenant-signer",
            std::path::Path::new("no-source-needed"),
            &spoof,
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        assert!(err.to_string().contains("trusted"));
    }
}
