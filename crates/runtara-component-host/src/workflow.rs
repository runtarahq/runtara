//! Embedded execution of composed direct-workflow components.
//!
//! A composed workflow component (the direct pipeline's `workflow.wasm`)
//! exports `wasi:cli/run@0.2.3` and imports only WASI cli/http interfaces —
//! the stdlib/runtime/agent imports are satisfied internally by composition.
//! This module is the in-process replacement for `wasmtime run --wasi http
//! --wasi inherit-network <workflow.wasm>`: same env contract, same
//! no-filesystem sandbox, but no process spawn and no per-run JIT (compiled
//! components are cached per image path).
//!
//! Interruption model, two rings:
//! - epoch deadline callback: fires at guest branch points every
//!   [`EPOCH_TICK`], checks cancel flag + wall-clock budget, yields to tokio
//!   otherwise. Catches pure-wasm loops.
//! - watchdog `select!`: polls the same conditions outside the store and
//!   cancels by dropping the in-flight call future. Catches guests blocked
//!   inside host calls (e.g. a hung outbound HTTP request) where the epoch
//!   callback never fires.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use async_trait::async_trait;
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store, UpdateDeadline};
use wasmtime_wasi::cli::OutputFile;
use wasmtime_wasi::p2::bindings::CommandPre;
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    WasiHttpCtx,
    p2::{
        HttpResult, WasiHttpCtxView, WasiHttpHooks, WasiHttpView, bindings::http::types::ErrorCode,
        body::HyperOutgoingBody, types::HostFutureIncomingResponse, types::OutgoingRequestConfig,
    },
};

use crate::engine::EPOCH_TICK;
use crate::host_io::{DEFAULT_HTTP_TIMEOUT, HostIoContext};

/// Outgoing-body stream tuning, kept identical to the flags `WasmRunner`
/// passes the wasmtime CLI (`--wasi http-outgoing-body-buffer-chunks=4096`,
/// `--wasi http-outgoing-body-chunk-size=1048576`).
const OUTGOING_BODY_BUFFER_CHUNKS: usize = 4096;
const OUTGOING_BODY_CHUNK_SIZE: usize = 1024 * 1024;

/// Most entries the component cache holds before evicting least-recently-used
/// images. A composed workflow compiles to one cache entry per image path.
const COMPONENT_CACHE_MAX: usize = 32;

/// Whether prepared (precompile-worker) artifacts may be reused across
/// launches, via `RUNTARA_PREPARED_COMPONENT_CACHE`.
///
/// Off by default: [`WorkflowExecutor::prepare_precompiled`] documents that a
/// prepared artifact's native Wasmtime allocations are meant to drop with its
/// token, and a count-bounded cache does not bound those bytes. Deployments
/// that run a small, fixed set of workflow images at high rate can trade that
/// for skipping one process spawn and one full compile per launch, which is
/// otherwise paid on *every* execution of the same image.
fn prepared_cache_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("RUNTARA_PREPARED_COMPONENT_CACHE")
            .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes"))
            .unwrap_or(false)
    })
}

/// Per-instance resource limits applied to the guest `Store`.
#[derive(Clone, Debug)]
pub struct WorkflowLimits {
    /// Cap on any single guest linear memory, in bytes. A composed component
    /// carries one memory per inner core module, so this bounds each, not
    /// their sum. Growth beyond the cap fails the grow in-guest (OOM trap).
    pub max_memory_bytes: usize,
    /// Cap on elements in any single guest table.
    pub max_table_elements: usize,
}

impl Default for WorkflowLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: 1024 * 1024 * 1024,
            max_table_elements: 10_000_000,
        }
    }
}

/// Why a workflow run ended.
#[derive(Debug)]
pub enum WorkflowExit {
    /// `wasi:cli/run` returned `Ok(())` — the SDK has already reported the
    /// final status to runtara-core over HTTP.
    Completed,
    /// `wasi:cli/run` returned `Err(())` — the guest signalled failure the
    /// same way the CLI surfaces exit code 1. Details, if any, were reported
    /// to runtara-core by the SDK before returning.
    GuestError,
    /// Instantiation failed or the guest trapped. The reason chain is the
    /// closest equivalent of the CLI process's stderr.
    Failed { reason: String },
    /// The wall-clock budget elapsed.
    Timeout,
    /// The cancel flag was raised.
    Cancelled,
}

/// Result of one embedded workflow run.
#[derive(Debug)]
pub struct WorkflowRunResult {
    pub exit: WorkflowExit,
    /// Largest single guest linear memory observed, in bytes. Exact (from the
    /// resource limiter), unlike the RSS sampling the process runner reports.
    pub memory_peak_bytes: u64,
    pub duration: Duration,
}

/// Inputs for one run. `env` is the same merged map `WasmRunner::build_env`
/// produces; `stderr` (when given) receives both guest stderr writes and the
/// host-side failure reason, mirroring the per-run `stderr.log` contract.
pub struct WorkflowRunSpec {
    pub env: HashMap<String, String>,
    pub stderr: Option<std::fs::File>,
    pub timeout: Duration,
    pub cancel: Option<Arc<AtomicBool>>,
    pub limits: WorkflowLimits,
    /// Native runtime host for artifacts composed with
    /// `RuntimeBinding::HostImport` (they import
    /// `runtara:workflow-runtime/runtime` instead of carrying the composed
    /// HTTP runtime component). `None` for legacy composed artifacts — a
    /// HostImport artifact run without a host traps loudly on first use.
    pub runtime: Option<Arc<dyn crate::runtime_host::RuntimeHost>>,
}

/// One final durable handoff confirmation immediately before guest
/// instantiation.
///
/// Environment uses this to retain its durable start-gate marker through all
/// Store/WASI setup. The confirmation lives at this component-host boundary,
/// rather than in the runner task, because `instantiate_async` is the first
/// operation that can execute guest-controlled code.
#[async_trait]
pub trait WorkflowStartConfirmation: Send + Sync {
    /// Confirm the caller's durable handoff before instantiating the guest.
    async fn confirm_before_instantiate(&self) -> Result<()>;
}

/// Marker recorded by the epoch callback so a `Trap::Interrupt` can be told
/// apart from a genuine guest trap after the fact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Termination {
    Timeout,
    Cancelled,
}

struct WorkflowLimiter {
    max_memory_bytes: usize,
    max_table_elements: usize,
    memory_peak_bytes: u64,
    denied_memory_grow: bool,
}

impl wasmtime::ResourceLimiter for WorkflowLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.max_memory_bytes {
            self.denied_memory_grow = true;
            return Ok(false);
        }
        self.memory_peak_bytes = self.memory_peak_bytes.max(desired as u64);
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= self.max_table_elements)
    }
}

struct WorkflowHooks;

impl WasiHttpHooks for WorkflowHooks {
    fn send_request(
        &mut self,
        request: http::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        // Generated workflow and agent HTTP goes through `runtara:host-io`,
        // where a single absolute deadline and response cap are enforced.
        // The legacy raw wasi:http route must not remain as an ungoverned
        // fallback: it would bypass both protections.
        let _ = (request, config);
        Err(ErrorCode::HttpRequestDenied.into())
    }

    fn outgoing_body_buffer_chunks(&mut self) -> usize {
        OUTGOING_BODY_BUFFER_CHUNKS
    }

    fn outgoing_body_chunk_size(&mut self) -> usize {
        OUTGOING_BODY_CHUNK_SIZE
    }
}

/// Store data for a workflow run.
pub struct WorkflowState {
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    hooks: WorkflowHooks,
    /// The active execution deadline. It is reset only after the durable
    /// runner handoff has been confirmed, so Store/WASI construction does not
    /// consume the guest's execution budget.
    /// Host-io caps every nested HTTP request by this absolute deadline.
    http_deadline: tokio::time::Instant,
    /// The same active deadline used by the epoch interruption callback.
    /// Keeping it in store data lets the callback observe the post-handoff
    /// reset without sharing a wall-clock timestamp across tasks.
    active_deadline: tokio::time::Instant,
    limiter: WorkflowLimiter,
    termination: Option<Termination>,
    /// Present when the artifact imports the runtime interface (HostImport
    /// binding); `None` for legacy composed artifacts.
    runtime: Option<Arc<dyn crate::runtime_host::RuntimeHost>>,
    connection_resolver:
        Result<Arc<dyn crate::connection_resolver_host::ConnectionResolverHost>, String>,
}

impl HostIoContext for WorkflowState {
    fn http_deadline(&self) -> Option<tokio::time::Instant> {
        Some(self.http_deadline)
    }
}

impl WorkflowState {
    fn begin_active_execution(&mut self, timeout: Duration) {
        let deadline = tokio::time::Instant::now() + timeout;
        self.http_deadline = deadline;
        self.active_deadline = deadline;
    }

    /// The run's native runtime host, when configured.
    pub(crate) fn runtime_host(&self) -> Option<&Arc<dyn crate::runtime_host::RuntimeHost>> {
        self.runtime.as_ref()
    }

    pub(crate) fn connection_resolver_host(
        &self,
    ) -> Option<&Arc<dyn crate::connection_resolver_host::ConnectionResolverHost>> {
        self.connection_resolver.as_ref().ok()
    }

    pub(crate) fn connection_resolver_error(&self) -> Option<&str> {
        self.connection_resolver.as_ref().err().map(String::as_str)
    }
}

impl WasiView for WorkflowState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for WorkflowState {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: &mut self.hooks,
        }
    }
}

struct CachedComponent {
    mtime: Option<SystemTime>,
    len: u64,
    last_used: Instant,
    /// Source digest recorded when a *prepared* artifact was cached, so a
    /// later cache hit can re-run the caller's checksum check instead of
    /// trusting mtime+len alone. `None` for entries built by
    /// [`WorkflowExecutor::load_instance_pre`], which never had one.
    source_digest: Option<[u8; 32]>,
    /// The linked component, export-shape-agnostic.
    instance_pre: Arc<wasmtime::component::InstancePre<WorkflowState>>,
    /// Lazily-derived `wasi:cli/run` wrapper — present only once a legacy
    /// (run-shaped) artifact has been loaded through [`WorkflowExecutor::load`].
    command: Option<Arc<CommandPre<WorkflowState>>>,
}

/// A linked workflow artifact prepared from an exact, verified child result.
///
/// This is intentionally separate from the opportunistic path/mtime cache.
/// A durable launch can spend time in a queue between compilation and guest
/// execution. The child response binds the source digest and serialized
/// component together, so the runner executes those exact prepared bytes
/// without reopening the mutable artifact path before it accepts a run permit.
#[derive(Clone)]
pub struct PreparedWorkflow {
    instance_pre: Arc<wasmtime::component::InstancePre<WorkflowState>>,
    command: Option<Arc<CommandPre<WorkflowState>>>,
}

impl PreparedWorkflow {
    /// Whether this artifact uses the lifecycle `invoke` export rather than
    /// the retired `wasi:cli/run` entrypoint.
    pub fn is_lifecycle_invoke(&self, engine: &Arc<Engine>) -> bool {
        crate::lifecycle::exports_lifecycle_invoke(&self.instance_pre, engine)
    }

    /// The linked invoke-shaped component, when [`Self::is_lifecycle_invoke`]
    /// is true.
    pub fn instance_pre(&self) -> &Arc<wasmtime::component::InstancePre<WorkflowState>> {
        &self.instance_pre
    }

    /// The linked legacy CLI wrapper, when this is a legacy artifact.
    pub fn command(&self) -> Option<&Arc<CommandPre<WorkflowState>>> {
        self.command.as_ref()
    }
}

/// Loads composed workflow components and executes them in-process.
pub struct WorkflowExecutor {
    engine: Arc<Engine>,
    linker: Linker<WorkflowState>,
    cache: tokio::sync::Mutex<HashMap<PathBuf, CachedComponent>>,
}

impl WorkflowExecutor {
    /// `engine` must have epoch interruption enabled (see
    /// [`crate::engine::build_engine`]) and an epoch ticker running.
    pub fn new(engine: Arc<Engine>) -> Result<Self> {
        let mut linker = Linker::<WorkflowState>::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
        // Native runtime interface for HostImport-composed artifacts. Extra
        // definitions are invisible to components that don't import them (the
        // WASI surface above works the same way), so legacy composed artifacts
        // are unaffected by this registration.
        crate::runtime_host::add_runtime_to_linker(&mut linker)?;
        crate::connection_resolver_host::add_connection_resolver_to_linker(&mut linker)?;
        // Concurrent HTTP hop for agent requests (wasip3 route (b)) — bound
        // func_wrap_concurrent so parallel Split subtasks overlap their I/O.
        crate::host_io::add_host_io_to_linker(&mut linker)?;
        Ok(Self {
            engine,
            linker,
            cache: tokio::sync::Mutex::new(HashMap::new()),
        })
    }

    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// Load (or fetch from cache) the composed component at `wasm_path`. The
    /// cache key is the path; entries are revalidated against file mtime+len
    /// so a re-deployed image at the same path recompiles.
    pub async fn load(&self, wasm_path: &Path) -> Result<Arc<CommandPre<WorkflowState>>> {
        let instance_pre = self.load_instance_pre(wasm_path).await?;
        {
            let mut cache = self.cache.lock().await;
            if let Some(entry) = cache.get_mut(wasm_path) {
                if let Some(command) = &entry.command {
                    return Ok(Arc::clone(command));
                }
                let command = Arc::new(
                    CommandPre::new(entry.instance_pre.as_ref().clone()).map_err(|e| {
                        anyhow::anyhow!("workflow component does not export wasi:cli/run: {e:#}")
                    })?,
                );
                entry.command = Some(Arc::clone(&command));
                return Ok(command);
            }
        }
        // The entry was evicted between the two locks — derive without caching.
        Ok(Arc::new(
            CommandPre::new(instance_pre.as_ref().clone()).map_err(|e| {
                anyhow::anyhow!("workflow component does not export wasi:cli/run: {e:#}")
            })?,
        ))
    }

    /// Load (or fetch from cache) the linked component, export-shape-agnostic
    /// — the entry point for invoke-shaped artifacts (which do not export
    /// `wasi:cli/run` and therefore cannot go through [`Self::load`]).
    pub async fn load_instance_pre(
        &self,
        wasm_path: &Path,
    ) -> Result<Arc<wasmtime::component::InstancePre<WorkflowState>>> {
        let meta = std::fs::metadata(wasm_path)
            .with_context(|| format!("stat workflow component {}", wasm_path.display()))?;
        let mtime = meta.modified().ok();
        let len = meta.len();

        {
            let mut cache = self.cache.lock().await;
            if let Some(entry) = cache.get_mut(wasm_path)
                && entry.mtime == mtime
                && entry.len == len
            {
                entry.last_used = Instant::now();
                return Ok(Arc::clone(&entry.instance_pre));
            }
        }

        // Compile outside the lock — Cranelift work can take seconds for a
        // large workflow and must not serialize unrelated instance starts.
        // A concurrent miss on the same path wastes a compile, nothing more.
        let engine = Arc::clone(&self.engine);
        let path = wasm_path.to_path_buf();
        let component = tokio::task::spawn_blocking(move || {
            Component::from_file(&engine, &path).map_err(|e| {
                anyhow::anyhow!("compile workflow component {}: {e:#}", path.display())
            })
        })
        .await
        .context("workflow component compile task panicked")??;

        let instance_pre = Arc::new(
            self.linker
                .instantiate_pre(&component)
                .map_err(|e| anyhow::anyhow!("link workflow component: {e:#}"))?,
        );

        let mut cache = self.cache.lock().await;
        cache.insert(
            wasm_path.to_path_buf(),
            CachedComponent {
                mtime,
                len,
                last_used: Instant::now(),
                source_digest: None,
                instance_pre: Arc::clone(&instance_pre),
                command: None,
            },
        );
        if cache.len() > COMPONENT_CACHE_MAX {
            evict_lru(&mut cache);
        }
        Ok(instance_pre)
    }

    /// Link a component that a short-lived precompile worker already read,
    /// hashed, and compiled.
    ///
    /// The caller must have received `component` over the private worker
    /// protocol and verified its source digest against immutable image
    /// metadata where applicable. This method deliberately does no filesystem
    /// access or compilation: those operations live in the killable worker
    /// boundary so an uninterruptible mount read or Cranelift compile cannot
    /// strand an in-process preparation permit.
    ///
    /// Prepared artifacts deliberately do *not* enter the global path cache.
    /// A queue cancellation or stale promotion must drop their native Wasmtime
    /// allocations with the opaque prepared token; a count-only cache cannot
    /// safely bound those allocations.
    pub async fn prepare_precompiled(&self, component: Component) -> Result<PreparedWorkflow> {
        let instance_pre =
            Arc::new(self.linker.instantiate_pre(&component).map_err(|error| {
                anyhow::anyhow!("link precompiled workflow component: {error:#}")
            })?);
        let command = if crate::lifecycle::exports_lifecycle_invoke(&instance_pre, &self.engine) {
            None
        } else {
            Some(Arc::new(
                CommandPre::new(instance_pre.as_ref().clone()).map_err(|error| {
                    anyhow::anyhow!("workflow component does not export wasi:cli/run: {error:#}")
                })?,
            ))
        };

        Ok(PreparedWorkflow {
            instance_pre,
            command,
        })
    }

    /// Look up a previously prepared artifact for `wasm_path`.
    ///
    /// Returns the prepared workflow together with the source digest recorded
    /// when it was cached, so the caller can repeat the same checksum check it
    /// would have run against a freshly precompiled artifact — a cache hit is
    /// never allowed to skip that verification.
    ///
    /// Opt-in: see [`prepared_cache_enabled`]. When the cache is off this
    /// always misses, preserving the drop-with-the-token allocation behaviour
    /// documented on [`Self::prepare_precompiled`].
    pub async fn cached_prepared(&self, wasm_path: &Path) -> Option<(PreparedWorkflow, [u8; 32])> {
        if !prepared_cache_enabled() {
            return None;
        }
        let meta = std::fs::metadata(wasm_path).ok()?;
        let mtime = meta.modified().ok();
        let len = meta.len();

        let mut cache = self.cache.lock().await;
        let entry = cache.get_mut(wasm_path)?;
        if entry.mtime != mtime || entry.len != len {
            return None;
        }
        let digest = entry.source_digest?;
        entry.last_used = Instant::now();
        Some((
            PreparedWorkflow {
                instance_pre: Arc::clone(&entry.instance_pre),
                command: entry.command.clone(),
            },
            digest,
        ))
    }

    /// Record a prepared artifact so later launches of the same image skip the
    /// precompile worker entirely.
    ///
    /// Bounded by the same `COMPONENT_CACHE_MAX` LRU as compiled components.
    /// No-op unless the cache is enabled.
    pub async fn cache_prepared(
        &self,
        wasm_path: &Path,
        source_digest: [u8; 32],
        prepared: &PreparedWorkflow,
    ) {
        if !prepared_cache_enabled() {
            return;
        }
        let Ok(meta) = std::fs::metadata(wasm_path) else {
            return;
        };
        let mut cache = self.cache.lock().await;
        cache.insert(
            wasm_path.to_path_buf(),
            CachedComponent {
                mtime: meta.modified().ok(),
                len: meta.len(),
                last_used: Instant::now(),
                source_digest: Some(source_digest),
                instance_pre: Arc::clone(&prepared.instance_pre),
                command: prepared.command.clone(),
            },
        );
        if cache.len() > COMPONENT_CACHE_MAX {
            evict_lru(&mut cache);
        }
    }

    /// Execute one workflow instance to completion (or interruption).
    pub async fn execute(
        &self,
        pre: &CommandPre<WorkflowState>,
        spec: WorkflowRunSpec,
    ) -> WorkflowRunResult {
        self.execute_with_start_confirmation(pre, spec, None).await
    }

    /// Execute one CLI-shaped workflow after an optional durable handoff
    /// confirmation at the exact pre-instantiation boundary.
    pub async fn execute_with_start_confirmation(
        &self,
        pre: &CommandPre<WorkflowState>,
        spec: WorkflowRunSpec,
        start_confirmation: Option<Arc<dyn WorkflowStartConfirmation>>,
    ) -> WorkflowRunResult {
        // Keep the externally reported duration, but do not begin the active
        // guest timeout until the runner has durably crossed its start gate.
        let overall_started = Instant::now();

        let mut builder = WasiCtxBuilder::new();
        // No preopens, no stdin, stdout discarded — parity with
        // `wasmtime run --wasi http` which grants no filesystem access and
        // the runner's `Stdio::null()` stdout.
        let mut env: Vec<(&String, &String)> = spec.env.iter().collect();
        env.sort();
        for (k, v) in env {
            builder.env(k, v);
        }
        let host_stderr = match &spec.stderr {
            Some(file) => match file.try_clone() {
                Ok(clone) => {
                    builder.stderr(OutputFile::new(clone));
                    spec.stderr
                }
                Err(_) => spec.stderr,
            },
            None => None,
        };

        let initial_deadline = tokio::time::Instant::now() + spec.timeout;
        let state = WorkflowState {
            wasi: builder.build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            hooks: WorkflowHooks,
            http_deadline: initial_deadline,
            active_deadline: initial_deadline,
            limiter: WorkflowLimiter {
                max_memory_bytes: spec.limits.max_memory_bytes,
                max_table_elements: spec.limits.max_table_elements,
                memory_peak_bytes: 0,
                denied_memory_grow: false,
            },
            termination: None,
            runtime: spec.runtime.clone(),
            connection_resolver: crate::connection_resolver_host::resolver_from_env(&spec.env),
        };

        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limiter);

        let timeout = spec.timeout;
        let cancel = spec.cancel.clone();
        store.epoch_deadline_callback(move |mut ctx| {
            if let Some(flag) = &cancel
                && flag.load(Ordering::Relaxed)
            {
                ctx.data_mut().termination = Some(Termination::Cancelled);
                return Ok(UpdateDeadline::Interrupt);
            }
            if tokio::time::Instant::now() >= ctx.data().active_deadline {
                ctx.data_mut().termination = Some(Termination::Timeout);
                return Ok(UpdateDeadline::Interrupt);
            }
            Ok(UpdateDeadline::Yield(1))
        });
        store.set_epoch_deadline(1);

        // Watchdog ring: catches the guest blocked in a host call, where the
        // epoch callback can't fire. Cancellation = dropping the run future.
        let watchdog_cancel = spec.cancel.clone();
        let run_ended = {
            // Store/WASI setup is host work before guest execution. A closed
            // durable gate must not burn the active execution budget; only a
            // successful confirmation starts it.
            let confirmation = async {
                if let Some(confirmation) = start_confirmation.as_ref() {
                    confirmation.confirm_before_instantiate().await?;
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;

            if let Err(error) = confirmation {
                Ok(Err(error))
            } else {
                store.data_mut().begin_active_execution(timeout);
                let active_started = Instant::now();
                let run = async {
                    let command = pre
                        .instantiate_async(&mut store)
                        .await
                        .map_err(anyhow::Error::from)?;
                    command
                        .wasi_cli_run()
                        .call_run(&mut store)
                        .await
                        .map_err(anyhow::Error::from)
                };
                tokio::pin!(run);
                let watchdog = async {
                    loop {
                        tokio::time::sleep(EPOCH_TICK).await;
                        if let Some(flag) = &watchdog_cancel
                            && flag.load(Ordering::Relaxed)
                        {
                            return Termination::Cancelled;
                        }
                        if active_started.elapsed() >= timeout {
                            return Termination::Timeout;
                        }
                    }
                };
                tokio::select! {
                    result = &mut run => Ok(result),
                    termination = watchdog => Err(termination),
                }
            }
        };

        let data = store.data();
        let exit = match run_ended {
            Err(Termination::Timeout) => WorkflowExit::Timeout,
            Err(Termination::Cancelled) => WorkflowExit::Cancelled,
            Ok(Ok(Ok(()))) => WorkflowExit::Completed,
            Ok(Ok(Err(()))) => WorkflowExit::GuestError,
            Ok(Err(trap)) => match data.termination {
                Some(Termination::Timeout) => WorkflowExit::Timeout,
                Some(Termination::Cancelled) => WorkflowExit::Cancelled,
                None if data.limiter.denied_memory_grow => WorkflowExit::Failed {
                    reason: format!(
                        "guest memory limit exceeded ({} bytes)",
                        data.limiter.max_memory_bytes
                    ),
                },
                None => WorkflowExit::Failed {
                    reason: format!("{trap:#}"),
                },
            },
        };

        // Mirror the CLI runner's stderr.log contract: the process's stderr
        // carried trap/abort diagnostics; embedded, we append the reason.
        if let Some(mut file) = host_stderr
            && let WorkflowExit::Failed { reason } = &exit
        {
            let _ = writeln!(file, "workflow failed: {reason}");
        }

        WorkflowRunResult {
            exit,
            memory_peak_bytes: store.data().limiter.memory_peak_bytes,
            duration: overall_started.elapsed(),
        }
    }

    /// Execute one invoke-shaped workflow instance (the unified agent-shaped
    /// export): `input` is passed as the call argument; the terminal result
    /// is the lifted return value. Same sandbox, limits, and interruption
    /// rings as [`Self::execute`].
    pub async fn execute_invoke(
        &self,
        pre: &wasmtime::component::InstancePre<WorkflowState>,
        spec: WorkflowRunSpec,
        input: Vec<u8>,
    ) -> InvokeRunResult {
        self.execute_invoke_with_start_confirmation(pre, spec, input, None)
            .await
    }

    /// Execute one invoke-shaped workflow after an optional durable handoff
    /// confirmation at the exact pre-instantiation boundary.
    pub async fn execute_invoke_with_start_confirmation(
        &self,
        pre: &wasmtime::component::InstancePre<WorkflowState>,
        spec: WorkflowRunSpec,
        input: Vec<u8>,
        start_confirmation: Option<Arc<dyn WorkflowStartConfirmation>>,
    ) -> InvokeRunResult {
        // Keep the externally reported duration, but do not begin the active
        // guest timeout until the runner has durably crossed its start gate.
        let overall_started = Instant::now();

        let mut builder = WasiCtxBuilder::new();
        let mut env: Vec<(&String, &String)> = spec.env.iter().collect();
        env.sort();
        for (k, v) in env {
            builder.env(k, v);
        }
        let host_stderr = match &spec.stderr {
            Some(file) => match file.try_clone() {
                Ok(clone) => {
                    builder.stderr(OutputFile::new(clone));
                    spec.stderr
                }
                Err(_) => spec.stderr,
            },
            None => None,
        };

        let initial_deadline = tokio::time::Instant::now() + spec.timeout;
        let state = WorkflowState {
            wasi: builder.build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            hooks: WorkflowHooks,
            http_deadline: initial_deadline,
            active_deadline: initial_deadline,
            limiter: WorkflowLimiter {
                max_memory_bytes: spec.limits.max_memory_bytes,
                max_table_elements: spec.limits.max_table_elements,
                memory_peak_bytes: 0,
                denied_memory_grow: false,
            },
            termination: None,
            runtime: spec.runtime.clone(),
            connection_resolver: crate::connection_resolver_host::resolver_from_env(&spec.env),
        };

        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limiter);

        let timeout = spec.timeout;
        let cancel = spec.cancel.clone();
        store.epoch_deadline_callback(move |mut ctx| {
            if let Some(flag) = &cancel
                && flag.load(Ordering::Relaxed)
            {
                ctx.data_mut().termination = Some(Termination::Cancelled);
                return Ok(UpdateDeadline::Interrupt);
            }
            if tokio::time::Instant::now() >= ctx.data().active_deadline {
                ctx.data_mut().termination = Some(Termination::Timeout);
                return Ok(UpdateDeadline::Interrupt);
            }
            Ok(UpdateDeadline::Yield(1))
        });
        store.set_epoch_deadline(1);

        let watchdog_cancel = spec.cancel.clone();
        let run_ended = {
            // Store/WASI setup is host work before guest execution. A closed
            // durable gate must not burn the active execution budget; only a
            // successful confirmation starts it.
            let confirmation = async {
                if let Some(confirmation) = start_confirmation.as_ref() {
                    confirmation.confirm_before_instantiate().await?;
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;

            if let Err(error) = confirmation {
                Ok(Err(error))
            } else {
                store.data_mut().begin_active_execution(timeout);
                let active_started = Instant::now();
                let run =
                    async {
                        let instance = pre.instantiate_async(&mut store).await?;
                        // v2 (0.2.0, async-typed invoke) is the current compile shape;
                        // 0.1.0 (sync-typed) artifacts from before ABI v2 keep working.
                        let iface_idx = instance
                    .get_export_index(&mut store, None, crate::lifecycle::LIFECYCLE_INTERFACE_NAME)
                    .or_else(|| {
                        instance.get_export_index(
                            &mut store,
                            None,
                            runtara_workflow_wit::LIFECYCLE_INTERFACE_NAME_V1,
                        )
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "workflow component does not export {} (or the 0.1.0 variant) — \
                             not an invoke-shaped artifact (use execute() for wasi:cli/run \
                             artifacts)",
                            crate::lifecycle::LIFECYCLE_INTERFACE_NAME
                        )
                    })?;
                        let invoke_idx = instance
                            .get_export_index(&mut store, Some(&iface_idx), "invoke")
                            .ok_or_else(|| {
                                anyhow::anyhow!("lifecycle interface has no `invoke` export")
                            })?;
                        type InvokeFunc = wasmtime::component::TypedFunc<
                            (Vec<u8>,),
                            (
                                Result<
                                    crate::lifecycle::WorkflowOutcome,
                                    crate::lifecycle::WorkflowErrorInfo,
                                >,
                            ),
                        >;
                        let invoke: InvokeFunc = instance.get_typed_func(&mut store, invoke_idx)?;
                        let (result,) = invoke.call_async(&mut store, (input,)).await?;
                        // post-return is driven automatically by wasmtime 44's typed
                        // call path; the store is single-use anyway (fresh per run).
                        Ok::<_, anyhow::Error>(result)
                    };
                tokio::pin!(run);
                let watchdog = async {
                    loop {
                        tokio::time::sleep(EPOCH_TICK).await;
                        if let Some(flag) = &watchdog_cancel
                            && flag.load(Ordering::Relaxed)
                        {
                            return Termination::Cancelled;
                        }
                        if active_started.elapsed() >= timeout {
                            return Termination::Timeout;
                        }
                    }
                };
                tokio::select! {
                    result = &mut run => Ok(result),
                    termination = watchdog => Err(termination),
                }
            }
        };

        let data = store.data();
        let exit = match run_ended {
            Err(Termination::Timeout) => InvokeExit::Timeout,
            Err(Termination::Cancelled) => InvokeExit::Cancelled,
            Ok(Ok(Ok(crate::lifecycle::WorkflowOutcome::Completed(output)))) => {
                InvokeExit::Completed(output)
            }
            Ok(Ok(Ok(crate::lifecycle::WorkflowOutcome::Suspended(wakes)))) => {
                InvokeExit::Suspended(wakes)
            }
            Ok(Ok(Err(error))) => InvokeExit::Failed(error),
            Ok(Err(trap)) => match data.termination {
                Some(Termination::Timeout) => InvokeExit::Timeout,
                Some(Termination::Cancelled) => InvokeExit::Cancelled,
                None if data.limiter.denied_memory_grow => InvokeExit::Trapped {
                    reason: format!(
                        "guest memory limit exceeded ({} bytes)",
                        data.limiter.max_memory_bytes
                    ),
                },
                None => InvokeExit::Trapped {
                    reason: format!("{trap:#}"),
                },
            },
        };

        if let Some(mut file) = host_stderr
            && let InvokeExit::Trapped { reason } = &exit
        {
            let _ = writeln!(file, "workflow trapped: {reason}");
        }

        InvokeRunResult {
            exit,
            memory_peak_bytes: store.data().limiter.memory_peak_bytes,
            duration: overall_started.elapsed(),
        }
    }

    /// Invoke a workflow-as-agent's `capabilities.invoke(capability-id, input,
    /// connection) -> result<list<u8>, error-info>` export directly, without a
    /// catalog entry — for verifying the `AgentCapabilities` ABI. A pure,
    /// agent-shaped workflow imports no runtime, so a runtime-less state
    /// suffices; `iface_name` is the fully-qualified capabilities interface
    /// export (e.g. `runtara:agent-<id>/capabilities@0.3.0`).
    /// The connection (if any) must already be injected into `input` under
    /// `_connection` by the caller — the invoke ABI has no connection argument.
    pub async fn invoke_capability(
        &self,
        pre: &wasmtime::component::InstancePre<WorkflowState>,
        iface_name: &str,
        capability_id: &str,
        input: Vec<u8>,
    ) -> anyhow::Result<Result<Vec<u8>, crate::ErrorInfo>> {
        let limits = WorkflowLimits::default();
        let deadline = tokio::time::Instant::now() + DEFAULT_HTTP_TIMEOUT;
        let state = WorkflowState {
            wasi: WasiCtxBuilder::new().build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            hooks: WorkflowHooks,
            http_deadline: deadline,
            active_deadline: deadline,
            limiter: WorkflowLimiter {
                max_memory_bytes: limits.max_memory_bytes,
                max_table_elements: limits.max_table_elements,
                memory_peak_bytes: 0,
                denied_memory_grow: false,
            },
            termination: None,
            runtime: None,
            connection_resolver: Err(
                "connection resolution is unavailable for direct capability invocation".to_string(),
            ),
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.limiter);
        // The engine uses epoch interruption; a large finite deadline (not
        // u64::MAX, which overflows the engine's `current + delta`) lets a pure
        // (sub-millisecond) capability run to completion.
        store.set_epoch_deadline(1 << 40);
        let instance = pre.instantiate_async(&mut store).await?;
        let iface_idx = instance
            .get_export_index(&mut store, None, iface_name)
            .ok_or_else(|| anyhow::anyhow!("missing interface export `{iface_name}`"))?;
        let invoke_idx = instance
            .get_export_index(&mut store, Some(&iface_idx), "invoke")
            .ok_or_else(|| anyhow::anyhow!("interface `{iface_name}` has no `invoke` export"))?;
        type InvokeFunc =
            wasmtime::component::TypedFunc<(String, Vec<u8>), (Result<Vec<u8>, crate::ErrorInfo>,)>;
        let invoke: InvokeFunc = instance.get_typed_func(&mut store, invoke_idx)?;
        let (result,) = invoke
            .call_async(&mut store, (capability_id.to_string(), input))
            .await?;
        Ok(result)
    }
}

/// Why an invoke-shaped workflow run ended. Unlike [`WorkflowExit`], terminal
/// output/error/suspension arrive in-band as the lifted return value — no
/// out-of-band status read is needed.
#[derive(Debug)]
pub enum InvokeExit {
    /// `Ok(outcome::completed(bytes))` — the terminal output.
    Completed(Vec<u8>),
    /// `Ok(outcome::suspended(wakes))` — re-invoke when ANY wake fires.
    Suspended(Vec<crate::lifecycle::WorkflowWake>),
    /// `Err(error-info)` — the terminal failure.
    Failed(crate::lifecycle::WorkflowErrorInfo),
    /// Instantiation failed or the guest trapped.
    Trapped { reason: String },
    /// The wall-clock budget elapsed.
    Timeout,
    /// The cancel flag was raised.
    Cancelled,
}

/// Result of one invoke-shaped workflow run.
#[derive(Debug)]
pub struct InvokeRunResult {
    pub exit: InvokeExit,
    pub memory_peak_bytes: u64,
    pub duration: Duration,
}

fn evict_lru(cache: &mut HashMap<PathBuf, CachedComponent>) {
    // Drop entries whose backing file is gone first, then the oldest by use.
    cache.retain(|path, _| path.exists());
    while cache.len() > COMPONENT_CACHE_MAX {
        let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, e)| e.last_used)
            .map(|(p, _)| p.clone())
        else {
            return;
        };
        cache.remove(&oldest);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::ResourceLimiter;

    fn limiter(max_memory: usize) -> WorkflowLimiter {
        WorkflowLimiter {
            max_memory_bytes: max_memory,
            max_table_elements: 1000,
            memory_peak_bytes: 0,
            denied_memory_grow: false,
        }
    }

    #[test]
    fn limiter_allows_growth_under_cap_and_tracks_peak() {
        let mut l = limiter(1024);
        assert!(l.memory_growing(0, 512, None).unwrap());
        assert!(l.memory_growing(512, 1024, None).unwrap());
        assert_eq!(l.memory_peak_bytes, 1024);
        assert!(!l.denied_memory_grow);
    }

    #[test]
    fn limiter_denies_growth_over_cap_and_records_oom() {
        let mut l = limiter(1024);
        assert!(!l.memory_growing(512, 2048, None).unwrap());
        assert!(l.denied_memory_grow);
        // Peak only tracks granted growth.
        assert_eq!(l.memory_peak_bytes, 0);
    }

    #[test]
    fn limiter_bounds_table_elements() {
        let mut l = limiter(1024);
        assert!(l.table_growing(0, 1000, None).unwrap());
        assert!(!l.table_growing(0, 1001, None).unwrap());
    }

    /// Shared fixture: one engine (ticker running) + executor + a minimal
    /// `wasi:cli/run@0.2.3` component written to a temp file so `load()`'s
    /// cache path is exercised. A `CommandPre` only instantiates against the
    /// engine that compiled it, hence the bundled tuple.
    struct Fixture {
        executor: WorkflowExecutor,
        wasm_path: PathBuf,
        _dir: tempfile::TempDir,
    }

    fn fixture() -> &'static Fixture {
        static ONCE: std::sync::OnceLock<Fixture> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            let engine = crate::engine::build_engine(&crate::engine::EngineConfig::default())
                .expect("test engine");
            crate::engine::spawn_epoch_ticker(Arc::clone(&engine));
            let executor = WorkflowExecutor::new(engine).expect("executor");
            let dir = tempfile::tempdir().expect("tempdir");
            let wasm_path = dir.path().join("minimal-run.wasm");
            std::fs::write(&wasm_path, MINIMAL_RUN_COMPONENT_WAT).expect("write component");
            Fixture {
                executor,
                wasm_path,
                _dir: dir,
            }
        })
    }

    /// Smallest component exporting `wasi:cli/run@0.2.3`; `run` returns ok.
    /// Parsed from WAT via the `wat` dev-feature on the wasmtime crate.
    const MINIMAL_RUN_COMPONENT_WAT: &str = r#"
        (component
            (core module $m
                (func (export "run") (result i32) (i32.const 0))
            )
            (core instance $i (instantiate $m))
            (func $run (result (result)) (canon lift (core func $i "run")))
            (instance $run_iface (export "run" (func $run)))
            (export "wasi:cli/run@0.2.3" (instance $run_iface))
        )
    "#;

    fn run_spec(timeout: Duration) -> WorkflowRunSpec {
        WorkflowRunSpec {
            env: HashMap::new(),
            stderr: None,
            timeout,
            cancel: None,
            limits: WorkflowLimits::default(),
            runtime: None,
        }
    }

    #[tokio::test]
    async fn executes_minimal_run_component_and_caches_it() {
        let fx = fixture();
        let pre = fx.executor.load(&fx.wasm_path).await.expect("load");
        let result = fx
            .executor
            .execute(&pre, run_spec(Duration::from_secs(5)))
            .await;
        assert!(
            matches!(result.exit, WorkflowExit::Completed),
            "unexpected exit: {:?}",
            result.exit
        );

        // Second load with unchanged mtime+len must hit the cache (same Arc).
        let pre2 = fx.executor.load(&fx.wasm_path).await.expect("reload");
        assert!(Arc::ptr_eq(&pre, &pre2), "expected component cache hit");
    }

    /// Same shape, but `run` spins forever — the only way out is the epoch
    /// deadline ring. Proves timeout + cancellation actually interrupt wasm.
    const BUSY_LOOP_COMPONENT_WAT: &str = r#"
        (component
            (core module $m
                (func (export "run") (result i32)
                    (loop $spin (br $spin))
                    (i32.const 0))
            )
            (core instance $i (instantiate $m))
            (func $run (result (result)) (canon lift (core func $i "run")))
            (instance $run_iface (export "run" (func $run)))
            (export "wasi:cli/run@0.2.3" (instance $run_iface))
        )
    "#;

    fn busy_loop_pre(fx: &Fixture) -> Arc<CommandPre<WorkflowState>> {
        let path = fx.wasm_path.with_file_name("busy-loop.wasm");
        if !path.exists() {
            std::fs::write(&path, BUSY_LOOP_COMPONENT_WAT).expect("write busy loop");
        }
        futures_block_on(fx.executor.load(&path)).expect("load busy loop")
    }

    /// Tiny block_on shim so fixture helpers stay callable from async tests
    /// without nesting runtimes.
    fn futures_block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(fut))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn timeout_interrupts_busy_loop() {
        let fx = fixture();
        let pre = busy_loop_pre(fx);
        let result = fx
            .executor
            .execute(&pre, run_spec(Duration::from_millis(300)))
            .await;
        assert!(
            matches!(result.exit, WorkflowExit::Timeout),
            "unexpected exit: {:?}",
            result.exit
        );
        assert!(result.duration < Duration::from_secs(5), "runaway loop");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_interrupts_busy_loop() {
        let fx = fixture();
        let pre = busy_loop_pre(fx);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut spec = run_spec(Duration::from_secs(30));
        spec.cancel = Some(Arc::clone(&cancel));
        let raise = {
            let cancel = Arc::clone(&cancel);
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                cancel.store(true, Ordering::Relaxed);
            })
        };
        let result = fx.executor.execute(&pre, spec).await;
        raise.await.expect("cancel raiser");
        assert!(
            matches!(result.exit, WorkflowExit::Cancelled),
            "unexpected exit: {:?}",
            result.exit
        );
        assert!(result.duration < Duration::from_secs(5), "cancel ignored");
    }
}
