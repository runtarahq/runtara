//! `ComponentDispatcherService` — the host-facing API that
//! `AgentTestingService` calls into instead of dispatcher-image roundtrips.
//!
//! Loads `runtara_agent_*.wasm` + `runtara_agent_*.meta.json` pairs from a
//! directory at construction time, pre-instantiates each `.wasm`, and serves
//! the parsed `AgentInfo` directly to the server. The `.wasm` exports only
//! `invoke`; all metadata travels through the sidecar JSON.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use runtara_dsl::agent_meta::{AgentInfo, canonical_agent_id};
use serde::{Deserialize, Serialize};
use wasmtime::component::{ComponentNamedList, Lift, Lower, TypedFunc};
use wasmtime::{Engine, Store, UpdateDeadline};

use crate::bindings::exports::runtara::agent::capabilities::ErrorInfo;
use crate::engine::{EPOCH_TICK, EngineConfig, build_engine, spawn_epoch_ticker};
use crate::host_state::{
    CallContext, DEFAULT_GUEST_MEMORY_MAX_BYTES, DEFAULT_GUEST_TABLE_MAX_ELEMENTS, HostState,
    Termination,
};
use crate::registry::{LoadedAgent, build_linker, instantiate, load_agent};

/// Server-facing per-call request shape. Mirrors today's `TestAgentRequest`
/// in `runtara-server/src/api/dto/agent_testing.rs` so wiring is a near-pass-
/// through.
#[derive(Debug, Clone)]
pub struct TestCapabilityRequest {
    pub tenant_id: String,
    pub agent_id: String,
    pub capability_id: String,
    pub input: serde_json::Value,
    pub connection: Option<ResolvedConnection>,
}

/// A connection record resolved by the host before invoke. Mirrors today's
/// `ConnectionsFacade::get_with_parameters` output.
#[derive(Debug, Clone)]
pub struct ResolvedConnection {
    pub connection_id: String,
    pub integration_id: String,
    pub connection_subtype: Option<String>,
    pub parameters: serde_json::Value,
    pub rate_limit_config: Option<serde_json::Value>,
}

/// Result shape returned to the server. Mirrors today's `TestResult` in
/// `runtara-server/src/api/dto/agent_testing.rs`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub error: Option<TestError>,
    pub execution_time_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestError {
    pub code: String,
    pub message: String,
    pub category: String,
    pub severity: String,
    pub retryable: bool,
}

/// Routing context shared across calls — proxy URL, agent-service URL, etc.
/// Per-tenant fields go into `TestCapabilityRequest`.
#[derive(Debug, Clone)]
pub struct DispatcherEnv {
    pub proxy_url: String,
    pub agent_service_url: String,
    pub object_model_url: String,
    pub core_http_url: String,
}

/// Default wall-clock budget for a single `test_capability` invocation. The
/// operator-test surface is interactive; a capability that hasn't produced a
/// result in this long is wedged, not slow. Override with
/// `RUNTARA_TEST_CAPABILITY_TIMEOUT_SECS`.
const DEFAULT_TEST_CAPABILITY_TIMEOUT: Duration = Duration::from_secs(30);

fn parse_timeout(raw: Option<String>) -> Duration {
    raw.and_then(|v| v.parse::<u64>().ok())
        .filter(|s| *s > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_TEST_CAPABILITY_TIMEOUT)
}

fn parse_memory_max(raw: Option<String>) -> usize {
    raw.and_then(|v| v.parse::<usize>().ok())
        .filter(|b| *b > 0)
        .unwrap_or(DEFAULT_GUEST_MEMORY_MAX_BYTES)
}

pub struct ComponentDispatcherService {
    engine: Arc<Engine>,
    agents: HashMap<String, Arc<LoadedAgent>>,
    /// Snapshot of every loaded agent's metadata. Shared (`Arc`) so the
    /// server-side `AgentsService` + workflow validation paths can hold the
    /// same data without copying.
    catalog: Arc<runtara_dsl::agent_meta::AgentCatalog>,
    env: DispatcherEnv,
    /// Per-call wall-clock budget for `test_capability`.
    test_timeout: Duration,
    /// Per-call guest linear-memory cap for `test_capability`, in bytes.
    memory_max_bytes: usize,
}

impl ComponentDispatcherService {
    /// Build the service from a directory of `runtara_agent_*.wasm` files,
    /// each accompanied by a sibling `runtara_agent_*.meta.json`. The filename
    /// stem after the `runtara_agent_` prefix becomes the agent id (e.g.
    /// `runtara_agent_crypto.wasm` → agent id `crypto`).
    ///
    /// A missing `.meta.json` is a hard error — the `.wasm` is unusable to the
    /// server without metadata. Mismatched ids (filename stem vs.
    /// `meta.id`) are also rejected so registration can't silently misroute.
    pub async fn from_dir(component_dir: &Path, env: DispatcherEnv) -> Result<Self> {
        let engine = build_engine(&EngineConfig::default())?;
        // Drive the epoch clock for this engine so the per-call deadlines set in
        // `test_capability` can actually fire — without a ticker the epoch never
        // advances and an unbounded guest would run forever.
        spawn_epoch_ticker(Arc::clone(&engine));
        let linker = build_linker(&engine)?;

        let mut agents = HashMap::new();
        let mut agent_info: HashMap<String, AgentInfo> = HashMap::new();

        let entries = std::fs::read_dir(component_dir)
            .with_context(|| format!("read component directory {}", component_dir.display()))?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("wasm") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(stem_id) = stem.strip_prefix("runtara_agent_") else {
                continue;
            };
            // cargo-component drops the .wasm filename in snake_case (it
            // converts hyphens in the crate name to underscores). The
            // canonical agent id everywhere else — `meta.json`,
            // `AgentsService`, workflow DSL refs — is kebab. Convert here so
            // both halves of the bundle agree on the id format.
            let agent_id = canonical_agent_id(stem_id);

            let meta_path = path.with_extension("meta.json");
            let meta_bytes = std::fs::read(&meta_path).with_context(|| {
                format!(
                    "agent `{agent_id}`: missing sidecar metadata at {}",
                    meta_path.display()
                )
            })?;
            let mut info: AgentInfo = serde_json::from_slice(&meta_bytes).with_context(|| {
                format!(
                    "agent `{agent_id}`: failed to parse sidecar metadata {}",
                    meta_path.display()
                )
            })?;
            // Normalize both sides to kebab for the equality check —
            // cargo-component drops snake_case filenames, but the canonical
            // id everywhere else is kebab, so an agent crate can sensibly
            // write either form in its `agent_info().id` literal.
            if canonical_agent_id(&info.id) != agent_id {
                anyhow::bail!(
                    "agent id mismatch: filename stem is `{agent_id}` but meta.id is `{}`",
                    info.id
                );
            }
            // Force the catalog to key on the same kebab form `agents` uses
            // — otherwise an agent whose `agent_info().id` literal is
            // snake_case (e.g. "azure_blob_storage") loads into `agents` as
            // "azure-blob-storage" but registers in the catalog as
            // "azure_blob_storage", and `agent_info_of("azure-blob-storage")`
            // returns None while `agent_ids()` yields it.
            info.id = agent_id.clone();

            let loaded = load_agent(&engine, &linker, &path, &agent_id)?;

            agent_info.insert(agent_id.clone(), info);
            agents.insert(agent_id, loaded);
        }

        // Linker is consumed by `linker.instantiate_pre`; after every agent
        // is pre-instantiated we drop it — InstancePre carries everything we
        // need for repeated per-call instantiation.
        drop(linker);

        // Build the public catalog from the parsed `AgentInfo`s. Sorted by
        // id so API output + tests are deterministic.
        let mut by_id: Vec<(String, AgentInfo)> = agent_info.into_iter().collect();
        by_id.sort_by(|a, b| a.0.cmp(&b.0));
        let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(
            by_id.into_iter().map(|(_, v)| v).collect(),
        ));

        Ok(Self {
            engine,
            agents,
            catalog,
            env,
            test_timeout: parse_timeout(std::env::var("RUNTARA_TEST_CAPABILITY_TIMEOUT_SECS").ok()),
            memory_max_bytes: parse_memory_max(
                std::env::var("RUNTARA_TEST_CAPABILITY_MEMORY_MAX_BYTES").ok(),
            ),
        })
    }

    /// Whether the dispatcher knows about an agent. Used by the server-side
    /// routing decision: components-mode for known agents, legacy fallback
    /// for the rest. Matched canonically, so legacy snake_case or mixed-case
    /// ids resolve to the same agent.
    pub fn has_agent(&self, agent_id: &str) -> bool {
        self.agents.contains_key(&canonical_agent_id(agent_id))
    }

    /// All loaded agent ids.
    pub fn agent_ids(&self) -> impl Iterator<Item = &str> {
        self.agents.keys().map(String::as_str)
    }

    /// Full metadata for one agent (parsed from its sidecar `meta.json`).
    pub fn agent_info_of(&self, agent_id: &str) -> Option<&AgentInfo> {
        self.catalog.agent(agent_id)
    }

    /// The shared agent catalog. Server-side validators + the
    /// `AgentsService` consume this instead of `runtara_agents::registry`
    /// so the runtime, not compile-time, is the source of truth.
    pub fn catalog(&self) -> Arc<runtara_dsl::agent_meta::AgentCatalog> {
        Arc::clone(&self.catalog)
    }

    /// Execute one capability and return a `TestResult` shaped for the
    /// server's existing `TestResult` DTO.
    pub async fn test_capability(&self, req: TestCapabilityRequest) -> Result<TestResult> {
        let agent = self
            .agents
            .get(&canonical_agent_id(&req.agent_id))
            .with_context(|| format!("unknown agent `{}`", req.agent_id))?;

        // Inject the connection id into the input under `_connection` — the
        // single connection channel now that `invoke` has no out-of-band
        // connection argument. Id-only, exactly like the composed-workflow
        // path (stdlib `agent-connection-input`): a connection is an opaque id,
        // and the proxy resolves credentials by (id, tenant), so nothing secret
        // rides the input. This also retires the old secret-materialization
        // here (`parameters` used to carry the real connection params).
        let mut input_value = req.input.clone();
        if let Some(c) = req.connection.as_ref()
            && let serde_json::Value::Object(ref mut obj) = input_value
        {
            obj.insert(
                "connection_id".into(),
                serde_json::Value::String(c.connection_id.clone()),
            );
            obj.insert(
                "_connection".into(),
                serde_json::json!({
                    "connection_id": c.connection_id,
                    "integration_id": "",
                    "parameters": {}
                }),
            );
        }
        let input_bytes = serde_json::to_vec(&input_value)?;

        let ctx = Arc::new(CallContext::for_test(
            &req.tenant_id,
            &self.env.proxy_url,
            &self.env.agent_service_url,
            &self.env.object_model_url,
            &self.env.core_http_url,
        ));
        let mut state = HostState::new(ctx);
        state.set_limits(self.memory_max_bytes, DEFAULT_GUEST_TABLE_MAX_ELEMENTS);
        let (mut store, instance) = instantiate(&self.engine, &agent.pre, state).await?;

        // Dynamic dispatch: look up the agent's capabilities interface by the
        // name we cached at load time (`runtara:agent-<id>/capabilities@…` for
        // per-agent WIT, `runtara:agent/capabilities@…` for the legacy
        // shared-WIT layout), then resolve `invoke` inside it and call with
        // the canonical signature.
        let iface_idx = instance
            .get_export_index(&mut store, None, &agent.capabilities_iface)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "agent `{}` instance is missing the `{}` interface export",
                    req.agent_id,
                    agent.capabilities_iface
                )
            })?;
        let invoke_idx = instance
            .get_export_index(&mut store, Some(&iface_idx), "invoke")
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "agent `{}` `{}` interface has no `invoke` export",
                    req.agent_id,
                    agent.capabilities_iface
                )
            })?;
        type InvokeFunc =
            wasmtime::component::TypedFunc<(String, Vec<u8>), (Result<Vec<u8>, ErrorInfo>,)>;
        let invoke: InvokeFunc = instance.get_typed_func(&mut store, invoke_idx)?;

        let started = Instant::now();
        let outcome = call_with_guards(
            &mut store,
            self.test_timeout,
            invoke,
            (req.capability_id.clone(), input_bytes),
        )
        .await;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

        let (result,) = match outcome {
            GuardOutcome::Returned(ret) => ret,
            GuardOutcome::TimedOut => {
                return Ok(timeout_result(elapsed_ms, self.test_timeout));
            }
            GuardOutcome::Trapped(trap) => {
                // A guest that blew the memory cap traps on the failed grow; the
                // limiter flags it so we can surface a clean error instead of an
                // opaque wasm trap. Any other trap is a genuine guest fault and
                // propagates as before (the server maps it to an ExecutionError).
                if store.data().limiter.denied_memory_grow {
                    return Ok(memory_limit_result(
                        elapsed_ms,
                        store.data().limiter.max_memory_bytes,
                    ));
                }
                return Err(trap);
            }
        };

        Ok(match result {
            Ok(out_bytes) => TestResult {
                success: true,
                output: serde_json::from_slice(&out_bytes).ok(),
                error: None,
                execution_time_ms: elapsed_ms,
            },
            Err(e) => TestResult {
                success: false,
                output: None,
                error: Some(TestError {
                    code: e.code,
                    message: e.message,
                    category: e.category,
                    severity: e.severity,
                    retryable: e.retryable,
                }),
                execution_time_ms: elapsed_ms,
            },
        })
    }
}

/// Outcome of a host-guarded guest call (see [`call_with_guards`]).
pub(crate) enum GuardOutcome<R> {
    /// The guest returned without trapping. Carries the typed return; a
    /// guest-level error lives inside `R`, not here.
    Returned(R),
    /// The guest trapped. Distinguish a memory-cap OOM from a genuine fault via
    /// `store.data().limiter.denied_memory_grow` after this returns.
    Trapped(anyhow::Error),
    /// The per-call wall-clock budget elapsed and the call was interrupted —
    /// either a pure-wasm loop caught by the epoch ring, or a guest parked in a
    /// host call caught by the watchdog ring.
    TimedOut,
}

/// Run one typed guest call under two interruption rings, mirroring the runtime
/// path in `workflow.rs`:
/// - an epoch deadline callback that interrupts a pure-wasm loop at the next
///   guest branch point once `timeout` elapses, and
/// - a watchdog `select!` that drops the in-flight future when the guest is
///   parked in a host call (where the epoch callback can't fire) past budget.
///
/// The engine behind `store` must have epoch interruption enabled and an epoch
/// ticker running (both hold for the dispatcher engine). `store` is left
/// readable afterward so the caller can inspect e.g. `limiter.denied_memory_grow`.
pub(crate) async fn call_with_guards<P, R>(
    store: &mut Store<HostState>,
    timeout: Duration,
    func: TypedFunc<P, R>,
    params: P,
) -> GuardOutcome<R>
where
    P: ComponentNamedList + Lower,
    R: ComponentNamedList + Lift + 'static,
{
    let started = Instant::now();

    // Epoch ring: fires at guest branch points every EPOCH_TICK; interrupts
    // once the wall-clock budget is spent, otherwise re-arms for one more tick.
    store.epoch_deadline_callback(move |mut ctx| {
        if started.elapsed() >= timeout {
            ctx.data_mut().termination = Some(Termination::Timeout);
            return Ok(UpdateDeadline::Interrupt);
        }
        Ok(UpdateDeadline::Yield(1))
    });
    store.set_epoch_deadline(1);

    // Watchdog ring: catches a guest blocked inside a host call, where the epoch
    // callback can't fire. Cancellation = dropping the in-flight future. Scoped
    // so the `&mut store` reborrow is released before we read the store back.
    let outcome = {
        let call = func.call_async(&mut *store, params);
        tokio::pin!(call);
        let watchdog = async {
            loop {
                tokio::time::sleep(EPOCH_TICK).await;
                if started.elapsed() >= timeout {
                    return;
                }
            }
        };
        tokio::select! {
            result = &mut call => match result {
                Ok(ret) => GuardOutcome::Returned(ret),
                Err(trap) => GuardOutcome::Trapped(trap.into()),
            },
            _ = watchdog => GuardOutcome::TimedOut,
        }
    };

    // A pure-wasm loop trips the epoch ring instead: the call returns
    // Err(trap) with our Timeout marker set. Reclassify that as TimedOut so the
    // caller doesn't mistake it for a genuine guest fault.
    if matches!(outcome, GuardOutcome::Trapped(_))
        && store.data().termination == Some(Termination::Timeout)
    {
        return GuardOutcome::TimedOut;
    }
    outcome
}

/// Structured result for a capability that blew the wall-clock budget.
fn timeout_result(elapsed_ms: f64, timeout: Duration) -> TestResult {
    TestResult {
        success: false,
        output: None,
        error: Some(TestError {
            code: "EXECUTION_TIMEOUT".into(),
            message: format!("capability did not complete within {timeout:?} and was interrupted"),
            category: "transient".into(),
            severity: "error".into(),
            retryable: false,
        }),
        execution_time_ms: elapsed_ms,
    }
}

/// Structured result for a capability that exceeded the guest memory cap.
fn memory_limit_result(elapsed_ms: f64, max_bytes: usize) -> TestResult {
    TestResult {
        success: false,
        output: None,
        error: Some(TestError {
            code: "MEMORY_LIMIT_EXCEEDED".into(),
            message: format!(
                "capability exceeded the {max_bytes}-byte guest memory limit and was terminated"
            ),
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
        }),
        execution_time_ms: elapsed_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;
    use wasmtime::component::Component;

    /// One engine with a ticker running, shared across the guard tests so we
    /// don't leak a ticker thread per test. The epoch ring only fires when the
    /// engine's epoch actually advances, which is exactly what the ticker does.
    fn guarded_engine() -> Arc<Engine> {
        static ENGINE: OnceLock<Arc<Engine>> = OnceLock::new();
        ENGINE
            .get_or_init(|| {
                let engine = build_engine(&EngineConfig::default()).expect("build engine");
                spawn_epoch_ticker(Arc::clone(&engine));
                engine
            })
            .clone()
    }

    fn test_ctx() -> Arc<CallContext> {
        Arc::new(CallContext::for_test(
            "tenant-test",
            "http://localhost:1",
            "http://localhost:2",
            "http://localhost:3",
            "http://localhost:4",
        ))
    }

    /// Instantiate a minimal WAT component that exports a no-arg `run` func and
    /// return the store plus a typed handle to `run`. Goes through the real
    /// `instantiate` so the resource limiter is installed exactly as in
    /// production.
    async fn instantiate_run(
        engine: &Arc<Engine>,
        wat: &str,
    ) -> (Store<HostState>, TypedFunc<(), ()>) {
        let linker = build_linker(engine).expect("linker");
        let component = Component::new(engine, wat).expect("compile wat component");
        let pre = linker.instantiate_pre(&component).expect("instantiate_pre");
        let (mut store, instance) = instantiate(engine, &pre, HostState::new(test_ctx()))
            .await
            .expect("instantiate");
        let idx = instance
            .get_export_index(&mut store, None, "run")
            .expect("run export");
        let func = instance
            .get_typed_func::<(), ()>(&mut store, idx)
            .expect("typed run");
        (store, func)
    }

    /// Smallest component exporting `run`; returns immediately.
    const RETURNS_IMMEDIATELY: &str = r#"
        (component
            (core module $m (func (export "run")))
            (core instance $i (instantiate $m))
            (func (export "run") (canon lift (core func $i "run")))
        )
    "#;

    /// Same shape, but `run` spins forever — the only way out is the guard.
    const BUSY_LOOP: &str = r#"
        (component
            (core module $m
                (func (export "run") (loop $spin (br $spin))))
            (core instance $i (instantiate $m))
            (func (export "run") (canon lift (core func $i "run")))
        )
    "#;

    #[tokio::test(flavor = "multi_thread")]
    async fn call_with_guards_returns_for_a_fast_call() {
        let engine = guarded_engine();
        let (mut store, func) = instantiate_run(&engine, RETURNS_IMMEDIATELY).await;
        let outcome = call_with_guards(&mut store, Duration::from_secs(5), func, ()).await;
        assert!(
            matches!(outcome, GuardOutcome::Returned(())),
            "a fast call should return, not time out or trap"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn call_with_guards_interrupts_a_runaway_loop() {
        let engine = guarded_engine();
        let (mut store, func) = instantiate_run(&engine, BUSY_LOOP).await;
        let started = Instant::now();
        let outcome = call_with_guards(&mut store, Duration::from_millis(300), func, ()).await;
        assert!(
            matches!(outcome, GuardOutcome::TimedOut),
            "a runaway guest must be interrupted, not run forever"
        );
        // Without the epoch ring this call never returns; bound the wall clock
        // generously to keep the test robust under load while still failing a
        // genuine regression (the loop would otherwise hang the suite).
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "runaway loop was not bounded promptly: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn parse_timeout_uses_default_when_absent_or_invalid() {
        assert_eq!(parse_timeout(None), DEFAULT_TEST_CAPABILITY_TIMEOUT);
        assert_eq!(
            parse_timeout(Some("0".into())),
            DEFAULT_TEST_CAPABILITY_TIMEOUT
        );
        assert_eq!(
            parse_timeout(Some("abc".into())),
            DEFAULT_TEST_CAPABILITY_TIMEOUT
        );
        assert_eq!(parse_timeout(Some("5".into())), Duration::from_secs(5));
    }

    #[test]
    fn parse_memory_max_uses_default_when_absent_or_invalid() {
        assert_eq!(parse_memory_max(None), DEFAULT_GUEST_MEMORY_MAX_BYTES);
        assert_eq!(
            parse_memory_max(Some("0".into())),
            DEFAULT_GUEST_MEMORY_MAX_BYTES
        );
        assert_eq!(
            parse_memory_max(Some("nope".into())),
            DEFAULT_GUEST_MEMORY_MAX_BYTES
        );
        assert_eq!(parse_memory_max(Some("4096".into())), 4096);
    }
}
