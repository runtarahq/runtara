# Measured workflow performance baseline

Recorded 2026-09-06; source `0a04277c243fde9d2cee0ed416b2fb77ff0099ba`; 2 runs.

Host: Apple M3 Max, 16 CPU cores, 64 GiB RAM; macos/aarch64; release profile; Wasmtime 46.0.1; four Tokio workers.

Values below are ranges across runs, not confidence intervals. Each run uses five warmups, 100 measured fresh-Store executions for small cases and 30 for 100-call/large-payload cases. Compile/cold phases have three samples per case per run.

| Workflow | WASM bytes | Gzip bytes | Logic bytes | Cached full run p50 (ms) | Cached full run p95 (ms) | JSON → first result p50 (ms) |
|---|---:|---:|---:|---:|---:|---:|
| random_1_defaults | 3,308,070 | 848,296 | 60,967 | 0.143–0.148 | 0.159–0.231 | 230.086–231.506 |
| random_1 | 3,306,019 | 847,880 | 58,916 | 0.140–0.143 | 0.158–0.171 | 222.916–228.332 |
| random_1_durable | 3,306,423 | 847,980 | 59,320 | 0.144–0.148 | 0.162–0.175 | 220.657–226.341 |
| random_1_events | 3,308,276 | 848,021 | 61,173 | 0.171–0.188 | 0.262–0.302 | 241.138–247.830 |
| random_chain_10 | 3,324,803 | 849,099 | 77,700 | 0.454–0.490 | 0.506–0.618 | 224.327–251.405 |
| random_chain_100 | 3,515,648 | 860,233 | 268,545 | 19.855–20.129 | 20.219–20.762 | 255.258–268.245 |
| random_split_100_sequential | 3,310,703 | 848,716 | 63,600 | 9.782–10.499 | 9.998–11.090 | 229.375–244.521 |
| random_split_100_parallel4 | 3,318,568 | 850,186 | 69,523 | 12.454–12.758 | 12.954–13.191 | 243.785–245.143 |
| random_embed_1 | 3,309,475 | 848,459 | 62,372 | 0.185–0.207 | 0.211–0.296 | 219.271–227.946 |
| finish_only | 2,803,799 | 691,044 | 56,144 | 0.081–0.088 | 0.092–0.105 | 186.590–191.220 |
| finish_payload_1mib | 2,803,823 | 691,050 | 56,168 | 4.250–4.556 | 4.439–4.884 | 192.168–199.346 |

## Preparation and memory

| Workflow | JSON → WASM p50 (ms) | Native compile p50 (ms) | Prepared linking p50 (µs) | Serialized native bytes | Largest guest memory bytes |
|---|---:|---:|---:|---:|---:|
| random_1_defaults | 32.434–33.348 | 196.325–197.183 | 32.000–35.375 | 12,456,600 | 1,310,720 |
| random_1 | 32.138–32.330 | 189.426–194.837 | 32.625–32.750 | 12,456,600 | 1,310,720 |
| random_1_durable | 31.461–32.533 | 188.685–192.787 | 37.458–42.791 | 12,456,600 | 1,310,720 |
| random_1_events | 32.366–32.771 | 206.469–214.067 | 33.166–35.875 | 12,456,600 | 1,310,720 |
| random_chain_10 | 33.495–33.536 | 189.817–216.121 | 34.583–36.209 | 12,489,368 | 1,310,720 |
| random_chain_100 | 36.365–37.174 | 198.215–207.726 | 36.375–38.250 | 12,964,504 | 1,769,472 |
| random_split_100_sequential | 31.230–32.984 | 186.818–198.887 | 37.208–38.416 | 12,456,600 | 1,310,720 |
| random_split_100_parallel4 | 32.309–33.229 | 195.974–199.432 | 31.833–33.750 | 12,615,512 | 1,310,720 |
| random_embed_1 | 32.019–32.786 | 185.889–193.950 | 33.875–39.292 | 12,456,600 | 1,310,720 |
| finish_only | 26.916–28.381 | 158.785–161.399 | 31.000–34.375 | 10,703,536 | 1,179,648 |
| finish_payload_1mib | 27.283–28.600 | 160.358–164.596 | 33.708–40.084 | 10,703,536 | 8,519,680 |

The memory column is the largest individual guest memory, **not aggregate guest memory or process RSS**. Serialized native bytes are cache-artifact size, not resident memory.

## Durability and comparison boundary

- `random_1_defaults`: cached-result replay p50 0.130–0.135 ms; checkpoint entries after initial execution: 1. Replay asserts byte-identical random output.
- `random_1_durable`: cached-result replay p50 0.131–0.141 ms; checkpoint entries after initial execution: 1. Replay asserts byte-identical random output.

`random_1_defaults` leaves durability and retries at DSL defaults. Other random cases set maxRetries=0 and explicitly select durability. A single-call workflow contains one Agent plus a Finish that exposes its result. Chains expose every generated number; Splits validate 100 results. The embedded case invokes a child with one random call.

`cached_full_run` measures the production WorkflowExecutor entry through return, including input cloning, fresh Store/WASI construction, instantiation, invocation, in-memory runtime calls, outcome handling and teardown. It excludes component compilation/preparation and construction of the test runtime host. It is not a bare agent-function measurement.

`JSON → first result` includes parsing/emission, composition and artifact I/O, uncached native compilation, linking and first execution. The engine and Tokio runtime already exist; filesystem pages may be warm. It excludes Cargo compilation, server request handling, authentication, image registration/download, admission queues and real persistence. Native serialization and gzip measurement occur after this timer. Phase medians do not necessarily sum to the median total.

The runtime is the existing in-memory capturing test host. No database, external HTTP service, sleep or signal wait is involved; durable figures exclude network/storage latency. Random-double uses the real utils component and WASI randomness. Event tracking uses in-memory capture, not production log ingestion.

Selective-isolation results are **not yet available**. This baseline is not evidence of isolated DSL performance. Re-run these exact cases through the implemented backend and compare raw sizes, p50/p95, preparation and full-run totals. Earlier per-Store microbenchmarks must not be substituted or simply multiplied into these numbers.

Raw data includes workload definitions and hashes, dependency hashes, compiler artifact hashes, sample counts and configuration. Use `workflow_baseline_report.py --compare baseline.json candidate.json` for deltas after measuring a candidate on the same host; run identity and dependency differences require review.
