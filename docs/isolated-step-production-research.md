# Independent step cancellation: production research

Next: [detailed selective-isolation plan and compatibility gates](selective-isolation-plan.md).

Research date: 2026-09-06. Baseline: `0a04277c`, Wasmtime 46.0.1,
Rust 1.97.0. This is a proposed architecture supported by experiments, not an
implemented production cancellation feature. The [live-execution proof](isolated-step-cancellation-poc.md)
establishes the separate-Store mechanism; this document examines its costs and
the missing production contracts.

## Recommended boundary

Keep the graph, branching, retries, deadlines, and recovery decisions in parent
WASM. Give independently cancellable Agent invocations and embedded workflows
separate disposable Stores. Retain ordinary local execution for small transforms
that do not require independent hard interruption.

Package all required child WASM inside the workflow artifact, once per unique
artifact. A generic host executor resolves package-local artifact references and
provides start, cancellation, join, and resource cleanup. It must not choose the
next workflow step or implement retry policy. A custom section is a candidate
container; it requires a versioned runtime loader and is not executable by an
unmodified host. Self-contained code still depends on compatible runtime imports.

| Choice | Benefit | Cost or limitation |
|---|---|---|
| Existing shared Store and cooperative cancellation | Least new machinery; preserves current composition | Cannot forcibly remove arbitrary non-cooperative guest computation while preserving that Store |
| Separate Stores at selected boundaries | Hard interruption and trap containment; orchestration stays in WASM | New ABI, copies, per-invocation state, durable attempt contract |
| Separate Store for every tiny operation | Uniform cancellation boundary | Pays setup/marshalling repeatedly; inefficient for microsecond transforms |
| Separate process per boundary | Stronger containment of native failures and process memory | IPC and process lifecycle costs; not benchmarked here |

The pinned Wasmtime concurrent-call implementation documents Store destruction
as the hard-cancellation boundary. Dropping a call's result future alone does not
remove the guest task. Upstream also distinguishes cooperative task cancellation
from hard termination; this research does not assume a future API will change
the isolation requirement. [Wasmtime cancellation discussion](https://github.com/bytecodealliance/wasmtime/issues/11833).

## Measurements: runtime performance

Two local optimized runs used real staged `utils` and `crypto` components,
production engine configuration, linker, `HostState`, and registry `InstancePre`.
The comparison is a warm call in a reused Store versus creating a fresh Store,
instantiating cached code, finding the export, calling it, and dropping the Store.
The benchmark checks real outputs, including SHA-256. It excludes compilation
from the fresh-Store timings and checks correctness outside measured intervals.

| Case, 16-byte payload | Reused Store median | Cached fresh Store median | Added median cost |
|---|---:|---:|---:|
| Utils return-input | 1.79–1.83 µs | 32.67–33.50 µs | About 31–32 µs |
| Crypto hash | 4.62–4.71 µs | 35.12–36.29 µs | About 30–32 µs |

For a tiny transform this is roughly 8–19 times the measured call cost. For an
operation waiting hundreds of milliseconds on I/O, this setup cost would be a
small fraction of elapsed time, although that inference does not establish
throughput under load. First-run 1 MiB payload medians were 0.97 ms reused versus
1.30 ms fresh for utils, and 2.54 ms versus 2.81 ms for crypto. Large payloads
introduce material serialization, allocation, and copying costs.

Uncached compilation medians were 30–32 ms for utils and about 24 ms for crypto
(three samples per agent per run). Do not compile on each step. Existing
[registry code](../crates/runtara-component-host/src/registry.rs) already supports
compiled definitions and prepared linking. Wasmtime recommends moving compilation
off the invocation path and preparing linking with `InstancePre`. Pool allocation
can reduce allocation cost but reserves capacity ahead of time; it needs separate
measurement before adoption. [Wasmtime instantiation guidance](https://docs.wasmtime.dev/examples-fast-instantiation.html).

These are macOS/aarch64 microbenchmarks with no pooling allocator or disk cache.
They do not include a generated parent workflow, parent-to-host-to-child payload
transport, persistence, admission queues, concurrent load, or cancellation
signalling. A complete production path will add those costs. No Linux latency,
throughput, or cancellation SLA is established. The existing proof's five-second
assertion is a generous correctness bound, not a measured latency target.

Raw data, sample counts, percentiles, native-code sizes and resident-Store probes:
[performance measurements](research/isolated-step-performance-measurements.json).

## Measurements: WASM and native-code size

The [packaging experiment](../scripts/research/isolated_step_package_sizes.py)
constructs and parses a WASM custom-section container with an artifact table and
call-site references. It verifies whole-artifact hashes and measures actual raw
and gzip sizes using the staged component bundle.

| Container | Deduplicated raw bytes | One copy per call, raw bytes |
|---|---:|---:|
| HTTP, 1 call site | 471,108 | 471,108 |
| HTTP, 10 call sites | 471,126 | 4,710,431 |
| HTTP, 100 call sites | 471,306 | 47,103,839 |
| All 29 staged components once | 23,999,025 | 23,999,025 |

The deduplicated 100-call HTTP container is about 460 KiB raw and 148 KiB gzip.
The naive container is about 44.9 MiB raw and 14.5 MiB gzip: transport compression
does not eliminate the need to deduplicate. The full staged bundle is about
22.9 MiB raw and 6.8 MiB gzip; a workflow should include only its dependencies.

These containers contain no workflow control code or executable loader. They
prove packaging size behavior, **not the size delta from today's composed DSL
artifact**. Whole-artifact deduplication also does not remove shared internals
from different child components. The production emitter must measure that delta
on representative graphs, particularly nested embedded workflows.

More simultaneous Stores do not require more copies of packaged WASM. Compiled
code can be shared, but has its own footprint: utils measured 498,513 bytes raw
WASM versus 1,773,752 bytes serialized native code; crypto 380,004 versus
1,402,800 bytes. These approximately 3.6–3.7× ratios are examples, not a general
bound or a process RSS measurement. Budget native caches separately.

[Packaging data and exact source artifact hashes](research/isolated-step-package-measurements.json).

## Memory, admission, and isolation limits

Each active invocation needs separate mutable guest state, host resources and
task bookkeeping. Sharing compiled code does not share globals or mutable memory;
an executable two-Store counter test confirms this distinction.

The current guest limiter is **per memory**, not an aggregate Store budget. A new
test sets the limit to 1 MiB, successfully creates three 1 MiB memories, and observes
that `memory_peak_bytes` reports 1 MiB. Consequently, summing that metric across
Stores can still undercount guest memory. It also excludes native allocations,
I/O buffers, code, stacks and retained results. Resident probes are recorded in
the raw data but cannot be extrapolated to worst-case memory from their small,
lightly touched workloads.

Production needs limits for aggregate guest memory, active children, nesting,
input/output bytes, retained results, compiled code and pending work, scoped to
root workflow and tenant as well as process. Reservations must be released after
cleanup, including traps and failed initialization. The PoC retains task entries
until parent cleanup; production needs bounded handle/result lifetime and an
explicit release contract.

A single semaphore shared by roots and children can deadlock: all roots can hold
permits while waiting to start children. A deterministic one-permit test exhibits
this failure mode; it is a scheduler counterexample, not a claim that production
currently uses that policy. Reserve child capacity or enforce bounded tree
budgets with explicit admission failure. Do not let nested spawns wait indefinitely
while holding all capacity needed to satisfy them. Guest WASM chooses what to do
with admission failure; the host enforces fairness and quotas.

Store isolation contains guest traps. It does not contain process OOM, native
panics/bugs, or arbitrary blocking native work. Audit every host operation for
owned cancellable futures versus detached tasks, blocking calls and subprocesses.
The HTTP proof covers stalled headers and bodies with the actual host-io path;
it does not establish cancellation of every host capability.

## Cancellation and durable execution contract

The PoC is insufficient as a production state machine. A new test requests
cancellation before a short ready invocation, and the current result-first select
still returns success. That is a documented counterexample, not the desired
contract. A cancellation flag alone does not decide durable races.

Proposed contract: identify each attempt by root execution, graph step, invocation
path/iteration, attempt generation and pinned artifact identity. Persist a
conditional transition that arbitrates completion versus cancellation. Once
cancellation wins, reject publication by that attempt; a previously committed
success remains success. Acknowledgement of a cancellation request and completion
of teardown must be distinct.

```text
created → running → completed
   │         │
   └─────────┴→ stopping → cancelled (only after execution resources are reaped)
```

The transition must cover cancellation before admission, during instantiation,
during a call and while publishing the result. Install interruption guards
**before instantiation**: the registry helper currently sets a distant epoch
deadline and instantiates; guarding only the later exported call misses an
infinite initializer. Use a typed outcome with success/error/cancelled/trapped
states, and scope handles and persistence permissions to the owning attempt.
Children must not have authority to complete their parent instance.

Existing `save_checkpoint(instance_id, checkpoint_id, state)` is an upsert, not a
conditional attempt transition. Both [memory persistence](../crates/runtara-core/src/persistence/memory.rs)
and [SQL checkpoint operations](../crates/runtara-store-postgres/src/ops_common/ops/checkpoints.rs)
allow replacement. Reuse durable infrastructure, but add a fenced attempt
operation rather than assuming checkpoint overwrite resolves competing writers.
The host's atomic persistence primitive implements consistency; WASM still owns
retry and recovery policy.

| Failure window | Required behavior in the proposed design |
|---|---|
| Cancel wins before start | Prevent invocation and external I/O |
| Cancel wins during execution | Stop and reap child; reject late result; parent can recover |
| Completion commits first | Return persisted success; late cancel is a no-op |
| Crash after cancel intent, before teardown acknowledgement | Recovery reconciles the attempt and prevents stale execution from publishing |
| Remote effect occurs before crash/result persistence | Outcome can be uncertain; use remote idempotency/reconciliation where available |
| Parent traps or is cancelled | Reap descendants and release handles; do not invent host-side recovery policy |

Stopping a request does not undo a remote side effect. Nor does Store destruction
run arbitrary guest cleanup/finally code. Required cleanup or compensation belongs
in surviving parent logic and must itself be durable. Persist logical state and
attempt outcomes, not an assumption that the live WASM stack survives a crash.

## Compilation and package trust

Prepare children before execution. Synchronous compilation is outside the guest
epoch mechanism; dropping an async wrapper cannot reliably stop native compilation.
The existing [precompile protocol](../crates/runtara-component-host/src/precompile.rs)
supports the separate worker approach and caps source components at 64 MiB and
serialized native output at 128 MiB. A package loader additionally needs limits
on total uncompressed dependencies, artifact count and nesting.

Cache by artifact digest plus engine/ABI/configuration compatibility. Reuse the
trusted precompile verification path; never deserialize caller-supplied native
code as if it were ordinary untrusted WASM. Keep package lookup immutable and
scoped to the workflow artifact so child selection requires no external workflow
registry. Version the new imports and retain compatibility for existing artifacts.

## Hypotheses and rollout gates

| Hypothesis | Research result |
|---|---|
| Parent WASM can recover after hard child cancellation | Confirmed by the ten live proof cases |
| Fresh Store requires recompilation | Falsified; actual agents instantiate from cached prepared definitions |
| Independent cancellation necessarily multiplies WASM by call count | Falsified for a deduplicated container; actual compiler delta remains unmeasured |
| Current memory limit bounds total Store memory | Falsified by a multi-memory execution test |
| Cancellation flag alone defines completion races | Falsified by the ready-call counterexample |
| A shared root/child permit pool is always safe | Falsified by the exhausted-pool counterexample |
| Production throughput and replay safety follow from live isolation | Not established; require integration work and load/crash tests |

Recommended implementation order:

1. Specify versioned generic execution imports, attempt outcomes and durable race
   semantics. Add tests for pre-start cancellation, concurrent result/cancel,
   duplicate commands, stale generations and every crash window above.
2. Implement artifact-table loading, bounded prepared-code caching and selective
   DSL lowering behind a feature flag. Test artifact tampering, missing imports,
   nesting limits, repeated dependencies, input/output parity and parent recovery.
3. Integrate aggregate reservations, descendant teardown, capability scoping and
   the user signal path. Test saturated admission, queue cancellation, parent
   failure, tenant isolation, detached I/O and result retention.
4. Benchmark generated workflows on the deployment Linux architecture: cold/warm
   latency, cancellation p50/p95/p99 under CPU and I/O saturation, 1/4/16/64 active
   children, memory pressure, cache eviction and representative large payloads.
   Compare actual package sizes against the existing emitter. Evaluate pooling
   separately; agree capacity/SLA thresholds from these measurements before rollout.

## Reproduction and verification

The four nonignored research tests run with the PoC feature and need no external
services. The manual benchmark requires the real staged release component bundle:

```sh
RUSTC_WRAPPER= cargo test -p runtara-component-host \
  --features isolated-step-poc --lib production_research
RUSTC_WRAPPER= cargo test --release -p runtara-component-host \
  --features isolated-step-poc --lib production_research_benchmark -- --ignored --nocapture
python3 scripts/research/isolated_step_package_sizes.py --self-test \
  --components target/wasm32-wasip2/release \
  --output docs/research/isolated-step-package-measurements.json
```

Benchmark implementation: [production_research.rs](../crates/runtara-component-host/src/isolated_step_poc/production_research.rs).
Performance output is emitted as `RESEARCH_JSON`; the checked-in report preserves
two local runs. Timing tests deliberately have no performance pass/fail threshold.

Follow-up verification completed locally:

- Component-host tests with `isolated-step-poc,component-integration-tests`:
  **59 passed**, including all ten live proof cases and four assumption checks;
  one manual benchmark ignored in the ordinary suite.
- Manual release benchmark: passed twice with real utils/crypto outputs verified.
- Component-host Clippy, all targets and both features, with `-D warnings`: passed.
- Packaging codec self-tests and `wasm-tools validate` on the 100-reference HTTP
  research container: passed. Validation does not make it an executable workflow.
- Workspace formatting, diff whitespace and local document links: passed.

The earlier full workflow run remains documented in the proof: 840 tests passed
and one existing doctest was ignored. It was not repeated for this follow-up's
test-only module, measurement script and documents. No production DSL/WIT code
changed. Linux load tests, database crash/replay tests, remote CI and production
rollout were not performed. Changes remain local and uncommitted.
