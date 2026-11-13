// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The guest: a compiled workflow under Wasmtime, given what the embedded
//! runner gives it (`runtara-component-host`'s `workflow.rs`) and nothing
//! more. Its only way out is `wasi:http@0.3` to the workflow API origin,
//! which goes over the socket this process was handed.

use std::collections::BTreeMap;
use std::future::Future;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::client::conn::http2::SendRequest;
use runtara_workflow_api::{Caps, headers};
use wasmtime::component::{Component, Linker, ResourceTable, TypedFunc};
use wasmtime::{Config, Engine, OptLevel, Store, UpdateDeadline};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{
    Error as HttpError, RequestOptions, WasiBody, WasiHttp, WasiHttpCtx, WasiHttpCtxView,
    WasiHttpHooks, WasiHttpView,
};

/// The export a compiled workflow runs by.
pub const RUN_EXPORT: &str = "wasi:cli/run@0.3.0";
/// How often the epoch advances: how soon a guest that only computes is
/// interrupted, and how often it lets the relay's tasks run.
const EPOCH_TICK: Duration = Duration::from_millis(100);
/// The longest reason reported for a guest that failed.
const MAX_REASON: usize = 2048;

/// The engine a component is compiled for and run on: `compile` and `run`
/// must agree on it, which they do by being this one function in one binary.
pub fn engine() -> Result<Engine, String> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.async_stack_size(2 * 1024 * 1024);
    config.consume_fuel(false);
    config.epoch_interruption(true);
    config.cranelift_opt_level(OptLevel::Speed);
    config.parallel_compilation(true);
    config.wasm_backtrace_max_frames(NonZeroUsize::new(64));
    Engine::new(&config).map_err(|error| format!("{error:#}"))
}

/// Compiles `wasm` for [`engine`], refusing a component this process could
/// not run: one that does not export [`RUN_EXPORT`], or that imports
/// something a run is not given. That is decided as a run decides it, by
/// linking: a guest is given WASI and `wasi:http`, and an import of anything
/// else that has a function in it has nothing behind it here. (Interfaces of
/// types alone, and trusted pins, need nothing behind them.)
pub fn compile(engine: &Engine, wasm: &[u8]) -> Result<Vec<u8>, String> {
    let component = Component::new(engine, wasm).map_err(|error| format!("{error:#}"))?;
    if !component
        .component_type()
        .exports(engine)
        .any(|(name, _)| name == RUN_EXPORT)
    {
        return Err(format!("the component does not export {RUN_EXPORT}"));
    }
    linker(engine)
        .and_then(|linker| linker.instantiate_pre(&component))
        .map_err(|error| format!("the component cannot be linked: {error:#}"))?;
    component.serialize().map_err(|error| format!("{error:#}"))
}

/// What a run may use.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Linear memory, summed over the guest's memories.
    pub memory_bytes: usize,
    /// Elements in any one table.
    pub table_elements: usize,
    /// How long the guest may run.
    pub lifetime: Duration,
    pub env: BTreeMap<String, String>,
}

/// How the guest's `run` ended. What the run's outcome is, the guest posted
/// (or did not) over the API; this says only what became of the guest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ended {
    /// `run` returned ok.
    Returned,
    /// `run` returned an error.
    Failed,
    /// The guest trapped, or could not be started.
    Trapped(String),
    /// The guest ran past its lifetime.
    Timeout,
}

struct Limiter {
    max_memory_bytes: usize,
    max_table_elements: usize,
    memory_bytes: usize,
}

impl wasmtime::ResourceLimiter for Limiter {
    // Also called for each memory's first allocation (`current` is 0).
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let total = (self.memory_bytes - current).saturating_add(desired);
        if total > self.max_memory_bytes {
            return Ok(false);
        }
        self.memory_bytes = total;
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

type IoFuture = Box<dyn Future<Output = Result<(), HttpError>> + Send>;
type SendFuture =
    Box<dyn Future<Output = Result<(http::Response<WasiBody>, IoFuture), HttpError>> + Send>;

fn denied() -> SendFuture {
    Box::new(async { Err(HttpError::HttpRequestDenied) })
}

/// `wasi:http@0.2`: always denied, as on the embedded runner.
struct Denied;

impl WasiHttpHooks for Denied {
    fn send_request(
        &mut self,
        _request: http::Request<WasiBody>,
        _options: Option<RequestOptions>,
        _fut: IoFuture,
    ) -> SendFuture {
        denied()
    }
}

/// A request in flight, dropped with the guest's call.
struct AbortOnDrop<T>(tokio::task::JoinHandle<Result<T, HttpError>>);

impl<T> Future for AbortOnDrop<T> {
    type Output = Result<T, HttpError>;

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map(|joined| {
            joined.unwrap_or_else(|error| Err(HttpError::InternalError(Some(error.to_string()))))
        })
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// `wasi:http@0.3`: the workflow API origin goes over the socket; every
/// other request is denied.
struct Api {
    socket: SendRequest<Full<Bytes>>,
    caps: Caps,
}

fn is_workflow_api(uri: &http::Uri) -> bool {
    uri.authority().is_some_and(|authority| {
        runtara_workflow_api::is_api_origin(
            uri.scheme_str(),
            authority.as_str(),
            authority.host(),
            authority.port_u16(),
        )
    })
}

impl WasiHttpHooks for Api {
    fn send_request(
        &mut self,
        request: http::Request<WasiBody>,
        _options: Option<RequestOptions>,
        _fut: IoFuture,
    ) -> SendFuture {
        if !is_workflow_api(request.uri()) {
            return denied();
        }
        let mut socket = self.socket.clone();
        let caps = self.caps;
        // On a task of its own, so the call makes progress while the guest
        // computes: the Store's event loop polls the returned future only
        // when the guest yields to it.
        let served = AbortOnDrop(tokio::spawn(async move {
            let (parts, body) = request.into_parts();
            let limit = caps.request_body_bytes;
            let body = Limited::new(body, limit)
                .collect()
                .await
                .map_err(|_| HttpError::HttpRequestBodySize(Some(limit as u64)))?
                .to_bytes();
            let mut relayed = http::Request::builder()
                .method(parts.method)
                .uri(parts.uri)
                .body(Full::new(body))
                .map_err(|_| HttpError::HttpRequestUriInvalid)?;
            for (name, value) in &parts.headers {
                if !headers::is_stripped(name.as_str()) && name != http::header::HOST {
                    relayed.headers_mut().append(name.clone(), value.clone());
                }
            }
            let response = socket
                .send_request(relayed)
                .await
                .map_err(|_| HttpError::ConnectionTerminated)?;
            let (parts, body) = response.into_parts();
            let limit = caps.response_body_bytes;
            let body = Limited::new(body, limit)
                .collect()
                .await
                .map_err(|_| HttpError::HttpResponseBodySize(Some(limit as u64)))?
                .to_bytes();
            Ok(http::Response::from_parts(parts, body))
        }));
        Box::new(async move {
            let response: http::Response<Bytes> = served.await?;
            let io: IoFuture = Box::new(async { Ok(()) });
            let response = response.map(|body| {
                Full::new(body)
                    .map_err(|never| match never {})
                    .boxed_unsync()
            });
            Ok((response, io))
        })
    }
}

struct State {
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    denied: Denied,
    api: Api,
    limiter: Limiter,
    deadline: Instant,
    timed_out: bool,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl WasiHttpView for State {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: &mut self.api,
        }
    }
}

impl State {
    fn p2_http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: &mut self.denied,
        }
    }
}

fn linker(engine: &Engine) -> wasmtime::Result<Linker<State>> {
    let mut linker = Linker::<State>::new(engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    {
        use wasmtime_wasi_http::p2::bindings;
        let options = bindings::LinkOptions::default();
        bindings::http::outgoing_handler::add_to_linker::<_, WasiHttp>(
            &mut linker,
            State::p2_http,
        )?;
        bindings::http::types::add_to_linker::<_, WasiHttp>(
            &mut linker,
            &options.into(),
            State::p2_http,
        )?;
    }
    wasmtime_wasi::p3::add_to_linker(&mut linker)?;
    wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
    Ok(linker)
}

fn reason(error: impl std::fmt::Display) -> String {
    let mut text = format!("{error:#}");
    if text.len() > MAX_REASON {
        let mut end = MAX_REASON;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// Runs the precompiled workflow `compiled` (what [`compile`] made, for
/// [`engine`]) to its end, its workflow API calls sent on `socket`.
///
/// # Safety
///
/// `compiled` must be what [`compile`] returned for this build: Wasmtime runs
/// it as machine code without checking it.
pub async unsafe fn run(
    engine: &Engine,
    compiled: &[u8],
    limits: Limits,
    socket: SendRequest<Full<Bytes>>,
) -> Ended {
    // SAFETY: the caller's contract.
    let component = match unsafe { Component::deserialize(engine, compiled) } {
        Ok(component) => component,
        Err(error) => return Ended::Trapped(reason(error)),
    };
    let linker = match linker(engine) {
        Ok(linker) => linker,
        Err(error) => return Ended::Trapped(reason(error)),
    };

    let mut wasi = WasiCtxBuilder::new();
    // No socket of any kind, and no file: nothing is preopened.
    wasi.allow_tcp(false)
        .allow_udp(false)
        .allow_ip_name_lookup(false)
        .stdout(std::io::stderr())
        .stderr(std::io::stderr());
    for (name, value) in &limits.env {
        wasi.env(name, value);
    }
    let state = State {
        wasi: wasi.build(),
        http: WasiHttpCtx::new(),
        table: ResourceTable::new(),
        denied: Denied,
        api: Api {
            socket,
            caps: Caps::default(),
        },
        limiter: Limiter {
            max_memory_bytes: limits.memory_bytes,
            max_table_elements: limits.table_elements,
            memory_bytes: 0,
        },
        deadline: Instant::now() + limits.lifetime,
        timed_out: false,
    };
    let mut store = Store::new(engine, state);
    store.limiter(|state| &mut state.limiter);
    // At each tick a guest past its lifetime is interrupted; any other lets
    // this thread's other tasks (the relay) run, then goes on.
    store.epoch_deadline_callback(|mut context| {
        if Instant::now() >= context.data().deadline {
            context.data_mut().timed_out = true;
            return Ok(UpdateDeadline::Interrupt);
        }
        Ok(UpdateDeadline::Yield(1))
    });
    store.set_epoch_deadline(1);
    let ticker = engine.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(EPOCH_TICK);
            ticker.increment_epoch();
        }
    });

    let called = async {
        let instance = linker.instantiate_async(&mut store, &component).await?;
        let interface = instance
            .get_export_index(&mut store, None, RUN_EXPORT)
            .ok_or_else(|| wasmtime::Error::msg(format!("no {RUN_EXPORT} export")))?;
        let run = instance
            .get_export_index(&mut store, Some(&interface), "run")
            .ok_or_else(|| wasmtime::Error::msg(format!("{RUN_EXPORT} has no run")))?;
        let run: TypedFunc<(), (Result<(), ()>,)> = instance.get_typed_func(&mut store, run)?;
        let (ran,) = run.call_async(&mut store, ()).await?;
        Ok::<_, wasmtime::Error>(ran)
    };
    // A guest waiting on a call computes nothing, so no tick interrupts it:
    // its lifetime is bounded here as well.
    let ran = tokio::time::timeout(limits.lifetime, called).await;
    match ran {
        Err(_) => Ended::Timeout,
        Ok(Ok(Ok(()))) => Ended::Returned,
        Ok(Ok(Err(()))) => Ended::Failed,
        Ok(Err(_)) if store.data().timed_out => Ended::Timeout,
        Ok(Err(error)) => Ended::Trapped(reason(error)),
    }
}
