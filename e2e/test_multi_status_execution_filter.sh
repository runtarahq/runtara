#!/bin/bash
# E2E Test: a multi-status execution filter must apply every status it is given.
#
# `GET /api/runtime/executions?status=failed,cancelled` documents a
# comma-separated list, and the handler validates every entry — but the runtime
# query used to be built from the first entry alone, so the rest were dropped
# with no error. The caller got a page (and a totalElements count) narrower than
# what it asked for, presented as if it were complete.
#
# This exercises the whole chain the unit tests only cover in pieces: HTTP
# handler -> execution engine -> management SDK -> environment HTTP -> SQL.
#
# The runtime rows are seeded directly, one per status, so the assertions are
# about the filter and not about how a run happens to end.
#
# Asserts:
#   * status=failed,cancelled returns exactly the failed + cancelled runs
#   * totalElements agrees with the page (both come from the same filter)
#   * failed,timeout collapses onto one runtime status without double counting
#   * a single status still filters to just that status
#
# On UNPATCHED code this FAILS: only the first status is applied, so
# failed,cancelled comes back with the failed run alone.
#
# Prereqs: Postgres reachable on ${POSTGRES_HOST}:${POSTGRES_PORT} (host `psql`,
# or a `runtara-dev-postgres` docker container as fallback), docker (isolated
# Valkey), and a built runtara-server (cargo build -p runtara-server).

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

TEST_DB_SERVER="multistatus_e2e_server_$$"
TEST_DB_RUNTIME="multistatus_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17740}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17741}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18741}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18742}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18743}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18744}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16394}"
TEST_DATA_DIR="$(mktemp -d -t runtara_multistatus_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="multistatus_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
# The server refuses to boot without a components directory. Nothing here
# executes a workflow, so the components only have to be present.
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

FAILURES=0

# psql shim: prefer host psql, fall back to the dev postgres container.
if command -v psql >/dev/null 2>&1; then
    psql_quiet() { PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"; }
elif docker exec "${PG_CONTAINER}" true >/dev/null 2>&1; then
    print_warn "host psql not found — using docker exec ${PG_CONTAINER}"
    psql_quiet() { docker exec -e PGPASSWORD="${POSTGRES_PASSWORD}" "${PG_CONTAINER}" psql -U "${POSTGRES_USER}" -tA "$@"; }
else
    print_error "no psql available (host psql or ${PG_CONTAINER} container required)"; exit 1
fi

# Statuses returned by GET /executions?status=<filter>, one per line, sorted.
api_statuses() { curl -sS "${API}/executions?size=100&status=$1" | jq -r '.data.content[].status' | sort; }
api_total()    { curl -sS "${API}/executions?size=100&status=$1" | jq -r '.data.totalElements'; }
api_http_code() { curl -sS -o /dev/null -w '%{http_code}' "${API}/executions?size=100&status=$1"; }

expect_eq() {
    local label="$1" expected="$2" actual="$3"
    if [ "${expected}" = "${actual}" ]; then
        echo "  ✓ ${label}: ${actual}"
    else
        print_error "${label}: expected [${expected}], got [${actual}]"
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

# One instance per status, all under the tenant the API reads.
seed_instance() {
    local instance_id="$1" status="$2"
    psql_quiet -d "${TEST_DB_RUNTIME}" -c "
        INSERT INTO instances (instance_id, tenant_id, status, started_at, finished_at)
        VALUES ('${instance_id}', '${TENANT}', '${status}'::instance_status, now(), now());
        INSERT INTO instance_images (instance_id, image_id, tenant_id)
        VALUES ('${instance_id}', '${IMAGE_ID}', '${TENANT}');
    " >/dev/null
}

echo "==============================================================="
echo "E2E: multi-status execution filter"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN} (cargo build -p runtara-server --bin runtara-server)"; exit 1; }
command -v jq >/dev/null 2>&1 || { print_error "jq required"; exit 1; }
[ -d "${COMPONENTS_DIR}" ] || { print_error "Missing components dir ${COMPONENTS_DIR} — run scripts/build-agent-components.sh"; exit 1; }
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

print_step "Seeding one instance per status..."
IMAGE_ID="multistatus-image"
psql_quiet -d "${TEST_DB_RUNTIME}" -c "
    INSERT INTO images (image_id, tenant_id, name, binary_path)
    VALUES ('${IMAGE_ID}', '${TENANT}', 'multistatus:1', '/dev/null');
" >/dev/null
seed_instance "multistatus-failed"    failed
seed_instance "multistatus-cancelled" cancelled
seed_instance "multistatus-completed" completed
seed_instance "multistatus-running"   running
echo "  4 instances seeded (failed, cancelled, completed, running)"

print_step "Asserting the filter applies every status..."

# The bug: only `failed` was applied, so `cancelled` never came back.
expect_eq "status=failed,cancelled returns both" \
    "$(printf 'cancelled\nfailed')" "$(api_statuses 'failed,cancelled')"

# totalElements comes from a separate count query against the same filter — it
# has to agree with the page, or pagination lies about how much is there.
expect_eq "status=failed,cancelled totalElements" "2" "$(api_total 'failed,cancelled')"

# failed and timeout both mean the same runtime status; asking for both must not
# double count.
expect_eq "status=failed,timeout collapses to one status" \
    "failed" "$(api_statuses 'failed,timeout')"
expect_eq "status=failed,timeout totalElements" "1" "$(api_total 'failed,timeout')"

# Three of the four, to catch an "any status matches" over-correction.
expect_eq "status=failed,cancelled,completed returns three" \
    "$(printf 'cancelled\ncompleted\nfailed')" "$(api_statuses 'failed,cancelled,completed')"

# A single status must still behave exactly as before.
expect_eq "status=failed alone" "failed" "$(api_statuses 'failed')"
expect_eq "status=failed alone totalElements" "1" "$(api_total 'failed')"

# No filter at all returns everything.
expect_eq "no status filter totalElements" "4" \
    "$(curl -sS "${API}/executions?size=100" | jq -r '.data.totalElements')"

# `suspended` is a status listings report, so it has to be accepted as a filter.
expect_eq "status=suspended is accepted" "200" "$(api_http_code 'suspended')"

# An unknown status in the list is still rejected outright.
expect_eq "status=failed,bogus is rejected" "400" "$(api_http_code 'failed,bogus')"

echo
if [ "${FAILURES}" -eq 0 ]; then
    print_success "All multi-status filter assertions passed"
    exit 0
fi
print_error "${FAILURES} assertion(s) failed"
tail -40 "${TEST_LOG}"
exit 1
