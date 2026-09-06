//! Test-only architectural proof. No production linker exposes these imports.
//! The parent component contains the child binaries and owns all orchestration.
//! The host only spawns isolated executions, delivers messages, cancels and joins.

mod production_research;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, ensure};
use tokio::sync::{Notify, watch};
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store, UpdateDeadline};

use crate::engine::{EPOCH_TICK, EngineConfig, build_engine};
use crate::host_io::{HostIoContext, add_host_io_to_linker};

// A tagged outcome: low 32 bits are the guest's u32 return value; the high
// word belongs to the executor. Guest output cannot forge a cancellation.
const CANCELLED: u64 = 1 << 32;
const TRAPPED: u64 = 2 << 32;
const FAILSAFE: Duration = Duration::from_secs(15);
const CPU: &str = include_str!("../tests/fixtures/isolated_step_poc/cpu.wat");
const SIBLING: &str = include_str!("../tests/fixtures/isolated_step_poc/sibling.wat");
const PARENT: &str = include_str!("../tests/fixtures/isolated_step_poc/parent.wat");
const RECOVERY: &str = r#"(component
    (core module $m (func (export "run") (param i32) (result i32) local.get 0))
    (core instance $i (instantiate $m))
    (func (export "run") (param "input" u32) (result u32)
        (canon lift (core func $i "run"))))"#;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Event {
    Spawned(u32),
    Started(u32),
    CancelRequested(u32),
    StoreDropped(u32),
    Settled(u32, u64),
    Joined(u32),
}

type Events = Arc<watch::Sender<Vec<Event>>>;

fn record(events: &Events, event: Event) {
    events.send_modify(|events| events.push(event));
}

struct Cancel {
    flag: AtomicBool,
    wake: Notify,
}

impl Cancel {
    fn request(&self, engine: &Engine) {
        self.flag.store(true, Ordering::SeqCst);
        self.wake.notify_one();
        // Shared engine, but each Store callback checks its OWN flag.
        engine.increment_epoch();
    }
}

struct Task {
    cancel: Arc<Cancel>,
    result: watch::Receiver<Option<Result<u64, String>>>,
    mailbox: watch::Sender<u32>,
    worker: tokio::task::JoinHandle<()>,
}

struct Executor {
    engine: Arc<Engine>,
    events: Events,
    tasks: Mutex<BTreeMap<u32, Task>>,
}

impl Drop for Executor {
    fn drop(&mut self) {
        // Parent trap/cancellation cannot leave a non-cooperative step running.
        // Tests additionally await every worker before shutting down the ticker.
        for task in self.tasks.get_mut().unwrap().values() {
            task.cancel.request(&self.engine);
        }
    }
}

struct ChildState {
    id: u32,
    events: Events,
    inbox: watch::Receiver<u32>,
    limits: wasmtime::StoreLimits,
}

impl Drop for ChildState {
    fn drop(&mut self) {
        record(&self.events, Event::StoreDropped(self.id));
    }
}

impl HostIoContext for ChildState {
    fn http_deadline(&self) -> Option<tokio::time::Instant> {
        None
    }
}

impl Executor {
    fn spawn(&self, bytes: Vec<u8>, input: u32) -> Result<u32> {
        // Fixed bounds for the PoC package/handle representation. Compilation is
        // admission work, not something the step cancellation guard interrupts.
        ensure!(bytes.len() < 120_000, "PoC child artifact too large");
        let component = Component::new(&self.engine, &bytes)?;
        let mut tasks = self.tasks.lock().unwrap();
        let id = u32::try_from(tasks.len() + 1)?;
        let cancel = Arc::new(Cancel {
            flag: AtomicBool::new(false),
            wake: Notify::new(),
        });
        let (result_tx, result) = watch::channel(None);
        let (mailbox, inbox) = watch::channel(0);
        let engine = self.engine.clone();
        let events = self.events.clone();
        let task_cancel = cancel.clone();
        record(&events, Event::Spawned(id));
        let worker = tokio::spawn(async move {
            let outcome = run_child(
                engine,
                component,
                id,
                input,
                task_cancel,
                inbox,
                events.clone(),
            )
            .await
            .map_err(|error| format!("{error:#}"));
            if let Ok(value) = outcome.as_ref() {
                record(&events, Event::Settled(id, *value));
            }
            result_tx.send_replace(Some(outcome));
        });
        tasks.insert(
            id,
            Task {
                cancel,
                result,
                mailbox,
                worker,
            },
        );
        Ok(id)
    }

    fn cancel(&self, id: u32) -> Result<()> {
        let tasks = self.tasks.lock().unwrap();
        let task = tasks.get(&id).context("unknown task")?;
        record(&self.events, Event::CancelRequested(id));
        task.cancel.request(&self.engine);
        Ok(())
    }

    async fn join(&self, id: u32) -> Result<u64> {
        let mut result = self
            .tasks
            .lock()
            .unwrap()
            .get(&id)
            .context("unknown task")?
            .result
            .clone();
        let value = result
            .wait_for(Option::is_some)
            .await?
            .as_ref()
            .unwrap()
            .clone();
        record(&self.events, Event::Joined(id));
        value.map_err(|error| anyhow!(error))
    }

    fn send(&self, id: u32, message: u32) -> Result<()> {
        self.tasks
            .lock()
            .unwrap()
            .get(&id)
            .context("unknown task")?
            .mailbox
            .send(message)?;
        Ok(())
    }

    async fn reap(&self) -> Result<()> {
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
        for task in tasks.values() {
            task.cancel.request(&self.engine);
        }
        for task in tasks.into_values() {
            tokio::time::timeout(FAILSAFE, task.worker).await??;
        }
        Ok(())
    }
}

async fn run_child(
    engine: Arc<Engine>,
    component: Component,
    id: u32,
    input: u32,
    cancel: Arc<Cancel>,
    inbox: watch::Receiver<u32>,
    events: Events,
) -> Result<u64> {
    let mut linker = Linker::<ChildState>::new(&engine);
    linker.root().func_wrap("mark-started", |store, (): ()| {
        record(&store.data().events, Event::Started(store.data().id));
        Ok(())
    })?;
    linker.root().func_wrap_async("receive", |store, (): ()| {
        let mut inbox = store.data().inbox.clone();
        Box::new(async move { Ok((*inbox.wait_for(|value| *value != 0).await?,)) })
    })?;
    // The actual production HTTP transport, not a simulated hanging future.
    add_host_io_to_linker(&mut linker)?;
    let mut store = Store::new(
        &engine,
        ChildState {
            id,
            events,
            inbox,
            limits: wasmtime::StoreLimitsBuilder::new()
                .memory_size(1024 * 1024)
                .instances(16)
                .memories(4)
                .tables(4)
                .build(),
        },
    );
    store.limiter(|state| &mut state.limits);
    let epoch_cancel = cancel.clone();
    store.epoch_deadline_callback(move |_| {
        Ok(if epoch_cancel.flag.load(Ordering::SeqCst) {
            UpdateDeadline::Interrupt
        } else {
            UpdateDeadline::Yield(1)
        })
    });
    store.set_epoch_deadline(1);
    let outcome = {
        let run = async {
            let instance = linker.instantiate_async(&mut store, &component).await?;
            let func = instance.get_typed_func::<(u32,), (u32,)>(&mut store, "run")?;
            Ok::<u32, anyhow::Error>(func.call_async(&mut store, (input,)).await?.0)
        };
        tokio::pin!(run);
        tokio::select! {
            biased;
            result = &mut run => match result {
                Ok(value) => Ok(u64::from(value)),
                Err(error) if cancel.flag.load(Ordering::SeqCst)
                    && error.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::Interrupt) => Ok(CANCELLED),
                Err(error) => {
                    eprintln!("PoC child {id} trapped: {error:#}");
                    Ok(TRAPPED)
                },
            },
            _ = cancel.wake.notified() => Ok(CANCELLED),
            _ = tokio::time::sleep(FAILSAFE) => Err(anyhow!("test failsafe expired; cancellation failed")),
        }
    };
    // The parent must never recover while the victim still owns guest memory,
    // host futures, or a live execution task in this Store.
    drop(store);
    outcome
}

struct ParentState {
    executor: Arc<Executor>,
    commands: watch::Receiver<u32>,
}

impl Drop for ParentState {
    fn drop(&mut self) {
        for task in self.executor.tasks.lock().unwrap().values() {
            task.cancel.request(&self.executor.engine);
        }
    }
}

async fn run_parent(engine: Arc<Engine>, bytes: Vec<u8>, state: ParentState) -> Result<u32> {
    let component = Component::new(&engine, bytes)?;
    let mut linker = Linker::<ParentState>::new(&engine);
    linker
        .root()
        .func_wrap("spawn", |store, (bytes, input): (Vec<u8>, u32)| {
            Ok((store
                .data()
                .executor
                .spawn(bytes, input)
                .map_err(|e| wasmtime::Error::msg(e.to_string()))?,))
        })?;
    linker.root().func_wrap("cancel", |store, (id,): (u32,)| {
        store
            .data()
            .executor
            .cancel(id)
            .map_err(|e| wasmtime::Error::msg(e.to_string()))
    })?;
    linker
        .root()
        .func_wrap("send", |store, (id, value): (u32, u32)| {
            store
                .data()
                .executor
                .send(id, value)
                .map_err(|e| wasmtime::Error::msg(e.to_string()))
        })?;
    linker
        .root()
        .func_wrap_async("join", |store, (id,): (u32,)| {
            let executor = store.data().executor.clone();
            Box::new(async move {
                Ok((executor
                    .join(id)
                    .await
                    .map_err(|e| wasmtime::Error::msg(e.to_string()))?,))
            })
        })?;
    linker
        .root()
        .func_wrap_async("receive-command", |store, (): ()| {
            let mut commands = store.data().commands.clone();
            Box::new(async move { Ok((*commands.wait_for(|id| *id != 0).await?,)) })
        })?;
    let mut store = Store::new(&engine, state);
    store.epoch_deadline_callback(|_| Ok(UpdateDeadline::Yield(1)));
    store.set_epoch_deadline(1);
    let instance = linker.instantiate_async(&mut store, &component).await?;
    let run = instance.get_typed_func::<(), (u32,)>(&mut store, "run")?;
    Ok(run.call_async(&mut store, ()).await?.0)
}

fn escaped(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("\\{byte:02x}")).collect()
}

fn package(victim: &str, before_cancel: &str) -> Result<Vec<u8>> {
    package_with_expected(victim, before_cancel, CANCELLED)
}

fn package_with_expected(victim: &str, before_cancel: &str, expected: u64) -> Result<Vec<u8>> {
    let mut parent = PARENT
        .replace("{{BEFORE_CANCEL}}", before_cancel)
        .replace("(i64.const 4294967296)", &format!("(i64.const {expected})"));
    for (name, source) in [
        ("VICTIM", victim),
        ("SIBLING", SIBLING),
        ("RECOVERY", RECOVERY),
    ] {
        let bytes = wat::parse_str(source)?;
        parent = parent.replace(&format!("{{{{{name}}}}}"), &escaped(&bytes));
        parent = parent.replace(&format!("{{{{{name}_LEN}}}}"), &bytes.len().to_string());
    }
    Ok(wat::parse_str(parent)?)
}

struct Harness {
    executor: Arc<Executor>,
    commands: watch::Sender<u32>,
    parent: tokio::task::JoinHandle<Result<u32>>,
    ticker: tokio::task::JoinHandle<()>,
}

impl Harness {
    fn start(bytes: Vec<u8>) -> Result<Self> {
        let engine = build_engine(&EngineConfig {
            cache_dir: None,
            ..EngineConfig::default()
        })?;
        let events = Arc::new(watch::channel(Vec::new()).0);
        let executor = Arc::new(Executor {
            engine: engine.clone(),
            events,
            tasks: Mutex::new(BTreeMap::new()),
        });
        let (commands, receiver) = watch::channel(0);
        let ticker_engine = engine.clone();
        let ticker = tokio::spawn(async move {
            loop {
                tokio::time::sleep(EPOCH_TICK).await;
                ticker_engine.increment_epoch();
            }
        });
        let parent = tokio::spawn(run_parent(
            engine,
            bytes,
            ParentState {
                executor: executor.clone(),
                commands: receiver,
            },
        ));
        Ok(Self {
            executor,
            commands,
            parent,
            ticker,
        })
    }

    async fn wait_for(&self, event: Event) -> Result<()> {
        let mut events = self.executor.events.subscribe();
        tokio::time::timeout(FAILSAFE, events.wait_for(|events| events.contains(&event))).await??;
        Ok(())
    }

    async fn finish(mut self, expected_victim: u64) -> Result<Vec<Event>> {
        let result = tokio::time::timeout(FAILSAFE, &mut self.parent).await;
        self.executor.reap().await?;
        self.ticker.abort();
        assert_eq!(
            result???, 42,
            "parent must run its recovery and continuation"
        );
        let events = self.executor.events.borrow().clone();
        let at = |event| {
            events
                .iter()
                .position(|candidate| *candidate == event)
                .unwrap()
        };
        assert!(at(Event::StoreDropped(1)) < at(Event::Joined(1)));
        assert!(at(Event::Joined(1)) < at(Event::Spawned(3)));
        assert!(at(Event::StoreDropped(1)) < at(Event::StoreDropped(2)));
        assert!(events.contains(&Event::Settled(1, expected_victim)));
        assert!(events.contains(&Event::Settled(2, 7)));
        assert!(events.contains(&Event::Settled(3, 11)));
        Ok(events)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // Failure cleanup: signal first, then abort Rust waiters. Epoch checking
        // must remain available until workers next yield; request bumps it.
        for task in self.executor.tasks.lock().unwrap().values() {
            task.cancel.request(&self.executor.engine);
        }
        self.parent.abort();
        self.ticker.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guest_cancels_cpu_bound_step_and_preserves_parent_and_live_sibling() -> Result<()> {
    let harness = Harness::start(package(CPU, "")?)?;
    harness.wait_for(Event::Started(1)).await?;
    harness.wait_for(Event::Started(2)).await?;
    ensure!(
        !harness.parent.is_finished(),
        "parent must be awaiting the user command"
    );
    let started = std::time::Instant::now();
    harness.commands.send(1)?;
    let events = harness.finish(CANCELLED).await?;
    assert!(started.elapsed() < Duration::from_secs(5));
    eprintln!("CPU cancellation: {events:?}");
    Ok(())
}

async fn http_cancellation(send_headers: bool) -> Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let (accepted_tx, accepted) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await?;
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).await?;
            request.push(byte[0]);
            ensure!(request.len() < 8192, "unexpected request size");
        }
        if send_headers {
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nx")
                .await?;
        }
        let _ = accepted_tx.send(());
        // A cancelled client must release its connection without waiting for
        // the server's response or the HTTP timeout. EOF or reset both count.
        let mut byte = [0];
        let closed = stream.read(&mut byte).await;
        ensure!(matches!(closed, Ok(0) | Err(_)), "unexpected client bytes");
        Ok::<(), anyhow::Error>(())
    });
    let request = serde_json::to_vec(&serde_json::json!({
        "method": "GET", "url": format!("http://{address}/hung"), "timeout_ms": 120_000
    }))?;
    let victim = include_str!("../tests/fixtures/isolated_step_poc/http.wat")
        .replace("{{REQUEST}}", &escaped(&request))
        .replace("{{REQUEST_LEN}}", &request.len().to_string());
    let harness = Harness::start(package(&victim, "")?)?;
    harness.wait_for(Event::Started(2)).await?;
    tokio::time::timeout(FAILSAFE, accepted).await??;
    assert!(
        !harness
            .executor
            .events
            .borrow()
            .iter()
            .any(|event| matches!(event, Event::Settled(1, _)))
    );
    let started = std::time::Instant::now();
    harness.commands.send(1)?;
    let events = harness.finish(CANCELLED).await?;
    tokio::time::timeout(Duration::from_secs(5), server).await???;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "must not wait for the 120-second HTTP deadline"
    );
    eprintln!("HTTP cancellation (headers={send_headers}): {events:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guest_cancels_http_waiting_for_headers_and_continues() -> Result<()> {
    http_cancellation(false).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guest_cancels_http_stalled_body_and_continues() -> Result<()> {
    http_cancellation(true).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn no_cancel_command_leaves_busy_step_and_parent_running() -> Result<()> {
    let mut harness = Harness::start(package(CPU, "")?)?;
    harness.wait_for(Event::Started(1)).await?;
    harness.wait_for(Event::Started(2)).await?;
    assert!(
        tokio::time::timeout(EPOCH_TICK * 3, &mut harness.parent)
            .await
            .is_err()
    );
    assert!(
        !harness
            .executor
            .events
            .borrow()
            .iter()
            .any(|event| matches!(event, Event::StoreDropped(_)))
    );
    // The same still-running parent now receives a real user command.
    harness.commands.send(1)?;
    harness.finish(CANCELLED).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_trap_cancels_and_reaps_its_live_children() -> Result<()> {
    let mut harness = Harness::start(package(CPU, "unreachable")?)?;
    harness.wait_for(Event::Started(1)).await?;
    harness.wait_for(Event::Started(2)).await?;
    harness.commands.send(1)?;
    let error = tokio::time::timeout(FAILSAFE, &mut harness.parent)
        .await??
        .unwrap_err();
    assert!(error.to_string().contains("wasm"), "{error:#}");
    // Observe cleanup BEFORE harness.reap: it must come from parent teardown.
    harness.wait_for(Event::StoreDropped(1)).await?;
    harness.wait_for(Event::StoreDropped(2)).await?;
    assert!(
        !harness
            .executor
            .events
            .borrow()
            .contains(&Event::Spawned(3))
    );
    harness.executor.reap().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn duplicate_cancel_is_safe_and_does_not_cancel_the_sibling() -> Result<()> {
    let harness = Harness::start(package(CPU, "(call $cancel (local.get $victim))")?)?;
    harness.wait_for(Event::Started(1)).await?;
    harness.wait_for(Event::Started(2)).await?;
    harness.commands.send(1)?;
    harness.finish(CANCELLED).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancellation_after_completion_preserves_the_result() -> Result<()> {
    // A completed child is not retrospectively relabelled cancelled.
    // Only the guest's expected outcome changes; executor behavior is identical.
    let harness = Harness::start(package_with_expected(
        RECOVERY,
        "(drop (call $join (local.get $victim)))",
        17,
    )?)?;
    harness.wait_for(Event::Settled(1, 17)).await?;
    harness.wait_for(Event::Started(2)).await?;
    harness.commands.send(1)?;
    harness.finish(17).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guest_cancels_infinite_component_initialization_and_continues() -> Result<()> {
    let victim = include_str!("../tests/fixtures/isolated_step_poc/initialization_loop.wat");
    let harness = Harness::start(package(victim, "")?)?;
    harness.wait_for(Event::Started(1)).await?;
    harness.wait_for(Event::Started(2)).await?;
    harness.commands.send(1)?;
    harness.finish(CANCELLED).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_trap_is_contained_and_parent_runs_recovery() -> Result<()> {
    let victim = RECOVERY.replace("local.get 0", "unreachable");
    let harness = Harness::start(package_with_expected(&victim, "", TRAPPED)?)?;
    harness.wait_for(Event::Settled(1, TRAPPED)).await?;
    harness.wait_for(Event::Started(2)).await?;
    harness.commands.send(1)?;
    harness.finish(TRAPPED).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn arbitrary_guest_return_value_cannot_forge_a_cancelled_outcome() -> Result<()> {
    let victim = RECOVERY.replace("local.get 0", "i32.const -1");
    let expected = u64::from(u32::MAX);
    let harness = Harness::start(package_with_expected(&victim, "", expected)?)?;
    harness.wait_for(Event::Settled(1, expected)).await?;
    harness.wait_for(Event::Started(2)).await?;
    harness.commands.send(1)?;
    harness.finish(expected).await?;
    Ok(())
}
