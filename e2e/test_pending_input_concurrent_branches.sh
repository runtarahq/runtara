#!/bin/bash
# E2E Test: the executions list must keep reporting pending input while any
# branch of a run is still blocked.
#
# The list used to derive `hasPendingInput` from a whole-instance timestamp
# heuristic: the newest `external_input_requested` versus the newest
# `step_debug_end`, with "end is newer" taken to mean "answered". But
# `step_debug_end` is emitted for every step type, and branches genuinely
# overlap (Split carries a `parallelism` setting), so answering ONE iteration
# produced an end event newer than every other iteration's still-open request —
# and the indicator vanished while the run sat blocked.
#
# The per-instance pending-input endpoint always paired properly (by synthetic
# tool step id, and by per-invocation-site signal id for standalone waits), so
# the list and the detail view actively disagreed about the same run.
#
# This exercises the whole chain the unit tests only cover in pieces: HTTP
# handler -> execution engine -> DTO enrichment -> management SDK ->
# environment HTTP -> SQL.
#
# The instance rows and their events are seeded directly, so the assertions are
# about the pairing and not about how a Split happens to schedule its branches.
#
# Asserts:
#   * a run with 3 of 4 branches still waiting reports hasPendingInput=true
#   * the detail endpoint lists exactly those 3 open requests
#   * the list and the detail endpoint agree on the same run
#   * the open-actions endpoint (gated on the same flag) returns those 3
#   * a fully answered run reports false, and offers no actions
#   * a step completing later on an unrelated step id resolves nothing
#
# On UNPATCHED code this FAILS: the answered branch's end event is newer than
# the other branches' requests, so the list reports hasPendingInput=false and
# the actions endpoint short-circuits to an empty list.
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

TEST_DB_SERVER="pendinginput_e2e_server_$$"
TEST_DB_RUNTIME="pendinginput_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17750}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17751}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18751}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18752}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18753}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18754}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16395}"
TEST_DATA_DIR="$(mktemp -d -t runtara_pendinginput_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="pendinginput_e2e"

# The instance -> workflow link is checked by image_name prefix, so the image is
# named "{WORKFLOW}:1" and no workflow row is needed for the detail endpoints.
WORKFLOW="syn-split-approvals"
IMAGE_ID="pendinginput-image"
# Instance ids reach UUID-validating handlers, so they have to be real UUIDs.
INSTANCE_BLOCKED="11111111-1111-4111-8111-111111111111"
INSTANCE_ANSWERED="22222222-2222-4222-8222-222222222222"

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

# hasPendingInput for one instance, as the executions list reports it.
list_flag() {
    curl -sS "${API}/executions?size=100" \
        | jq -r --arg id "$1" '.data.content[] | select(.id == $id) | .hasPendingInput'
}
# Signal ids the per-instance detail endpoint reports as still open, sorted.
detail_signals() {
    curl -sS "${API}/workflows/${WORKFLOW}/instances/$1/pending-input" \
        | jq -r '.data.pendingInputs[].signalId' | sort
}
# Signal ids the open-actions endpoint offers — gated on hasPendingInput.
action_signals() {
    curl -sS "${API}/workflows/${WORKFLOW}/instances/$1/actions" \
        | jq -r '.data.actions[].signalId' | sort
}

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

seed_instance() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c "
        INSERT INTO instances (instance_id, tenant_id, status, started_at)
        VALUES ('$1', '${TENANT}', 'running'::instance_status, now());
        INSERT INTO instance_images (instance_id, image_id, tenant_id)
        VALUES ('$1', '${IMAGE_ID}', '${TENANT}');
    " >/dev/null
}

# Event payloads are stored as JSON bytes. `offset_seconds` places the event on
# the instance timeline, which is what the old heuristic keyed off.
seed_event() {
    local instance_id="$1" subtype="$2" offset_seconds="$3" payload="$4"
    psql_quiet -d "${TEST_DB_RUNTIME}" -c "
        INSERT INTO instance_events (instance_id, event_type, subtype, payload, created_at)
        VALUES ('${instance_id}', 'custom'::instance_event_type, '${subtype}',
                convert_to(\$json\$${payload}\$json\$, 'UTF8'),
                now() + interval '${offset_seconds} seconds');
    " >/dev/null
}

echo "==============================================================="
echo "E2E: pending input across concurrent branches"
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

print_step "Seeding a blocked run and a fully answered run..."
psql_quiet -d "${TEST_DB_RUNTIME}" -c "
    INSERT INTO images (image_id, tenant_id, name, binary_path)
    VALUES ('${IMAGE_ID}', '${TENANT}', '${WORKFLOW}:1', '/dev/null');
" >/dev/null
seed_instance "${INSTANCE_BLOCKED}"
seed_instance "${INSTANCE_ANSWERED}"

# Four iterations of one Split step, each waiting at its own invocation site.
# The step id is deliberately shared — only the signal ids differ.
for i in 1 2 3 4; do
    seed_event "${INSTANCE_BLOCKED}" external_input_requested "${i}" \
        "{\"step_id\":\"approve\",\"step_name\":\"Approve order\",\"signal_id\":\"signal-${i}\",\"iteration\":${i}}"
done
# Iteration 1 is answered. Its end event lands AFTER every other iteration's
# request — exactly the ordering the old heuristic read as "all resolved".
seed_event "${INSTANCE_BLOCKED}" step_debug_end 20 \
    "{\"step_id\":\"approve\",\"outputs\":{\"signal_id\":\"signal-1\"}}"
# An unrelated step finishing even later must not resolve anything either.
seed_event "${INSTANCE_BLOCKED}" step_debug_end 25 \
    "{\"step_id\":\"fetch_orders\",\"outputs\":{\"count\":12}}"
echo "  ${INSTANCE_BLOCKED}: 4 requests, 1 answered, 2 later step completions"

seed_event "${INSTANCE_ANSWERED}" external_input_requested 1 \
    "{\"step_id\":\"approve\",\"step_name\":\"Approve order\",\"signal_id\":\"signal-only\"}"
seed_event "${INSTANCE_ANSWERED}" step_debug_end 2 \
    "{\"step_id\":\"approve\",\"outputs\":{\"signal_id\":\"signal-only\"}}"
echo "  ${INSTANCE_ANSWERED}: 1 request, answered"

print_step "Asserting the list keeps the indicator while branches are blocked..."

# The bug: the answered branch's end event hid the other three.
expect_eq "blocked run reports hasPendingInput" "true" "$(list_flag "${INSTANCE_BLOCKED}")"

# The detail endpoint always paired correctly — this is what the list must match.
expect_eq "detail endpoint lists the 3 open branches" \
    "$(printf 'signal-2\nsignal-3\nsignal-4')" "$(detail_signals "${INSTANCE_BLOCKED}")"

# The disagreement in the ticket: list said "nothing pending", detail said 3.
BLOCKED_DETAIL_COUNT="$(detail_signals "${INSTANCE_BLOCKED}" | grep -c .)"
expect_eq "list and detail agree that work is pending" \
    "true" "$([ "$(list_flag "${INSTANCE_BLOCKED}")" = "true" ] && [ "${BLOCKED_DETAIL_COUNT}" -gt 0 ] && echo true || echo false)"

# The actions endpoint short-circuits on the same flag, so a wrong flag makes
# the still-open branches unanswerable, not merely invisible.
expect_eq "open actions are offered for the blocked branches" \
    "$(printf 'signal-2\nsignal-3\nsignal-4')" "$(action_signals "${INSTANCE_BLOCKED}")"

print_step "Asserting a fully answered run reports nothing pending..."

# Guards against an over-correction that just always reports true.
expect_eq "answered run reports no pending input" "false" "$(list_flag "${INSTANCE_ANSWERED}")"
expect_eq "answered run lists no open requests" "" "$(detail_signals "${INSTANCE_ANSWERED}")"
expect_eq "answered run offers no actions" "" "$(action_signals "${INSTANCE_ANSWERED}")"

echo
if [ "${FAILURES}" -eq 0 ]; then
    print_success "All pending-input assertions passed"
    exit 0
fi
print_error "${FAILURES} assertion(s) failed"
tail -40 "${TEST_LOG}"
exit 1
