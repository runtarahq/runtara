#!/bin/bash
# E2E Test: tenant metrics must bucket at the width the caller asks for.
#
# `GET /api/runtime/metrics/tenant` used to offer two bucket widths, hourly and
# daily, because it aligned buckets with `date_trunc`. The console's activity
# map needs minutes, so the aggregation now floors the Unix epoch to an
# arbitrary width and the endpoint accepts one.
#
# This exercises the whole chain the unit and db tests only cover in pieces:
# HTTP handler -> granularity parsing -> bucket-count cap -> runtime client ->
# environment handler -> SQL.
#
# The runtime rows are seeded directly with chosen timestamps, because
# `complete_instance` stamps finished_at with NOW() and a bucketing test cannot
# use a clock it does not control.
#
# Asserts:
#   * omitting granularity still means hourly, unchanged for existing callers
#   * a width in minutes produces minute-aligned buckets, and the right number
#   * the window's totals are identical at every width (spine/aggregate align)
#   * empty buckets are present and zeroed rather than absent
#   * a width that would overrun the bucket cap is a 400 naming both numbers
#   * an unparseable granularity is a 400, not a silent fallback to hourly
#   * the response echoes the width it actually used
#
# On UNPATCHED code this FAILS: `granularity=6m` falls through the handler's
# `_ =>` arm to hourly and the response reports "hourly".
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

TEST_DB_SERVER="metricsgran_e2e_server_$$"
TEST_DB_RUNTIME="metricsgran_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17760}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17761}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18761}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18762}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18763}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18764}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16396}"
TEST_DATA_DIR="$(mktemp -d -t runtara_metricsgran_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="metricsgran_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
# The server refuses to boot without runnable agent components. Nothing here
# executes a workflow, so they only have to be present.
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

# The window every assertion is made over: a fixed, epoch-aligned hour well in
# the past, so bucket boundaries are arithmetic rather than clock luck.
WINDOW_START="2026-01-05T00:00:00Z"
WINDOW_END="2026-01-05T01:00:00Z"

metrics() { curl -sS "${API}/metrics/tenant?startTime=${WINDOW_START}&endTime=${WINDOW_END}$1"; }
metrics_code() { curl -sS -o /dev/null -w '%{http_code}' "${API}/metrics/tenant?startTime=$2&endTime=${WINDOW_END}$1"; }

expect_eq() {
    local label="$1" expected="$2" actual="$3"
    if [ "${expected}" = "${actual}" ]; then
        echo "  ✓ ${label}: ${actual}"
    else
        print_error "${label}: expected [${expected}], got [${actual}]"
        FAILURES=$((FAILURES + 1))
    fi
}

expect_contains() {
    local label="$1" needle="$2" haystack="$3"
    if [[ "${haystack}" == *"${needle}"* ]]; then
        echo "  ✓ ${label}"
    else
        print_error "${label}: [${needle}] not found in [${haystack}]"
        FAILURES=$((FAILURES + 1))
    fi
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
echo "E2E: tenant metrics bucket granularity"
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

# Four runs inside the window, placed so that minute buckets separate them but
# a single hourly bucket does not: 00:00, 00:02, 00:02 and 00:59.
print_step "Seeding runs at chosen offsets inside the window..."
psql_quiet -d "${TEST_DB_RUNTIME}" -c "
    INSERT INTO instances (instance_id, tenant_id, status, started_at, finished_at, memory_peak_bytes) VALUES
      ('mg-a', '${TENANT}', 'completed'::instance_status, '${WINDOW_START}'::timestamptz,                        '${WINDOW_START}'::timestamptz + interval '10 seconds',   1048576),
      ('mg-b', '${TENANT}', 'completed'::instance_status, '${WINDOW_START}'::timestamptz + interval '100 sec',   '${WINDOW_START}'::timestamptz + interval '130 seconds',  2097152),
      ('mg-c', '${TENANT}', 'failed'::instance_status,    '${WINDOW_START}'::timestamptz + interval '120 sec',   '${WINDOW_START}'::timestamptz + interval '150 seconds',  NULL),
      ('mg-d', '${TENANT}', 'cancelled'::instance_status, '${WINDOW_START}'::timestamptz + interval '3500 sec',  '${WINDOW_START}'::timestamptz + interval '3580 seconds', NULL),
      ('mg-live', '${TENANT}', 'running'::instance_status, '${WINDOW_START}'::timestamptz, NULL, NULL);
" >/dev/null
echo "  4 terminal runs + 1 still running"

print_step "1. Omitting granularity still means hourly"
BODY="$(metrics '')"
expect_eq "granularity echoed" "hourly" "$(echo "${BODY}" | jq -r '.data.granularity')"
expect_eq "hourly bucket count" "2" "$(echo "${BODY}" | jq -r '.data.metrics | length')"

print_step "2. A minute width buckets by the minute"
BODY="$(metrics '&granularity=1m')"
expect_eq "granularity echoed" "1m" "$(echo "${BODY}" | jq -r '.data.granularity')"
expect_eq "bucket count over one hour" "61" "$(echo "${BODY}" | jq -r '.data.metrics | length')"
expect_eq "every bucket starts on a whole minute" "true" \
  "$(echo "${BODY}" | jq -r '[.data.metrics[].bucket_time | fromdateiso8601 % 60] | all(. == 0)')"
expect_eq "minute 0 holds one success" "1" "$(echo "${BODY}" | jq -r '.data.metrics[0].success_count')"
expect_eq "minute 2 holds two runs" "2" "$(echo "${BODY}" | jq -r '.data.metrics[2].invocation_count')"
expect_eq "minute 2 holds the failure" "1" "$(echo "${BODY}" | jq -r '.data.metrics[2].failure_count')"
expect_eq "minute 59 holds the cancellation" "1" "$(echo "${BODY}" | jq -r '.data.metrics[59].cancelled_count')"
expect_eq "empty buckets are present and zeroed" "0" "$(echo "${BODY}" | jq -r '.data.metrics[30].invocation_count')"
expect_eq "empty buckets claim no duration" "null" "$(echo "${BODY}" | jq -r '.data.metrics[30].avg_duration_seconds')"

print_step "3. A six-minute width, the console's 24h setting"
BODY="$(metrics '&granularity=6m')"
expect_eq "granularity echoed" "6m" "$(echo "${BODY}" | jq -r '.data.granularity')"
expect_eq "bucket count" "11" "$(echo "${BODY}" | jq -r '.data.metrics | length')"
expect_eq "every bucket starts on a six-minute boundary" "true" \
  "$(echo "${BODY}" | jq -r '[.data.metrics[].bucket_time | fromdateiso8601 % 360] | all(. == 0)')"

print_step "4. Totals do not change with the width"
for WIDTH in 1m 2m 6m 30m hourly daily; do
  TOTAL="$(metrics "&granularity=${WIDTH}" | jq -r '[.data.metrics[].invocation_count] | add')"
  expect_eq "total at ${WIDTH}" "4" "${TOTAL}"
done
# The live run has no finished_at, so it is invisible to the aggregation.
expect_eq "the running instance is not counted" "4" \
  "$(metrics '&granularity=1m' | jq -r '[.data.metrics[].invocation_count] | add')"

print_step "5. A width that would overrun the bucket cap is refused"
CODE="$(metrics_code '&granularity=1s' '2025-10-07T00:00:00Z')"
expect_eq "HTTP status for 1s over 90 days" "400" "${CODE}"
MSG="$(curl -sS "${API}/metrics/tenant?startTime=2025-10-07T00:00:00Z&endTime=${WINDOW_END}&granularity=1s" | jq -r '.message')"
expect_contains "error names the cap" "1000" "${MSG}"
expect_contains "error names the requested width" "1s" "${MSG}"

print_step "6. An unparseable granularity is a 400, not a silent fallback"
expect_eq "HTTP status for 'fortnightly'" "400" "$(metrics_code '&granularity=fortnightly' "${WINDOW_START}")"
expect_eq "HTTP status for '0m'" "400" "$(metrics_code '&granularity=0m' "${WINDOW_START}")"
expect_contains "error suggests the accepted forms" "hourly" \
  "$(metrics '&granularity=fortnightly' | jq -r '.message')"

print_step "7. The named granularities are unchanged"
expect_eq "daily echoed" "daily" "$(metrics '&granularity=daily' | jq -r '.data.granularity')"
expect_eq "hourly echoed" "hourly" "$(metrics '&granularity=hourly' | jq -r '.data.granularity')"
expect_eq "daily buckets sit on UTC midnight" "true" \
  "$(metrics '&granularity=daily' | jq -r '[.data.metrics[].bucket_time | fromdateiso8601 % 86400] | all(. == 0)')"

echo
echo "==============================================================="
if [ "${FAILURES}" -eq 0 ]; then
    print_success "All assertions passed"
    exit 0
else
    print_error "${FAILURES} assertion(s) failed"
    exit 1
fi
