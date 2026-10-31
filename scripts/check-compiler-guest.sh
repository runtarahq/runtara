#!/bin/bash
# The workflow compiler built as a WASI command (to compile in a sandbox,
# such as a Fenders VM) compiles a workflow to the same component as the
# native compiler, up to a long step chain; and runtara-compile-guest, with
# no filesystem, compiles one over the workflow API (served here by
# scripts/tests/compile_guest_host.py) to a valid component, fetching only the
# components it reads (that a compile so made is the in-process one is
# runtara-workflows' compile_request test).
#
# Usage: scripts/check-compiler-guest.sh COMPONENTS_DIR
# Needs the wasm32-wasip2 target, wasmtime and wasm-tools on PATH.
set -euo pipefail

COMPONENTS_DIR=$(cd "${1:?the agent components directory}" && pwd)
ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "${WORK}"' EXIT

cd "${ROOT}"
cargo build -q -p runtara-workflows --bin runtara-compile --features compiler --release
# A target directory of their own: built here, the stdlib these guests link
# would replace the component build's in target/wasm32-wasip2.
GUESTS="${ROOT}/target/guests"
cargo build -q -p runtara-workflows --bin runtara-compile --features compiler --release --target wasm32-wasip2 --target-dir "${GUESTS}"
cargo build -q -p runtara-compile-guest --release --target wasm32-wasip2 --target-dir "${GUESTS}"
GUEST="${GUESTS}/wasm32-wasip2/release/runtara-compile.wasm"
API_GUEST="${GUESTS}/wasm32-wasip2/release/runtara-compile-guest.wasm"

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

# Over the workflow API, with no filesystem: the stand-in host serves the
# request and components and takes the outcome.
PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
mkdir -p "${WORK}/api"
python3 "${ROOT}/scripts/tests/compile_guest_host.py" "${PORT}" "${WORK}/chain100.json" "${COMPONENTS_DIR}" "${WORK}/api" &
HOST_PID=$!
trap 'kill "${HOST_PID}" 2>/dev/null; rm -rf "${WORK}"' EXIT
for _ in $(seq 1 50); do (echo > "/dev/tcp/127.0.0.1/${PORT}") 2>/dev/null && break; sleep 0.1; done
wasmtime run -W max-wasm-stack=67108864 -S http --env "RUNTARA_COMPILE_API=http://127.0.0.1:${PORT}" "${API_GUEST}"
python3 - "${WORK}/api" "${WORK}/native/chain100.wasm" "${COMPONENTS_DIR}" <<'PY'
import hashlib, json, os, sys
api, native, components = sys.argv[1:]
outcome = json.load(open(os.path.join(api, "outcome.json")))
assert "compiled" in outcome, outcome
wasm = open(os.path.join(api, "workflow.wasm"), "rb").read()
assert outcome["compiled"]["workflow_wasm"] == hashlib.sha256(wasm).hexdigest()
assert outcome["compiled"]["outcome"]["binary_checksum"] == hashlib.sha256(wasm).hexdigest()
assert wasm[:8] == b"\0asm\x0d\0\x01\0", "not a component"
fetched = len(set(open(os.path.join(api, "fetched")).read().split()))
assert 0 < fetched < len(os.listdir(components)), fetched
print(f"  over the workflow API with no filesystem: a {len(wasm)}-byte component, from {fetched} fetched files ✓")
PY
wasm-tools validate --features all "${WORK}/api/workflow.wasm" \
    || { echo "the guest's component does not validate" >&2; exit 1; }

# As its tenant's warm compiler (SYN-930): two compiles, one after the other,
# the second fetching nothing the first fetched, then it ends once idle.
kill "${HOST_PID}" 2>/dev/null; wait "${HOST_PID}" 2>/dev/null || true
mkdir -p "${WORK}/worker"
python3 "${ROOT}/scripts/tests/compile_guest_host.py" "${PORT}" "${WORK}/chain100.json" "${COMPONENTS_DIR}" "${WORK}/worker" worker &
HOST_PID=$!
for _ in $(seq 1 50); do (echo > "/dev/tcp/127.0.0.1/${PORT}") 2>/dev/null && break; sleep 0.1; done
wasmtime run -W max-wasm-stack=67108864 -S http --env "RUNTARA_COMPILE_API=http://127.0.0.1:${PORT}" "${API_GUEST}"
python3 - "${WORK}/worker" "${WORK}/api/workflow.wasm" <<'PY'
import hashlib, json, os, sys
worker, single = sys.argv[1:]
expected = hashlib.sha256(open(single, "rb").read()).hexdigest()
for compile_id in ("j0", "j1"):
    outcome = json.load(open(os.path.join(worker, f"outcome-{compile_id}.json")))
    assert outcome["compiled"]["workflow_wasm"] == expected, (compile_id, outcome)
first = len(set(open(os.path.join(worker, "fetched-j0")).read().split()))
assert first > 0 and not os.path.exists(os.path.join(worker, "fetched-j1")), "the second compile fetched again"
print(f"  as a warm compiler: two compiles to the same component, the first fetching {first} files and the second none ✓")
PY
