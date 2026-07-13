#!/bin/bash
# E2E Test: a suspended-then-relaunched instance must never render a negative
# execution duration.
#
# Reproduces the "Running + Completed + negative duration" data inconsistency:
# a guest that is force-stopped by the drain at grace expiry is marked
# `suspended` with `finished_at` stamped at drain time; on restart the wake
# scheduler relaunches it and re-registration overwrites `started_at` with a
# LATER time. If the running transition doesn't clear `finished_at`, the row
# has `finished_at < started_at` and reports a negative duration for the whole
# relaunched run.
#
# The fix clears `finished_at` (and `termination_reason`) when a row transitions
# back to `running`. This test asserts:
#   * the drain stamps `finished_at` (the poison source really occurs), then
#   * across the relaunch the running row has `finished_at IS NULL`, and
#   * neither the runtime DB nor the API ever reports a negative duration.
#
# On UNPATCHED code this FAILS: the relaunched running row keeps the stale
# `finished_at` and both DB and API report a negative duration.
#
# The workload is a single blocking `utils:delay-in-ms` step (the guest stays
# *running*, not self-suspended, like a slow synchronous HTTP call), longer than
# the drain grace so a mid-run restart force-stops it.
#
# Prereqs: Postgres reachable on ${POSTGRES_HOST}:${POSTGRES_PORT} (host `psql`,
# or a `runtara-dev-postgres` docker container as fallback), docker (isolated
# Valkey), and prebuilt components in target/wasm32-wasip2/release
# (scripts/build-agent-components.sh).

set -uo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

DELAY_MS="${DELAY_MS:-15000}"       # single blocking step, > grace
GRACE_MS="${GRACE_MS:-1500}"        # drain grace (short → mid-run force-stop)

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"
PG_CONTAINER="${PG_CONTAINER:-runtara-dev-postgres}"

TEST_DB_SERVER="negdur_e2e_server_$$"
TEST_DB_RUNTIME="negdur_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17730}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17731}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18731}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18732}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18733}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18734}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16393}"
TEST_DATA_DIR="$(mktemp -d -t runtara_negdur_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="negdur_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

# psql shim: prefer host psql, fall back to the dev postgres container.
if command -v psql >/dev/null 2>&1; then
    psql_quiet() { PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"; }
elif docker exec "${PG_CONTAINER}" true >/dev/null 2>&1; then
    print_warn "host psql not found — using docker exec ${PG_CONTAINER}"
    psql_quiet() { docker exec -e PGPASSWORD="${POSTGRES_PASSWORD}" "${PG_CONTAINER}" psql -U "${POSTGRES_USER}" -tA "$@"; }
else
    print_error "no psql available (host psql or ${PG_CONTAINER} container required)"; exit 1
fi

api_post() { curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"; }
instance_status() { curl -sS "${API}/workflows/instances/$1" | jq -r '.data.status // .status // empty'; }
api_duration() { curl -sS "${API}/workflows/instances/$1" | jq -r '.data.executionDurationSeconds // .executionDurationSeconds // "null"'; }
# runtime DB row fields
db_field() { psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT COALESCE($1::text,'NULL') FROM instances WHERE instance_id='$2'" | tr -d '[:space:]'; }
db_duration_ms() { psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT COALESCE((EXTRACT(EPOCH FROM (finished_at - started_at))*1000)::text,'NULL') FROM instances WHERE instance_id='$1'" | tr -d '[:space:]'; }

cleanup() {
    if [ -n "${SERVER_PID}" ] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        kill -9 "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    [ -n "${VALKEY_CONTAINER}" ] && docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    if [ "${KEEP_DB:-0}" = "1" ]; then
        echo "KEEP_DB=1 — leaving ${TEST_DB_SERVER}/${TEST_DB_RUNTIME} and ${TEST_DATA_DIR}"
        return
    fi
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME}" >/dev/null 2>&1 || true
    rm -rf "${TEST_DATA_DIR}" 2>/dev/null || true
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
    RUNTARA_SHUTDOWN_GRACE_MS="${GRACE_MS}" \
    RUNTARA_AUTO_RECOVER="${RUNTARA_AUTO_RECOVER:-true}" \
    RUNTARA_MAX_AUTO_RESTARTS="${RUNTARA_MAX_AUTO_RESTARTS:-5}" \
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

    for i in {1..60}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        sleep 1
        if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
            print_error "Server exited during boot."; tail -40 "${TEST_LOG}"; exit 1
        fi
    done
    print_error "Server did not become healthy."; tail -40 "${TEST_LOG}"; exit 1
}

echo "==============================================================="
echo "E2E: negative-duration-on-resume  (delay=${DELAY_MS}ms grace=${GRACE_MS}ms)"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN} (cargo build -p runtara-server --bin runtara-server)"; exit 1; }
for f in runtara_agent_utils.wasm runtara_workflow_stdlib.wasm runtara_workflow_runtime.wasm; do
    [ -f "${COMPONENTS_DIR}/${f}" ] || { print_error "Missing component ${COMPONENTS_DIR}/${f} — run scripts/build-agent-components.sh"; exit 1; }
done
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for i in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server (boot 1) on :${TEST_PORT_PUBLIC}..."
start_server
echo "  Server up (PID ${SERVER_PID})"

print_step "Creating workflow (single blocking utils:delay-in-ms ${DELAY_MS}ms -> Finish)..."
RESP=$(api_post /workflows/create '{"name": "negdur-repro", "description": "single blocking delay for negative-duration repro"}')
WF_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${WF_ID}" ] || { print_error "Workflow create failed: ${RESP}"; exit 1; }
DEFINITION=$(jq -n --argjson delay "${DELAY_MS}" '{
  name: "negdur-repro",
  steps: {
    wait: {
      stepType: "Agent", id: "wait",
      agentId: "utils", capabilityId: "delay-in-ms",
      inputMapping: { delay_value: {valueType: "immediate", value: $delay} }
    },
    finish: {
      stepType: "Finish", id: "finish",
      inputMapping: { done: {valueType: "immediate", value: true} }
    }
  },
  entryPoint: "wait",
  executionPlan: [ {fromStep: "wait", toStep: "finish"} ],
  variables: {}, inputSchema: {}, outputSchema: {}
}')
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${DEFINITION}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Workflow update failed: ${RESP}"; exit 1; }
VERSION=$(curl -sS "${API}/workflows/${WF_ID}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')

print_step "Compiling version ${VERSION}..."
RESP=$(api_post "/workflows/${WF_ID}/versions/${VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Compile failed: ${RESP}"; tail -40 "${TEST_LOG}"; exit 1; }

print_step "Executing workflow..."
RESP=$(api_post "/workflows/${WF_ID}/execute" '{"inputs": {"data": {}}}')
INSTANCE_ID=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${INSTANCE_ID}" ] || { print_error "Execute failed: ${RESP}"; exit 1; }
echo "  Instance ${INSTANCE_ID}"

print_step "Waiting until running & mid-delay..."
for i in {1..40}; do
    S=$(instance_status "${INSTANCE_ID}")
    [ "${S}" = "running" ] && break
    case "${S}" in completed|failed) print_error "Instance reached '${S}' before kill window; increase DELAY_MS"; exit 1 ;; esac
    sleep 0.3
done
sleep 3
echo "  pre-kill: status=$(db_field status "${INSTANCE_ID}") finished_at=$(db_field finished_at "${INSTANCE_ID}")"

print_step "SIGTERM server (grace ${GRACE_MS}ms; force-stops the mid-run guest)..."
kill -TERM "${SERVER_PID}" 2>/dev/null || true
wait "${SERVER_PID}" 2>/dev/null || true
SERVER_PID=""
DRAIN_STATUS=$(db_field status "${INSTANCE_ID}")
DRAIN_FINISHED=$(db_field finished_at "${INSTANCE_ID}")
DRAIN_REASON=$(db_field termination_reason "${INSTANCE_ID}")
echo "  post-drain: status=${DRAIN_STATUS} finished_at=${DRAIN_FINISHED} reason=${DRAIN_REASON}"
# Precondition: the drain must have created the poison source (finished_at set).
if [ "${DRAIN_STATUS}" != "suspended" ] || [ "${DRAIN_FINISHED}" = "NULL" ]; then
    print_error "Precondition not met: expected suspended with finished_at stamped (got status=${DRAIN_STATUS}, finished_at=${DRAIN_FINISHED}). Adjust DELAY_MS/GRACE_MS so the guest is force-stopped mid-run."
    exit 1
fi

print_step "Restarting runtara-server (boot 2)..."
start_server
echo "  Server up (PID ${SERVER_PID})"

print_step "Polling across relaunch — asserting no negative duration & finished_at cleared while running..."
NEG_SEEN=0
STALE_FINISHED_WHILE_RUNNING=0
SAW_RUNNING_AFTER_RELAUNCH=0
FINAL=""
for i in {1..60}; do
    S=$(db_field status "${INSTANCE_ID}")
    FIN=$(db_field finished_at "${INSTANCE_ID}")
    DUR=$(db_duration_ms "${INSTANCE_ID}")
    ADUR=$(api_duration "${INSTANCE_ID}")
    printf "  t=%02d status=%-9s finished_at=%s db_dur_ms=%s api_dur=%s\n" "$i" "${S}" "${FIN}" "${DUR}" "${ADUR}"

    if [ "${S}" = "running" ]; then
        SAW_RUNNING_AFTER_RELAUNCH=1
        [ "${FIN}" != "NULL" ] && STALE_FINISHED_WHILE_RUNNING=1
    fi
    if [ "${DUR}" != "NULL" ] && awk "BEGIN{exit !(${DUR} < 0)}"; then NEG_SEEN=1; fi
    if [ "${ADUR}" != "null" ] && awk "BEGIN{exit !(${ADUR} < 0)}" 2>/dev/null; then NEG_SEEN=1; fi

    case "${S}" in completed) FINAL="completed"; break ;; failed) FINAL="failed"; break ;; esac
    sleep 1
done

echo "---------------------------------------------------------------"
echo "RESULT"
echo "  final status                    : ${FINAL:-<timeout>}"
echo "  saw running after relaunch      : ${SAW_RUNNING_AFTER_RELAUNCH}"
echo "  stale finished_at while running : ${STALE_FINISHED_WHILE_RUNNING} (want 0)"
echo "  negative duration seen          : ${NEG_SEEN} (want 0)"
echo "  final row                       : status=$(db_field status "${INSTANCE_ID}") finished_at=$(db_field finished_at "${INSTANCE_ID}") db_dur_ms=$(db_duration_ms "${INSTANCE_ID}")"
echo "---------------------------------------------------------------"

FAIL=0
if [ "${SAW_RUNNING_AFTER_RELAUNCH}" != "1" ]; then
    print_error "Never observed the relaunched instance in 'running' — test window missed it; increase DELAY_MS."
    FAIL=1
fi
if [ "${NEG_SEEN}" != "0" ]; then
    print_error "Negative duration observed after relaunch (the resume-duration bug)."
    FAIL=1
fi
if [ "${STALE_FINISHED_WHILE_RUNNING}" != "0" ]; then
    print_error "A running row still carried a stale finished_at (root fix not applied)."
    FAIL=1
fi
if [ "${FINAL}" != "completed" ]; then
    print_error "Instance did not complete (status='$(db_field status "${INSTANCE_ID}")'); recovery/relaunch failed."
    tail -60 "${TEST_LOG}"
    FAIL=1
fi
[ "${FAIL}" = "0" ] || exit 1

print_success "Resumed run cleared finished_at and never reported a negative duration."
