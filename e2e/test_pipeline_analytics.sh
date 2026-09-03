#!/bin/bash
# E2E Test: the execution pipeline must be observable while it is running.
#
# The System analytics page could describe the host — cores, memory, disk — but
# not how much of it was in use. That gap let a stalled runner hold every run
# permit for forty-eight minutes with nothing on screen changing, and it is the
# gap this closes.
#
# This exercises the whole chain no unit test can reach: the admission gate's
# counters -> the sampler's tick -> the broadcast -> the HTTP snapshot and the
# SSE stream. In particular it covers the one piece deliberately left untested
# in unit tests, because faking the global config to reach it costs more than
# it proves: that the gate actually calls the counters at all.
#
# Asserts:
#   * the snapshot endpoint reports all seven stages, in pipeline order
#   * every bound is a real knob name an operator can act on
#   * the run-permit stage carries a bound read from the live runner
#   * driving executions moves offered/accepted, proving the gate is wired
#   * offered == accepted + denied holds across real traffic
#   * an unbounded stage reports no limit rather than a fabricated one
#   * unmeasured steps serialise as null, never as a stalled-looking zero
#   * a workflow compiled WITH tracking makes steps a real, non-null measurement
#   * the SSE stream opens with a snapshot immediately, not after a tick
#   * the stream keeps delivering, and its window is a real elapsed time
#   * a second concurrent subscriber does not disturb the first
#   * the server's stuck-stage policy reaches both polling and stream snapshots
#
# On UNPATCHED code this FAILS at the first assertion: there is no
# /analytics/pipeline route at all.
#
# Prereqs: Postgres (POSTGRES_* env or the PG_CONTAINER docker container),
# docker (isolated Valkey), jq, and a built runtara-server.

set -uo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"
PG_CONTAINER="${PG_CONTAINER:-runtara-dev-postgres}"

TEST_DB_SERVER="pipeline_e2e_server_$$"
TEST_DB_RUNTIME="pipeline_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17780}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17781}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18781}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18782}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18783}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18784}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16398}"
TEST_DATA_DIR="$(mktemp -d -t runtara_pipeline_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="pipeline_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

FAILURES=0

if command -v psql >/dev/null 2>&1 && PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA -d postgres -c "SELECT 1" >/dev/null 2>&1; then
    psql_quiet() { PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"; }
elif docker exec "${PG_CONTAINER}" true >/dev/null 2>&1; then
    print_warn "host psql unusable — using docker exec ${PG_CONTAINER}"
    psql_quiet() { docker exec -e PGPASSWORD="${POSTGRES_PASSWORD}" "${PG_CONTAINER}" psql -U "${POSTGRES_USER}" -tA "$@"; }
else
    print_error "no psql available (host psql or ${PG_CONTAINER} container required)"; exit 1
fi

expect_eq() {
    local label="$1" expected="$2" actual="$3"
    if [ "${expected}" = "${actual}" ]; then
        echo "  ✓ ${label}: ${actual}"
    else
        print_error "${label}: expected [${expected}], got [${actual}]"
        FAILURES=$((FAILURES + 1))
    fi
}

expect_true() {
    local label="$1" actual="$2"
    if [ "${actual}" = "true" ]; then
        echo "  ✓ ${label}"
    else
        print_error "${label}: expected true, got [${actual}]"
        FAILURES=$((FAILURES + 1))
    fi
}

pipeline() { curl -sS "${API}/analytics/pipeline"; }

api_post() {
    local path="$1" body="$2" timeout="${3:-60}"
    curl -sS --max-time "${timeout}" -X POST "${API}${path}" \
        -H 'Content-Type: application/json' -d "${body}"
}

# Create + compile a trivial workflow, echo its id.
#
# A real one is required rather than a probe against a made-up id: `queue()`
# resolves the workflow first and returns NotFound *before* it reaches the
# admission gate, so requests for a workflow that does not exist never touch
# the counters. A test built on those would assert `offered == accepted +
# denied` and pass vacuously at 0 == 0 + 0, proving nothing about the wiring
# it exists to check.
make_workflow() {
    local name="$1" track="${2:-false}" resp wf_id definition version
    resp=$(api_post /workflows/create "{\"name\": \"${name}\", \"description\": \"pipeline analytics harness\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || { print_error "Workflow create failed: ${resp}"; exit 1; }

    definition=$(jq -n '{
      name: "pipeline-harness",
      durable: false,
      entryPoint: "finish",
      steps: {
        finish: { stepType: "Finish", id: "finish",
                  inputMapping: { ok: { valueType: "immediate", value: true } } }
      },
      executionPlan: [],
      variables: {}, inputSchema: {}, outputSchema: {}
    }')
    resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${definition}, \"trackEvents\": ${track}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Update failed: ${resp}"; exit 1; }
    # `versionNumber` is the field the API returns; `.version` is always null,
    # and jq's `//` treats that as falsy only per-element — so a list of nulls
    # yields a null max and silently falls back to 1. That compiles version 1
    # of a workflow whose steps live in version 2, which fails as "no steps
    # defined" nowhere near the mistake.
    version=$(curl -sS "${API}/workflows/${wf_id}/versions" | jq -r '[.data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf_id}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Compile failed: ${resp}"; exit 1; }
    echo "${wf_id}"
}

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
    DATA_DIR="${TEST_DATA_DIR}" \
    RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
    RUNTARA_DEV_MODE=false \
    RUST_LOG="${RUST_LOG_OVERRIDE:-warn,runtara_server=info}" \
    AUTH_PROVIDER=local \
    SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2 \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    RUNTARA_PIPELINE_STUCK_AFTER_SECS=7 \
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
echo "E2E: execution pipeline analytics"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN} (cargo build -p runtara-server --bin runtara-server)"; exit 1; }
command -v jq >/dev/null 2>&1 || { print_error "jq required"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }
[ -d "${COMPONENTS_DIR}" ] || { print_error "Missing components dir ${COMPONENTS_DIR} — run scripts/build-agent-components.sh"; exit 1; }

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for i in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
start_server
echo "  Server up (PID ${SERVER_PID})"

# The sampler has to have ticked at least twice before rates exist: the first
# tick has no earlier reading to difference against.
print_step "Waiting for the sampler's first snapshots..."
SNAPSHOT=""
for i in {1..30}; do
    SNAPSHOT="$(pipeline)"
    if [ "$(echo "${SNAPSHOT}" | jq -r '.success // false')" = "true" ]; then break; fi
    sleep 1
done
if [ "$(echo "${SNAPSHOT}" | jq -r '.success // false')" != "true" ]; then
    print_error "Sampler never produced a snapshot"; echo "${SNAPSHOT}"; tail -40 "${TEST_LOG}"; exit 1
fi
echo "  First snapshot after ~${i}s"

print_step "1. The snapshot is the pipeline, in order"
expect_eq "stage count" "7" "$(echo "${SNAPSHOT}" | jq -r '.data.stages | length')"
expect_eq "stage keys in pipeline order" \
  "admission triggerQueue triggerWorkers pendingStarts runPermits executing parked" \
  "$(echo "${SNAPSHOT}" | jq -r '[.data.stages[].key] | join(" ")')"
expect_eq "server stuck policy is carried in milliseconds" "7000" \
  "$(echo "${SNAPSHOT}" | jq -r '.data.stuckAfterMs')"

print_step "2. Every bound names a knob an operator can act on"
expect_eq "admission knob" "MAX_CONCURRENT_EXECUTIONS" \
  "$(echo "${SNAPSHOT}" | jq -r '.data.stages[] | select(.key=="admission") | .knob')"
expect_eq "trigger worker knob" "RUNTARA_TRIGGER_CONCURRENCY" \
  "$(echo "${SNAPSHOT}" | jq -r '.data.stages[] | select(.key=="triggerWorkers") | .knob')"
expect_eq "run permit knob" "RUNTARA_MAX_CONCURRENT_RUNS" \
  "$(echo "${SNAPSHOT}" | jq -r '.data.stages[] | select(.key=="runPermits") | .knob')"

print_step "3. The run-permit bound comes from the live runner"
# Not a constant echoed back: the runner reports the semaphore it actually
# waits on, so this is the number that will refuse the next launch.
RUN_LIMIT="$(echo "${SNAPSHOT}" | jq -r '.data.stages[] | select(.key=="runPermits") | .limit')"
expect_true "run permit limit is a positive number" \
  "$(echo "${SNAPSHOT}" | jq -r '(.data.stages[] | select(.key=="runPermits") | .limit) > 0')"
echo "    runner reports a bound of ${RUN_LIMIT}"
expect_true "run permit occupancy is readable" \
  "$(echo "${SNAPSHOT}" | jq -r '(.data.stages[] | select(.key=="runPermits") | .used) != null')"

print_step "4. Unbounded stages report no limit rather than inventing one"
for key in triggerQueue executing parked; do
    expect_eq "${key} has no ceiling" "null" \
      "$(echo "${SNAPSHOT}" | jq -r ".data.stages[] | select(.key==\"${key}\") | .limit")"
done

print_step "5. Driving real executions moves the gate's counters"
# The piece no unit test here reaches: that check_concurrency_gate actually
# calls the counters on the live path. A made-up workflow id would be rejected
# before the gate, so this uses a real compiled one.
WF_ID="$(make_workflow "pipeline-probe")"
echo "  workflow ${WF_ID} compiled"

# Drive continuously while sampling, so at least one window contains traffic.
# A single burst can land entirely between two ticks and read as zero.
(
  for _ in $(seq 1 60); do
      curl -sS -o /dev/null --max-time 5 -X POST "${API}/workflows/${WF_ID}/execute" \
        -H 'Content-Type: application/json' -d '{"inputs": {"data": {}}}' 2>/dev/null || true
  done
) &
DRIVER_PID=$!

SAW_OFFERED=0
SAW_ACCEPTED=0
IDENTITY_HELD=1
for _ in $(seq 1 12); do
    sleep 1
    SNAP="$(pipeline)"
    [ "$(echo "${SNAP}" | jq -r '.data.rates != null')" = "true" ] || continue
    OFFERED="$(echo "${SNAP}" | jq -r '.data.rates.offered')"
    ACCEPTED="$(echo "${SNAP}" | jq -r '.data.rates.accepted')"
    [ "$(echo "${SNAP}" | jq -r '.data.rates.offered > 0')" = "true" ] && SAW_OFFERED=1
    [ "$(echo "${SNAP}" | jq -r '.data.rates.accepted > 0')" = "true" ] && SAW_ACCEPTED=1
    # Check the identity on every sampled window, not just a lucky one.
    if [ "$(echo "${SNAP}" | jq -r '.data.rates as $r | (($r.accepted + $r.denied) - $r.offered | fabs) < 0.001')" != "true" ]; then
        IDENTITY_HELD=0
        print_error "identity broke: offered=${OFFERED} accepted=${ACCEPTED} denied=$(echo "${SNAP}" | jq -r '.data.rates.denied')"
    fi
    [ "${SAW_OFFERED}" = "1" ] && [ "${SAW_ACCEPTED}" = "1" ] && break
done
wait "${DRIVER_PID}" 2>/dev/null || true

expect_eq "the gate counted offers while traffic ran" "1" "${SAW_OFFERED}"
expect_eq "the gate counted admissions while traffic ran" "1" "${SAW_ACCEPTED}"

AFTER="$(pipeline)"
expect_true "the snapshot carries rates" "$(echo "${AFTER}" | jq -r '.data.rates != null')"
expect_true "the rate window is a real elapsed time, not the nominal interval" \
  "$(echo "${AFTER}" | jq -r '.data.windowMs > 0')"

print_step "6. offered == accepted + denied on every sampled window"
# The identity a viewer relies on to show refusals as a share of demand. If it
# drifts, the refusal rate renders above 100% or below zero.
expect_eq "held across every window sampled under load" "1" "${IDENTITY_HELD}"

print_step "7. Unmeasured steps are null, never a stalled-looking zero"
# trackEvents is compile-time. Nothing here was compiled with it, so the
# honest answer is "not measured" — and rendering that as 0/s would let a
# reader conclude a healthy deployment had stopped dead.
expect_eq "steps report as unmeasured" "null" \
  "$(echo "${AFTER}" | jq -r '.data.rates.steps')"
expect_true "the other rates are still fully reported" \
  "$(echo "${AFTER}" | jq -r '.data.rates | (.offered != null) and (.accepted != null) and (.denied != null)')"

print_step "7b. A tracked workflow turns steps into a real measurement"
# The counterpart to step 7, and the reason it matters. Absent steps must be
# absent because nothing could report one — not because nothing is wired.
# `record_step` shipped once with no caller at all, so this asserts the whole
# chain: guest -> host event seam -> observer -> counter -> sampler -> wire.
WF_TRACKED="$(make_workflow "pipeline-probe-tracked" true)"
echo "  tracked workflow ${WF_TRACKED} compiled"

(
  for _ in $(seq 1 40); do
      curl -sS -o /dev/null --max-time 5 -X POST "${API}/workflows/${WF_TRACKED}/execute" \
        -H 'Content-Type: application/json' -d '{"inputs": {"data": {}}}' 2>/dev/null || true
  done
) &
TRACKED_PID=$!

SAW_STEPS=0
for _ in $(seq 1 15); do
    sleep 1
    SNAP="$(pipeline)"
    [ "$(echo "${SNAP}" | jq -r '.data.rates != null')" = "true" ] || continue
    if [ "$(echo "${SNAP}" | jq -r '.data.rates.steps != null')" = "true" ]; then
        SAW_STEPS=1
        echo "    steps now measured: $(echo "${SNAP}" | jq -r '.data.rates.steps')/s"
        break
    fi
done
wait "${TRACKED_PID}" 2>/dev/null || true

expect_eq "steps become a measurement once something tracked has run" "1" "${SAW_STEPS}"

print_step "8. The stream opens with a snapshot immediately"
# A freshly loaded page must not sit blank waiting for the next tick.
STREAM_OUT="${TEST_DATA_DIR}/stream.txt"
curl -sS --max-time 4 -H 'Accept: text/event-stream' \
  "${API}/analytics/pipeline/stream" > "${STREAM_OUT}" 2>/dev/null || true

FIRST_FRAME="$(grep -m1 '^data:' "${STREAM_OUT}" | sed 's/^data: *//')"
expect_true "the first frame arrives and is a snapshot" \
  "$(echo "${FIRST_FRAME}" | jq -r '(.stages | length) == 7 and .stuckAfterMs == 7000' 2>/dev/null || echo false)"

print_step "9. The stream keeps delivering"
FRAME_COUNT="$(grep -c '^data:' "${STREAM_OUT}" || true)"
if [ "${FRAME_COUNT}" -ge 2 ]; then
    echo "  ✓ received ${FRAME_COUNT} frames in ~4s at a 1s cadence"
else
    print_error "expected at least 2 frames in 4 seconds, got ${FRAME_COUNT}"
    FAILURES=$((FAILURES + 1))
fi

print_step "10. A second subscriber does not disturb the first"
# One tick feeds every viewer from a broadcast, so a second reader must cost
# nothing and interfere with nothing.
A_OUT="${TEST_DATA_DIR}/sub_a.txt"; B_OUT="${TEST_DATA_DIR}/sub_b.txt"
curl -sS --max-time 4 -H 'Accept: text/event-stream' "${API}/analytics/pipeline/stream" > "${A_OUT}" 2>/dev/null &
A_PID=$!
curl -sS --max-time 4 -H 'Accept: text/event-stream' "${API}/analytics/pipeline/stream" > "${B_OUT}" 2>/dev/null &
B_PID=$!
wait "${A_PID}" 2>/dev/null || true
wait "${B_PID}" 2>/dev/null || true

A_FRAMES="$(grep -c '^data:' "${A_OUT}" || true)"
B_FRAMES="$(grep -c '^data:' "${B_OUT}" || true)"
if [ "${A_FRAMES}" -ge 2 ] && [ "${B_FRAMES}" -ge 2 ]; then
    echo "  ✓ both subscribers fed (${A_FRAMES} and ${B_FRAMES} frames)"
else
    print_error "concurrent subscribers starved: ${A_FRAMES} and ${B_FRAMES} frames"
    FAILURES=$((FAILURES + 1))
fi

print_step "11. The server is still healthy after all of it"
expect_eq "health" "200" \
  "$(curl -sS -o /dev/null -w '%{http_code}' "http://127.0.0.1:${TEST_PORT_PUBLIC}/health")"

echo
echo "==============================================================="
if [ "${FAILURES}" -eq 0 ]; then
    print_success "All pipeline analytics assertions passed"
    exit 0
fi
print_error "${FAILURES} assertion(s) failed"
echo "Server log: ${TEST_LOG}"
tail -30 "${TEST_LOG}"
exit 1
