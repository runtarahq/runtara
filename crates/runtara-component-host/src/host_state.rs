//! Per-call host state for runtara agent components.
//!
//! `HostState` lives inside a `wasmtime::Store` and provides:
//! - the WASI Preview 2 context (`WasiCtx`) — env vars, stderr, no fs/stdin.
//! - the WASI HTTP context (`WasiHttpCtx`) — outbound HTTP handling.
//! - a `WasiHttpHooks` impl that defensively injects `X-Org-Id` on every
//!   outbound request and strips `Authorization`/`Cookie` from requests
//!   that don't target our proxy. See § 6 / § 9 of the migration plan.

use std::sync::Arc;
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    WasiHttpCtx,
    p2::{
        HttpResult, WasiHttpCtxView, WasiHttpHooks, WasiHttpView,
        body::HyperOutgoingBody,
        default_send_request,
        types::{HostFutureIncomingResponse, OutgoingRequestConfig},
    },
};

/// Per-call context. One of these is built before each component invocation.
/// Carries everything the host needs to know about the call but the component
/// must not see — secrets (proxy holds them) and tenancy that we enforce
/// host-side.
#[derive(Clone, Debug)]
pub struct CallContext {
    pub tenant_id: String,
    pub instance_id: Option<String>,
    pub proxy_url: String,
    pub proxy_host: String,
    pub core_http_url: String,
    pub agent_service_url: String,
    pub object_model_url: String,
    pub connection_service_url: Option<String>,
}

impl CallContext {
    /// Build a context for the test-dispatcher path (no instance id, no
    /// checkpoint id).
    pub fn for_test(
        tenant_id: impl Into<String>,
        proxy_url: impl Into<String>,
        agent_service_url: impl Into<String>,
        object_model_url: impl Into<String>,
        core_http_url: impl Into<String>,
    ) -> Self {
        let proxy_url = proxy_url.into();
        let proxy_host = url_host(&proxy_url);
        Self {
            tenant_id: tenant_id.into(),
            instance_id: None,
            proxy_url,
            proxy_host,
            core_http_url: core_http_url.into(),
            agent_service_url: agent_service_url.into(),
            object_model_url: object_model_url.into(),
            connection_service_url: None,
        }
    }

    /// Placeholder context used at registry-load time to call
    /// `list-capabilities`. The agent should not make outbound HTTP during
    /// metadata enumeration; if it does the request goes nowhere useful.
    pub fn placeholder_for_metadata() -> Self {
        Self {
            tenant_id: String::new(),
            instance_id: None,
            proxy_url: String::new(),
            proxy_host: String::new(),
            core_http_url: String::new(),
            agent_service_url: String::new(),
            object_model_url: String::new(),
            connection_service_url: None,
        }
    }
}

fn url_host(s: &str) -> String {
    s.parse::<http::Uri>()
        .ok()
        .filter(|u| u.scheme().is_some())
        .and_then(|u| u.host().map(str::to_string))
        .unwrap_or_default()
}

/// Hooks installed into the `wasi:http` host impl. Implements
/// `WasiHttpHooks` so the host can intercept every outbound request.
pub struct HostHooks {
    pub ctx: Arc<CallContext>,
}

impl WasiHttpHooks for HostHooks {
    fn send_request(
        &mut self,
        mut request: http::Request<HyperOutgoingBody>,
        config: OutgoingRequestConfig,
    ) -> HttpResult<HostFutureIncomingResponse> {
        // Defensive header injection. Force X-Org-Id from the host; override
        // any value the guest set. Closes the "tampered SDK could spoof
        // tenancy" hole.
        if !self.ctx.tenant_id.is_empty()
            && let Ok(v) = self.ctx.tenant_id.parse::<http::HeaderValue>()
        {
            request.headers_mut().insert("X-Org-Id", v);
        }
        if let Some(iid) = &self.ctx.instance_id
            && let Ok(v) = iid.parse::<http::HeaderValue>()
        {
            request.headers_mut().insert("X-Runtara-Instance-Id", v);
        }

        // Credentials must flow via the proxy, never directly from the agent.
        let dest_host = request.uri().host().map(str::to_string).unwrap_or_default();
        if !self.ctx.proxy_host.is_empty() && dest_host != self.ctx.proxy_host {
            request.headers_mut().remove(http::header::AUTHORIZATION);
            request.headers_mut().remove(http::header::COOKIE);
        }

        Ok(default_send_request(request, config))
    }
}

pub struct HostState {
    pub wasi: WasiCtx,
    pub http: WasiHttpCtx,
    pub table: ResourceTable,
    pub hooks: HostHooks,
    pub ctx: Arc<CallContext>,
}

impl HostState {
    pub fn new(ctx: Arc<CallContext>) -> Self {
        let mut builder = WasiCtxBuilder::new();
        builder.inherit_stderr();

        if !ctx.tenant_id.is_empty() {
            builder.env("RUNTARA_TENANT_ID", &ctx.tenant_id);
        }
        if !ctx.proxy_url.is_empty() {
            builder.env("RUNTARA_HTTP_PROXY_URL", &ctx.proxy_url);
        }
        if !ctx.core_http_url.is_empty() {
            builder.env("RUNTARA_HTTP_URL", &ctx.core_http_url);
        }
        if !ctx.agent_service_url.is_empty() {
            builder.env("RUNTARA_AGENT_SERVICE_URL", &ctx.agent_service_url);
        }
        if !ctx.object_model_url.is_empty() {
            builder.env("RUNTARA_OBJECT_MODEL_URL", &ctx.object_model_url);
        }
        if let Some(url) = &ctx.connection_service_url {
            builder.env("CONNECTION_SERVICE_URL", url);
        }
        if let Some(iid) = &ctx.instance_id {
            builder.env("RUNTARA_INSTANCE_ID", iid);
        }

        Self {
            wasi: builder.build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            hooks: HostHooks { ctx: ctx.clone() },
            ctx,
        }
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
    fn host_state_builds_with_full_context() {
        let ctx = Arc::new(CallContext::for_test(
            "tenant-1",
            "http://proxy.local:7001",
            "http://agent.local:7002",
            "http://obj.local:7003",
            "http://core.local:7004",
        ));
        let _state = HostState::new(ctx);
    }

    #[test]
    fn url_host_extracts_authority() {
        assert_eq!(url_host("http://proxy.local:7001"), "proxy.local");
        assert_eq!(url_host("https://example.com/path"), "example.com");
        assert_eq!(url_host("not-a-url"), "");
    }
}
