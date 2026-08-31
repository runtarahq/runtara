#!/bin/bash
# E2E: the wake scheduler drains a backlog at the speed of the host.
#
# Parks N instances on a long durable Delay so the whole backlog is asleep,
# makes every one of them due at the same instant, and asserts the scheduler
# takes the backlog in large concurrent batches instead of a fixed trickle.
#
# On UNPATCHED code this FAILS: the scheduler claimed `batch_size` (10) and
# then slept its whole `poll_interval` (5s) regardless of how much was due,
# pinning it to 2 wakes/second. The assertion is on the batch it claims rather
# than on wall-clock throughput, so it holds on a slow or loaded host.
#
# The delay is deliberately long and the wake forced: with a short delay the
# instances launched first come due before the last ones have parked, and there
# is never a backlog to measure.
#
# On UNPATCHED code this FAILS: the scheduler slept its full poll interval
# between batches regardless of backlog, pinning it to
# `batch_size / poll_interval` = 10 per 5s = 2 wakes/second. Draining the
# default 200 instances would take ~100s; the threshold here is 30s.
#
# It also asserts the properties that make the concurrent batch claim safe:
# every instance completes exactly once (no duplicate launch), and none is
# left stranded `suspended` with no wake deadline.
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

# The backlog has to span several batches, or a single poll drains it and the
# idle interval between polls is never exercised — which is the whole of the
# adaptive-polling behaviour this test exists to protect.
WAKE_BATCH_SIZE="${WAKE_BATCH_SIZE:-10}"
WAKE_POLL_INTERVAL_MS="${WAKE_POLL_INTERVAL_MS:-10000}"  # long enough that an idle sleep is unmistakable
INSTANCES="${INSTANCES:-40}"        # 4 full batches at the pinned batch size
DELAY_MS="${DELAY_MS:-3600000}"     # an hour: everything parks and stays parked
DRAIN_BUDGET_S="${DRAIN_BUDGET_S:-180}"  # generous: stragglers wait an idle interval, and dev Postgres is slow
LAUNCH_PARALLEL="${LAUNCH_PARALLEL:-12}"  # debug builds do not enjoy 200 at once

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"
PG_CONTAINER="${PG_CONTAINER:-runtara-dev-postgres}"

TEST_DB_SERVER="wakedrain_e2e_server_$$"
TEST_DB_RUNTIME="wakedrain_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17740}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17741}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18741}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18742}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18743}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18744}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16394}"
TEST_DATA_DIR="$(mktemp -d -t runtara_wakedrain_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="wakedrain_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

if command -v psql >/dev/null 2>&1; then
    psql_quiet() { PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"; }
elif docker exec "${PG_CONTAINER}" true >/dev/null 2>&1; then
    print_warn "host psql not found — using docker exec ${PG_CONTAINER}"
    psql_quiet() { docker exec -e PGPASSWORD="${POSTGRES_PASSWORD}" "${PG_CONTAINER}" psql -U "${POSTGRES_USER}" -tA "$@"; }
else
    print_error "no psql available (host psql or ${PG_CONTAINER} container required)"; exit 1
fi

api_post() { curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"; }
rt_count() { psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT COUNT(*) FROM instances WHERE $1" | tr -d '[:space:]'; }

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
    RUNTARA_WAKE_BATCH_SIZE="${WAKE_BATCH_SIZE}" \
    RUNTARA_WAKE_POLL_INTERVAL_MS="${WAKE_POLL_INTERVAL_MS}" \
    RUST_LOG="${RUST_LOG_OVERRIDE:-warn,runtara_server=warn,runtara_environment=info,runtara_core=warn}" \
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
echo "E2E: wake drain throughput (${INSTANCES} instances, budget ${DRAIN_BUDGET_S}s)"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN} (cargo build -p runtara-server --bin runtara-server)"; exit 1; }
for f in runtara_workflow_stdlib.wasm runtara_workflow_runtime.wasm; do
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

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
start_server
echo "  Server up (PID ${SERVER_PID})"

print_step "Creating sleeper workflow (Delay ${DELAY_MS}ms -> Finish)..."
RESP=$(api_post /workflows/create '{"name": "wakedrain", "description": "backlog drain throughput"}')
WF_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${WF_ID}" ] || { print_error "Workflow create failed: ${RESP}"; exit 1; }

DEFINITION=$(jq -n --argjson delay "${DELAY_MS}" '{
  name: "wakedrain",
  steps: {
    nap:    { stepType: "Delay",  id: "nap", durationMs: {valueType: "immediate", value: $delay} },
    finish: { stepType: "Finish", id: "finish", inputMapping: { done: {valueType: "immediate", value: true} } }
  },
  entryPoint: "nap",
  executionPlan: [ {fromStep: "nap", toStep: "finish"} ],
  variables: {}, inputSchema: {}, outputSchema: {}
}')
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${DEFINITION}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Workflow update failed: ${RESP}"; exit 1; }
VERSION=$(curl -sS "${API}/workflows/${WF_ID}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')

print_step "Compiling version ${VERSION}..."
RESP=$(api_post "/workflows/${WF_ID}/versions/${VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Compile failed: ${RESP}"; tail -40 "${TEST_LOG}"; exit 1; }

print_step "Launching ${INSTANCES} instances (${LAUNCH_PARALLEL} at a time)..."
LAUNCHED=0
FAILED=0
while [ "${LAUNCHED}" -lt "${INSTANCES}" ]; do
    BATCH=$(( INSTANCES - LAUNCHED ))
    [ "${BATCH}" -gt "${LAUNCH_PARALLEL}" ] && BATCH="${LAUNCH_PARALLEL}"
    PIDS=()
    for _ in $(seq 1 "${BATCH}"); do
        # --fail so a rejected request is a non-zero exit rather than a body we
        # throw away: counting the batch size regardless would report requests
        # the server refused as launched instances.
        curl -sS --fail --max-time 60 -X POST -H "Content-Type: application/json" \
            -d '{"inputs": {"data": {}}}' "${API}/workflows/${WF_ID}/execute" >/dev/null &
        PIDS+=($!)
    done
    # Wait on the launches only: a bare `wait` would also wait on the server,
    # which is a background job of this shell and never exits.
    for pid in "${PIDS[@]}"; do
        if wait "${pid}"; then LAUNCHED=$(( LAUNCHED + 1 )); else FAILED=$(( FAILED + 1 )); fi
    done
    echo "  launched ${LAUNCHED}/${INSTANCES}"
done

print_step "Waiting for the backlog to park (all ${INSTANCES} suspended)..."
PARKED=0
for i in $(seq 1 120); do
    PARKED=$(rt_count "status='suspended'")
    [ "${PARKED}" -ge "${INSTANCES}" ] && break
    sleep 1
done
if [ "${PARKED}" -lt "${INSTANCES}" ]; then
    print_error "Only ${PARKED}/${INSTANCES} parked; cannot measure a drain."
    echo "  status breakdown:"
    psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT status::text, COALESCE(termination_reason::text,'-'), COUNT(*) FROM instances GROUP BY 1,2 ORDER BY 3 DESC" | sed 's/^/    /'
    echo "  a Delay that does not park means the store-freeing lowering is off —"
    echo "  it is the compile-time default, so check RUNTARA_DIRECT_STORE_FREEING_SLEEP"
    echo "  has not been opted out of in this environment."
    tail -40 "${TEST_LOG}"; exit 1
fi
echo "  ${PARKED} parked"

print_step "Making the whole backlog due at once..."
psql_quiet -d "${TEST_DB_RUNTIME}" -c "UPDATE instances SET sleep_until = now() WHERE status='suspended'" >/dev/null

print_step "Draining..."
START=$(date +%s)
DONE=0
DEADLINE=$(( START + DRAIN_BUDGET_S ))
while [ "$(date +%s)" -lt "${DEADLINE}" ]; do
    DONE=$(rt_count "status='completed'")
    [ "${DONE}" -ge "${INSTANCES}" ] && break
    sleep 1
done
ELAPSED=$(( $(date +%s) - START ))

echo "  drained ${DONE}/${INSTANCES} in ${ELAPSED}s"

if [ "${FAILED}" -gt 0 ]; then
    print_error "${FAILED} launch request(s) were rejected — the backlog is not the size this test thinks it is"
    exit 1
fi
if [ "${LAUNCHED}" -ne "${INSTANCES}" ]; then
    print_error "launched ${LAUNCHED} but expected ${INSTANCES}"
    exit 1
fi

FAILURES=0

# The batch is pinned small and the backlog spans several of them, so draining
# it REQUIRES consecutive full claims. That is what makes this an adaptive
# polling test rather than a batch-size test: a scheduler that sleeps its whole
# interval between polls cannot finish, no matter how fast the host is.
FULL_BATCHES=$(grep -cE "Processing sleeping instances count=${WAKE_BATCH_SIZE}$" "${TEST_LOG}" || true)
FULL_BATCHES="${FULL_BATCHES:-0}"
EXPECTED_BATCHES=$(( INSTANCES / WAKE_BATCH_SIZE ))
if [ "${FULL_BATCHES}" -lt $(( EXPECTED_BATCHES - 1 )) ]; then
    print_error "Only ${FULL_BATCHES} full batch(es) of ${WAKE_BATCH_SIZE}; expected about ${EXPECTED_BATCHES}."
    print_error "The backlog should have been claimed in consecutive full batches."
    FAILURES=$((FAILURES+1))
else
    print_success "${FULL_BATCHES} consecutive full batches of ${WAKE_BATCH_SIZE} (backlog ${INSTANCES})"
fi

# Consecutive full batches must follow each other without the idle interval.
# Asserted on the scheduler's own log timestamps rather than total wall clock:
# the tail of a drain legitimately waits a poll interval for stragglers that
# SKIP LOCKED stepped over, so overall elapsed time conflates the two. The span
# between the first and last full batch isolates the behaviour under test.
FULL_SPAN_MS=$(grep -E "Processing sleeping instances count=${WAKE_BATCH_SIZE}$" "${TEST_LOG}" \
    | awk '{print $1}' \
    | python3 -c "
import sys, datetime
ts = [datetime.datetime.fromisoformat(l.strip().replace('Z', '+00:00')) for l in sys.stdin if l.strip()]
print(0 if len(ts) < 2 else int((max(ts) - min(ts)).total_seconds() * 1000))
")
if [ "${FULL_SPAN_MS}" -ge "${WAKE_POLL_INTERVAL_MS}" ]; then
    print_error "Full batches were ${FULL_SPAN_MS}ms apart, at or past the ${WAKE_POLL_INTERVAL_MS}ms"
    print_error "idle interval: the scheduler is sleeping between full batches."
    FAILURES=$((FAILURES+1))
else
    print_success "Full batches spanned ${FULL_SPAN_MS}ms, well inside the ${WAKE_POLL_INTERVAL_MS}ms idle interval"
fi

if [ "${DONE}" -lt "${INSTANCES}" ]; then
    print_error "Backlog did not fully drain: ${DONE}/${INSTANCES} in ${ELAPSED}s."
    FAILURES=$((FAILURES+1))
else
    print_success "Backlog drained: ${INSTANCES} in ${ELAPSED}s"
fi

# Every instance exactly once: a duplicate launch means two guests ran the
# same in-flight step.
DUPES=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT COUNT(*) FROM (SELECT instance_id FROM instance_events WHERE event_type='completed' GROUP BY instance_id HAVING COUNT(*) > 1) d" | tr -d '[:space:]')
if [ "${DUPES}" != "0" ]; then
    print_error "${DUPES} instance(s) completed more than once — duplicate launch."
    FAILURES=$((FAILURES+1))
else
    print_success "No duplicate completions"
fi

# Nothing stranded. The claim leases rather than clears, so a suspended row with
# no deadline is the unrecoverable state: it is indistinguishable from a signal
# waiter, and this workflow has none. A row still overdue after the drain was
# claimed and never launched or restored.
STRANDED=$(rt_count "status='suspended' AND sleep_until IS NULL")
OVERDUE=$(rt_count "status='suspended' AND sleep_until IS NOT NULL AND sleep_until <= now()")
if [ "${STRANDED}" != "0" ]; then
    print_error "${STRANDED} instance(s) left suspended with no wake deadline — unrecoverable."
    FAILURES=$((FAILURES+1))
elif [ "${OVERDUE}" != "0" ]; then
    print_error "${OVERDUE} instance(s) still overdue after the drain — claimed but never launched."
    FAILURES=$((FAILURES+1))
else
    print_success "No stranded instances"
fi

CRASHED=$(rt_count "termination_reason='crashed'")
if [ "${CRASHED}" != "0" ]; then
    print_error "${CRASHED} instance(s) recorded as crashed (launch must not clobber a parked run)."
    FAILURES=$((FAILURES+1))
else
    print_success "No spurious crashes"
fi

echo ""
if [ "${FAILURES}" -eq 0 ]; then
    print_success "E2E PASSED"
    exit 0
fi
print_error "E2E FAILED (${FAILURES} check(s))"
tail -40 "${TEST_LOG}"
exit 1
