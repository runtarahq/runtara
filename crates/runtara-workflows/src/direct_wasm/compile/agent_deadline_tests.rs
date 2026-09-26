//! Actual composed Agent deadlines through public compilation and composition.
use super::*;
#[path = "../../../tests/support/managed_inputs.rs"]
mod managed_inputs;
#[path = "../../../../runtara-component-host/tests/common/outbound.rs"]
mod outbound_fixture;
use runtara_component_host::runtime_host::{
    RuntimeCheckpointResult, RuntimeHost, RuntimeInputState, RuntimeSignalInfo,
};
use runtara_component_host::{InvokeExit, WorkflowExecutor, WorkflowRunSpec};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[path = "deadline_cleanup_tests.rs"]
mod cleanup;

#[path = "late_completion_tests.rs"]
mod late_completion;

#[path = "scope_alarm_tests.rs"]
mod scope_alarm;

#[path = "embed_deadline_tests.rs"]
mod embed;

#[path = "embed_tool_deadline_tests.rs"]
mod embed_tool;

#[path = "nested_suspend_tests.rs"]
mod nested_suspend;

struct CheckpointFault {
    pattern: String,
    write: bool,
    skip: usize,
    remaining: usize,
}

struct Host {
    database: Mutex<Option<Arc<dyn runtara_component_host::DatabaseHost>>>,
    checkpoints: Mutex<HashMap<String, Vec<u8>>>,
    started: Instant,
    first_invocation_start: Mutex<Option<Instant>>,
    clock_override: AtomicU64,
    cancel: AtomicBool,
    acknowledged: AtomicBool,
    cancel_cleanup: Mutex<Option<Arc<tokio::sync::Notify>>>,
    recovery_cleanup: Mutex<Option<Arc<tokio::sync::Notify>>>,
    recovery_observed: AtomicBool,
    checkpoint_fault: Mutex<Option<CheckpointFault>>,
    checkpoint_calls: Mutex<Vec<(String, bool)>>,
    failure_cleanup: Mutex<Option<Arc<tokio::sync::Notify>>>,
    failure_observed: AtomicBool,
    checkpoint_signal: Mutex<Option<String>>,
    checkpoint_cancel: AtomicBool,
    checkpoint_signal_remaining: AtomicUsize,
    blocked_checkpoint: Mutex<Option<(String, usize)>>,
    checkpoint_blocked: AtomicBool,
    managed_inputs: managed_inputs::ManagedInputs,
    debug_enabled: AtomicBool,
    breakpoint_pause_calls: AtomicUsize,
    breakpoint_pause_error: Mutex<Option<String>>,
    breakpoint_hits: AtomicUsize,
    reject_checkpoint_signal: AtomicBool,
    late_return_cancel_on_start: AtomicBool,
    late_return_starts: AtomicUsize,
    late_return_cleanups: AtomicUsize,
    /// How many times a managed-input poll has run, which for a waiting child
    /// counts how often the parent actually invoked it.
    input_polls: AtomicUsize,
    /// Report a lifecycle suspend once this many managed-input polls have
    /// happened. `usize::MAX` never suspends.
    suspend_after_input_poll: AtomicUsize,
    /// Report a root cancel once this many managed-input polls have happened.
    cancel_after_input_poll: AtomicUsize,
    /// Every managed-input address polled, in order. A nested wait's route
    /// encodes its whole call path, so this also shows that replay rebuilds
    /// the same route after a park.
    input_keys: Mutex<Vec<String>>,
}
impl Host {
    fn new() -> Self {
        Self {
            database: Mutex::new(None),
            checkpoints: Mutex::new(HashMap::new()),
            started: Instant::now(),
            first_invocation_start: Mutex::new(None),
            clock_override: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
            acknowledged: AtomicBool::new(false),
            cancel_cleanup: Mutex::new(None),
            recovery_cleanup: Mutex::new(None),
            recovery_observed: AtomicBool::new(false),
            checkpoint_fault: Mutex::new(None),
            checkpoint_calls: Mutex::new(Vec::new()),
            failure_cleanup: Mutex::new(None),
            failure_observed: AtomicBool::new(false),
            checkpoint_signal: Mutex::new(None),
            checkpoint_cancel: AtomicBool::new(false),
            checkpoint_signal_remaining: AtomicUsize::new(usize::MAX),
            blocked_checkpoint: Mutex::new(None),
            checkpoint_blocked: AtomicBool::new(false),
            managed_inputs: managed_inputs::ManagedInputs::new("agent-deadline"),
            debug_enabled: AtomicBool::new(false),
            breakpoint_pause_calls: AtomicUsize::new(0),
            breakpoint_pause_error: Mutex::new(Some("unexpected breakpoint".into())),
            breakpoint_hits: AtomicUsize::new(0),
            reject_checkpoint_signal: AtomicBool::new(false),
            late_return_cancel_on_start: AtomicBool::new(false),
            late_return_starts: AtomicUsize::new(0),
            late_return_cleanups: AtomicUsize::new(0),
            input_polls: AtomicUsize::new(0),
            suspend_after_input_poll: AtomicUsize::new(usize::MAX),
            cancel_after_input_poll: AtomicUsize::new(usize::MAX),
            input_keys: Mutex::new(Vec::new()),
        }
    }
    fn fail_checkpoints(&self, pattern: &str, write: bool) {
        *self.checkpoint_fault.lock().unwrap() = Some(CheckpointFault {
            pattern: pattern.into(),
            write,
            skip: 0,
            remaining: usize::MAX,
        });
    }
    fn checkpoint_error(&self, key: &str, write: bool) -> bool {
        self.checkpoint_calls
            .lock()
            .unwrap()
            .push((key.into(), write));
        let mut fault = self.checkpoint_fault.lock().unwrap();
        let Some(fault) = fault.as_mut() else {
            return false;
        };
        if fault.write != write || !key.contains(&fault.pattern) || fault.remaining == 0 {
            return false;
        }
        if fault.skip > 0 {
            fault.skip -= 1;
            return false;
        }
        fault.remaining -= 1;
        true
    }
}
#[async_trait::async_trait]
impl RuntimeHost for Host {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(Some(b"{}".to_vec()))
    }
    fn instance_id(&self) -> Result<String, String> {
        Ok("agent-deadline".into())
    }
    async fn complete(&self, _: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    async fn fail(&self, _: Vec<u8>) -> Result<(), String> {
        let cleanup = self.failure_cleanup.lock().unwrap().clone();
        if let Some(cleanup) = cleanup {
            tokio::time::timeout(Duration::from_secs(2), cleanup.notified())
                .await
                .map_err(|_| "storage failure preceded peer cleanup")?;
            self.failure_observed.store(true, Ordering::SeqCst);
        }
        Ok(())
    }
    async fn custom_event(&self, kind: String, payload: Vec<u8>) -> Result<(), String> {
        if kind == "late-return-start" {
            self.late_return_starts.fetch_add(1, Ordering::SeqCst);
            if self.late_return_cancel_on_start.load(Ordering::SeqCst) {
                self.cancel.store(true, Ordering::SeqCst);
            }
        }
        if kind == "late-return-cleanup" {
            self.late_return_cleanups.fetch_add(1, Ordering::SeqCst);
        }
        if kind == "breakpoint_hit" {
            self.breakpoint_hits.fetch_add(1, Ordering::SeqCst);
        }
        if kind == "step_debug_start"
            && serde_json::from_slice::<Value>(&payload).unwrap()["step_id"] == "handled"
        {
            let cleanup = self.recovery_cleanup.lock().unwrap().clone();
            if let Some(cleanup) = cleanup {
                tokio::time::timeout(Duration::from_secs(2), cleanup.notified())
                    .await
                    .map_err(|_| "Embed recovery preceded child cleanup")?;
                self.recovery_observed.store(true, Ordering::SeqCst);
            }
        }
        Ok(())
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        Ok(self.debug_enabled.load(Ordering::SeqCst)
            || self.recovery_cleanup.lock().unwrap().is_some())
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        self.breakpoint_pause_calls.fetch_add(1, Ordering::SeqCst);
        match self.breakpoint_pause_error.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    async fn heartbeat(&self) -> Result<(), String> {
        Ok(())
    }
    async fn poll_signal(&self) -> Result<Option<RuntimeSignalInfo>, String> {
        Ok((self.cancel.load(Ordering::SeqCst)
            || self.input_polls.load(Ordering::SeqCst)
                >= self.cancel_after_input_poll.load(Ordering::SeqCst))
        .then(|| RuntimeSignalInfo {
            signal_type: "cancel".into(),
            command_id: "root-cancel".into(),
            payload: vec![],
            checkpoint_id: None,
        }))
    }
    async fn is_cancelled(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn check_signals(&self) -> Result<bool, String> {
        Ok(self.input_polls.load(Ordering::SeqCst)
            >= self.suspend_after_input_poll.load(Ordering::SeqCst))
    }
    async fn poll_custom_signal(&self, _: String) -> Result<Option<Vec<u8>>, String> {
        Err("compiled waits must use managed input polling".into())
    }
    async fn register_input(
        &self,
        descriptor: Vec<u8>,
        deadline: Option<u64>,
    ) -> Result<(), String> {
        let now = self.now_ms()?;
        self.managed_inputs
            .register(descriptor, deadline, Some(now))
            .await
    }
    async fn poll_input(&self, key: String) -> Result<RuntimeInputState, String> {
        self.input_polls.fetch_add(1, Ordering::SeqCst);
        self.input_keys.lock().unwrap().push(key.clone());
        self.managed_inputs.poll(&key).await
    }
    async fn close_input(&self, key: String) -> Result<RuntimeInputState, String> {
        self.managed_inputs.close(&key).await
    }
    async fn get_checkpoint(&self, key: String) -> Result<Option<Vec<u8>>, String> {
        if self.checkpoint_error(&key, false) {
            return Err("fixture checkpoint read failure".into());
        }
        Ok(self.checkpoints.lock().unwrap().get(&key).cloned())
    }
    async fn checkpoint(
        &self,
        key: String,
        state: Vec<u8>,
    ) -> Result<RuntimeCheckpointResult, String> {
        if self.checkpoint_error(&key, true) {
            return Err("fixture checkpoint write failure".into());
        }
        let block = {
            let mut blocked = self.blocked_checkpoint.lock().unwrap();
            match blocked.as_mut() {
                Some((pattern, skip)) if key.contains(pattern.as_str()) => {
                    if *skip == 0 {
                        true
                    } else {
                        *skip -= 1;
                        false
                    }
                }
                _ => false,
            }
        };
        if block {
            self.checkpoint_blocked.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
        }
        let pending_signal = self
            .checkpoint_signal
            .lock()
            .unwrap()
            .as_ref()
            .filter(|prefix| key.starts_with(prefix.as_str()))
            .filter(|_| {
                self.checkpoint_signal_remaining
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok()
            })
            .map(|_| RuntimeSignalInfo {
                signal_type: if self.checkpoint_cancel.load(Ordering::SeqCst) {
                    "cancel"
                } else {
                    "pause"
                }
                .into(),
                command_id: if self.checkpoint_cancel.load(Ordering::SeqCst) {
                    "root-cancel"
                } else {
                    "response-pause"
                }
                .into(),
                payload: vec![],
                checkpoint_id: None,
            });
        let mut values = self.checkpoints.lock().unwrap();
        let existing = values.get(&key).cloned();
        if !state.is_empty() {
            values.entry(key).or_insert_with(|| state.clone());
        }
        Ok(RuntimeCheckpointResult {
            found: existing.is_some(),
            state: existing.unwrap_or(state),
            pending_signal,
            custom_signal: None,
        })
    }
    async fn handle_checkpoint_signal(
        &self,
        kind: String,
        command: String,
    ) -> Result<bool, String> {
        assert!(matches!(
            (kind.as_str(), command.as_str()),
            ("cancel", "root-cancel") | ("pause", "response-pause")
        ));
        if self.reject_checkpoint_signal.swap(false, Ordering::SeqCst) {
            return Ok(false);
        }
        let cleanup = self.cancel_cleanup.lock().unwrap().clone();
        if let Some(cleanup) = cleanup {
            tokio::time::timeout(Duration::from_secs(2), cleanup.notified())
                .await
                .map_err(|_| "cancellation acknowledgement preceded pending call cleanup")?;
        }
        if self.late_return_cancel_on_start.load(Ordering::SeqCst) {
            assert_eq!(
                self.late_return_cleanups.load(Ordering::SeqCst),
                1,
                "root acknowledgement must follow the returning cleanup callback"
            );
        }
        assert!(!self.acknowledged.swap(true, Ordering::SeqCst));
        Ok(true)
    }
    async fn record_retry_attempt(
        &self,
        _: String,
        _: u32,
        _: Option<String>,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn durable_sleep_checkpoint(&self, _: String, _: Vec<u8>, _: u64) -> Result<(), String> {
        Err("lifecycle retry must park, not sleep in the host".into())
    }
    fn now_ms(&self) -> Result<u64, String> {
        let clock = self.clock_override.load(Ordering::SeqCst);
        Ok(if clock == 0 {
            1_000 + self.started.elapsed().as_millis() as u64
        } else {
            clock
        })
    }
}

fn executor() -> &'static WorkflowExecutor {
    static EXECUTOR: std::sync::OnceLock<WorkflowExecutor> = std::sync::OnceLock::new();
    EXECUTOR.get_or_init(|| {
        let engine = runtara_component_host::build_engine(&Default::default()).unwrap();
        runtara_component_host::spawn_epoch_ticker(engine.clone());
        let executor = WorkflowExecutor::new(engine).unwrap();
        executor
            .set_outbound_http(Arc::new(outbound_fixture::PublicHttp::default()))
            .unwrap();
        executor
            .set_connection_resolver(Arc::new(FixtureConnections))
            .unwrap();
        executor
    })
}

#[derive(Clone, Copy, Debug)]
enum Shape {
    Root,
    LongContinuation,
    Preparation,
    Published(usize),
    InlineWhile(usize),
    InheritedWhile {
        depth: usize,
        inner: Option<u64>,
        agent: Option<u64>,
        split: Option<(bool, u32)>,
    },
}

fn compile(
    dir: &Path,
    url: &str,
    timeout: u64,
    durable: bool,
    retries: u32,
    delay: u64,
    recover: bool,
) -> anyhow::Result<DirectCompilationResult> {
    compile_shaped(
        dir,
        url,
        timeout,
        durable,
        retries,
        delay,
        recover,
        Shape::Root,
    )
}

#[allow(clippy::too_many_arguments)]
fn compile_shaped(
    dir: &Path,
    url: &str,
    timeout: u64,
    durable: bool,
    retries: u32,
    delay: u64,
    recover: bool,
    shape: Shape,
) -> anyhow::Result<DirectCompilationResult> {
    let mut graph = json!({"durable":durable,"entryPoint":"fetch","steps":{
        "fetch":{"id":"fetch","stepType":"Agent","agentId":"http","capabilityId":"http-request",
            "maxRetries":retries,"retryDelay":delay,"inputMapping":{
                "url":{"valueType":"immediate","value":url},
                "fail_on_error":{"valueType":"immediate","value":true}}},
        "finish":{"id":"finish","stepType":"Finish","inputMapping":{
            "ok":{"valueType":"immediate","value":true}}},
        "handled":{"id":"handled","stepType":"Finish","inputMapping":{
            "code":{"valueType":"reference","value":"steps.__error.code"},
            "retryable":{"valueType":"reference","value":"steps.__error.retryable"},
            "stepId":{"valueType":"reference","value":"steps.__error.stepId"}}}},
        "executionPlan":[{"fromStep":"fetch","toStep":"finish"},
            {"fromStep":"fetch","toStep":"handled","label":"onError"}]});
    if matches!(shape, Shape::LongContinuation) {
        graph["steps"]["after"] = json!({"id":"after","stepType":"Agent",
            "agentId":"http","capabilityId":"http-request","maxRetries":0,
            "inputMapping":{"url":{"valueType":"immediate","value":format!("{url}/after")}}});
        graph["executionPlan"][0]["toStep"] = "after".into();
        graph["executionPlan"]
            .as_array_mut()
            .unwrap()
            .push(json!({"fromStep":"after","toStep":"finish"}));
    }
    if matches!(shape, Shape::Preparation) {
        graph["steps"]["fetch"]["connectionId"] = "conn".into();
    }
    if !recover {
        graph["steps"].as_object_mut().unwrap().remove("handled");
        graph["executionPlan"]
            .as_array_mut()
            .unwrap()
            .retain(|edge| edge["label"] != "onError");
    }
    if let Shape::InlineWhile(depth) = shape {
        for _ in 0..depth {
            graph = json!({"durable":false,"entryPoint":"loop","steps":{
                "loop":{"id":"loop","stepType":"While","condition":{"type":"operation","op":"EQ",
                    "arguments":[{"valueType":"immediate","value":1},{"valueType":"immediate","value":1}]},
                    "config":{"maxIterations":1},"subgraph":graph},
                "finish":{"id":"finish","stepType":"Finish","inputMapping":{
                    "result":{"valueType":"reference","value":"steps.loop.outputs.outputs"}}}},
                "executionPlan":[{"fromStep":"loop","toStep":"finish"}]});
        }
    }
    if let Shape::InheritedWhile {
        depth,
        inner,
        split,
        ..
    } = shape
    {
        if let Some((aggregate, retries)) = split {
            graph = json!({"durable":durable,"entryPoint":"items","steps":{
                "items":{"id":"items","stepType":"Split",
                    "config":{"value":{"valueType":"immediate","value":[{},{}]},"sequential":true,
                        "dontStopOnFailed":aggregate,"maxRetries":retries,"retryDelay":60_000},"subgraph":graph},
                "finish":{"id":"finish","stepType":"Finish","inputMapping":{"bad":{"valueType":"immediate","value":"continued split"}}},
                "handled":{"id":"handled","stepType":"Finish","inputMapping":{"bad":{"valueType":"immediate","value":"inner split handler"}}}},
                "executionPlan":[{"fromStep":"items","toStep":"finish"},
                    {"fromStep":"items","toStep":"handled","label":"onError"}]});
        }
        for i in 0..depth {
            let id = if i + 1 == depth { "outer" } else { "loop" };
            let budget = if i + 1 == depth { Some(timeout) } else { inner };
            graph = json!({"durable":false,"entryPoint":"pre","steps":{
                "pre":{"id":"pre","stepType":"Agent","agentId":"utils","capabilityId":"return-input",
                    "maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":i}}},
                id:{"id":id,"stepType":"While","condition":{"type":"operation","op":"EQ",
                    "arguments":[{"valueType":"immediate","value":1},{"valueType":"immediate","value":1}]},
                    "config":{"maxIterations":1,"timeout":budget},"subgraph":graph},
                "finish":{"id":"finish","stepType":"Finish","inputMapping":{
                    "result":{"valueType":"reference","value":format!("steps.{id}.outputs.outputs")}}},
                "handled":{"id":"handled","stepType":"Finish","inputMapping":{
                    "code":{"valueType":"reference","value":"steps.__error.code"},
                    "retryable":{"valueType":"reference","value":"steps.__error.retryable"},
                    "stepId":{"valueType":"reference","value":"steps.__error.stepId"},
                    "parent":{"valueType":"reference","value":"steps.pre.outputs"}}}},
                "executionPlan":[{"fromStep":"pre","toStep":id},{"fromStep":id,"toStep":"finish"},
                    {"fromStep":id,"toStep":"handled","label":"onError"}]});
        }
    }
    let published = matches!(shape, Shape::Published(_));
    assert!(!published || !durable);
    let abi = if published {
        super::super::component::WorkflowAbi::AgentCapabilities
    } else {
        super::super::component::WorkflowAbi::InvokeHostImports
    };
    let slug = published.then_some("timed-child");
    let mut scope = &mut graph;
    if let Shape::InlineWhile(depth) = shape {
        for _ in 0..depth {
            scope = &mut scope["steps"]["loop"]["subgraph"];
        }
    }
    let own_timeout = if let Shape::InheritedWhile {
        depth,
        agent,
        split,
        ..
    } = shape
    {
        for i in 0..depth {
            let id = if i == 0 { "outer" } else { "loop" };
            scope = &mut scope["steps"][id]["subgraph"];
        }
        if split.is_some() {
            scope = &mut scope["steps"]["items"]["subgraph"];
        }
        agent
    } else {
        Some(timeout)
    };
    if let Some(timeout) = own_timeout {
        scope["steps"]["fetch"]["timeout"] = timeout.into();
    }

    let input = DirectCompilationInput {
        workflow_id: "deadline".into(),
        version: 1,
        source_checksum: None,
        execution_graph: serde_json::from_value(graph)?,
        child_workflows: vec![],
        output_dir: dir.into(),
        track_events: false,
        agent_catalog: None,
        agent_slug: slug.map(str::to_owned),
    };
    let mut compiled = if published {
        compile_direct_workflow_with_abi(input, abi, false)?
    } else {
        crate::direct_wasm::compile_direct_workflow(input)?
    };
    assert!(compiled.support_report.supported);
    assert!(compiled.parallel_pools.is_empty());
    let components = std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")
        .expect("build components and set RUNTARA_AGENT_COMPONENTS_DIR");
    compose_direct_workflow(&mut compiled, &components)?;
    if let Shape::Published(depth) = shape {
        assert!(compiled.omit_runtime);
        assert_runtime_free(&compiled)?;
        wrap_published(compiled, depth, dir, &components)
    } else {
        Ok(compiled)
    }
}

fn assert_runtime_free(compiled: &DirectCompilationResult) -> anyhow::Result<()> {
    let wit_component::DecodedWasm::Component(resolve, world) =
        wit_component::decode(&fs::read(&compiled.wasm_path)?)?
    else {
        anyhow::bail!("not a component")
    };
    let imports: Vec<_> = resolve.worlds[world]
        .imports
        .keys()
        .map(|key| resolve.name_world_key(key))
        .collect();
    assert!(
        imports
            .iter()
            .any(|name| name.starts_with("wasi:clocks/monotonic-clock@0.2.")),
        "missing standard clock: {imports:?}"
    );
    assert!(
        !imports.iter().any(|name| name.contains("workflow-runtime")),
        "published child imports runtime: {imports:?}"
    );
    assert!(compiled.scoped_agents.is_empty() && compiled.invocation_manifest.is_none());
    Ok(())
}

/// Publish the publicly compiled child into one or more normal composed Agent
/// callers. Only the final root imports the runtime and receives user signals.
fn wrap_published(
    mut child: DirectCompilationResult,
    depth: usize,
    dir: &Path,
    components: &str,
) -> anyhow::Result<DirectCompilationResult> {
    use super::super::component::WorkflowAbi;
    assert!(depth > 0);
    let staging = dir.join("published");
    fs::create_dir(&staging)?;
    let mut slug = "timed-child".to_string();
    for level in 0..depth {
        let mut info = runtara_dsl::agent_meta::workflow_agent_info(
            &slug,
            &slug,
            "fixture",
            &HashMap::new(),
            &HashMap::new(),
        );
        runtara_dsl::agent_meta::certify_workflow_agent_non_suspending(&mut info);
        fs::copy(
            &child.wasm_path,
            staging.join(format!("runtara_agent_{}.wasm", slug.replace('-', "_"))),
        )?;
        fs::write(
            staging.join(format!(
                "runtara_agent_{}.meta.json",
                slug.replace('-', "_")
            )),
            serde_json::to_vec(&info)?,
        )?;
        let graph = serde_json::from_value(json!({"durable":false,"entryPoint":"call","steps":{
            "call":{"id":"call","stepType":"Agent","agentId":slug,"capabilityId":"run","maxRetries":0},
            "finish":{"id":"finish","stepType":"Finish","inputMapping":{
                "result":{"valueType":"reference","value":"steps.call.outputs"}}}},
            "executionPlan":[{"fromStep":"call","toStep":"finish"}]}))?;
        let root = level + 1 == depth;
        slug = format!("timed-layer-{}", char::from(b'a' + u8::try_from(level)?));
        child = compile_direct_workflow_with_abi(
            DirectCompilationInput {
                workflow_id: slug.clone(),
                version: 1,
                source_checksum: None,
                execution_graph: graph,
                child_workflows: vec![],
                output_dir: dir.join(&slug),
                track_events: false,
                agent_catalog: Some(Arc::new(
                    runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![info]),
                )),
                agent_slug: (!root).then(|| slug.clone()),
            },
            if root {
                WorkflowAbi::InvokeHostImports
            } else {
                WorkflowAbi::AgentCapabilities
            },
            false,
        )?;
        compose_direct_workflow_with_extra_dirs(
            &mut child,
            components,
            std::slice::from_ref(&staging),
        )?;
        if !root {
            assert!(child.omit_runtime);
            assert_runtime_free(&child)?;
        }
    }
    Ok(child)
}

async fn invoke(compiled: &DirectCompilationResult, host: Arc<Host>) -> anyhow::Result<InvokeExit> {
    invoke_with_outbound(
        compiled,
        host,
        Arc::new(outbound_fixture::PublicHttp::default()),
    )
    .await
}

async fn invoke_with_outbound(
    compiled: &DirectCompilationResult,
    host: Arc<Host>,
    outbound: Arc<dyn runtara_component_host::OutboundHttpHost>,
) -> anyhow::Result<InvokeExit> {
    invoke_with_connections(compiled, host, outbound, None).await
}

async fn invoke_with_connections(
    compiled: &DirectCompilationResult,
    host: Arc<Host>,
    outbound: Arc<dyn runtara_component_host::OutboundHttpHost>,
    connections: Option<Arc<dyn runtara_component_host::ConnectionResolverHost>>,
) -> anyhow::Result<InvokeExit> {
    let database = host.database.lock().unwrap().clone();
    let local = {
        let local = WorkflowExecutor::new(Arc::clone(executor().engine()))?;
        local
            .set_connection_resolver(connections.unwrap_or_else(|| Arc::new(FixtureConnections)))?;
        if let Some(database) = database {
            local.set_database(database)?;
        }
        local.set_outbound_http(outbound)?;
        local
    };
    let executor = &local;
    let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
    host.first_invocation_start
        .lock()
        .unwrap()
        .get_or_insert_with(Instant::now);
    Ok(executor
        .execute_invoke(
            &pre,
            WorkflowRunSpec {
                trusted_instance: None,
                trusted_tenant: Some("fixture".into()),
                env: HashMap::new(),
                stderr: None,
                timeout: Duration::from_secs(5),
                cancel: None,
                limits: Default::default(),
                runtime: Some(host),
            },
            b"{}".to_vec(),
        )
        .await
        .exit)
}

#[derive(Clone, Copy)]
enum Response {
    Hang,
    Ok,
    Error,
    RetryThenHang,
    RollbackThenHang,
    RootCancel,
}

async fn run(
    response: Response,
    timeout: u64,
    durable: bool,
    retries: u32,
    delay: u64,
) -> anyhow::Result<()> {
    run_shaped(response, timeout, durable, retries, delay, Shape::Root).await
}

async fn run_shaped(
    response: Response,
    timeout: u64,
    durable: bool,
    retries: u32,
    delay: u64,
    shape: Shape,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}", listener.local_addr()?);
    let requests = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(AtomicUsize::new(0));
    let host = Arc::new(Host::new());
    let server_host = host.clone();
    let first_request = Arc::new(Mutex::new(None));
    let first = first_request.clone();
    let req = requests.clone();
    let eof = closed.clone();
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await?;
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let n = stream.read(&mut buffer).await?;
                anyhow::ensure!(n > 0, "request ended before headers");
                request.extend_from_slice(&buffer[..n]);
            }
            anyhow::ensure!(
                !matches!(shape, Shape::Preparation),
                "Agent invoked after cancelled preparation"
            );
            let attempt = req.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                *first.lock().unwrap() = Some(Instant::now());
                if matches!(
                    response,
                    Response::RetryThenHang | Response::RollbackThenHang
                ) {
                    server_host.clock_override.store(
                        if matches!(response, Response::RollbackThenHang) {
                            1
                        } else {
                            server_host.now_ms().unwrap() + 100_000
                        },
                        Ordering::SeqCst,
                    );
                }
            }
            let response = match response {
                Response::RetryThenHang | Response::RollbackThenHang if attempt == 0 => {
                    Response::Error
                }
                Response::RetryThenHang | Response::RollbackThenHang => Response::Hang,
                Response::RootCancel => {
                    server_host.cancel.store(true, Ordering::SeqCst);
                    Response::Hang
                }
                other => other,
            };
            match response {
                Response::Hang => {
                    assert_eq!(stream.read(&mut buffer).await?, 0);
                }
                Response::Ok
                | Response::Error
                | Response::RetryThenHang
                | Response::RollbackThenHang
                | Response::RootCancel => {
                    let status = if matches!(response, Response::Ok) {
                        "200 OK"
                    } else {
                        "503 Unavailable"
                    };
                    stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").as_bytes()).await?;
                    stream.shutdown().await?;
                }
            }
            eof.fetch_add(1, Ordering::SeqCst);
        }
        #[allow(unreachable_code)]
        anyhow::Ok(())
    });
    let result = async {
        let dir = tempfile::tempdir()?;
        let compiled = compile_shaped(
            dir.path(),
            &url,
            timeout,
            durable,
            retries,
            delay,
            true,
            shape,
        )?;
        let mut exit = if matches!(shape, Shape::Preparation) {
            let executor = WorkflowExecutor::new(Arc::clone(executor().engine()))?;
            executor.set_connection_resolver(Arc::new(PendingPreparation {
                host: host.clone(), requests: requests.clone(), closed: closed.clone(),
                first: first_request.clone(), cancel: matches!(response, Response::RootCancel),
            }))?;
            let pre = executor.load_instance_pre(&compiled.wasm_path).await?;
            executor.execute_invoke(&pre, WorkflowRunSpec {
                trusted_instance: None,
                trusted_tenant: Some("fixture".into()), env: HashMap::new(), stderr: None,
                timeout: Duration::from_secs(5), cancel: None, limits: Default::default(), runtime: Some(host.clone()),
            }, b"{}".to_vec()).await.exit
        } else { invoke(&compiled, host.clone()).await? };
        if matches!(response, Response::RootCancel) {
            anyhow::ensure!(
                matches!(exit, InvokeExit::Suspended(_)),
                "root cancel reached recovery: {exit:?}"
            );
            assert!(host.acknowledged.load(Ordering::SeqCst));
            assert_eq!(requests.load(Ordering::SeqCst), 1);
            tokio::time::timeout(Duration::from_secs(1), async {
                while closed.load(Ordering::SeqCst) != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            return Ok(());
        }
        if durable && retries > 0 && matches!(response, Response::Error) && timeout != 0 {
            anyhow::ensure!(
                matches!(exit, InvokeExit::Suspended(_)),
                "expected durable park: {exit:?}"
            );
            let initial = host.checkpoints.lock().unwrap().clone();
            let budgets: Vec<_> = initial
                .iter()
                .filter(|(key, _)| {
                    key.contains(if matches!(shape, Shape::InheritedWhile { .. }) {
                        "loop-deadline"
                    } else {
                        "agent-deadline"
                    })
                })
                .collect();
            assert_eq!(budgets.len(), 1);
            let budget = u64::from_le_bytes(budgets[0].1.as_slice().try_into()?);
            for (key, value) in &initial {
                if key.contains("retry") && value.len() == 8 {
                    assert!(u64::from_le_bytes(value.as_slice().try_into()?) <= budget);
                }
            }
            // An early operator resume must preserve the original wake and
            // budget without sending another request or rewriting history.
            let first_wake = initial
                .iter()
                .find(|(key, value)| key.contains("retry_sleep::2") && value.len() == 8)
                .unwrap();
            let wake = u64::from_le_bytes(first_wake.1.as_slice().try_into()?);
            host.clock_override.store(wake - 2_000, Ordering::SeqCst);
            let early = invoke(&compiled, host.clone()).await?;
            anyhow::ensure!(
                matches!(early, InvokeExit::Suspended(_)),
                "early replay: {early:?}"
            );
            assert_eq!(*host.checkpoints.lock().unwrap(), initial);
            assert_eq!(requests.load(Ordering::SeqCst), 1);
            host.clock_override.store(budget, Ordering::SeqCst);
            exit = invoke(&compiled, host.clone()).await?;
            assert_eq!(
                host.checkpoints.lock().unwrap().get(budgets[0].0),
                Some(budgets[0].1)
            );
        }
        let InvokeExit::Completed(output) = exit else {
            anyhow::bail!("expected recovery/success: {exit:?}")
        };
        let mut output: Value = serde_json::from_slice(&output)?;
        let complete_output = output.clone();
        let success = matches!(response, Response::Ok) && timeout != 0;
        // An onError Finish already terminates the surrounding workflow;
        // successful body completion instead contributes the While output.
        let layers = match shape {
            Shape::Published(depth) => depth,
            Shape::InlineWhile(depth) if success => depth,
            Shape::InheritedWhile { depth, .. }
                if success || matches!(response, Response::Error) && retries == 0 =>
            {
                depth
            }
            Shape::InheritedWhile {
                depth,
                inner,
                agent,
                ..
            } => {
                if agent
                    .is_some_and(|value| value < timeout && inner.is_none_or(|inner| value < inner))
                {
                    depth
                } else if inner.is_some_and(|value| value < timeout) {
                    depth - 1
                } else {
                    0
                }
            }
            _ => 0,
        };
        for _ in 0..layers {
            output = output["result"].take();
        }
        if success {
            assert_eq!(output, json!({"ok":true}));
            if durable {
                host.clock_override.store(100_000, Ordering::SeqCst);
                assert!(matches!(
                    invoke(&compiled, host.clone()).await?,
                    InvokeExit::Completed(_)
                ));
            }
        } else {
            assert_eq!(
                output,
                if matches!(response, Response::Error) && retries == 0 {
                    json!({"code":"HTTP_5XX","retryable":true,"stepId":"fetch"})
                } else if let Shape::InheritedWhile { depth, inner, agent, .. } = shape {
                    if agent.is_some_and(|value| {
                        value < timeout && inner.is_none_or(|inner| value < inner)
                    }) {
                        json!({"code":"AGENT_TIMEOUT","retryable":false,"stepId":"fetch"})
                    } else {
                        let id = if inner.is_some_and(|value| value < timeout) {
                            "loop"
                        } else {
                            "outer"
                        };
                        json!({"code":"WHILE_TIMEOUT","retryable":null,"stepId":id,"parent":if id == "outer" { depth - 1 } else { 0 }})
                    }
                } else {
                    json!({"code":"AGENT_TIMEOUT","retryable":false,"stepId":"fetch"})
                },
                "complete output: {complete_output}; shape: {shape:?}, durable: {durable}"
            );
        }
        assert_eq!(
            requests.load(Ordering::SeqCst),
            if matches!(
                response,
                Response::RetryThenHang | Response::RollbackThenHang
            ) {
                2
            } else {
                usize::from(timeout != 0)
            },
            "timeout={timeout}, durable={durable}, retries={retries}, shape={shape:?}"
        );
        if matches!(
            response,
            Response::RetryThenHang | Response::RollbackThenHang
        ) {
            let elapsed = first_request.lock().unwrap().unwrap().elapsed();
            // The budget starts before component initialization and the first
            // provider request. Measure its lower bound from invocation entry.
            let active_elapsed = host.first_invocation_start.lock().unwrap().expect("invocation start recorded").elapsed();
            assert!(
                active_elapsed >= Duration::from_millis(1_700),
                "wall clock jump shortened a live budget: {active_elapsed:?}"
            );
            assert!(
                elapsed < Duration::from_millis(2_700),
                "a retry restarted the two-second budget: {elapsed:?}"
            );
        }
        if !durable && !matches!(shape, Shape::InheritedWhile { .. }) {
            assert!(host.checkpoints.lock().unwrap().is_empty());
        }
        if matches!(shape, Shape::InheritedWhile { .. })
            && matches!(response, Response::Hang)
            && output["code"] != "AGENT_TIMEOUT"
        {
            assert!(
                !host
                    .checkpoints
                    .lock()
                    .unwrap()
                    .keys()
                    .any(|key| key.contains("attempt"))
            );
        }
        if matches!(
            response,
            Response::Hang | Response::RetryThenHang | Response::RollbackThenHang
        ) && timeout != 0
        {
            tokio::time::timeout(Duration::from_secs(1), async {
                while closed.load(Ordering::SeqCst) != requests.load(Ordering::SeqCst) {
                    tokio::task::yield_now().await;
                }
            })
            .await?;
        }
        anyhow::Ok(())
    }
    .await;
    server.abort();
    let server_result = server.await;
    if let Ok(inner) = server_result {
        inner?;
    }
    result
}

#[tokio::test]
async fn agent_deadline_cancels_pending_http_before_recovery() -> anyhow::Result<()> {
    for durable in [false, true] {
        for retries in [0, 5] {
            // Leave room for first component activation before requiring an
            // observed pending request. The zero-budget case is tested below.
            run(Response::Hang, 1_000, durable, retries, 60_000).await?;
        }
    }
    Ok(())
}
#[tokio::test]
async fn agent_deadline_zero_never_dispatches_or_retries() -> anyhow::Result<()> {
    for durable in [false, true] {
        run(Response::Hang, 0, durable, 5, 60_000).await?;
    }
    Ok(())
}
#[tokio::test]
async fn agent_deadline_includes_retry_backoff_and_durable_replay() -> anyhow::Result<()> {
    for durable in [false, true] {
        run(
            Response::Error,
            if durable { 60_000 } else { 1_000 },
            durable,
            5,
            60_000,
        )
        .await?;
    }
    Ok(())
}
#[tokio::test]
async fn agent_deadline_success_and_saturating_budget_preserve_output() -> anyhow::Result<()> {
    for durable in [false, true] {
        for timeout in [10_000, u64::MAX] {
            run(Response::Ok, timeout, durable, 5, 60_000).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_covers_later_attempt_without_resetting_budget() -> anyhow::Result<()> {
    for response in [Response::RetryThenHang, Response::RollbackThenHang] {
        run(response, 2_000, false, 5, 1_000).await?;
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_root_cancel_bypasses_step_recovery() -> anyhow::Result<()> {
    for durable in [false, true] {
        run(Response::RootCancel, 60_000, durable, 5, 60_000).await?;
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_published_workflows_use_standard_clock_without_runtime()
-> anyhow::Result<()> {
    // Include cold callable-component startup while still reaching pending I/O.
    for depth in [1, 2] {
        for (response, timeout, retries, delay) in [
            (Response::Hang, 1_000, 5, 60_000),
            (Response::Hang, 0, 5, 60_000),
            (Response::Error, 1_000, 5, 60_000),
            (Response::Ok, u64::MAX, 0, 0),
            (Response::RootCancel, 60_000, 5, 60_000),
        ] {
            run_shaped(
                response,
                timeout,
                false,
                retries,
                delay,
                Shape::Published(depth),
            )
            .await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_without_recovery_returns_typed_failure() -> anyhow::Result<()> {
    for durable in [false, true] {
        let dir = tempfile::tempdir()?;
        let compiled = compile(
            dir.path(),
            "http://127.0.0.1:9",
            0,
            durable,
            5,
            60_000,
            false,
        )?;
        let exit = invoke(&compiled, Arc::new(Host::new())).await?;
        let InvokeExit::Failed(error) = exit else {
            anyhow::bail!("expected typed failure: {exit:?}")
        };
        assert_eq!(error.code, "AGENT_TIMEOUT");
        assert_eq!(error.category, "timeout");
        assert!(!error.retryable);
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_rejects_corrupted_durable_budget() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let compiled = compile(dir.path(), "http://127.0.0.1:9", 0, true, 0, 0, false)?;
    let host = Arc::new(Host::new());
    assert!(matches!(
        invoke(&compiled, host.clone()).await?,
        InvokeExit::Failed(_)
    ));
    let key = host
        .checkpoints
        .lock()
        .unwrap()
        .keys()
        .find(|key| key.contains("agent-deadline"))
        .unwrap()
        .clone();
    for width in [0, 1, 7, 9, 16] {
        host.checkpoints
            .lock()
            .unwrap()
            .insert(key.clone(), vec![0; width]);
        let exit = invoke(&compiled, host.clone()).await?;
        let InvokeExit::Failed(error) = exit else {
            anyhow::bail!("corrupted budget was accepted: {exit:?}")
        };
        assert_eq!(error.code, "AGENT_DEADLINE_STATE");
        assert!(!error.retryable);
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_inventory_includes_inline_nested_definitions() -> anyhow::Result<()> {
    for depth in [1, 2] {
        for response in [Response::Hang, Response::Ok, Response::Error] {
            // Include cold component activation while still requiring the
            // first request to enter before testing its timeout/error path.
            run_shaped(response, 1_000, false, 0, 0, Shape::InlineWhile(depth)).await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_inherited_while_owns_pending_child_cancellation() -> anyhow::Result<()> {
    for durable in [false, true] {
        for (depth, inner, agent) in [
            (1, None, None),
            (2, None, None),
            (2, Some(60_000), Some(60_000)),
            (2, Some(50), Some(60_000)),
            (2, Some(60_000), Some(50)),
        ] {
            run_shaped(
                Response::Hang,
                200,
                durable,
                3,
                60_000,
                Shape::InheritedWhile {
                    depth,
                    inner,
                    agent,
                    split: None,
                },
            )
            .await?;
        }
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_inherited_timeout_bypasses_split_aggregation_and_retry()
-> anyhow::Result<()> {
    for durable in [false, true] {
        for aggregate in [false, true] {
            for split_retries in [0, 3] {
                run_shaped(
                    Response::Hang,
                    // Leave startup headroom so this proves cancellation of
                    // pending I/O, not expiry before the first request.
                    1_000,
                    durable,
                    3,
                    60_000,
                    Shape::InheritedWhile {
                        depth: 1,
                        inner: None,
                        agent: None,
                        split: Some((aggregate, split_retries)),
                    },
                )
                .await?;
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_inherited_scopes_preserve_success_error_and_root_cancel()
-> anyhow::Result<()> {
    for durable in [false, true] {
        for response in [Response::Ok, Response::Error, Response::RootCancel] {
            run_shaped(
                response,
                60_000,
                durable,
                0,
                60_000,
                Shape::InheritedWhile {
                    depth: 2,
                    inner: None,
                    agent: None,
                    split: None,
                },
            )
            .await?;
        }
        run_shaped(
            Response::Hang,
            200,
            durable,
            0,
            60_000,
            Shape::InheritedWhile {
                depth: 2,
                inner: Some(60_000),
                agent: Some(50),
                split: None,
            },
        )
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_inherited_budget_interrupts_untimed_cpu_loop_with_frozen_epoch()
-> anyhow::Result<()> {
    let condition = json!({"type":"operation","op":"EQ", "arguments":[
        {"valueType":"immediate","value":1},{"valueType":"immediate","value":1}]});
    let graph = json!({"durable":false,"entryPoint":"outer","steps":{
        "outer":{"id":"outer","stepType":"While","condition":condition,
            "config":{"maxIterations":1,"timeout":50},"subgraph":{
                "entryPoint":"inner","steps":{
                    "inner":{"id":"inner","stepType":"While","condition":condition,
                        "config":{"maxIterations":4294967295_u64},"subgraph":{
                            "entryPoint":"done","steps":{"done":{"id":"done","stepType":"Finish"}}}},
                    "bad":{"id":"bad","stepType":"Finish","inputMapping":{
                        "wrong":{"valueType":"immediate","value":"inner handler"}}}},
                "executionPlan":[{"fromStep":"inner","toStep":"bad","label":"onError"}]}},
        "recovery":{"id":"recovery","stepType":"While","condition":condition,
            "config":{"maxIterations":3},"subgraph":{
                "entryPoint":"done","steps":{"done":{"id":"done","stepType":"Finish"}}}},
        "handled":{"id":"handled","stepType":"Finish","inputMapping":{
            "code":{"valueType":"reference","value":"steps.__error.code"},
            "stepId":{"valueType":"reference","value":"steps.__error.stepId"}}}},
        "executionPlan":[{"fromStep":"outer","toStep":"recovery","label":"onError"},
            {"fromStep":"recovery","toStep":"handled"}]});
    let temp = tempfile::tempdir()?;
    let mut compiled = compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "inherited-cpu".into(),
            version: 1,
            source_checksum: None,
            execution_graph: serde_json::from_value(graph)?,
            child_workflows: vec![],
            output_dir: temp.path().into(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        super::super::component::WorkflowAbi::InvokeHostImports,
        false,
    )?;
    let components = std::env::var("RUNTARA_AGENT_COMPONENTS_DIR")?;
    compose_direct_workflow(&mut compiled, &components)?;
    let host = Arc::new(Host::new());
    host.clock_override.store(1_000_000, Ordering::SeqCst);
    let InvokeExit::Completed(output) = invoke(&compiled, host).await? else {
        anyhow::bail!("expected outer recovery")
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output)?,
        json!({"code":"WHILE_TIMEOUT","stepId":"outer"})
    );
    Ok(())
}

#[tokio::test]
async fn agent_deadline_inherited_budget_bounds_backoff_and_durable_replay() -> anyhow::Result<()> {
    for durable in [false, true] {
        run_shaped(
            Response::Error,
            4_000,
            durable,
            3,
            60_000,
            Shape::InheritedWhile {
                depth: 1,
                inner: None,
                agent: None,
                split: None,
            },
        )
        .await?;
    }
    Ok(())
}

#[tokio::test]
async fn agent_deadline_interrupts_connection_preparation_without_invocation() -> anyhow::Result<()>
{
    for durable in [false, true] {
        {
            run_shaped(Response::Hang, 500, durable, 3, 0, Shape::Preparation).await?;
            run_shaped(
                Response::RootCancel,
                5_000,
                durable,
                3,
                0,
                Shape::Preparation,
            )
            .await?;
        }
        run_shaped(Response::Hang, 0, durable, 3, 0, Shape::Preparation).await?;
    }
    Ok(())
}

struct PendingPreparation {
    host: Arc<Host>,
    requests: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
    first: Arc<Mutex<Option<Instant>>>,
    cancel: bool,
}
#[async_trait::async_trait]
impl runtara_component_host::ConnectionResolverHost for PendingPreparation {
    async fn describe(&self, _: &str, _: String) -> Result<Vec<u8>, String> {
        struct Cleanup(Arc<AtomicUsize>);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let _cleanup = Cleanup(self.closed.clone());
        self.requests.fetch_add(1, Ordering::SeqCst);
        *self.first.lock().unwrap() = Some(Instant::now());
        if self.cancel {
            self.host.cancel.store(true, Ordering::SeqCst);
        }
        std::future::pending().await
    }
    async fn resolve_resource(&self, _: &str, _: String, _: Vec<u8>) -> Result<Vec<u8>, String> {
        Err("unsupported fixture resource".into())
    }
}

struct FixtureConnections;
#[async_trait::async_trait]
impl runtara_component_host::ConnectionResolverHost for FixtureConnections {
    async fn describe(&self, tenant: &str, connection: String) -> Result<Vec<u8>, String> {
        assert_eq!(tenant, "fixture");
        if connection == "om-conn" {
            return Ok(embed_tool::sql_fixture::descriptor(&connection));
        }
        Ok(serde_json::to_vec(&json!({"connectionId": connection,
            "integrationId": if connection == "mcp-conn" {"mcp"} else {"openai_api_key"},
            "status":"ACTIVE","resources":[],"metadata":null}))
        .unwrap())
    }
    async fn resolve_resource(&self, _: &str, _: String, _: Vec<u8>) -> Result<Vec<u8>, String> {
        Err("unsupported fixture resource".into())
    }
}
