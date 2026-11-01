#!/bin/bash
# E2E Test: a parked linked workflow resumes on the stdlib it was built with,
# across a component release upgrade.
#
# A linked artifact does not carry the workflow stdlib: it pins the stdlib's
# SHA-256 and the host links that exact stdlib into each run. A run parked on a
# durable Delay drops its Store and replays from the start when woken, so if
# the server was upgraded meanwhile, the replay must still get the old stdlib
# (from the artifact store, where the old release keeps it), not the new one.
#
#   1. Release A: compile a workflow with a long durable Delay, start it, and
#      wait until it parks.
#   2. Stop the server. Release B: the same components with a different stdlib
#      (a custom section appended, so its digest changes). Restart on B.
#   3. The wake scheduler relaunches the parked run: it must complete, served
#      stdlib A.
#   4. A workflow compiled on B pins stdlib B and completes too.
#
# Usage:  ./e2e/test_linked_stdlib_release_upgrade.sh
#
# Prereqs: Postgres + docker (isolated Valkey), the server binary, and
# prebuilt components in target/wasm32-wasip2/release
# (scripts/build-agent-components.sh).
set -euo pipefail
RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

DELAY_MS="${DELAY_MS:-45000}"
POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"
TEST_DB_SERVER="stdlib_e2e_server_$$"
TEST_DB_RUNTIME="stdlib_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17730}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18731}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18732}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18733}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18734}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16393}"
TEST_DATA_DIR="$(mktemp -d -t runtara_stdlib_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="stdlib_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
BUILT_COMPONENTS="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
RELEASE_A="${TEST_DATA_DIR}/release-a"
RELEASE_B="${TEST_DATA_DIR}/release-b"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
}
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}

stop_server() {
    [ -n "${SERVER_PID}" ] && kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
    SERVER_PID=""
}

cleanup() {
    local code=$?
    stop_server
    [ -n "${VALKEY_CONTAINER}" ] && docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER} WITH (FORCE)" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME} WITH (FORCE)" >/dev/null 2>&1 || true
    [ ${code} -ne 0 ] && [ -f "${TEST_LOG}" ] && { echo "--- server log tail ---"; tail -60 "${TEST_LOG}"; }
    rm -rf "${TEST_DATA_DIR}"
    exit ${code}
}
trap cleanup EXIT

# start_server <components dir>
start_server() {
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
    OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
    RUNTARA_DATABASE_URL="${RUNTIME_DB_URL}" \
    TENANT_ID="${TENANT}" \
    SERVER_HOST=127.0.0.1 \
    SERVER_PORT="${TEST_PORT_PUBLIC}" \
    RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
    RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
    RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
    RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
    RUNTARA_AGENT_COMPONENTS_DIR="$1" \
    DATA_DIR="${TEST_DATA_DIR}/data" \
    RUNTARA_DEV_MODE=false \
    RUST_LOG="${RUST_LOG_OVERRIDE:-warn,runtara_server=info,runtara_environment=info}" \
    AUTH_PROVIDER=local \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true \
    RUNTARA_SDK_BACKEND=http \
    SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}" >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!
    for _ in {1..90}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        sleep 1
        kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "Server exited during boot."; exit 1; }
    done
    print_error "Server did not become healthy."; exit 1
}

instance_status() {
    curl -sS "${API}/workflows/instances/$1" | jq -r '.data.status // .status // empty'
}
instance_row_status() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT COALESCE(status::text,'') FROM instances WHERE instance_id = '$1'" | tr -d '[:space:]'
}

# make_workflow <name> <delay ms>: create and compile a durable Delay workflow.
make_workflow() {
    local name="$1" duration_ms="$2" resp wf_id definition version
    resp=$(api_post /workflows/create "{\"name\": \"${name}\", \"description\": \"linked stdlib upgrade\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || { print_error "Workflow create failed: ${resp}"; exit 1; }
    definition=$(jq -n --argjson ms "${duration_ms}" '{
      name: "linked-stdlib-upgrade",
      durable: true,
      entryPoint: "delay",
      steps: {
        delay: { stepType: "Delay", id: "delay", name: "Wait", durationMs: { valueType: "immediate", value: $ms } },
        finish: { stepType: "Finish", id: "finish",
                  inputMapping: { waited: { valueType: "immediate", value: true } } }
      },
      executionPlan: [ { fromStep: "delay", toStep: "finish" } ],
      variables: {}, inputSchema: {}, outputSchema: {}
    }')
    resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${definition}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Update failed: ${resp}"; exit 1; }
    version=$(curl -sS "${API}/workflows/${wf_id}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf_id}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Compile failed: ${resp}"; exit 1; }
    echo "${wf_id}"
}

# run_and_wait <workflow id> <seconds>: execute and wait for completion.
wait_completed() {
    local inst="$1" seconds="$2" st deadline
    deadline=$(( $(date +%s) + seconds ))
    while [ "$(date +%s)" -lt "${deadline}" ]; do
        st=$(instance_status "${inst}")
        [ "${st}" = "completed" ] && return 0
        if [ "${st}" = "failed" ] || [ "${st}" = "cancelled" ]; then
            print_error "Instance ${inst} ended ${st}"; exit 1
        fi
        sleep 2
    done
    print_error "Instance ${inst} did not complete (status: $(instance_status "${inst}"))"; exit 1
}

sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }

echo "==============================================================="
echo "E2E: a parked linked workflow survives a stdlib release upgrade"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN} (cargo build -p runtara-server --bin runtara-server)"; exit 1; }
[ -f "${BUILT_COMPONENTS}/runtara_workflow_stdlib.wasm" ] || { print_error "Missing runtara_workflow_stdlib.wasm; run scripts/build-agent-components.sh"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }

print_step "Preparing releases A and B..."
mkdir -p "${RELEASE_A}" "${RELEASE_B}"
cp "${BUILT_COMPONENTS}"/*.wasm "${BUILT_COMPONENTS}"/*.meta.json "${RELEASE_A}/"
cp "${RELEASE_A}"/* "${RELEASE_B}/"
# Release B's stdlib: the same code with a custom section appended, so it is a
# different artifact with a different digest. Its sidecar names the new bytes.
python3 - "${RELEASE_B}" <<'PY'
import hashlib, json, os, sys
d = sys.argv[1]
wasm = os.path.join(d, "runtara_workflow_stdlib.wasm")
meta = os.path.join(d, "runtara_workflow_stdlib.meta.json")
def leb(n):
    out = bytearray()
    while True:
        b = n & 0x7F; n >>= 7
        out.append(b | (0x80 if n else 0))
        if not n: return bytes(out)
name = b"runtara-e2e-release-b"
payload = leb(len(name)) + name + b"b"
data = open(wasm, "rb").read() + b"\x00" + leb(len(payload)) + payload
open(wasm, "wb").write(data)
m = json.load(open(meta))
m["sha256"] = hashlib.sha256(data).hexdigest()
m["sizeBytes"] = len(data)
json.dump(m, open(meta, "w"), indent=2)
PY
STDLIB_A=$(sha256_of "${RELEASE_A}/runtara_workflow_stdlib.wasm")
STDLIB_B=$(sha256_of "${RELEASE_B}/runtara_workflow_stdlib.wasm")
echo "  stdlib A ${STDLIB_A}"
echo "  stdlib B ${STDLIB_B}"
[ "${STDLIB_A}" != "${STDLIB_B}" ] || { print_error "releases share a stdlib"; exit 1; }

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Release A: starting the server and parking a run..."
start_server "${RELEASE_A}"
WF_A=$(make_workflow "linked-upgrade-a" "${DELAY_MS}")
RESP=$(api_post "/workflows/${WF_A}/execute" '{"inputs": {"data": {}}}')
INST_A=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${INST_A}" ] || { print_error "Execute failed: ${RESP}"; exit 1; }
for _ in {1..60}; do
    [ "$(instance_row_status "${INST_A}")" = "suspended" ] && break
    sleep 1
done
[ "$(instance_row_status "${INST_A}")" = "suspended" ] || { print_error "Run never parked (status: $(instance_status "${INST_A}"))"; exit 1; }
print_success "Run ${INST_A} parked on release A ✓"

print_step "Upgrading: restarting the server on release B..."
stop_server
start_server "${RELEASE_B}"

print_step "Waiting for the wake scheduler to relaunch the parked run on release B..."
wait_completed "${INST_A}" $(( DELAY_MS / 1000 + 90 ))
print_success "The parked run completed after the upgrade, served stdlib A ✓"

print_step "A workflow compiled on release B..."
WF_B=$(make_workflow "linked-upgrade-b" 1000)
RESP=$(api_post "/workflows/${WF_B}/execute" '{"inputs": {"data": {}}}')
INST_B=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${INST_B}" ] || { print_error "Execute failed: ${RESP}"; exit 1; }
wait_completed "${INST_B}" 120
print_success "A workflow compiled on release B ran ✓"

if grep -q "workflow stdlib" "${TEST_LOG}" && grep -E "not in the artifact store|was linked against" "${TEST_LOG}" >/dev/null; then
    print_error "the server log reports a stdlib it could not serve"; exit 1
fi

echo
print_success "A parked linked run resumes on its own stdlib across a release upgrade."
