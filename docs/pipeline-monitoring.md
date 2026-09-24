# Pipeline monitoring through OpenTelemetry

Runtara exports execution pipeline signals through its existing OTLP provider.
Your observability stack owns collection, retention, rates, windows, percentiles,
aggregation, dashboards, and alerts. There is no pipeline sampler, pipeline SQL
aggregation, Valkey backlog query, snapshot cache, or browser stream.

The System page retains host information. `/api/runtime/analytics/pipeline` and
`/api/runtime/analytics/pipeline/stream` have been removed from the API and OpenAPI.
Historical [Usage](usage-history.md) has its own retained PostgreSQL aggregates
and exports completion measurements through OTEL. Operational admission checks,
durable execution state, and product events remain authoritative. OTEL is not
a source of truth for admission, billing, or durable queue depth.

## Data path and cost

- Admission and persisted launch transitions update counters and histograms at
  their existing result boundary. No telemetry readback is issued.
- Persisted tracked step-start events increment a counter. Workflows compiled
  without event tracking do not emit this signal; zero steps does not prove a stall.
- Runner and trigger owners register callbacks over weak references to their
  operational semaphores. Collection reads local semaphore/atomic values; it
  performs no SQL, filesystem, network, task-registry, or instance scan.
- Trigger collection sums the configured worker semaphores once per collection,
  O(number of trigger workers). Other pool observations are O(1). There is no
  per-execution sampling loop or growing telemetry map.
- A held permit owns an optional timer. Release records a histogram, including
  unwind/cancellation. A precompile child retains its permit and timer until it
  is reaped after a timeout. The operational task registry and reaper count remain.
- OTEL performs local aggregation and periodic batch export. Metric labels are
  bounded; pipeline metrics have no tenant, workflow, instance, launch, image,
  request, owner, or raw-error labels. Normal SDK record/collection/export costs
  remain, as do operational database work and separately configured trace/log costs.

A held-duration histogram describes released permits. It cannot reveal the age
of a permit still held. Use occupancy, throughput, timeouts, and traces together;
this replaces the old oldest-holder and workflow-attribution views. It does not
recreate their exact information in metric labels.

## Instrument contract

All metric names below are OTLP names. Backends may translate dots to underscores
and add unit or `_total` suffixes. Counts are process-local and reset on restart.

| Instrument | Type / unit | Attributes and meaning |
| --- | --- | --- |
| `runtara.admission.requests` | Counter | `outcome`: `accepted`, `rejected` (entitlement denial), `duplicate`, `error`. One result per completed source-admission call, including early idempotency returns. Cancelled calls that never return are not counted. |
| `runtara.admission.duration` | Histogram / seconds | Same `outcome`; elapsed source-admission call time. |
| `runtara.trigger.events.total` | Counter | `trigger_type` from the finite trigger-source vocabulary. Counts processing attempts; redelivery can count again. |
| `runtara.trigger.events.failed` | Counter | Same `trigger_type`; failed attempts and exhausted retry policy retain existing semantics. |
| `runtara.trigger.processing.duration` | Histogram / seconds | `trigger_type`, `status`: `success`, `deduplicated`, `permanent_failure`, `not_runnable`, `retry_later`, `handoff_in_progress`. |
| `runtara.launch.transitions` | Counter | `state`: stored launch state; `reason`: fixed operation vocabulary below. Only applied, committed changes count. |
| `runtara.launch.queue.duration` | Histogram / seconds | `outcome`: `started`, `failed`, `cancelled`, `completed`, `expired`. Creation to confirmed start-gate handoff, or a terminal transition while still waiting. Requeues do not reset creation time. |
| `runtara.pipeline.pool.usage` | Observable UpDownCounter / permits | `pool`: `trigger`, `preparation`, `precompile`, `run`. Actual held permits on this process. |
| `runtara.pipeline.pool.capacity` | Observable UpDownCounter / permits | Same `pool`; configured capacity of live local owners. Trigger workers export their sum. |
| `runtara.pipeline.pool.hold.duration` | Histogram / seconds | Same `pool`; permit lifetime, including cleanup and unsuccessful attempts. |
| `runtara.runner.runs` | Counter | `event`: `started`, `stopped`. Physical run-slot acquisitions/releases, including errors before guest invocation and parked workflows; not logical workflow completions. |
| `runtara.precompile.children.reaping` | Observable UpDownCounter | Child processes awaiting the detached reaper; no attributes. A subset of precompile pool usage. |
| `runtara.workflow.steps.started` | Counter | Persisted tracked step starts; no attributes. |

Launch reasons: `enqueue`, `claim`, `prepare`, `prepared`, `start`,
`runner_handoff`, `preparation_timeout`, `lease_expired`, `run_capacity`,
`preparation_capacity`, `retry`, `gate_failed`, `preparation_failed`, `park`,
`terminal`, `queue_timeout`, `cancel`, `reconcile`. Unknown retry diagnostics map
to `retry`. Start-gate confirmation records queue duration, not a second running
state transition. Lease renewals and idempotent replays record no transition.

New pipeline duration buckets, in seconds:
`0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1, 5, 10, 30, 60, 300, 3600, +Inf`.
Existing trigger-duration bucket configuration is unchanged.

## Deployment and interpretation

Configure the existing gRPC OTLP endpoint with `OTEL_EXPORTER_OTLP_ENDPOINT`.
The provider is initialized before runtime owners. `OTEL_METRIC_EXPORT_INTERVAL`
is honored in milliseconds (SDK default: 60000). A shorter interval trades
resolution for collection and export work. No dashboard needs to be open.
`service.instance.id` is a fresh UUID per process lifetime; keep it through
collection so separate processes and restarts do not overwrite one another.

Set `OTEL_SDK_DISABLED=true` before startup to omit pipeline observer registration,
step observer installation, instruments, attribute construction, and permit and
admission timers. Operational semaphores and reaping continue to work. This is a
startup setting, not a live toggle. Other application telemetry is outside the
pipeline contract described here.

Shutdown attempts to flush retained providers and limits the application's wait
to five seconds. Export failure does not block admission or runner paths. Data is
best effort: process crashes, export failures, and a lost database commit response
can lose measurements. There is no telemetry outbox or reconciliation scan.
The durable operational tables remain authoritative.

Sum pool usage and capacity across live process identities to calculate fleet
utilization. Expire stale instances in the backend. Do not sum transition counters
to reconstruct durable queue depth: restarts, recovery, and lost exports make that
incorrect. Use rate/increase functions that handle resets before summing counters.

For a Prometheus-compatible backend using conventional OTLP name translation,
examples are (verify the translated names in your collector):

```promql
# Admission throughput, including replay and errors
sum by (outcome) (rate(runtara_admission_requests_total[5m]))

# Occupied share of live run capacity
sum(runtara_pipeline_pool_usage{pool="run"})
/
sum(runtara_pipeline_pool_capacity{pool="run"})

# Queue residence p95 for launches that reached the start gate
histogram_quantile(0.95,
  sum by (le) (rate(runtara_launch_queue_duration_seconds_bucket{outcome="started"}[5m]))
)

# Capacity retry pressure
sum by (reason) (rate(runtara_launch_transitions_total{reason=~"run_capacity|preparation_capacity"}[5m]))
```

Alert thresholds and windows belong in the observability stack. A full run pool
with healthy release throughput indicates load; a full pool with falling release
throughput and timeouts warrants investigation. Queue-duration histograms measure
finished waits, so they alone cannot detect work that has never left the queue.

## Local verification

Unit coverage uses the OTEL in-memory exporter. Database coverage asserts that
rolled-back transitions and duplicate cancellation emit nothing, queue duration
closes once, and collecting after closing the DB pool still works. Existing launch
queue and embedded-runner suites cover leases, cancellation, and capacity fencing.

Build the binaries and components:

```sh
cargo build -p runtara-server --bin runtara-server --example otel_test_receiver
scripts/build-agent-components.sh
```

Then run `e2e/test_pipeline_analytics.sh` against **dedicated local test** PostgreSQL
(with pgvector available) and Valkey. Set `PGHOST`, `PGPORT`, `PGUSER`, `PSQL` if
needed, and `TEST_VALKEY_PORT` (default 16399). The default database connection uses
local trust authentication. The harness creates uniquely named server/runtime
databases and retains them and logs for inspection; it never drops a database.
`TEST_PORT_PUBLIC` and `TEST_OTLP_PORT` can override the local listener ports.

The harness starts its own server and real gRPC receiver, executes tracked and
untracked workflows, checks bounded labels and summed worker capacity, verifies
the old routes are gone and host analytics still works, stops the exporter while
executing more work, checks bounded shutdown, and restarts with telemetry disabled.
The receiver is test-only and discards traces and logs. The previous
`scripts/pipeline-playground.sh` sampler/UI harness has been removed.
