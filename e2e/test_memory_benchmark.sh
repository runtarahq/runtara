#!/usr/bin/env bash
# E2E Test: Memory benchmark harness
#
# Provisions the benchmark dependencies, compiles a generated 100-step
# workflow, starts an isolated WASM-backed runtime, executes the workflow, and
# writes memory results under .data/e2e-memory-benchmark by default.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

OUTPUT_DIR="${RUNTARA_MEMORY_BENCH_OUTPUT_DIR:-${PROJECT_ROOT}/.data/e2e-memory-benchmark}"
PROFILE="${RUNTARA_MEMORY_BENCH_PROFILE:-release}"
SHAPE="${RUNTARA_MEMORY_BENCH_SHAPE:-linear}"
STEPS="${RUNTARA_MEMORY_BENCH_STEPS:-100}"
RUNS="${RUNTARA_MEMORY_BENCH_RUNS:-1}"
PAYLOAD_KB="${RUNTARA_MEMORY_BENCH_PAYLOAD_KB:-1}"

EXTRA_ARGS=()
if [ -n "${RUNTARA_MEMORY_BENCH_DATABASE_URL:-}" ]; then
    EXTRA_ARGS+=(--postgres-mode external --database-url "${RUNTARA_MEMORY_BENCH_DATABASE_URL}")
fi
if [ -n "${RUNTARA_MEMORY_BENCH_WASM_LIBRARY_DIR:-}" ]; then
    EXTRA_ARGS+=(--wasm-library-dir "${RUNTARA_MEMORY_BENCH_WASM_LIBRARY_DIR}")
fi

cd "${PROJECT_ROOT}"

python3 scripts/measure_memory.py \
    --phases e2e \
    --profile "${PROFILE}" \
    --output-dir "${OUTPUT_DIR}" \
    --shapes "${SHAPE}" \
    --step-counts "${STEPS}" \
    --runs "${RUNS}" \
    --payload-kb "${PAYLOAD_KB}" \
    --sample-interval "${RUNTARA_MEMORY_BENCH_SAMPLE_INTERVAL:-0.1}" \
    --timeout-seconds "${RUNTARA_MEMORY_BENCH_TIMEOUT_SECONDS:-1800}" \
    --runtime-start-timeout "${RUNTARA_MEMORY_BENCH_RUNTIME_START_TIMEOUT:-180}" \
    ${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"}

test -s "${OUTPUT_DIR}/memory_results.csv"
test -s "${OUTPUT_DIR}/memory_results.json"
