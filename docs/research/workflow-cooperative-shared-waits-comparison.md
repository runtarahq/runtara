# Shared-wait cooperative cancellation performance comparison

Measured 2026-09-07 UTC. Baseline: `4eff9cdf83342ad45ef89f73181934c5485085a0`. Candidate: `e8785f37637799bcee04a7e6face54af4fc200f4`.

This compares the normal execution path of two complete source revisions, including compiler, guest and runtime changes, with no cancellation requested. The candidate has 15 of 27 Agent bindings migrated. These results do not complete the cancellation or release gates.

[Raw samples and configuration manifest](workflow-cooperative-shared-waits-comparison.json) accompany every number below. The first cooperative, historical baseline and isolation reports remain unchanged.

## Findings and remaining work

This is an exploratory timing run, not a performance acceptance result. The
recorded one-minute host load ranged from 29.05 to 116.42 on 16 logical CPUs;
load averages do not measure utilization directly, but this large and changing
background load makes small timing deltas unsuitable for attributing overhead
to a particular change.
Repeat on a controlled machine before setting performance budgets. Neither this
cohort nor the earlier desktop cohort establishes Linux deployment performance.

The deterministic size comparison is useful. The previous cooperative revision
is `103c32cee4fccc1f52ac960c21a86294e8c48d77`, recorded in the
[first cooperative report](workflow-cooperative-cancellation-comparison.md).
The benchmark graphs, inputs and component dependencies are unchanged by the
helper factoring. These values describe complete composed artifacts:

| Workflow | Upstream bytes | Inline cooperative bytes | Shared cooperative bytes | Shared minus inline |
|---|---:|---:|---:|---:|
| One explicit non-durable random Agent | 3,306,036 | 3,310,916 | 3,314,935 | +4,019 |
| Ten-Agent chain | 3,324,820 | 3,354,884 | 3,338,516 | −16,368 |
| Hundred-Agent chain | 3,515,665 | 3,797,547 | 3,577,330 | −220,217 |
| Hundred-item parallel4 Split | 3,318,585 | 3,326,215 | 3,326,813 | +598 |

The hundred-Agent artifact is now 1.75% above upstream raw size, compared with
8.0% before factoring. Its generated logic shrinks from 550,310 to 330,093 bytes;
gzip shrinks from 864,572 to 862,312 bytes, and serialized native code from
14,066,424 to 13,296,808 bytes. The corresponding upstream inventories are
268,545 logic bytes, 860,232 gzip bytes and 12,964,520 native bytes. Fixed helper
bodies increase small artifacts: the single-Agent workflow is now 0.27% above
upstream raw size. Workflows without Agents retain the same bytes as the prior
cooperative revision.

Observed hundred-Agent prepared median changes versus upstream are +9.6%, +3.1%
and −0.3%; compilation through first result is 2.11×, 1.49× and 2.15× upstream.
Single-Agent prepared medians change by +11.0%, +12.3% and +7.5%. These are within-
pair observations under uncontrolled load, not an isolated estimate of helper
call overhead. Do not compare absolute times across the two historical cohorts
to claim a causal speedup. Native compilation remains the largest measured cold
phase and needs further investigation under controlled load.

Remaining optimization candidates are emitting only the helper bodies a graph
uses and reducing state passed to each helper. Any such change must retain the
cleanup-before-acknowledgement, deferred Pause/Shutdown, entry ABI and replay
coverage. Full Agent migration, nested workflow cancellation, public Stop/grace,
cooperative deadlines and the other qualification gates remain open.

## Measurement boundaries

- Host: Apple M3 Max, 16 logical CPUs, 64 GiB RAM; macOS-26.5.1-arm64-arm-64bit-Mach-O.
- Each revision uses its own worktree, normal component build and Cargo target directory. The identical test module is overlaid into the baseline; its production sources and Cargo configuration are unchanged.
- Three independent process pairs run in baseline/candidate, candidate/baseline, baseline/candidate order. Tables list pair 1 / pair 2 / pair 3; percentiles are never averaged across processes.
- Prepared execution: five warmups, then 1,000 samples per condition. Timing includes input cloning, a fresh guest Store, instantiation, graph execution, runtime callbacks and Store teardown. Fixture-host construction and result validation are outside the timer.
- Cold compilation: three samples per condition, with a shared engine inside each process and no Wasmtime disk cache. These are fresh artifacts/preparations, not an OS page-cache or machine cold boot. Engine setup is recorded separately.
- The revision-local in-memory runtime fixture captures events and checkpoints. Its data structures are identical; the candidate adds the required read-only poll_signal method returning None. The full method diff is in the manifest.
- Complete distributable .wasm sizes include composition. Logic and individual dependency sizes are separate inventories and must not be added to the complete artifact size. Gzip uses identical -n -c arguments (Apple gzip 479, captured in the driver log).
- Native sizes describe the serialized component. The raw largest_guest_memory_bytes field is the largest individual guest linear-memory high-water mark; it is not summed memory or process RSS.
- Measurements were collected on a desktop host with uncontrolled external load, higher than in the first cooperative report. The manifest records load before and after each run. Linux capacity and resource-soak qualification remain pending.

## Complete artifact size

| Workflow | Baseline .wasm bytes | Candidate .wasm bytes | Baseline gzip bytes | Candidate gzip bytes |
|---|---|---|---|---|
| random_1_defaults | 3,308,087 | 3,317,667 | 848,291 | 849,735 |
| random_1 | 3,306,036 | 3,314,935 | 847,880 | 849,362 |
| random_1_durable | 3,306,440 | 3,315,417 | 847,977 | 849,406 |
| random_1_events | 3,308,293 | 3,317,192 | 848,020 | 849,487 |
| random_chain_10 | 3,324,820 | 3,338,516 | 849,098 | 851,455 |
| random_chain_100 | 3,515,665 | 3,577,330 | 860,232 | 862,312 |
| random_split_100_sequential | 3,310,720 | 3,319,619 | 848,714 | 850,145 |
| random_split_100_parallel4 | 3,318,585 | 3,326,813 | 850,179 | 851,049 |
| random_embed_1 | 3,309,492 | 3,318,916 | 848,457 | 849,925 |
| finish_only | 2,803,809 | 2,804,173 | 691,029 | 691,120 |
| finish_payload_16k_minus1 | 2,803,851 | 2,804,215 | 691,047 | 691,141 |
| finish_payload_16k | 2,803,830 | 2,804,194 | 691,036 | 691,132 |
| finish_payload_16k_plus1 | 2,803,848 | 2,804,212 | 691,049 | 691,143 |
| finish_payload_1mib | 2,803,833 | 2,804,197 | 691,037 | 691,130 |

## Prepared full execution

All values are microseconds. Each cell lists the medians for the three separate pairs. Changes compare medians within the same pair.

| Workflow | Baseline p50 µs, pairs 1/2/3 | Candidate p50 µs, pairs 1/2/3 | Change per pair |
|---|---|---|---|
| random_1_defaults | 165.21 / 168.21 / 166.08 | 166.62 / 169.71 / 183.75 | +0.9% / +0.9% / +10.6% |
| random_1 | 159.46 / 148.21 / 156.46 | 176.96 / 166.50 / 168.12 | +11.0% / +12.3% / +7.5% |
| random_1_durable | 157.62 / 156.96 / 161.46 | 184.67 / 177.58 / 188.08 | +17.2% / +13.1% / +16.5% |
| random_1_events | 189.71 / 212.00 / 184.54 | 213.50 / 230.79 / 205.42 | +12.5% / +8.9% / +11.3% |
| random_chain_10 | 508.62 / 507.67 / 497.33 | 591.71 / 539.67 / 542.67 | +16.3% / +6.3% / +9.1% |
| random_chain_100 | 22,704.33 / 20,471.25 / 20,901.00 | 24,875.00 / 21,102.25 / 20,839.38 | +9.6% / +3.1% / -0.3% |
| random_split_100_sequential | 10,796.04 / 10,626.79 / 10,739.96 | 10,943.75 / 11,039.75 / 10,800.54 | +1.4% / +3.9% / +0.6% |
| random_split_100_parallel4 | 13,185.62 / 17,304.46 / 14,967.62 | 15,741.42 / 12,701.42 / 12,661.67 | +19.4% / -26.6% / -15.4% |
| random_embed_1 | 209.50 / 213.96 / 227.42 | 269.50 / 214.00 / 215.00 | +28.6% / +0.0% / -5.5% |
| finish_only | 85.88 / 98.25 / 96.54 | 102.21 / 88.12 / 89.83 | +19.0% / -10.3% / -6.9% |
| finish_payload_16k_minus1 | 165.96 / 177.08 / 174.46 | 175.17 / 164.50 / 164.92 | +5.5% / -7.1% / -5.5% |
| finish_payload_16k | 157.50 / 168.08 / 175.46 | 182.58 / 162.42 / 167.75 | +15.9% / -3.4% / -4.4% |
| finish_payload_16k_plus1 | 164.38 / 165.12 / 174.17 | 174.79 / 168.46 / 167.96 | +6.3% / +2.0% / -3.6% |
| finish_payload_1mib | 4,398.50 / 4,755.46 / 5,060.29 | 4,866.04 / 4,670.96 / 4,760.42 | +10.6% / -1.8% / -5.9% |

## Single random-double Agent service

The service timer starts after a fresh standalone Agent has been instantiated and its export resolved. It includes the typed invocation through return, including guest first-call work, and excludes host teardown. This is a separate boundary from a composed parent step; it should not be multiplied to predict chain execution.

| Metric | Baseline p50 µs, pairs 1/2/3 | Candidate p50 µs, pairs 1/2/3 |
|---|---|---|
| Agent invocation | 8.92 / 9.00 / 9.46 | 9.50 / 9.38 / 9.42 |

## Durable replay

Every replay must reproduce the previous random result byte-for-byte. These measurements reuse stored checkpoints in a fresh workflow Store.

| Workflow | Baseline p50 µs, pairs 1/2/3 | Candidate p50 µs, pairs 1/2/3 |
|---|---|---|
| random_1_defaults | 150.79 / 153.25 / 152.08 | 144.25 / 146.29 / 159.54 |
| random_1_durable | 145.96 / 144.29 / 148.50 | 159.21 / 154.04 / 161.38 |

## Compilation and first result

Three samples per process support median/minimum/maximum reporting, with no cold tail percentiles. The JSON retains all phase samples. The first-result timer covers JSON parsing/emission, composition, native compilation, linking and first execution; full public DSL validation is not measured yet.

| Workflow | Baseline first-result p50 µs, pairs 1/2/3 | Candidate first-result p50 µs, pairs 1/2/3 | Baseline native bytes | Candidate native bytes |
|---|---|---|---|---|
| random_1_defaults | 740,695.50 / 319,203.88 / 406,694.46 | 254,277.88 / 296,433.96 / 485,503.25 | 12,456,616 | 12,510,376 |
| random_1 | 417,178.62 / 273,276.62 / 411,416.79 | 405,192.92 / 357,355.38 / 503,747.83 | 12,456,616 | 12,510,376 |
| random_1_durable | 307,949.25 / 241,901.46 / 337,010.46 | 424,250.33 / 340,525.21 / 566,219.00 | 12,456,616 | 12,510,376 |
| random_1_events | 261,841.54 / 455,154.96 / 570,544.83 | 526,490.58 / 401,572.29 / 429,893.88 | 12,456,616 | 12,510,376 |
| random_chain_10 | 333,793.50 / 318,968.42 / 273,372.04 | 590,014.67 / 391,833.04 / 357,194.58 | 12,489,384 | 12,575,912 |
| random_chain_100 | 378,645.83 / 321,229.67 / 336,335.83 | 797,583.21 / 478,970.25 / 723,703.54 | 12,964,520 | 13,296,808 |
| random_split_100_sequential | 315,910.33 / 239,101.08 / 328,463.58 | 456,689.58 / 330,515.42 / 273,824.33 | 12,456,616 | 12,510,376 |
| random_split_100_parallel4 | 310,544.83 / 622,594.38 / 286,632.33 | 497,806.50 / 340,115.67 / 258,354.75 | 12,615,528 | 12,649,368 |
| random_embed_1 | 455,408.33 / 374,534.83 / 461,251.42 | 507,558.38 / 247,285.75 / 310,340.75 | 12,456,616 | 12,510,376 |
| finish_only | 277,234.67 / 298,686.79 / 422,258.88 | 389,897.58 / 202,390.46 / 216,191.58 | 10,703,544 | 10,703,992 |
| finish_payload_16k_minus1 | 222,241.50 / 305,728.08 / 520,669.62 | 416,295.96 / 207,119.92 / 203,127.62 | 10,703,544 | 10,703,992 |
| finish_payload_16k | 210,192.54 / 460,556.17 / 362,192.67 | 524,584.58 / 215,005.54 / 215,474.62 | 10,703,544 | 10,703,992 |
| finish_payload_16k_plus1 | 213,961.83 / 254,953.58 / 323,868.25 | 338,005.62 / 225,232.38 / 224,144.17 | 10,703,544 | 10,703,992 |
| finish_payload_1mib | 204,735.71 / 244,844.42 / 400,891.75 | 409,833.83 / 234,284.08 / 248,017.25 | 10,703,544 | 10,703,992 |

## Verification and limits

- Every measured workflow completed with validated output shape, count and random-value range. Defaults and durable variants created checkpoints; non-durable variants did not. The harness fails immediately on an execution error, and the driver preserves failure outcomes separately.
- Preserved .wasm, graph and input artifacts match the report hashes. Sources and all relevant component/metadata inputs were checked before and after each run. Agent workflows in the candidate contain standard cancellation and wait-set operations, and no artifact imports the superseded task service.
- The parallel4 workload composes four Agent instances. Random calls complete immediately; its timings do not establish HTTP overlap or cancellation latency. Existing functional overlap proofs are separate from this measurement.
- Raw samples include per-process p95/p99 for the 1,000-sample prepared conditions. Ten observations populate the highest one percent; these desktop tails do not establish deployment guarantees.
- Still required: parent-step phase spans and instrumentation cost, full validation timing, pending HTTP headers/body measurements, signal-poll/DB-query cost, public-server submission-to-terminal timing, cancellation/abort latency, remaining Agent/nested-workflow integration, Linux capacity, soak tests and explicit deployment budgets.
- Re-run this comparison after the remaining implementation and superseded-path cleanup. No release performance claim is made from this interim candidate.
