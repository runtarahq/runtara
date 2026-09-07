# Interim cooperative cancellation performance comparison

Measured 2026-09-07 UTC. Baseline: `4eff9cdf83342ad45ef89f73181934c5485085a0`. Candidate: `103c32cee4fccc1f52ac960c21a86294e8c48d77`.

This compares the normal execution path of two complete source revisions, including compiler, guest and runtime changes, with no cancellation requested. The candidate has 15 of 27 Agent bindings migrated. These results do not complete the cancellation or release gates.

[Raw samples and configuration manifest](workflow-cooperative-cancellation-comparison.json) accompany every number below. Historical baseline and isolation reports remain unchanged.

## Findings to act on

- The explicit non-durable single random-double workflow grows from 3,306,036 to 3,310,916 bytes (+0.15%). Its prepared full-run medians increase by 4.5–11.2% across the three pairs. The defaults-preserving variant grows by 0.24%, with median changes of +2.3% to +18.0%.
- The 100-step chain grows by 281,882 raw bytes (+8.0%), 4,340 gzip bytes (+0.50%) and 1,101,904 serialized native bytes (+8.5%). Its first-result medians are 2.26–3.64 times baseline, while prepared execution medians increase by 1.5–4.1%.
- Native compilation accounts for most of that chain's cold increase: per-process medians are 196.5–205.6 ms in the baseline and 512.9–881.1 ms in the candidate. Emission and composition also increase; full phase samples are retained in JSON.
- The utils Agent and JSON stdlib binaries are byte-identical across revisions. The generated 100-step workflow logic grows from 268,545 to 550,310 bytes. Inspection shows that `compile/cooperative_wait.rs::emit_await_call` expands its wait/poll/cleanup code at every `agent_invoke.rs` call site. Factoring repeated code into shared emitted helpers is a concrete next investigation; caller-local state, early returns and cleanup-before-acknowledgement must remain intact.
- Small-run timing is variable: even Finish-only medians change by -1.7% to +13.8% across pairs. The ranges above describe observed per-process medians, not confidence intervals or isolated costs of a single code change. The native-compilation regression warrants investigation before performance qualification.

## Measurement boundaries

- Host: Apple M3 Max, 16 logical CPUs, 64 GiB RAM; macOS-26.5.1-arm64-arm-64bit-Mach-O.
- Each revision uses its own worktree, normal component build and Cargo target directory. The identical test module is overlaid into the baseline; its production sources and Cargo configuration are unchanged.
- Three independent process pairs run in baseline/candidate, candidate/baseline, baseline/candidate order. Tables list pair 1 / pair 2 / pair 3; percentiles are never averaged across processes.
- Prepared execution: five warmups, then 1,000 samples per condition. Timing includes input cloning, a fresh guest Store, instantiation, graph execution, runtime callbacks and Store teardown. Fixture-host construction and result validation are outside the timer.
- Cold compilation: three samples per condition, with a shared engine inside each process and no Wasmtime disk cache. These are fresh artifacts/preparations, not an OS page-cache or machine cold boot. Engine setup is recorded separately.
- The revision-local in-memory runtime fixture captures events and checkpoints. Its data structures are identical; the candidate adds the required read-only poll_signal method returning None. The full method diff is in the manifest.
- Complete distributable .wasm sizes include composition. Logic and individual dependency sizes are separate inventories and must not be added to the complete artifact size. Gzip uses identical -n -c arguments (Apple gzip 479, recorded in the manifest).
- Native sizes describe the serialized component. The raw `largest_guest_memory_bytes` field is the largest individual guest linear-memory high-water mark; it is not summed memory or process RSS.
- Measurements were collected on a desktop host with uncontrolled external load. The manifest records load before and after each run. Linux capacity and resource-soak qualification remain pending.

## Complete artifact size

| Workflow | Baseline .wasm bytes | Candidate .wasm bytes | Baseline gzip bytes | Candidate gzip bytes |
|---|---|---|---|---|
| random_1_defaults | 3,308,087 | 3,315,940 | 848,291 | 849,458 |
| random_1 | 3,306,036 | 3,310,916 | 847,880 | 848,980 |
| random_1_durable | 3,306,440 | 3,311,916 | 847,977 | 849,072 |
| random_1_events | 3,308,293 | 3,313,174 | 848,020 | 849,125 |
| random_chain_10 | 3,324,820 | 3,354,884 | 849,098 | 851,487 |
| random_chain_100 | 3,515,665 | 3,797,547 | 860,232 | 864,572 |
| random_split_100_sequential | 3,310,720 | 3,315,601 | 848,714 | 849,796 |
| random_split_100_parallel4 | 3,318,585 | 3,326,215 | 850,179 | 850,823 |
| random_embed_1 | 3,309,492 | 3,316,155 | 848,457 | 849,603 |
| finish_only | 2,803,809 | 2,804,173 | 691,029 | 691,120 |
| finish_payload_16k_minus1 | 2,803,851 | 2,804,215 | 691,047 | 691,141 |
| finish_payload_16k | 2,803,830 | 2,804,194 | 691,036 | 691,132 |
| finish_payload_16k_plus1 | 2,803,848 | 2,804,212 | 691,049 | 691,143 |
| finish_payload_1mib | 2,803,833 | 2,804,197 | 691,037 | 691,130 |

## Prepared full execution

All values are microseconds. Each cell lists the medians for the three separate pairs. Changes compare medians within the same pair.

| Workflow | Baseline p50 µs, pairs 1/2/3 | Candidate p50 µs, pairs 1/2/3 | Change per pair |
|---|---|---|---|
| random_1_defaults | 146.71 / 146.21 / 149.12 | 173.17 / 157.83 / 152.50 | +18.0% / +8.0% / +2.3% |
| random_1 | 146.54 / 141.25 / 135.50 | 162.92 / 153.33 / 141.62 | +11.2% / +8.6% / +4.5% |
| random_1_durable | 140.42 / 151.38 / 150.62 | 171.08 / 161.58 / 149.83 | +21.8% / +6.7% / -0.5% |
| random_1_events | 170.46 / 172.62 / 162.83 | 196.62 / 191.92 / 178.62 | +15.4% / +11.2% / +9.7% |
| random_chain_10 | 468.79 / 476.50 / 467.29 | 574.83 / 500.83 / 491.67 | +22.6% / +5.1% / +5.2% |
| random_chain_100 | 20,016.58 / 20,034.88 / 19,740.04 | 20,832.08 / 20,330.42 / 20,099.08 | +4.1% / +1.5% / +1.8% |
| random_split_100_sequential | 10,352.96 / 10,155.29 / 10,017.92 | 10,822.96 / 10,628.88 / 10,268.33 | +4.5% / +4.7% / +2.5% |
| random_split_100_parallel4 | 12,587.08 / 12,368.08 / 12,242.04 | 12,477.00 / 12,493.17 / 12,636.58 | -0.9% / +1.0% / +3.2% |
| random_embed_1 | 201.67 / 190.33 / 195.92 | 200.50 / 212.92 / 203.62 | -0.6% / +11.9% / +3.9% |
| finish_only | 90.62 / 81.92 / 82.83 | 89.12 / 93.21 / 89.88 | -1.7% / +13.8% / +8.5% |
| finish_payload_16k_minus1 | 163.46 / 163.92 / 160.12 | 153.46 / 165.54 / 164.79 | -6.1% / +1.0% / +2.9% |
| finish_payload_16k | 166.62 / 160.88 / 156.17 | 161.79 / 160.38 / 168.54 | -2.9% / -0.3% / +7.9% |
| finish_payload_16k_plus1 | 166.00 / 161.88 / 161.00 | 163.33 / 166.96 / 160.83 | -1.6% / +3.1% / -0.1% |
| finish_payload_1mib | 4,887.50 / 4,319.08 / 4,341.88 | 4,368.62 / 4,394.62 / 4,446.62 | -10.6% / +1.7% / +2.4% |

## Single random-double Agent service

The service timer starts after a fresh standalone Agent has been instantiated and its export resolved. It includes the typed invocation through return, including guest first-call work, and excludes host teardown. This is a separate boundary from a composed parent step; it should not be multiplied to predict chain execution.

| Metric | Baseline p50 µs, pairs 1/2/3 | Candidate p50 µs, pairs 1/2/3 |
|---|---|---|
| Agent invocation | 10.08 / 8.50 / 8.25 | 8.54 / 9.50 / 8.29 |

## Durable replay

Every replay must reproduce the previous random result byte-for-byte. These measurements reuse stored checkpoints in a fresh workflow Store.

| Workflow | Baseline p50 µs, pairs 1/2/3 | Candidate p50 µs, pairs 1/2/3 |
|---|---|---|
| random_1_defaults | 133.50 / 133.67 / 136.67 | 148.67 / 136.92 / 132.67 |
| random_1_durable | 129.71 / 140.12 / 138.12 | 148.92 / 140.79 / 131.46 |

## Compilation and first result

Three samples per process support median/minimum/maximum reporting, with no cold tail percentiles. The JSON retains all phase samples. The first-result timer covers JSON parsing/emission, composition, native compilation, linking and first execution; full public DSL validation is not measured yet.

| Workflow | Baseline first-result p50 µs, pairs 1/2/3 | Candidate first-result p50 µs, pairs 1/2/3 | Baseline native bytes | Candidate native bytes |
|---|---|---|---|---|
| random_1_defaults | 239,314.58 / 243,868.21 / 228,266.50 | 466,190.42 / 227,148.00 / 225,342.67 | 12,456,616 | 12,509,944 |
| random_1 | 235,983.46 / 224,446.42 / 231,048.33 | 419,821.58 / 225,189.17 / 234,191.00 | 12,456,616 | 12,477,176 |
| random_1_durable | 222,423.21 / 231,092.96 / 220,719.71 | 329,628.71 / 227,738.75 / 219,488.42 | 12,456,616 | 12,477,176 |
| random_1_events | 223,730.62 / 229,599.71 / 225,344.04 | 288,099.58 / 223,475.29 / 233,757.04 | 12,456,616 | 12,493,560 |
| random_chain_10 | 230,616.75 / 226,606.00 / 221,929.46 | 259,596.29 / 241,617.88 / 236,591.88 | 12,489,384 | 12,624,632 |
| random_chain_100 | 263,236.50 / 260,470.12 / 252,835.83 | 958,997.75 / 589,225.75 / 574,647.79 | 12,964,520 | 14,066,424 |
| random_split_100_sequential | 260,830.75 / 234,286.67 / 233,304.67 | 487,163.46 / 244,723.75 / 233,103.88 | 12,456,616 | 12,493,560 |
| random_split_100_parallel4 | 249,058.25 / 244,863.71 / 240,699.08 | 309,037.67 / 252,151.12 / 239,767.42 | 12,615,528 | 12,648,936 |
| random_embed_1 | 237,666.62 / 224,892.58 / 228,341.04 | 228,340.29 / 313,518.92 / 232,053.46 | 12,456,616 | 12,493,560 |
| finish_only | 198,823.00 / 192,909.29 / 194,534.38 | 195,171.25 / 218,231.46 / 192,852.96 | 10,703,544 | 10,703,992 |
| finish_payload_16k_minus1 | 203,385.67 / 192,763.71 / 189,020.83 | 189,142.50 / 194,191.88 / 191,665.75 | 10,703,544 | 10,703,992 |
| finish_payload_16k | 205,182.71 / 193,185.38 / 190,540.21 | 197,048.67 / 194,122.71 / 199,261.71 | 10,703,544 | 10,703,992 |
| finish_payload_16k_plus1 | 211,959.58 / 208,605.21 / 200,901.38 | 202,661.00 / 206,551.25 / 196,550.04 | 10,703,544 | 10,703,992 |
| finish_payload_1mib | 207,264.46 / 206,418.79 / 198,809.62 | 201,056.08 / 212,791.75 / 195,325.50 | 10,703,544 | 10,703,992 |

## Verification and limits

- Every measured workflow completed with validated output shape, count and random-value range. Defaults and durable variants created checkpoints; non-durable variants did not. The harness fails immediately on an execution error, and the driver preserves failure outcomes separately.
- Preserved .wasm, graph and input artifacts match the report hashes. Sources and all relevant component/metadata inputs were checked before and after each run. Agent workflows in the candidate contain standard cancellation and wait-set operations, and no artifact imports the superseded task service.
- The parallel4 workload composes four Agent instances. Random calls complete immediately; its timings do not establish HTTP overlap or cancellation latency. Existing functional overlap proofs are separate from this measurement.
- Raw samples include per-process p95/p99 for the 1,000-sample prepared conditions. Ten observations populate the highest one percent; these desktop tails do not establish deployment guarantees.
- Still required: parent-step phase spans and instrumentation cost, full validation timing, pending HTTP headers/body measurements, signal-poll/DB-query cost, public-server submission-to-terminal timing, cancellation/abort latency, remaining Agent/nested-workflow integration, Linux capacity, soak tests and explicit deployment budgets.
- An earlier attempt completed its baseline process but the driver failed to parse the libtest-prefixed report. Its logs were retained separately; none of its timing samples are reused in these three fresh pairs.
- Re-run this comparison after the remaining implementation and superseded-path cleanup. No release performance claim is made from this interim candidate.
