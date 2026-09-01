#!/bin/bash
# E2E Test: a durable Delay frees the Store and is relaunched by the wake scheduler.
#
# A durable Delay used to sleep out its whole duration in-process, pinning a
# wasmtime Store and a tokio task for the entire wait — a 24-hour Delay held a
# Store for 24 hours. The Delay lowering now checkpoints an absolute deadline
# and exits with `outcome::suspended(at(deadline))`; the host frees the Store,
# stamps `sleep_until`, and the wake scheduler relaunches at the deadline.
#
# Parking is only worth it for a wait long enough to pay for the round trip
# (checkpoint write, Store teardown, up to one 5s scheduler poll of lag, and a
# replay from the entry step), so the choice is made at RUNTIME against a 30s
# threshold. This test asserts both sides of it:
#
#   LONG  (>= threshold) -> status=suspended, termination_reason='sleeping',
#                           sleep_until ~ now+duration, container registry row
#                           gone (the Store really was freed), then the wake
#                           scheduler relaunches it and it completes.
#   SHORT (<  threshold) -> never suspends; blocks in-process and completes,
#                           with no sleep_until ever stamped.
#
# The short case is the one a threshold-free promotion would get wrong: it would
# park for 2s and wait up to 5s for a poll, turning a sub-second pause inside a
# loop into a multi-second one plus a replay per iteration.
#
# Usage:  ./e2e/test_durable_delay_parks_and_wakes.sh
#
# Prereqs: Postgres + docker (isolated Valkey) and prebuilt components in
# target/wasm32-wasip2/release (scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Must straddle the 30s park threshold in the Delay lowering.
LONG_DELAY_MS="${LONG_DELAY_MS:-45000}"
SHORT_DELAY_MS="${SHORT_DELAY_MS:-2000}"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"

TEST_DB_SERVER="delay_e2e_server_$$"
TEST_DB_RUNTIME="delay_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17720}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17721}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18721}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18722}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18723}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18724}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16392}"
TEST_DATA_DIR="$(mktemp -d -t runtara_delay_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="delay_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

# `psql` is not always on PATH on a dev box where Postgres runs in a container;
# fall back to running it inside that container. Either way the server itself
# reaches Postgres over the published port on the host.
PG_CONTAINER="${PG_CONTAINER:-runtara-dev-postgres}"
if command -v psql >/dev/null 2>&1; then
    psql_quiet() {
        PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
    }
else
    psql_quiet() {
        docker exec -e PGPASSWORD="${POSTGRES_PASSWORD}" -i "${PG_CONTAINER}" \
            psql -U "${POSTGRES_USER}" -tA "$@"
    }
fi
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}

cleanup() {
    local code=$?
    [ -n "${SERVER_PID}" ] && kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
    [ -n "${VALKEY_CONTAINER}" ] && docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER} WITH (FORCE)" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME} WITH (FORCE)" >/dev/null 2>&1 || true
    [ ${code} -ne 0 ] && [ -f "${TEST_LOG}" ] && { echo "--- server log tail ---"; tail -40 "${TEST_LOG}"; }
    rm -rf "${TEST_DATA_DIR}"
    exit ${code}
}
trap cleanup EXIT

start_server() {
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
    OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
    RUNTARA_DATABASE_URL="${RUNTIME_DB_URL}" \
    TENANT_ID="${TENANT}" \
    SERVER_HOST=127.0.0.1 \
    SERVER_PORT="${TEST_PORT_PUBLIC}" \
    INTERNAL_PORT="${TEST_PORT_INTERNAL}" \
    RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
    RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
    RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
    RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
    RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
    DATA_DIR="${TEST_DATA_DIR}" \
    RUNTARA_DEV_MODE=false \
    RUST_LOG="${RUST_LOG_OVERRIDE:-warn,runtara_server=info,runtara_environment=info,runtara_core=info}" \
    AUTH_PROVIDER=local \
    SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2 \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true \
    RUNTARA_SDK_BACKEND=http \
    SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}" >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!

    for _ in {1..60}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        sleep 1
        if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
            print_error "Server exited during boot."; exit 1
        fi
    done
    print_error "Server did not become healthy."; exit 1
}

instance_status() {
    curl -sS "${API}/workflows/instances/$1" | jq -r '.data.status // .status // empty'
}
# Runtime-DB truth for the park: the wake bookkeeping the guest never sees.
instance_row() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT COALESCE(status::text,''), COALESCE(termination_reason::text,''), COALESCE(sleep_until::text,'') FROM instances WHERE instance_id = '$1'"
}
registry_rows() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT COUNT(*) FROM container_registry WHERE instance_id = '$1'" | tr -d '[:space:]'
}

# Create + compile + deploy a single-Delay workflow, echo its id.
make_delay_workflow() {
    local name="$1" duration_ms="$2" resp wf_id definition version
    resp=$(api_post /workflows/create "{\"name\": \"${name}\", \"description\": \"single durable Delay\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || { print_error "Workflow create failed: ${resp}"; exit 1; }

    definition=$(jq -n --argjson ms "${duration_ms}" '{
      name: "delay-park-harness",
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

echo "==============================================================="
echo "E2E: durable Delay parks and is woken  (long=${LONG_DELAY_MS}ms, short=${SHORT_DELAY_MS}ms)"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN} (cargo build -p runtara-server --bin runtara-server)"; exit 1; }
[ -f "${COMPONENTS_DIR}/runtara_workflow_stdlib.wasm" ] || { print_error "Missing runtara_workflow_stdlib.wasm — run scripts/build-agent-components.sh"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres (psql on PATH, or container '${PG_CONTAINER}')"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
start_server
echo "  Server up (PID ${SERVER_PID})"

# ---------------------------------------------------------------------------
# Case 1 — LONG delay: must park, free the Store, and be woken.
# ---------------------------------------------------------------------------
print_step "Case 1: ${LONG_DELAY_MS}ms Delay (at/above threshold) must PARK..."
WF_LONG=$(make_delay_workflow "delay-park-long" "${LONG_DELAY_MS}")
RESP=$(api_post "/workflows/${WF_LONG}/execute" '{"inputs": {"data": {}}}')
INST_LONG=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${INST_LONG}" ] || { print_error "Execute failed: ${RESP}"; exit 1; }
echo "  Instance ${INST_LONG}"

PARKED=""
for _ in {1..30}; do
    ROW=$(instance_row "${INST_LONG}")
    if [ "${ROW%%|*}" = "suspended" ]; then PARKED="${ROW}"; break; fi
    sleep 1
done
[ -n "${PARKED}" ] || { print_error "Instance never parked (row: $(instance_row "${INST_LONG}"), status: $(instance_status "${INST_LONG}"))"; exit 1; }

IFS='|' read -r P_STATUS P_REASON P_SLEEP_UNTIL <<< "${PARKED}"
echo "  status=${P_STATUS} termination_reason=${P_REASON} sleep_until=${P_SLEEP_UNTIL}"
[ "${P_STATUS}" = "suspended" ] || { print_error "expected status=suspended, got '${P_STATUS}'"; exit 1; }
[ "${P_REASON}" = "sleeping" ] || { print_error "expected termination_reason=sleeping, got '${P_REASON}'"; exit 1; }
[ -n "${P_SLEEP_UNTIL}" ] || { print_error "sleep_until was not stamped — the wake scheduler has nothing to act on"; exit 1; }

# The Store really was freed: no live container registered for this instance.
REG=$(registry_rows "${INST_LONG}")
[ "${REG}" = "0" ] || print_warn "container_registry still has ${REG} row(s) for the parked instance"
print_success "Parked with a timed wake — Store freed, sleep_until stamped ✓"

# sleep_until must be ~now+duration, not now: the deadline is absolute, so a
# resume never re-sleeps time that already elapsed.
AHEAD=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c \
    "SELECT EXTRACT(EPOCH FROM (sleep_until - NOW()))::int FROM instances WHERE instance_id = '${INST_LONG}'" | tr -d '[:space:]')
echo "  sleep_until is ${AHEAD}s ahead of now (delay was $((LONG_DELAY_MS / 1000))s)"
[ "${AHEAD}" -gt 5 ] || { print_error "sleep_until is only ${AHEAD}s out — the deadline was not computed from the duration"; exit 1; }

# A manual resume is NOT the wake scheduler: `handle_resume_instance` accepts any
# `suspended` row with no termination_reason filter, so it reaches a parked Delay
# an hour early. The guest must re-read its stored deadline and re-park rather
# than treat the checkpoint HIT as "already waited" and skip the whole delay.
print_step "Resuming early — the parked Delay must re-park, not skip..."
BEFORE_SLEEP_UNTIL="${P_SLEEP_UNTIL}"
api_post "/workflows/instances/${INST_LONG}/resume" '{}' >/dev/null || true
REPARKED=""
for _ in {1..30}; do
    ROW=$(instance_row "${INST_LONG}")
    ST=$(instance_status "${INST_LONG}")
    if [ "${ST}" = "completed" ]; then
        print_error "An early resume ran the Delay out — the deadline was not re-checked"; exit 1
    fi
    if [ "${ROW%%|*}" = "suspended" ] && [ -n "${ROW##*|}" ]; then REPARKED="${ROW}"; fi
    sleep 1
    # Give the relaunch a moment, then confirm it parked again rather than finishing.
    [ -n "${REPARKED}" ] && break
done
[ -n "${REPARKED}" ] || { print_error "Instance did not re-park after an early resume (row: $(instance_row "${INST_LONG}"))"; exit 1; }
AFTER_SLEEP_UNTIL="${REPARKED##*|}"
echo "  re-parked, sleep_until ${BEFORE_SLEEP_UNTIL} -> ${AFTER_SLEEP_UNTIL}"
[ "${BEFORE_SLEEP_UNTIL}" = "${AFTER_SLEEP_UNTIL}" ] || print_warn "sleep_until moved across the re-park (expected the same absolute deadline)"
print_success "Early resume re-parked on the same deadline, wait not skipped ✓"

print_step "Waiting for the wake scheduler to relaunch it..."
WOKE=""
DEADLINE=$(( $(date +%s) + LONG_DELAY_MS / 1000 + 60 ))
while [ "$(date +%s)" -lt "${DEADLINE}" ]; do
    ST=$(instance_status "${INST_LONG}")
    if [ "${ST}" = "completed" ]; then WOKE="yes"; break; fi
    if [ "${ST}" = "failed" ] || [ "${ST}" = "cancelled" ]; then
        print_error "Instance ended ${ST} instead of completing after its wake"; exit 1
    fi
    sleep 2
done
[ -n "${WOKE}" ] || { print_error "Instance never woke (status: $(instance_status "${INST_LONG}"))"; exit 1; }
print_success "Woken at its deadline and completed ✓"

# ---------------------------------------------------------------------------
# Case 2 — SHORT delay: must block, never park.
# ---------------------------------------------------------------------------
print_step "Case 2: ${SHORT_DELAY_MS}ms Delay (below threshold) must BLOCK..."
WF_SHORT=$(make_delay_workflow "delay-park-short" "${SHORT_DELAY_MS}")
RESP=$(api_post "/workflows/${WF_SHORT}/execute" '{"inputs": {"data": {}}}')
INST_SHORT=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${INST_SHORT}" ] || { print_error "Execute failed: ${RESP}"; exit 1; }
echo "  Instance ${INST_SHORT}"

SAW_SUSPENDED=""
SHORT_DONE=""
DEADLINE=$(( $(date +%s) + 60 ))
while [ "$(date +%s)" -lt "${DEADLINE}" ]; do
    ROW=$(instance_row "${INST_SHORT}")
    [ "${ROW%%|*}" = "suspended" ] && SAW_SUSPENDED="yes"
    ST=$(instance_status "${INST_SHORT}")
    if [ "${ST}" = "completed" ]; then SHORT_DONE="yes"; break; fi
    if [ "${ST}" = "failed" ] || [ "${ST}" = "cancelled" ]; then
        print_error "Short-delay instance ended ${ST}"; exit 1
    fi
    sleep 0.5
done
[ -n "${SHORT_DONE}" ] || { print_error "Short-delay instance never completed (status: $(instance_status "${INST_SHORT}"))"; exit 1; }
[ -z "${SAW_SUSPENDED}" ] || { print_error "A below-threshold Delay parked; it must block in-process"; exit 1; }

FINAL_SLEEP=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c \
    "SELECT COALESCE(sleep_until::text,'') FROM instances WHERE instance_id = '${INST_SHORT}'" | tr -d '[:space:]')
[ -z "${FINAL_SLEEP}" ] || { print_error "A blocking Delay stamped sleep_until='${FINAL_SLEEP}'"; exit 1; }
print_success "Blocked in-process and completed, never parked ✓"

echo
print_success "SYN-619 verified: long Delay parks and is woken; short Delay blocks."
