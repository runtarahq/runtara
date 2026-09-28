//! Built-in-only, credential-bearing execution in fresh restricted stores.
//! Registry entries can only be constructed by loading an operator-installed
//! bundle. Tenant catalogs and caller-provided component bytes are never used.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use anyhow::{Context, Result};
use runtara_agent_trusted::{EXECUTION_INTERFACE, EXECUTOR_INTERFACE, TrustedContext, error};
use runtara_dsl::agent_meta::{AgentInfo, canonical_agent_id};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use wasmtime::component::{Component, InstancePre, Linker};
use wasmtime::{Engine, Store, UpdateDeadline};

use crate::host_state::HostState;

const MAX_INPUT: usize = 1024 * 1024;
const MAX_OUTPUT: usize = 1024 * 1024;
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const TIME_LIMIT: Duration = Duration::from_secs(30);

/// Implementations must check tenant ownership, entitlement and the exact type
/// before decrypting; returned credentials must come from that same record.
#[async_trait::async_trait]
pub trait TrustedCredentials: Send + Sync {
    async fn resolve(
        &self,
        tenant: &str,
        agent: &str,
        connection: &str,
        allowed_types: &[String],
    ) -> Result<TrustedContext, String>;

    /// Host policy for the authorized input and endpoint, before guest startup.
    fn validate_input(
        &self,
        _agent: &str,
        _capability: &str,
        _input: &[u8],
        _context: &TrustedContext,
    ) -> Result<(), String> {
        Ok(())
    }
}

struct TrustedAgent {
    pre: InstancePre<HostState>,
    capabilities: HashSet<String>,
    integration_ids: Vec<String>,
    digest: String,
    pin: String,
}

/// Logs only routing identity and outcome, including early denial and dropped callers.
struct InvocationAudit<'a> {
    tenant: &'a str,
    agent: &'a str,
    capability: &'a str,
    connection: &'a str,
    artifact: Option<&'a str>,
    started: std::time::Instant,
    outcome: &'static str,
}

impl Drop for InvocationAudit<'_> {
    fn drop(&mut self) {
        tracing::info!(
            tenant = self.tenant,
            agent = self.agent,
            capability = self.capability,
            connection = self.connection,
            artifact = self.artifact,
            duration_ms = self.started.elapsed().as_millis() as u64,
            outcome = self.outcome,
            "trusted capability completed"
        );
    }
}

pub struct TrustedExecutor {
    engine: Arc<Engine>,
    agents: HashMap<String, TrustedAgent>,
    credentials: OnceLock<Arc<dyn TrustedCredentials>>,
    permits: Arc<Semaphore>,
    /// Approved, non-revoked trusted pins of earlier installed versions
    /// (`approved_builtin_artifacts`, loaded at boot). A parked run pinned to
    /// one of them may keep calling its agent on wake or resume; see
    /// [`TrustedExecutor::admits`].
    history: RwLock<Arc<HashSet<String>>>,
    /// Pins revoked in `approved_builtin_artifacts`. A revoked installed
    /// version denies every call, like a revoked control digest; see
    /// [`TrustedExecutor::admits`].
    revoked: RwLock<Arc<HashSet<String>>>,
}

/// How the host launched the run that makes a trusted call.
///
/// Host authority only: the environment takes it from the durable launch
/// queue row it is executing and hands it over through the run's
/// [`crate::runtime_host::RuntimeHost`], which the guest cannot reach or
/// replace. Anything that does not say otherwise is [`Self::Start`], the
/// strictest kind, so a missing or unknown kind fails closed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TrustedLaunch {
    /// A first start (or a caller that cannot tell): only installed pins.
    #[default]
    Start,
    /// The wake scheduler relaunched a parked run.
    Wake,
    /// A user resumed a parked or paused run.
    Resume,
}

impl TrustedLaunch {
    /// Whether this launch continues a run that parked before (wake or
    /// resume), which is what may still carry a pin of an earlier version.
    pub fn continues_parked_run(self) -> bool {
        matches!(self, Self::Wake | Self::Resume)
    }
}

#[derive(Clone)]
pub(crate) struct TrustedCall {
    pub executor: Arc<TrustedExecutor>,
    pub tenant: String,
    pub deadline: tokio::time::Instant,
    pub pins: Option<Arc<HashSet<String>>>,
    /// Host-supplied launch kind of the calling run.
    pub launch: TrustedLaunch,
}

pub(crate) trait TrustedCaller {
    fn trusted_call(&self) -> Option<TrustedCall>;
}

impl TrustedExecutor {
    pub(crate) fn new(engine: Arc<Engine>) -> Self {
        Self {
            engine,
            agents: HashMap::new(),
            credentials: OnceLock::new(),
            permits: Arc::new(Semaphore::new(16)),
            history: RwLock::new(Arc::new(HashSet::new())),
            revoked: RwLock::new(Arc::new(HashSet::new())),
        }
    }

    /// Called only by the built-in bundle loader with the very bytes used to
    /// instantiate the registered agent, never by a workflow publication path.
    pub(crate) fn register(&mut self, info: &AgentInfo, wasm: &[u8], meta: &[u8]) -> Result<()> {
        let capabilities: HashSet<_> = info
            .capabilities
            .iter()
            .filter(|c| c.trusted)
            .map(|c| c.id.clone())
            .collect();
        if capabilities.is_empty() {
            return Ok(());
        }
        anyhow::ensure!(
            info.supports_connections
                && !info.integration_ids.is_empty()
                && info
                    .integration_ids
                    .iter()
                    .all(|s| !s.is_empty() && !s.contains('*')),
            "trusted built-in requires explicit connection types"
        );
        let component = Component::new(&self.engine, wasm)?;
        use wasmtime::component::types::{ComponentItem, Type};
        let ty = component.component_type();
        let export = ty
            .get_export(&self.engine, EXECUTION_INTERFACE)
            .context("trusted built-in lacks trusted execution export")?;
        let ComponentItem::ComponentInstance(interface) = export.ty else {
            anyhow::bail!("trusted execution must be an interface")
        };
        let invoke = interface
            .get_export(&self.engine, "invoke")
            .context("trusted execution lacks invoke")?;
        let ComponentItem::ComponentFunc(func) = invoke.ty else {
            anyhow::bail!("trusted invoke must be a function")
        };
        let bytes = |ty: &Type| matches!(ty, Type::List(list) if matches!(list.ty(), Type::U8));
        let params: Vec<_> = func.params().map(|(_, ty)| ty).collect();
        let results: Vec<_> = func.results().collect();
        anyhow::ensure!(
            func.async_()
                && matches!(params.as_slice(), [Type::String, input, context] if bytes(input) && bytes(context))
                && matches!(results.as_slice(), [Type::Result(result)] if result.ok().is_some_and(|ty| bytes(&ty)) && matches!(result.err(), Some(Type::String))),
            "trusted execution has incompatible invoke ABI"
        );
        let linker = crate::registry::build_linker(&self.engine)?;
        let pre = linker.instantiate_pre(&component)?;
        let mut hash = Sha256::new();
        hash.update(wasm);
        hash.update(meta);
        let digest = format!("{:x}", hash.finalize());
        let id = canonical_agent_id(&info.id);
        anyhow::ensure!(!self.agents.contains_key(&id), "duplicate trusted built-in");
        let pin = runtara_dsl::agent_meta::trusted_artifact_import(
            &id,
            &format!("{:x}", Sha256::digest(wasm)),
            &format!("{:x}", Sha256::digest(meta)),
        );
        self.agents.insert(
            id,
            TrustedAgent {
                pre,
                capabilities,
                integration_ids: info.integration_ids.clone(),
                digest,
                pin,
            },
        );
        Ok(())
    }

    /// Content-bound import names of every installed trusted built-in. The
    /// server's compilation readiness requires an artifact's recorded pins to
    /// be among these.
    pub fn artifact_pins(&self) -> impl Iterator<Item = &str> + '_ {
        self.agents.values().map(|agent| agent.pin.as_str())
    }

    /// Register content-bound imports for the installed versions. A pin for
    /// any other version is an empty instance import, which Wasmtime links
    /// without a definition, so an artifact compiled before an upgrade still
    /// loads. Its trusted calls are checked per call by [`Self::admits`]:
    /// on a start they fail with TRUSTED_VERSION_REQUIRED; on a wake or
    /// resume an approved, non-revoked earlier pin runs the installed bytes.
    pub(crate) fn add_artifact_pins<T: Send + 'static>(
        &self,
        linker: &mut Linker<T>,
    ) -> Result<()> {
        for agent in self.agents.values() {
            linker.instance(&agent.pin)?;
        }
        Ok(())
    }

    /// Install the approved history: every approved, non-revoked trusted pin
    /// (`runtara:trusted-artifacts/…`), current or earlier. Revoked pins are
    /// simply absent. Replaces the previous history; set at boot, before any
    /// wake or recovery.
    pub fn set_approved_history(&self, pins: impl IntoIterator<Item = String>) {
        let pins = Arc::new(pins.into_iter().collect());
        *self
            .history
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = pins;
    }

    /// Install the revoked pins. A call whose agent's installed version is
    /// revoked is refused on every launch. Set at boot with the history.
    pub fn set_revoked_pins(&self, pins: impl IntoIterator<Item = String>) {
        let pins = Arc::new(pins.into_iter().collect());
        *self
            .revoked
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = pins;
    }

    /// The approved history currently in force.
    pub fn approved_history(&self) -> Arc<HashSet<String>> {
        Arc::clone(
            &self
                .history
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    /// Whether a run whose artifact pins `pins`, launched as `launch`, may
    /// call `agent`'s trusted capabilities (always the installed bytes).
    ///
    /// `None` pins (a direct agent-level call, not a workflow) are not
    /// version-bound. Otherwise the artifact must pin the installed version
    /// of `agent`, or (option B) the launch continues a parked run (wake or
    /// resume, never a start) and the artifact pins an earlier version of
    /// that same agent that is in the approved history and not revoked. A
    /// start with an earlier pin is refused, so new runs recompile against
    /// the installed version; a never-approved or revoked pin is refused on
    /// every launch. Decided before any credential lookup.
    pub fn admits(
        &self,
        pins: Option<&HashSet<String>>,
        agent: &str,
        launch: TrustedLaunch,
    ) -> bool {
        let agent_id = canonical_agent_id(agent);
        let Some(target) = self.agents.get(&agent_id) else {
            return pins.is_none();
        };
        // A revoked installed version is the operator's switch to stop the
        // agent: no call runs its bytes, whatever the artifact pins.
        if self
            .revoked
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&target.pin)
        {
            return false;
        }
        let Some(pins) = pins else {
            return true;
        };
        if pins.contains(&target.pin) {
            return true;
        }
        if !launch.continues_parked_run() {
            return false;
        }
        let history = self.approved_history();
        pins.iter().any(|pin| {
            runtara_dsl::agent_meta::trusted_artifact_import_agent_id(pin)
                == Some(agent_id.as_str())
                && history.contains(pin)
        })
    }

    /// [`Self::invoke`] behind the per-call version check of [`Self::admits`]:
    /// a refused call fails with `TRUSTED_VERSION_REQUIRED` before any
    /// credential lookup.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn invoke_pinned(
        &self,
        pins: Option<&HashSet<String>>,
        launch: TrustedLaunch,
        tenant: &str,
        agent: &str,
        capability: &str,
        connection: &str,
        input: Vec<u8>,
        deadline: tokio::time::Instant,
    ) -> Result<Vec<u8>, String> {
        if !self.admits(pins, agent, launch) {
            return Err(error(
                "TRUSTED_VERSION_REQUIRED",
                "Workflow has no matching approved trusted dependency",
            ));
        }
        self.invoke(tenant, agent, capability, connection, input, deadline)
            .await
    }

    pub fn set_credentials(&self, credentials: Arc<dyn TrustedCredentials>) -> Result<()> {
        self.credentials
            .set(credentials)
            .map_err(|_| anyhow::anyhow!("trusted credentials already configured"))
    }

    pub async fn invoke(
        &self,
        tenant: &str,
        agent: &str,
        capability: &str,
        connection: &str,
        input: Vec<u8>,
        deadline: tokio::time::Instant,
    ) -> Result<Vec<u8>, String> {
        let agent_id = canonical_agent_id(agent);
        let mut audit = InvocationAudit {
            tenant,
            agent: &agent_id,
            capability,
            connection,
            artifact: None,
            started: std::time::Instant::now(),
            outcome: "denied",
        };
        let target = self
            .agents
            .get(&agent_id)
            .filter(|a| a.capabilities.contains(capability))
            .ok_or_else(|| {
                error(
                    "TRUSTED_CAPABILITY_DENIED",
                    "Capability is not an approved trusted built-in",
                )
            })?;
        audit.artifact = Some(&target.digest);
        if tenant.is_empty() || connection.trim().is_empty() {
            return Err(error(
                "TRUSTED_CONNECTION_REQUIRED",
                "Tenant and connection are required",
            ));
        }
        if input.len() > MAX_INPUT {
            return Err(error("TRUSTED_INPUT_LIMIT", "Trusted input exceeds limit"));
        }
        let deadline = deadline.min(tokio::time::Instant::now() + TIME_LIMIT);
        if tokio::time::Instant::now() >= deadline {
            return Err(error(
                "TRUSTED_TIMEOUT",
                "Trusted execution deadline exceeded",
            ));
        }
        audit.outcome = "cancelled";
        let mut tasks = tokio::task::JoinSet::new();
        let result = tokio::time::timeout_at(deadline, async {
            let permit = Arc::clone(&self.permits)
                .acquire_owned()
                .await
                .map_err(|_| error("TRUSTED_UNAVAILABLE", "Executor unavailable"))?;
            if tokio::time::Instant::now() >= deadline {
                return Err(error(
                    "TRUSTED_TIMEOUT",
                    "Trusted execution deadline exceeded",
                ));
            }
            let provider = self
                .credentials
                .get()
                .ok_or_else(|| error("TRUSTED_UNAVAILABLE", "Credential service unavailable"))?;
            let context = provider
                .resolve(tenant, &agent_id, connection, &target.integration_ids)
                .await?;
            if !target.integration_ids.contains(&context.integration_id) {
                return Err(error(
                    "TRUSTED_CONNECTION_TYPE",
                    "Connection type is incompatible",
                ));
            }
            provider.validate_input(&agent_id, capability, &input, &context)?;
            let bytes = zeroize::Zeroizing::new(
                serde_json::to_vec(&context)
                    .map_err(|_| error("TRUSTED_CONTEXT", "Invalid credential context"))?,
            );
            drop(context);
            if bytes.len() > MAX_INPUT {
                return Err(error(
                    "TRUSTED_CONTEXT_LIMIT",
                    "Credential context exceeds limit",
                ));
            }
            let engine = Arc::clone(&self.engine);
            let pre = target.pre.clone();
            let capability = capability.to_owned();
            // Wasmtime forbids nested run_concurrent event loops, even across
            // stores. A separate task gives this Store its own polling context.
            // JoinSet aborts the task if this entire invocation is dropped.
            tasks.spawn(async move {
                let _permit = permit;
                Self::execute(engine, pre, &capability, input, bytes, deadline).await
            });
            tasks
                .join_next()
                .await
                .expect("one isolated task")
                .unwrap_or_else(|_| {
                    Err(error(
                        "TRUSTED_EXECUTION_FAILED",
                        "Trusted execution failed",
                    ))
                })
        })
        .await
        .unwrap_or_else(|_| {
            Err(error(
                "TRUSTED_TIMEOUT",
                "Trusted execution deadline exceeded",
            ))
        });
        // On a deadline, abort and join before returning. A dropped caller also
        // aborts via JoinSet's Drop; epoch yields bound its cleanup latency.
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        audit.outcome = if result.is_ok() { "success" } else { "failure" };
        result
    }

    async fn execute(
        engine: Arc<Engine>,
        pre: InstancePre<HostState>,
        capability: &str,
        input: Vec<u8>,
        context: zeroize::Zeroizing<Vec<u8>>,
        deadline: tokio::time::Instant,
    ) -> Result<Vec<u8>, String> {
        let mut state = HostState::restricted();
        state.set_limits(MEMORY_LIMIT, 100_000);
        let mut store = Store::new(&engine, state);
        store.limiter(|s| &mut s.limiter);
        store.epoch_deadline_callback(move |_| {
            Ok(if tokio::time::Instant::now() >= deadline {
                UpdateDeadline::Interrupt
            } else {
                UpdateDeadline::Yield(1)
            })
        });
        store.set_epoch_deadline(1);
        // Guards cover initialization as well as invoke. Dropping this future
        // drops its Store; the parent JoinSet owns and aborts this task.
        let run = async {
            let instance = pre.instantiate_async(&mut store).await?;
            let iface = instance
                .get_export_index(&mut store, None, EXECUTION_INTERFACE)
                .context("trusted export")?;
            let export = instance
                .get_export_index(&mut store, Some(&iface), "invoke")
                .context("trusted invoke")?;
            let func = instance
                .get_typed_func::<(&str, &[u8], &[u8]), (Result<Vec<u8>, String>,)>(
                    &mut store, export,
                )?;
            let (result,) = func
                .call_async(&mut store, (capability, &input, &context))
                .await?;
            Ok::<_, anyhow::Error>(result)
        };
        let result = run.await;
        drop(store);
        match result {
            Ok(Ok(output)) if output.len() <= MAX_OUTPUT => {
                serde_json::from_slice::<serde::de::IgnoredAny>(&output).map_err(|_| {
                    error("TRUSTED_INVALID_OUTPUT", "Trusted output is not valid JSON")
                })?;
                Ok(output)
            }
            Ok(Ok(_)) => Err(error(
                "TRUSTED_OUTPUT_LIMIT",
                "Trusted output exceeds limit",
            )),
            // Approved errors are structured; do not expose unstructured guest
            // diagnostics/traps that might include credential material.
            Ok(Err(_)) => Err(error(
                "TRUSTED_CAPABILITY_FAILED",
                "Trusted capability failed",
            )),
            Err(_) => Err(error(
                "TRUSTED_EXECUTION_FAILED",
                "Trusted execution failed or exceeded its limits",
            )),
        }
    }
}

pub(crate) fn add_to_linker<T: TrustedCaller + Send + 'static>(
    linker: &mut Linker<T>,
) -> Result<()> {
    linker.instance(EXECUTOR_INTERFACE)?.func_wrap_concurrent(
        "invoke",
        |accessor, (agent, capability, connection, input): (String, String, String, Vec<u8>)| {
            let call = accessor.with(|mut access| access.get().trusted_call());
            Box::pin(async move {
                let result = match call {
                    Some(call) => {
                        call.executor
                            .invoke_pinned(
                                call.pins.as_deref(),
                                call.launch,
                                &call.tenant,
                                &agent,
                                &capability,
                                &connection,
                                input,
                                call.deadline,
                            )
                            .await
                    }
                    None => Err(error(
                        "TRUSTED_CAPABILITY_DENIED",
                        "Trusted execution is not available in this context",
                    )),
                };
                Ok((result,))
            })
        },
    )?;
    Ok(())
}

impl TrustedCaller for HostState {
    fn trusted_call(&self) -> Option<TrustedCall> {
        Some(TrustedCall {
            executor: self.trusted.clone()?,
            tenant: self.ctx.tenant_id.clone(),
            deadline: self.http_deadline?,
            pins: None,
            launch: TrustedLaunch::Start,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Credentials(AtomicUsize);
    #[async_trait::async_trait]
    impl TrustedCredentials for Credentials {
        async fn resolve(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &[String],
        ) -> Result<TrustedContext, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(TrustedContext {
                integration_id: "test".into(),
                credentials: serde_json::json!({"key":"test-secret"}),
                now_ms: 0,
            })
        }
    }

    fn fixture(body: &str, pages: u32, start: &str, timer: bool) -> Vec<u8> {
        let import = if timer {
            r#"
            (import "runtara:host-io/timers@0.1.0" (instance $timers
                (export "sleep" (func async (param "ms" u64)))))
            (alias export $timers "sleep" (func $sleep))
            (core func $sleep-lower (canon lower (func $sleep)))
        "#
        } else {
            ""
        };
        let core_import = if timer {
            r#"(import "host" "sleep" (func $sleep (param i64)))"#
        } else {
            ""
        };
        let args = if timer {
            r#"(with "host" (instance (export "sleep" (func $sleep-lower))))"#
        } else {
            ""
        };
        wat::parse_str(format!(r#"(component
            {import}
            (core module $m
                {core_import}
                (memory (export "memory") {pages})
                (global $heap (mut i32) (i32.const 4096))
                (global $counter (mut i32) (i32.const 0))
                (func (export "realloc") (param i32 i32 i32 i32) (result i32) (local $old i32)
                    global.get $heap local.tee $old local.get 3 i32.add i32.const 16 i32.add global.set $heap local.get $old)
                (func $start {start})
                (start $start)
                (func (export "invoke") (param i32 i32 i32 i32 i32 i32) (result i32)
                    {body}
                    i32.const 0))
            (core instance $i (instantiate $m {args}))
            (func $invoke async (param "capability-id" string) (param "input" (list u8))
                (param "context" (list u8)) (result (result (list u8) (error string)))
                (canon lift (core func $i "invoke") (memory $i "memory") (realloc (func $i "realloc"))))
            (instance $execution (export "invoke" (func $invoke)))
            (export "runtara:trusted/execution@0.1.0" (instance $execution))
        )"#)).unwrap()
    }

    fn http_probe(startup: bool) -> Vec<u8> {
        let check = r#"i32.const 0 i32.const 128 call $request
            i32.const 128 i32.load i32.const 1 i32.ne if unreachable end
            i32.const 140 i32.load i32.const 16 i32.ne if unreachable end
            i32.const 136 i32.load i32.load8_u i32.const 72 i32.ne if unreachable end"#;
        let start = if startup { check } else { "" };
        let body = if startup { "" } else { check };
        wat::parse_str(format!(r#"(component
            (import "runtara:outbound-http/client@0.1.0" (instance $http
                (type $connection-def (record (field "connection-id" string) (field "url" string)
                    (field "endpoint" (option string)) (field "endpoint-ref" (option string))
                    (field "ai-provider" (option string)) (field "aws-service" (option string))))
                (export "connection-destination" (type $connection (eq $connection-def)))
                (type $destination-def (variant (case "connection" $connection) (case "public" string)))
                (export "destination" (type $destination (eq $destination-def)))
                (type $headers (list (tuple string string)))
                (type $request-def (record (field "destination" $destination) (field "method" string)
                    (field "headers" $headers) (field "body" (option (list u8)))
                    (field "timeout-ms" (option u64)) (field "max-response-bytes" (option u64))))
                (export "request-options" (type $request (eq $request-def)))
                (type $response-def (record (field "status" u16) (field "headers" $headers) (field "body" (list u8))))
                (export "response" (type $response (eq $response-def)))
                (type $error-def (record (field "code" string) (field "message" string)
                    (field "status" (option u16)) (field "body" (list u8)) (field "retry-after-ms" (option u64))))
                (export "outbound-error" (type $error (eq $error-def)))
                (export "request" (func async (param "options" $request) (result (result $response (error $error)))))))
            (alias export $http "request" (func $request))
            (core module $memory
                (memory (export "memory") 1)
                (global $heap (mut i32) (i32.const 4096))
                (func (export "realloc") (param i32 i32 i32 i32) (result i32) (local $old i32)
                    global.get $heap local.tee $old local.get 3 i32.add i32.const 16 i32.add global.set $heap local.get $old))
            (core instance $memory (instantiate $memory))
            (core func $request (canon lower (func $request) (memory $memory "memory") (realloc (func $memory "realloc"))))
            (core module $main
                (import "memory" "memory" (memory 1))
                (import "host" "request" (func $request (param i32 i32)))
                (func $start {start}) (start $start)
                (func (export "invoke") (param i32 i32 i32 i32 i32 i32) (result i32)
                    {body}
                    i32.const 16 i32.const 49 i32.store8
                    i32.const 4 i32.const 16 i32.store
                    i32.const 8 i32.const 1 i32.store
                    i32.const 0))
            (core instance $i (instantiate $main
                (with "memory" (instance $memory))
                (with "host" (instance (export "request" (func $request))))))
            (func $invoke async (param "capability-id" string) (param "input" (list u8)) (param "context" (list u8))
                (result (result (list u8) (error string)))
                (canon lift (core func $i "invoke") (memory $memory "memory") (realloc (func $memory "realloc"))))
            (instance $execution (export "invoke" (func $invoke)))
            (export "runtara:trusted/execution@0.1.0" (instance $execution)))"#)).unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn http_is_denied_before_parsing_or_sending_in_startup_and_invocation() {
        for startup in [false, true] {
            let (executor, _) = executor(&http_probe(startup));
            let result = call(&executor, Duration::from_secs(5)).await;
            if startup {
                // Component initialization cannot block on an async host
                // import. It traps before any network work can escape.
                assert!(result.is_err());
            } else {
                assert_eq!(result.unwrap(), b"1");
            }
        }
    }

    fn counting_body() -> &'static str {
        r#"global.get $counter i32.const 1 i32.add global.set $counter
           i32.const 16 global.get $counter i32.const 48 i32.add i32.store8
           i32.const 4 i32.const 16 i32.store
           i32.const 8 i32.const 1 i32.store"#
    }

    fn executor(wasm: &[u8]) -> (Arc<TrustedExecutor>, Arc<Credentials>) {
        let engine = crate::engine::build_engine(&crate::engine::EngineConfig::default()).unwrap();
        crate::engine::spawn_epoch_ticker(engine.clone());
        let mut executor = TrustedExecutor::new(engine);
        let mut info = runtara_agent_crypto::agent_info();
        info.supports_connections = true;
        info.integration_ids = vec!["test".into()];
        info.capabilities
            .iter_mut()
            .for_each(|cap| cap.trusted = cap.id == "hash");
        executor
            .register(&info, wasm, &serde_json::to_vec(&info).unwrap())
            .unwrap();
        let credentials = Arc::new(Credentials(AtomicUsize::new(0)));
        executor.set_credentials(credentials.clone()).unwrap();
        (Arc::new(executor), credentials)
    }

    async fn call(executor: &TrustedExecutor, timeout: Duration) -> Result<Vec<u8>, String> {
        executor
            .invoke(
                "tenant",
                "crypto",
                "hash",
                "connection",
                b"{}".to_vec(),
                tokio::time::Instant::now() + timeout,
            )
            .await
    }

    async fn pinned_call(
        executor: &TrustedExecutor,
        pins: &[&str],
        launch: TrustedLaunch,
    ) -> Result<Vec<u8>, String> {
        let pins: HashSet<String> = pins.iter().map(|pin| (*pin).to_owned()).collect();
        executor
            .invoke_pinned(
                Some(&pins),
                launch,
                "tenant",
                "crypto",
                "hash",
                "connection",
                b"{}".to_vec(),
                tokio::time::Instant::now() + Duration::from_secs(5),
            )
            .await
    }

    /// Trusted pins, option B: an earlier pin of the same agent that is in
    /// the approved history runs the installed bytes, only when the launch
    /// continues a parked run (wake or resume). A start under it, a revoked
    /// (absent from the history) or never-approved pin, and another agent's
    /// approved pin fail with TRUSTED_VERSION_REQUIRED before any credential
    /// lookup. The installed pin works on every launch.
    #[tokio::test(flavor = "multi_thread")]
    async fn earlier_approved_pins_run_installed_bytes_only_on_wake_or_resume() {
        use runtara_dsl::agent_meta::trusted_artifact_import;
        let (executor, credentials) = executor(&fixture(counting_body(), 1, "", false));
        let installed = executor.artifact_pins().next().unwrap().to_owned();
        let pin = |agent: &str, seed: &str| {
            trusted_artifact_import(agent, &seed.repeat(64), &"f".repeat(64))
        };
        let old = pin("crypto", "a");
        let revoked = pin("crypto", "b");
        let unapproved = pin("crypto", "c");
        let other_agent = pin("s3-storage", "a");
        // Boot loads the approved, non-revoked history: `revoked` was
        // approved once and then revoked, so it is not in it.
        executor.set_approved_history([installed.clone(), old.clone(), other_agent.clone()]);
        assert_eq!(TrustedLaunch::default(), TrustedLaunch::Start);

        let resolved = |expected: usize| {
            assert_eq!(credentials.0.load(Ordering::SeqCst), expected);
        };
        let mut calls = 0;
        for launch in [
            TrustedLaunch::Start,
            TrustedLaunch::Wake,
            TrustedLaunch::Resume,
        ] {
            pinned_call(&executor, &[&installed], launch)
                .await
                .expect("the installed pin runs on every launch");
            calls += 1;
            resolved(calls);
        }
        for launch in [TrustedLaunch::Wake, TrustedLaunch::Resume] {
            pinned_call(&executor, &[&old], launch)
                .await
                .expect("an approved earlier pin runs the installed bytes on wake or resume");
            calls += 1;
            resolved(calls);
        }
        for (pins, launch, why) in [
            (
                vec![&old],
                TrustedLaunch::Start,
                "a start under an earlier pin",
            ),
            (vec![&revoked], TrustedLaunch::Wake, "a revoked pin on wake"),
            (
                vec![&revoked],
                TrustedLaunch::Resume,
                "a revoked pin on resume",
            ),
            (
                vec![&unapproved],
                TrustedLaunch::Wake,
                "a never-approved pin",
            ),
            (
                vec![&unapproved],
                TrustedLaunch::Start,
                "a never-approved pin on start",
            ),
            (
                vec![&other_agent],
                TrustedLaunch::Wake,
                "another agent's approved pin",
            ),
            (vec![], TrustedLaunch::Resume, "no pin at all"),
        ] {
            let pins: Vec<&str> = pins.into_iter().map(String::as_str).collect();
            let error = pinned_call(&executor, &pins, launch).await.expect_err(why);
            assert!(error.contains("TRUSTED_VERSION_REQUIRED"), "{why}: {error}");
            resolved(calls);
        }

        // A later boot that no longer lists `old` (revoked since) refuses it.
        executor.set_approved_history([installed.clone()]);
        let error = pinned_call(&executor, &[&old], TrustedLaunch::Wake)
            .await
            .expect_err("revoked at the next boot");
        assert!(error.contains("TRUSTED_VERSION_REQUIRED"), "{error}");
        resolved(calls);
        // Agent-level calls (no workflow pins) are not version-bound.
        assert!(executor.admits(None, "crypto", TrustedLaunch::Start));

        // Revoking the installed version stops every workflow call to it,
        // on every launch, before any credential lookup.
        executor.set_revoked_pins([installed.clone()]);
        for launch in [
            TrustedLaunch::Start,
            TrustedLaunch::Wake,
            TrustedLaunch::Resume,
        ] {
            let error = pinned_call(&executor, &[&installed], launch)
                .await
                .expect_err("a revoked installed version");
            assert!(error.contains("TRUSTED_VERSION_REQUIRED"), "{error}");
            resolved(calls);
        }
        assert!(!executor.admits(None, "crypto", TrustedLaunch::Start));
    }

    #[test]
    fn registry_rejects_wildcards_missing_types_and_incompatible_exports() {
        let engine = crate::engine::build_engine(&crate::engine::EngineConfig::default()).unwrap();
        let mut registry = TrustedExecutor::new(engine);
        let mut info = runtara_agent_crypto::agent_info();
        info.capabilities[0].trusted = true;
        info.supports_connections = true;
        let wasm = fixture(counting_body(), 1, "", false);
        for types in [vec![], vec!["*".into()], vec!["".into()]] {
            info.integration_ids = types;
            assert!(registry.register(&info, &wasm, b"{}").is_err());
        }
        info.integration_ids = vec!["test".into()];
        info.supports_connections = false;
        assert!(registry.register(&info, &wasm, b"{}").is_err());
        info.supports_connections = true;
        assert!(
            registry
                .register(&info, &wat::parse_str("(component)").unwrap(), b"{}")
                .is_err()
        );
        registry.register(&info, &wasm, b"{}").unwrap();
        assert!(registry.register(&info, &wasm, b"{}").is_err());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn secret_guest_errors_are_discarded_and_malformed_output_is_rejected() {
        let secret = b"fixture-secret";
        let mut body = String::from("i32.const 0 i32.const 1 i32.store8 ");
        for (i, byte) in secret.iter().enumerate() {
            body.push_str(&format!(
                "i32.const {} i32.const {} i32.store8 ",
                16 + i,
                byte
            ));
        }
        body.push_str(&format!(
            "i32.const 4 i32.const 16 i32.store i32.const 8 i32.const {} i32.store",
            secret.len()
        ));
        let (executor, _) = executor(&fixture(&body, 1, "", false));
        let err = call(&executor, Duration::from_secs(5)).await.unwrap_err();
        assert!(err.contains("TRUSTED_CAPABILITY_FAILED"));
        assert!(!err.contains("fixture-secret"));
        let body = body.replace(
            "i32.const 0 i32.const 1 i32.store8",
            "i32.const 0 i32.const 0 i32.store8",
        );
        let (bad_output, _) = self::executor(&fixture(&body, 1, "", false));
        assert!(
            call(&bad_output, Duration::from_secs(5))
                .await
                .unwrap_err()
                .contains("TRUSTED_INVALID_OUTPUT")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fresh_instances_for_sequential_and_concurrent_calls() {
        let (executor, _) = executor(&fixture(counting_body(), 1, "", false));
        for _ in 0..2 {
            assert_eq!(call(&executor, Duration::from_secs(5)).await.unwrap(), b"1");
        }
        let mut calls = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let executor = executor.clone();
            calls.spawn(async move { call(&executor, Duration::from_secs(5)).await });
        }
        while let Some(result) = calls.join_next().await {
            assert_eq!(result.unwrap().unwrap(), b"1");
        }
        assert_eq!(executor.permits.available_permits(), 16);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn traps_memory_output_and_startup_failures_release_resources() {
        for (body, pages, start) in [
            ("unreachable", 1, ""),
            (counting_body(), 1025, ""),
            (
                "i32.const 4 i32.const 16 i32.store i32.const 8 i32.const 1048577 i32.store",
                17,
                "",
            ),
            (counting_body(), 1, "unreachable"),
        ] {
            let (executor, _) = executor(&fixture(body, pages, start, false));
            assert!(call(&executor, Duration::from_secs(5)).await.is_err());
            assert_eq!(executor.permits.available_permits(), 16);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn denied_timer_import_in_startup_and_invocation() {
        for (body, start) in [
            ("i64.const 0 call $sleep", ""),
            (counting_body(), "i64.const 0 call $sleep"),
        ] {
            let (executor, _) = executor(&fixture(body, 1, start, true));
            assert!(call(&executor, Duration::from_secs(5)).await.is_err());
            assert_eq!(executor.permits.available_permits(), 16);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deadlines_and_cancellation_stop_running_guest() {
        let (executor, credentials) =
            executor(&fixture("(loop $forever br $forever)", 1, "", false));
        assert!(call(&executor, Duration::from_millis(150)).await.is_err());
        assert_eq!(executor.permits.available_permits(), 16);
        let run = executor.clone();
        let task = tokio::spawn(async move { call(&run, Duration::from_secs(30)).await });
        while credentials.0.load(Ordering::SeqCst) < 2 {
            tokio::task::yield_now().await;
        }
        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_secs(2), async {
            while executor.permits.available_permits() != 16 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("aborting caller must stop isolated guest and release permit");
    }
}
