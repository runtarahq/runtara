# Paired emitted Agent isolation measurements

Recorded 2026-09-06; implementation `fe014566d3ee8d28e0f3e5bf3f26f7610bb868ba`; 3 paired sessions.

Host: Apple M3 Max, 16 CPU cores, 64 GiB RAM; macOS 26.5.1 (25F80); rustc 1.97.0; macos/aarch64; release; Wasmtime 46.0.1; 4 Tokio workers; disk cache disabled.

First backend in each session: legacy, isolated-agent-adapter-v1, legacy. Backends within each session ran serially.

These are exploratory local measurements of `isolated-agent-adapter-v1`, not production qualification. The same 11 graphs and inputs run through both backends. An embedded graph remains inline; only its Agent call is isolated. Finish-only workloads use the legacy executor on both sides and are controls.

Each cell shows the range across independent sessions. Timing cells contain per-session p50/p95 values; they are not pooled percentiles or confidence intervals. Deltas compare corresponding sessions and use `100 × (isolated − legacy) / legacy`; a zero baseline yields N/A. Raw samples and configuration are retained in the [JSON companion](workflow-performance-comparison.json).

## Complete artifact sizes

| Workload | Artifact metric | Legacy bytes | Isolated bytes | Delta bytes | Delta % |
|---|---|---:|---:|---:|---:|
| random_1_defaults | Complete WASM | 3,308,198 | 3,314,102 | +5,904 | +0.18% |
| random_1_defaults | Complete gzip | 848,314 | 849,699 | +1,385 | +0.16% |
| random_1_defaults | Native worker payload | 12,456,600 | 12,532,110 | +75,510 | +0.61% |
| random_1 | Complete WASM | 3,306,147 | 3,312,051 | +5,904 | +0.18% |
| random_1 | Complete gzip | 847,899 | 849,210 | +1,311 | +0.15% |
| random_1 | Native worker payload | 12,456,600 | 12,532,110 | +75,510 | +0.61% |
| random_1_durable | Complete WASM | 3,306,551 | 3,312,455 | +5,904 | +0.18% |
| random_1_durable | Complete gzip | 847,998 | 849,322 | +1,324 | +0.16% |
| random_1_durable | Native worker payload | 12,456,600 | 12,532,110 | +75,510 | +0.61% |
| random_1_events | Complete WASM | 3,308,404 | 3,314,308 | +5,904 | +0.18% |
| random_1_events | Complete gzip | 848,040 | 849,355 | +1,315 | +0.16% |
| random_1_events | Native worker payload | 12,456,600 | 12,532,110 | +75,510 | +0.61% |
| random_chain_10 | Complete WASM | 3,324,931 | 3,330,835 | +5,904 | +0.18% |
| random_chain_10 | Complete gzip | 849,118 | 850,651 | +1,533 | +0.18% |
| random_chain_10 | Native worker payload | 12,489,368 | 12,564,878 | +75,510 | +0.60% |
| random_chain_100 | Complete WASM | 3,515,776 | 3,521,680 | +5,904 | +0.17% |
| random_chain_100 | Complete gzip | 860,251 | 862,191 | +1,940 | +0.23% |
| random_chain_100 | Native worker payload | 12,964,504 | 13,040,014 | +75,510 | +0.58% |
| random_split_100_sequential | Complete WASM | 3,310,831 | 3,316,735 | +5,904 | +0.18% |
| random_split_100_sequential | Complete gzip | 848,735 | 850,151 | +1,416 | +0.17% |
| random_split_100_sequential | Native worker payload | 12,456,600 | 12,532,110 | +75,510 | +0.61% |
| random_split_100_parallel4 | Complete WASM | 3,318,696 | 3,323,496 | +4,800 | +0.14% |
| random_split_100_parallel4 | Complete gzip | 850,204 | 851,636 | +1,432 | +0.17% |
| random_split_100_parallel4 | Native worker payload | 12,615,512 | 12,627,790 | +12,278 | +0.10% |
| random_embed_1 | Complete WASM | 3,309,603 | 3,315,507 | +5,904 | +0.18% |
| random_embed_1 | Complete gzip | 848,477 | 849,885 | +1,408 | +0.17% |
| random_embed_1 | Native worker payload | 12,456,600 | 12,532,110 | +75,510 | +0.61% |
| finish_only | Complete WASM | 2,803,799 | 2,803,799 | +0 | +0.00% |
| finish_only | Complete gzip | 691,044 | 691,044 | +0 | +0.00% |
| finish_only | Native worker payload | 10,703,536 | 10,703,536 | +0 | +0.00% |
| finish_payload_1mib | Complete WASM | 2,803,823 | 2,803,823 | +0 | +0.00% |
| finish_payload_1mib | Complete gzip | 691,050 | 691,050 | +0 | +0.00% |
| finish_payload_1mib | Native worker payload | 10,703,536 | 10,703,536 | +0 | +0.00% |

WASM and gzip cover the complete distributable package. Native bytes cover the full worker payload, including its child index; they are neither RSS nor portable WASM. JSON also separates root component, logic, unique child bytes and package framing. Logic is a subset of the root and must not be added again. Every isolated random workload has one binding and one unique utils component, including 100-call and parallel workloads.

## Prepared full execution p50

| Workload | Legacy (ms) | Isolated Agent (ms) | Paired delta (ms) | Paired delta % | Samples/backend/run |
|---|---:|---:|---:|---:|---:|
| random_1_defaults | 0.155–0.162 | 0.215–0.227 | 0.053–0.073 | 32.69–46.90% | 100 |
| random_1 | 0.139–0.147 | 0.210–0.218 | 0.071–0.072 | 48.02–51.77% | 100 |
| random_1_durable | 0.145–0.151 | 0.208–0.221 | 0.063–0.072 | 43.32–49.20% | 100 |
| random_1_events | 0.175–0.193 | 0.238–0.261 | 0.062–0.068 | 35.00–37.59% | 100 |
| random_chain_10 | 0.474–0.487 | 1.041–1.127 | 0.562–0.639 | 116.95–131.08% | 100 |
| random_chain_100 | 20.104–20.612 | 26.246–27.056 | 6.142–6.444 | 30.55–31.26% | 30 |
| random_split_100_sequential | 10.328–10.544 | 16.144–16.712 | 5.723–6.238 | 54.92–60.40% | 30 |
| random_split_100_parallel4 | 12.267–12.782 | 15.120–16.045 | 2.481–3.263 | 19.62–25.53% | 30 |
| random_embed_1 | 0.198–0.210 | 0.259–0.322 | 0.061–0.122 | 29.26–61.08% | 100 |
| finish_only | 0.086–0.089 | 0.086–0.090 | -0.002–0.003 | -2.01–3.66% | 100 |
| finish_payload_1mib | 4.277–4.502 | 4.262–4.560 | -0.239–0.131 | -5.32–3.05% | 30 |

## Prepared full execution p95

| Workload | Legacy (ms) | Isolated Agent (ms) | Paired delta (ms) | Paired delta % | Samples/backend/run |
|---|---:|---:|---:|---:|---:|
| random_1_defaults | 0.197–0.207 | 0.243–0.278 | 0.040–0.070 | 19.87–33.92% | 100 |
| random_1 | 0.155–0.196 | 0.231–0.296 | 0.070–0.112 | 43.46–72.37% | 100 |
| random_1_durable | 0.170–0.224 | 0.240–0.269 | 0.045–0.099 | 20.03–58.30% | 100 |
| random_1_events | 0.204–0.252 | 0.275–0.365 | 0.060–0.112 | 27.67–44.61% | 100 |
| random_chain_10 | 0.517–0.653 | 1.141–1.304 | 0.620–0.650 | 99.59–120.62% | 100 |
| random_chain_100 | 20.453–20.886 | 27.392–27.651 | 6.640–7.112 | 31.99–34.77% | 30 |
| random_split_100_sequential | 10.810–11.183 | 16.967–17.945 | 5.925–6.762 | 53.66–61.00% | 30 |
| random_split_100_parallel4 | 12.771–13.455 | 15.659–20.623 | 2.623–7.169 | 20.12–53.28% | 30 |
| random_embed_1 | 0.248–0.344 | 0.300–0.527 | -0.018–0.279 | -5.11–112.53% | 100 |
| finish_only | 0.102–0.106 | 0.102–0.110 | -0.003–0.008 | -3.30–8.08% | 100 |
| finish_payload_1mib | 4.618–5.318 | 4.619–5.080 | -0.341–0.218 | -6.87–4.73% | 30 |

The timer surrounds fresh execution, including input cloning, per-run execution-context/launcher construction when needed, fresh Store setup, guest orchestration, result collection and owned child teardown. The in-memory runtime host is constructed before this timer. Headline runs do not increment the optional child-launch counter. Separate instrumented runs verify the actual child counts and zero retained task-result bytes. This is the full Agent + Finish workflow cost, not an Agent-only or parent-step span.

## DSL to first result p50

| Workload | Legacy (ms) | Isolated Agent (ms) | Paired delta (ms) | Paired delta % | Samples/backend/run |
|---|---:|---:|---:|---:|---:|
| random_1_defaults | 284.119–299.156 | 301.488–323.097 | 9.272–23.940 | 3.16–8.00% | 3 |
| random_1 | 283.780–297.628 | 300.389–338.865 | 16.609–41.237 | 5.85–13.86% | 3 |
| random_1_durable | 281.184–302.095 | 293.718–314.203 | 4.277–26.980 | 1.48–9.60% | 3 |
| random_1_events | 279.704–304.909 | 298.170–321.620 | 16.711–20.462 | 5.48–7.24% | 3 |
| random_chain_10 | 281.706–315.183 | 305.808–330.266 | 15.083–24.102 | 4.79–8.56% | 3 |
| random_chain_100 | 334.252–356.340 | 350.290–387.168 | 10.161–30.828 | 2.99–9.06% | 3 |
| random_split_100_sequential | 302.096–312.708 | 325.785–335.801 | 13.077–32.867 | 4.18–10.88% | 3 |
| random_split_100_parallel4 | 298.968–323.030 | 310.967–372.581 | 5.898–49.551 | 1.93–15.34% | 3 |
| random_embed_1 | 286.542–303.053 | 300.081–307.511 | 2.177–20.969 | 0.72–7.32% | 3 |
| finish_only | 242.029–313.722 | 242.735–255.024 | -58.699–0.706 | -18.71–0.29% | 3 |
| finish_payload_1mib | 248.554–279.217 | 248.657–301.115 | -5.813–21.898 | -2.28–7.84% | 3 |

Cold preparation has only three samples per session, and identical Finish-only controls show substantial variation. Treat these cold deltas as exploratory, not regression gates. Larger controlled runs are required for qualification.

The engine already exists. This timer includes DSL parse/emission, composition/packaging, worker-format precompile, trusted deserialization, linking and first execution. The production worker codec is invoked synchronously in process with an explicitly cache-disabled engine. Process startup, IPC, server queueing and database persistence are excluded. Filesystem pages may be warm; no claim is made about a cold OS cache. Gzip, output correctness checks and artifact size inspection occur after this timer.

## Parse and emit p50

| Workload | Legacy (ms) | Isolated Agent (ms) | Paired delta (ms) | Paired delta % | Samples/backend/run |
|---|---:|---:|---:|---:|---:|
| random_1_defaults | 3.585–3.803 | 3.596–3.608 | -0.195–0.011 | -5.12–0.29% | 3 |
| random_1 | 3.376–3.621 | 3.537–3.649 | 0.005–0.161 | 0.15–4.77% | 3 |
| random_1_durable | 3.458–3.674 | 3.537–3.685 | -0.137–0.160 | -3.72–4.53% | 3 |
| random_1_events | 3.485–3.843 | 3.533–3.595 | -0.258–0.110 | -6.71–3.16% | 3 |
| random_chain_10 | 3.973–4.183 | 3.810–4.267 | -0.163–0.215 | -4.11–5.32% | 3 |
| random_chain_100 | 6.233–6.635 | 6.197–6.271 | -0.364–-0.036 | -5.49–-0.58% | 3 |
| random_split_100_sequential | 3.740–3.979 | 3.728–4.008 | -0.250–0.063 | -6.30–1.60% | 3 |
| random_split_100_parallel4 | 3.909–4.409 | 3.864–3.988 | -0.421–0.002 | -9.55–0.05% | 3 |
| random_embed_1 | 3.676–4.239 | 3.539–3.639 | -0.700–-0.037 | -16.51–-1.02% | 3 |
| finish_only | 3.209–3.443 | 3.485–3.787 | 0.042–0.434 | 1.22–13.52% | 3 |
| finish_payload_1mib | 3.469–3.625 | 3.457–3.503 | -0.123–0.006 | -3.38–0.16% | 3 |

## Compose and package p50

| Workload | Legacy (ms) | Isolated Agent (ms) | Paired delta (ms) | Paired delta % | Samples/backend/run |
|---|---:|---:|---:|---:|---:|
| random_1_defaults | 28.522–29.768 | 29.838–30.966 | 0.736–1.316 | 2.52–4.61% | 3 |
| random_1 | 28.897–30.093 | 29.853–30.556 | -0.240–1.631 | -0.80–5.64% | 3 |
| random_1_durable | 28.421–29.401 | 30.063–30.728 | 1.191–1.642 | 4.11–5.78% | 3 |
| random_1_events | 28.514–30.095 | 29.706–30.883 | 0.554–2.167 | 1.90–7.60% | 3 |
| random_chain_10 | 28.802–29.946 | 30.279–32.210 | 0.582–2.264 | 1.96–7.56% | 3 |
| random_chain_100 | 30.122–31.132 | 31.561–32.672 | 0.430–2.034 | 1.38–6.64% | 3 |
| random_split_100_sequential | 29.175–30.218 | 31.000–31.324 | 1.107–1.972 | 3.66–6.76% | 3 |
| random_split_100_parallel4 | 28.937–32.766 | 30.780–31.125 | -1.641–2.133 | -5.01–7.37% | 3 |
| random_embed_1 | 28.811–29.783 | 30.355–30.858 | 0.571–1.947 | 1.92–6.76% | 3 |
| finish_only | 24.534–24.726 | 24.449–25.245 | -0.085–0.518 | -0.35–2.10% | 3 |
| finish_payload_1mib | 23.848–24.325 | 24.225–24.809 | 0.206–0.569 | 0.85–2.35% | 3 |

## Worker precompile p50

| Workload | Legacy (ms) | Isolated Agent (ms) | Paired delta (ms) | Paired delta % | Samples/backend/run |
|---|---:|---:|---:|---:|---:|
| random_1_defaults | 222.459–236.611 | 237.791–257.775 | 8.385–21.164 | 3.63–8.94% | 3 |
| random_1 | 221.228–234.948 | 236.985–275.092 | 15.757–40.144 | 7.12–17.09% | 3 |
| random_1_durable | 217.832–239.921 | 230.602–249.373 | 4.811–27.291 | 2.13–12.53% | 3 |
| random_1_events | 217.803–233.555 | 228.729–256.191 | 10.926–22.636 | 5.02–9.69% | 3 |
| random_chain_10 | 218.164–249.912 | 240.666–262.185 | 12.272–22.502 | 4.91–10.31% | 3 |
| random_chain_100 | 245.186–267.988 | 253.494–290.142 | 2.004–22.866 | 0.80–9.33% | 3 |
| random_split_100_sequential | 229.332–239.518 | 245.201–253.601 | 5.683–23.519 | 2.37–10.26% | 3 |
| random_split_100_parallel4 | 224.480–242.994 | 229.940–291.426 | -0.330–48.432 | -0.14–19.93% | 3 |
| random_embed_1 | 225.739–239.864 | 236.202–243.194 | 1.480–17.456 | 0.62–7.73% | 3 |
| finish_only | 189.088–259.549 | 190.528–201.058 | -58.490–1.440 | -22.54–0.76% | 3 |
| finish_payload_1mib | 191.585–221.680 | 191.160–242.329 | -4.871–20.649 | -2.48–9.31% | 3 |

## Trusted deserialize p50

| Workload | Legacy (ms) | Isolated Agent (ms) | Paired delta (ms) | Paired delta % | Samples/backend/run |
|---|---:|---:|---:|---:|---:|
| random_1_defaults | 28.802–29.513 | 28.539–29.229 | -0.284–0.136 | -0.96–0.47% | 3 |
| random_1 | 28.122–29.402 | 28.702–29.360 | -0.351–1.238 | -1.19–4.40% | 3 |
| random_1_durable | 28.039–28.850 | 28.853–29.327 | 0.412–0.937 | 1.45–3.34% | 3 |
| random_1_events | 28.116–28.518 | 28.484–29.718 | 0.061–1.200 | 0.21–4.21% | 3 |
| random_chain_10 | 28.585–29.156 | 28.739–29.587 | 0.053–0.431 | 0.19–1.48% | 3 |
| random_chain_100 | 29.793–30.214 | 29.678–30.173 | -0.531–0.379 | -1.76–1.27% | 3 |
| random_split_100_sequential | 28.333–28.741 | 28.704–29.072 | 0.179–0.371 | 0.62–1.31% | 3 |
| random_split_100_parallel4 | 28.725–29.338 | 28.636–29.442 | -0.089–0.104 | -0.31–0.36% | 3 |
| random_embed_1 | 28.186–28.534 | 28.729–28.999 | 0.431–0.782 | 1.51–2.77% | 3 |
| finish_only | 24.461–25.514 | 24.363–24.711 | -0.803–0.046 | -3.15–0.19% | 3 |
| finish_payload_1mib | 23.853–24.473 | 24.739–25.025 | 0.322–1.172 | 1.32–4.91% | 3 |

## Prepared linking p50

| Workload | Legacy (ms) | Isolated Agent (ms) | Paired delta (ms) | Paired delta % | Samples/backend/run |
|---|---:|---:|---:|---:|---:|
| random_1_defaults | 0.038–0.046 | 0.051–0.055 | 0.006–0.016 | 13.16–42.70% | 3 |
| random_1 | 0.032–0.038 | 0.048–0.055 | 0.015–0.016 | 40.97–49.42% | 3 |
| random_1_durable | 0.034–0.038 | 0.047–0.054 | 0.013–0.017 | 35.59–44.90% | 3 |
| random_1_events | 0.033–0.037 | 0.048–0.053 | 0.012–0.016 | 34.73–48.73% | 3 |
| random_chain_10 | 0.034–0.039 | 0.051–0.054 | 0.011–0.020 | 28.68–58.03% | 3 |
| random_chain_100 | 0.036–0.039 | 0.048–0.056 | 0.010–0.018 | 25.68–49.25% | 3 |
| random_split_100_sequential | 0.034–0.037 | 0.048–0.051 | 0.012–0.015 | 34.49–45.54% | 3 |
| random_split_100_parallel4 | 0.035–0.041 | 0.048–0.053 | 0.009–0.016 | 21.85–42.00% | 3 |
| random_embed_1 | 0.038–0.039 | 0.047–0.061 | 0.007–0.023 | 18.87–59.25% | 3 |
| finish_only | 0.033–0.044 | 0.034–0.036 | -0.009–0.003 | -21.13–8.35% | 3 |
| finish_payload_1mib | 0.033–0.040 | 0.032–0.037 | -0.004–-0.000 | -11.95–-0.13% | 3 |

## Durable checkpoint replay p50

| Workload | Legacy (ms) | Isolated Agent (ms) | Paired delta (ms) | Paired delta % | Samples/backend/run |
|---|---:|---:|---:|---:|---:|
| random_1_defaults | 0.142–0.147 | 0.150–0.156 | 0.003–0.014 | 1.90–10.02% | 100 |
| random_1_durable | 0.132–0.138 | 0.149–0.155 | 0.013–0.019 | 9.23–13.71% | 100 |

Worker precompile includes bounded source read, hash/validation, root and unique-child native compilation, serialization and integrity hashing. Deserialization includes integrity/configuration checks and loading native members. Phase medians need not sum to the median total. Replay asserts byte-identical cached random results and zero new isolated starts.

## Instrumentation comparison

| Workload | Isolated plain p50 (ms) | Isolated instrumented p50 (ms) | Paired delta (ms) | Paired delta % |
|---|---:|---:|---:|---:|
| random_1_defaults | 0.215–0.227 | 0.212–0.226 | -0.003–-0.001 | -1.35–-0.33% |
| random_1 | 0.210–0.218 | 0.208–0.219 | -0.004–0.001 | -1.89–0.42% |
| random_1_durable | 0.208–0.221 | 0.210–0.221 | -0.002–0.003 | -1.09–1.41% |
| random_1_events | 0.238–0.261 | 0.237–0.257 | -0.004–0.003 | -1.42–1.42% |
| random_chain_10 | 1.041–1.127 | 1.043–1.140 | 0.001–0.013 | 0.14–1.18% |
| random_chain_100 | 26.246–27.056 | 26.285–27.041 | -0.075–0.164 | -0.29–0.63% |
| random_split_100_sequential | 16.144–16.712 | 16.432–16.622 | -0.089–0.288 | -0.53–1.79% |
| random_split_100_parallel4 | 15.120–16.045 | 15.049–15.886 | -0.159–0.062 | -0.99–0.41% |
| random_embed_1 | 0.259–0.322 | 0.259–0.328 | -0.006–0.006 | -2.11–1.84% |
| finish_only | 0.086–0.090 | 0.085–0.091 | -0.001–0.002 | -1.02–2.05% |
| finish_payload_1mib | 4.262–4.560 | 4.301–4.560 | -0.107–0.132 | -2.42–3.09% |

Instrumented runs add launch counting and retained-result assertions; execution order alternates with plain runs. Small or negative differences can be measurement noise and must not be interpreted as an optimization. Instrumentation does not provide a direct Agent service span.

## Remaining measurements and acceptance

- Agent-only service and parent-step spans remain pending for both backends.
- Server end-to-end time, persistence, targeted cancellation and aggregate memory/RSS remain pending.
- The JSON root-memory diagnostic is the largest single memory in the root Store; it excludes isolated child Stores and is not a capacity comparison.
- Concurrency/load qualification, adequate production-tail samples, agreed budgets and all P5/P6 compatibility gates remain pending. These local timings do not authorize default enablement.
