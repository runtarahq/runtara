# Usage history and OpenTelemetry

Usage remains available in the application without an observability service.
PostgreSQL retains its aggregates for 200 days, independently of the default
three-day retention of detailed instances. Grafana or another OTEL consumer can
build dashboards from the same execution measurements.

The write path is:

1. An instance reaches its first terminal state (`completed`, `failed`, or
   `cancelled`). A PostgreSQL trigger captures one compact fact in
   `usage_pending` in that transaction. It reads the changed row directly;
   there is no pre-completion status query or post-completion metrics query.
2. The environment Usage worker claims at most 1,000 pending facts with
   `FOR UPDATE SKIP LOCKED`. It groups them by tenant and UTC minute, adds them
   to `usage_minutes`, and removes the pending facts in the same transaction.
3. After commit, it records those facts into OTEL instruments when enabled.
   Usage API requests read only `usage_minutes`, never raw instance history.

The trigger covers Core lifecycle operations, launch failures, recovery, and
other direct SQL writers. Suspensions, checkpoints, signals and heartbeats do
not create completion facts. There are no per-instance monitoring tasks or
scans across components. Resource reports arriving before completion are
captured with it. Later memory and CPU observations create at most one
additional fact each; repeated reports do not count again. The first terminal
timestamp, outcome and reason remain the attribution for late resources.

The worker waits one second after a partial batch and immediately continues
after a full batch, yielding between transactions. Each statement has a
five-second server timeout, each pass a 15-second client timeout, and shutdown
can interrupt every pass. Expiry removes at most 10,000 old buckets per minute.
Backfill processes at most 1,000 pre-upgrade terminal rows per batch through a
partial index; once caught up, its retry runs once a minute.

This adds a compact transactional write for Usage and removes execution-count
dependent reads from completion and dashboard requests. A busy tenant stores
at most one aggregate row per occupied minute: 288,000 rows over 200 days,
regardless of invocation volume. The pending queue is durable, not a lossy
telemetry buffer: if aggregation stops, its disk usage grows until processing
resumes. Batch sizes bound memory and transaction work, not outage backlog.
Different tenants do not share aggregate row locks; workers updating the same
tenant/minute serialize their batched contributions.

## Measurements

| Measurement | In-app aggregate | OTEL instrument |
| --- | --- | --- |
| Terminal invocations and outcomes | Total, success, failure, cancellation counts | `runtara.workflow.invocations.total`, Counter |
| Wall-clock execution duration | Observation count, sum, minimum, maximum | `runtara.workflow.execution.duration`, Histogram, seconds |
| Peak memory per invocation | Observation count, sum, maximum | `runtara.workflow.memory.peak`, Histogram, bytes |
| CPU time per invocation | Observation count, sum, maximum | `runtara.workflow.cpu.usage`, Histogram, seconds |

Duration is first start to first terminal completion, including suspension
time, preserving the meaning of the Usage view. Missing or invalid measurements
are absent observations, not zero. Averages and chart combinations use the
measurement's own observation count. CPU statistics are included in the API;
the existing Usage cards continue displaying counts, duration and memory.

OTEL attributes are `tenant_id`, `status`, and `termination_reason` (`none`
when absent). Tenant identity is intentionally present for per-tenant Usage
dashboards. There are no instance, workflow or image IDs. The existing SDK and
exporter configuration applies; no separate OTEL exporter is introduced.

For a Prometheus-compatible backend, sum the invocation counter by status for
outcome counts or rates, and divide histogram sums by histogram counts for
averages. Backend metric-name and unit translation depends on the collector.
Histograms describe observations; they do not measure live memory occupancy.

## API and retention behavior

`GET /api/runtime/metrics/tenant` retains its existing response fields and adds
`duration_observation_count`, `memory_observation_count`,
`cpu_observation_count`, `avg_cpu_seconds`, and `max_cpu_seconds` to each bucket.
The runtime client is generated from this contract.

History has one-minute resolution. Request start and exclusive end times are
rounded down to UTC minutes; the response reports those effective bounds.
Widths must be positive multiples of 60 seconds. `hourly`, `daily`, `1m`, `6m`,
`24m`, `2h` and other whole-minute widths work; sub-minute or fractional-minute
widths return 400. At most 1,000 output buckets are allowed. The default range
ends at the beginning of the current minute. The UI advances its bounds on
every refresh, so ordinary display latency is up to a minute plus batching and
the UI's refresh interval. Backlogs can increase that latency.

The 200-day retention covers both the 90-day view and its preceding 90-day
comparison. Detailed instance cleanup cannot cascade into Usage: pending facts
and aggregates have no instance FK. Deleting a pre-upgrade terminal row also
captures any contribution that backfill has not reached. Expired contributions
are discarded during aggregation so a late resource report cannot recreate an
expired bucket.

Backfill uses whatever instance history still exists. It cannot reconstruct
already-deleted executions, and it does not export historical completions as
current OTEL traffic. History becomes available progressively during rollout.
The migration adds columns, indexes and triggers, without rewriting all old
instances in a migration transaction; index creation still needs a deployment
window appropriate to the existing table size.

## Reliability and disabling telemetry

PostgreSQL Usage is durable and idempotent: capture rolls back with the lifecycle
write, and aggregation rolls back with deletion of the pending facts. Concurrent
workers, retries and late reports cannot duplicate a contribution. Capture
failure fails the lifecycle transaction rather than silently losing Usage data.

OTEL is best effort. It records after the Usage transaction commits; a process
crash in that gap or an exporter failure can lose telemetry without losing
in-app history. OTEL timestamps reflect processing/export time, while retained
Usage is attributed to completion time. During backlog recovery the time-series
shapes therefore differ. OTEL is not a billing ledger or a historical backfill
protocol.

`OTEL_SDK_DISABLED=true` creates no Usage instruments and skips returning
telemetry facts from SQL, constructing attributes, recording and export. The
durable Usage worker still aggregates counts for the application. Disabling
telemetry does not erase or disable product history.

Run the environment's `db-integration-tests` for capture, retry, concurrency,
late observations, retention and backfill. `e2e/test_pipeline_analytics.sh`
also checks live Usage/OTLP parity, raw-instance cleanup, exporter failure and
disabled telemetry with a real server and local OTLP receiver.
