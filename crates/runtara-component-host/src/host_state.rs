//! Per-call host state for runtara agent components.
//!
//! `HostState` lives inside a `wasmtime::Store` and provides:
//! - the WASI Preview 2 context (`WasiCtx`) — env vars, stderr, no fs/stdin.
//! - the WASI HTTP context (`WasiHttpCtx`) with raw HTTP denied.
//! - a `WasiHttpHooks` impl that denies raw WASI HTTP; outbound uses the typed host service.

use std::sync::Arc;
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    WasiHttpCtx,
    p2::{
        HttpResult, WasiHttpCtxView, WasiHttpHooks, WasiHttpView,
        bindings::http::types::ErrorCode,
        body::HyperOutgoingBody,
        types::{HostFutureIncomingResponse, OutgoingRequestConfig},
    },
};

use crate::host_io::HostIoContext;

/// Per-call context. One of these is built before each component invocation.
/// Carries authoritative identity and the optional legacy runtime address.
/// Credentials belong to injected native services, never this context.
#[derive(Clone, Debug)]
pub struct CallContext {
    pub tenant_id: String,
    pub instance_id: Option<String>,
    pub core_http_url: String,
}

impl CallContext {
    /// Build a context for the test-dispatcher path (no instance id, no
    /// checkpoint id).
    pub fn for_test(tenant_id: impl Into<String>, core_http_url: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            instance_id: None,
            core_http_url: core_http_url.into(),
        }
    }

    /// Placeholder context used at registry-load time to call
    /// `list-capabilities`. The agent should not make outbound HTTP during
    /// metadata enumeration; if it does the missing service fails explicitly.
    pub fn placeholder_for_metadata() -> Self {
        Self {
            tenant_id: String::new(),
            instance_id: None,
            core_http_url: String::new(),
        }
    }
}

/// Raw WASI HTTP cannot bypass the typed outbound service.
pub struct HostHooks;

impl WasiHttpHooks for HostHooks {
    fn send_request(
        &mut self,
        _request: http::Request<HyperOutgoingBody>,
        _config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        Err(ErrorCode::HttpRequestDenied.into())
    }
}

/// Cap on any single guest linear memory for a host-guarded invocation, in
/// bytes. A component carries one memory per inner core module, so this bounds
/// each, not their sum; growth past it fails the grow in-guest (an OOM trap)
/// rather than letting a runaway allocation exhaust the host. Matches the
/// runtime path's `WorkflowLimits` default.
pub const DEFAULT_GUEST_MEMORY_MAX_BYTES: usize = 1024 * 1024 * 1024;

/// Cap on elements in any single guest table for a host-guarded invocation.
pub const DEFAULT_GUEST_TABLE_MAX_ELEMENTS: usize = 10_000_000;

/// Per-instance resource limiter for a guest `Store`. Denies memory/table
/// growth past the configured caps and records the peak memory seen plus
/// whether a grow was ever denied. Mirrors the runtime path's
/// `WorkflowLimiter` (see `workflow.rs`); the test-dispatcher surface was
/// previously unlimited.
#[derive(Debug)]
pub struct GuestLimiter {
    pub max_memory_bytes: usize,
    pub max_table_elements: usize,
    pub memory_peak_bytes: u64,
    pub denied_memory_grow: bool,
}

impl GuestLimiter {
    fn new(max_memory_bytes: usize, max_table_elements: usize) -> Self {
        Self {
            max_memory_bytes,
            max_table_elements,
            memory_peak_bytes: 0,
            denied_memory_grow: false,
        }
    }
}

impl wasmtime::ResourceLimiter for GuestLimiter {
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

/// Marker recorded by the per-call epoch deadline callback so a
/// `Trap::Interrupt` can be told apart from a genuine guest trap once the call
/// returns. Mirrors the runtime path's `Termination`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Termination {
    /// The per-call wall-clock budget elapsed.
    Timeout,
}

pub struct HostState {
    pub(crate) outbound_http: Result<Arc<crate::outbound_http::RunOutboundHttp>, String>,
    pub(crate) database: Result<Arc<crate::database_host::RunDatabase>, String>,
    pub(crate) connection_resolver:
        Result<Arc<crate::connection_resolver_host::RunConnectionResolver>, String>,
    pub(crate) restricted: bool,
    pub(crate) trusted: Option<Arc<crate::trusted::TrustedExecutor>>,
    pub wasi: WasiCtx,
    pub http: WasiHttpCtx,
    pub table: ResourceTable,
    pub hooks: HostHooks,
    pub ctx: Arc<CallContext>,
    /// Memory/table caps for this call, enforced once the store installs it via
    /// `store.limiter(|s| &mut s.limiter)` (see `registry::instantiate`).
    pub limiter: GuestLimiter,
    /// Absolute active-execution deadline used by outbound HTTP. `None` is valid
    /// only for metadata/test stores, which still receive the outbound service's bounded
    /// per-request default.
    pub http_deadline: Option<tokio::time::Instant>,
    /// Set by the epoch deadline callback when it force-interrupts the guest.
    pub termination: Option<Termination>,
    pub(crate) cleanup_alarm: crate::cleanup_alarm::CleanupAlarmState,
}

impl HostState {
    pub fn with_outbound_http(mut self, service: Arc<dyn crate::OutboundHttpHost>) -> Self {
        self.outbound_http = crate::outbound_http::for_run(
            Some(&service),
            Some(&self.ctx.tenant_id),
            self.ctx.instance_id.as_deref(),
        );
        self
    }

    pub fn with_database(mut self, database: Arc<dyn crate::DatabaseHost>) -> Self {
        self.database =
            crate::database_host::database_for_run(Some(&database), Some(&self.ctx.tenant_id));
        self
    }

    /// Configure a native resolver using this invocation's host-owned tenant.
    pub fn with_connection_resolver(
        mut self,
        resolver: Arc<dyn crate::ConnectionResolverHost>,
    ) -> Self {
        self.connection_resolver = crate::connection_resolver_host::resolver_for_run(
            Some(&resolver),
            Some(&self.ctx.tenant_id),
        );
        self
    }

    pub(crate) fn restricted() -> Self {
        let mut state = Self::new(Arc::new(CallContext::placeholder_for_metadata()));
        state.wasi = WasiCtxBuilder::new()
            .allow_tcp(false)
            .allow_udp(false)
            .allow_ip_name_lookup(false)
            .build();
        state.restricted = true;
        state
    }

    pub fn new(ctx: Arc<CallContext>) -> Self {
        let mut builder = WasiCtxBuilder::new();
        builder.inherit_stderr();

        if !ctx.tenant_id.is_empty() {
            builder.env("RUNTARA_TENANT_ID", &ctx.tenant_id);
        }
        if !ctx.core_http_url.is_empty() {
            builder.env("RUNTARA_HTTP_URL", &ctx.core_http_url);
        }
        if let Some(iid) = &ctx.instance_id {
            builder.env("RUNTARA_INSTANCE_ID", iid);
        }

        Self {
            restricted: false,
            outbound_http: Err("native outbound HTTP service is not configured".into()),
            database: Err("native database service is not configured".into()),
            connection_resolver: Err("native connection resolver is not configured".into()),
            trusted: None,
            wasi: builder.build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            hooks: HostHooks,
            ctx,
            limiter: GuestLimiter::new(
                DEFAULT_GUEST_MEMORY_MAX_BYTES,
                DEFAULT_GUEST_TABLE_MAX_ELEMENTS,
            ),
            http_deadline: None,
            termination: None,
            cleanup_alarm: Default::default(),
        }
    }

    /// Attach the enclosing active-execution deadline before the store is
    /// instantiated. Host imports use it to make a request's deadline no
    /// later than the run that owns it.
    pub fn with_http_deadline(mut self, deadline: tokio::time::Instant) -> Self {
        self.http_deadline = Some(deadline);
        self
    }

    /// Override the per-call memory/table caps before the store is built.
    /// Callers set this to apply an operator-configured limit; the defaults are
    /// large enough that real agents never hit them.
    pub fn set_limits(&mut self, max_memory_bytes: usize, max_table_elements: usize) {
        self.limiter.max_memory_bytes = max_memory_bytes;
        self.limiter.max_table_elements = max_table_elements;
    }
}

impl HostIoContext for HostState {
    fn cleanup_alarm(&self) -> Option<&crate::cleanup_alarm::CleanupAlarmState> {
        (!self.restricted).then_some(&self.cleanup_alarm)
    }
    fn timers_allowed(&self) -> bool {
        !self.restricted
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for HostState {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: &mut self.hooks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_state_retains_authoritative_call_context() {
        let ctx = Arc::new(CallContext::for_test("tenant-1", "http://core.local:7004"));
        let state = HostState::new(Arc::clone(&ctx));

        // Native services must use the context supplied by the invocation host.
        assert!(Arc::ptr_eq(&state.ctx, &ctx));
    }

    #[test]
    fn host_state_starts_unterminated_with_default_table_cap() {
        let ctx = Arc::new(CallContext::for_test("tenant-1", "http://core.local:7004"));
        let state = HostState::new(ctx);

        // A fresh state has not been interrupted; the epoch callback is the
        // only thing allowed to set this.
        assert!(state.termination.is_none());
        assert_eq!(
            state.limiter.max_table_elements,
            DEFAULT_GUEST_TABLE_MAX_ELEMENTS
        );
        assert_eq!(state.limiter.memory_peak_bytes, 0);
        assert!(!state.limiter.denied_memory_grow);
    }

    #[test]
    fn guest_limiter_allows_growth_under_cap_and_tracks_peak() {
        use wasmtime::ResourceLimiter;
        let mut l = GuestLimiter::new(1024, 1000);
        assert!(l.memory_growing(0, 512, None).unwrap());
        assert!(l.memory_growing(512, 1024, None).unwrap());
        assert_eq!(l.memory_peak_bytes, 1024);
        assert!(!l.denied_memory_grow);
    }

    #[test]
    fn guest_limiter_denies_growth_over_cap_and_records_oom() {
        use wasmtime::ResourceLimiter;
        let mut l = GuestLimiter::new(1024, 1000);
        assert!(!l.memory_growing(512, 2048, None).unwrap());
        assert!(l.denied_memory_grow);
        // Peak only tracks granted growth.
        assert_eq!(l.memory_peak_bytes, 0);
    }

    #[test]
    fn guest_limiter_bounds_table_elements() {
        use wasmtime::ResourceLimiter;
        let mut l = GuestLimiter::new(1024, 1000);
        assert!(l.table_growing(0, 1000, None).unwrap());
        assert!(!l.table_growing(0, 1001, None).unwrap());
    }

    #[test]
    fn set_limits_overrides_defaults() {
        let ctx = Arc::new(CallContext::for_test("tenant-1", "http://core.local:7004"));
        let mut state = HostState::new(ctx);
        assert_eq!(
            state.limiter.max_memory_bytes,
            DEFAULT_GUEST_MEMORY_MAX_BYTES
        );
        state.set_limits(4096, 42);
        assert_eq!(state.limiter.max_memory_bytes, 4096);
        assert_eq!(state.limiter.max_table_elements, 42);
    }
}
