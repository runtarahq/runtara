#!/bin/bash
# The workflow compiler built as a WASI command (to compile in a sandbox,
# such as a Fenders VM) compiles a workflow to the same component as the
# native compiler, up to a long step chain.
#
# Usage: scripts/check-compiler-guest.sh COMPONENTS_DIR
# Needs the wasm32-wasip2 target and wasmtime on PATH.
set -euo pipefail

COMPONENTS_DIR=$(cd "${1:?the agent components directory}" && pwd)
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "${WORK}"' EXIT

cd "${ROOT}"
cargo build -q -p runtara-workflows --bin runtara-compile --features compiler --release
cargo build -q -p runtara-workflows --bin runtara-compile --features compiler --release --target wasm32-wasip2
GUEST="${ROOT}/target/wasm32-wasip2/release/runtara-compile.wasm"

# chain N: a workflow of N chained agent steps and a finish.
chain() {
    python3 - "$1" <<'PY'
import json, sys
n = int(sys.argv[1]); steps = {}; plan = []
for i in range(n):
    steps[f"s{i}"] = {"id": f"s{i}", "stepType": "Agent", "agentId": "utils", "capabilityId": "return-input",
                      "inputMapping": {"value": {"valueType": "immediate", "value": i}}}
    if i + 1 < n:
        plan.append({"fromStep": f"s{i}", "toStep": f"s{i + 1}"})
steps["finish"] = {"id": "finish", "stepType": "Finish",
                   "inputMapping": {"out": {"valueType": "reference", "value": f"steps.s{n - 1}.outputs"}}}
plan.append({"fromStep": f"s{n - 1}", "toStep": "finish"})
print(json.dumps({"name": "Chain", "steps": steps, "entryPoint": "s0", "executionPlan": plan,
                  "variables": {}, "inputSchema": {}, "outputSchema": {}}))
PY
}

for n in 1 100 1000; do
    chain "${n}" > "${WORK}/chain${n}.json"
    mkdir -p "${WORK}/native" "${WORK}/guest"
    target/release/runtara-compile --workflow "${WORK}/chain${n}.json" --components-dir "${COMPONENTS_DIR}" \
        --output "${WORK}/native/chain${n}.wasm" --build-dir "${WORK}/native/build" \
        --tenant t --workflow-id "chain${n}" >/dev/null
    # The guest's own stack is 32 MiB (build.rs); the host's is given room.
    wasmtime run -W max-wasm-stack=67108864 \
        --dir "${COMPONENTS_DIR}::/components" --dir "${WORK}::/work" "${GUEST}" \
        --workflow "/work/chain${n}.json" --components-dir /components \
        --output "/work/guest/chain${n}.wasm" --build-dir /work/guest/build \
        --tenant t --workflow-id "chain${n}" >/dev/null
    cmp -s "${WORK}/native/chain${n}.wasm" "${WORK}/guest/chain${n}.wasm" \
        || { echo "a chain of ${n} compiled differently in the guest" >&2; exit 1; }
    echo "  a chain of ${n}: the guest's component is the native compiler's ✓"
done
