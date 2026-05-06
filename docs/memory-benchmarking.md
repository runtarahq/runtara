# Memory Benchmarking

Use `scripts/measure_memory.py` to generate large local workflows and measure
resident memory across validation, compilation, and runtime execution.

The harness generates workflow JSON under `.data/memory-bench/workflows`, samples
the measured process tree with `ps`, and writes:

- `.data/memory-bench/memory_results.csv`
- `.data/memory-bench/memory_results.json`
- `.data/memory-bench/samples/*.csv`
- `.data/memory-bench/manifest.json`

## Quick Smoke Test

```bash
python3 scripts/measure_memory.py \
  --phases validate \
  --shapes linear \
  --step-counts 100 \
  --runs 1
```

## Isolated E2E Smoke Test

Use the `e2e` phase to provision the missing pieces instead of relying on a
preconfigured machine:

```bash
python3 scripts/measure_memory.py \
  --phases e2e \
  --step-counts 100 \
  --shapes linear \
  --runs 1
```

This mode:

- Builds `runtara-compile`, `runtara-ctl`, and `runtara-environment`.
- Builds and caches the `wasm32-wasip2` workflow stdlib under the output dir.
- Starts an isolated Postgres container with Docker.
- Starts an isolated `runtara-environment` with `RUNTARA_RUNNER=wasm`.
- Registers the compiled workflow as a WASM image.
- Executes the workflow and tears down the runtime/container.

If Docker is unavailable, or Docker cannot allocate storage, point the harness at
an existing disposable Postgres database:

```bash
python3 scripts/measure_memory.py \
  --phases validate,compile,execute \
  --provision \
  --postgres-mode external \
  --database-url postgresql://user:pass@localhost:5432/runtara_memory_bench \
  --step-counts 100 \
  --shapes linear
```

The same path is available as an `e2e/` wrapper:

```bash
RUNTARA_MEMORY_BENCH_DATABASE_URL=postgresql://user:pass@localhost:5432/runtara_memory_bench \
e2e/test_memory_benchmark.sh
```

## Compile Memory

```bash
python3 scripts/measure_memory.py \
  --phases compile \
  --shapes linear,branching,split,payload \
  --step-counts 100,250,500,1000 \
  --runs 3 \
  --sample-interval 0.05
```

Compilation uses the normal Runtara compiler path. The default compile target is
`wasm32-wasip2`. Add `--provision` to build and use an isolated stdlib cache
automatically, or pass `--wasm-library-dir` to reuse an existing cache. You can
pass `--compile-target <target>` to override `RUNTARA_COMPILE_TARGET` for the
benchmark run.

If the release tools are missing:

```bash
python3 scripts/measure_memory.py --build-tools --phases validate --step-counts 100
```

## Runtime Memory

For an isolated runtime, let the harness start and stop it:

```bash
python3 scripts/measure_memory.py \
  --phases execute \
  --provision \
  --shapes linear,branching,split,payload \
  --step-counts 100,250,500 \
  --runs 5 \
  --sample-interval 0.05
```

The execute phase needs a runtime PID so the script can sample server and runner
memory. With `--provision`, the script uses the isolated runtime PID
automatically. Without provisioning, pass `--server-pid <pid>` or
`--server-pid-file <path>`.

## Workflow Shapes

- `linear`: `N - 1` Log steps plus Finish.
- `branching`: repeated Conditional diamonds plus Finish.
- `split`: repeated Split steps; nested subgraph steps count toward `N`.
- `payload`: linear workflow with an embedded payload variable.

`--payload-kb` controls both the embedded payload used by the `payload` shape and
the runtime input payload. `--split-items` and `--split-parallelism` control the
runtime input and Split configuration.

## Sizing Columns

The CSV includes:

- `peak_rss_mb`: largest sampled resident set size for the measured process tree.
- `recommend_dev_mb`: `ceil(peak_rss_mb * 1.5)`.
- `recommend_prod_mb`: `ceil(peak_rss_mb * 2.0)`.

For concurrent workflow sizing, use the runtime rows to estimate:

```text
required_mb = baseline_runtime_mb + concurrency * per_workflow_delta_mb
```

Run repeated sequential executions first. If `peak_rss_mb` or post-run baseline
keeps rising across runs, investigate retention or leaks before using the
numbers for production limits.
