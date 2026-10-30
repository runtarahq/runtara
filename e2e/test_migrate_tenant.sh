#!/bin/bash
# E2E Test: a tenant moves from a server of its own onto one that serves
# several (`runtara-server migrate-tenant`, C11/M7.7).
#
#   1. SOURCE — a server serving only `mig_e2e` holds a completed run, a run
#      parked on a durable delay, a webhook trigger, an OAuth connection and
#      an API key. It is then stopped, as the tenant's old process is.
#   2. COPY — migrate-tenant copies the tenant into the databases and
#      artifact store of a server already serving another tenant, as a role
#      that may copy as a replica.
#   3. TARGET — restarted to serve both, the target serves the moved tenant
#      by its old API key: its workflows and run history are there, its
#      parked run wakes and completes, its webhook runs its workflow (compiled
#      again here), its connection's credentials open, and the other tenant
#      sees none of it. A second copy is refused.
#   4. SHARED DATABASES — a tenant whose own server already used the target's
#      databases, with artifacts in a data dir of its own: migrate-tenant
#      copies only its artifacts, and the target, serving it too, runs its
#      compiled workflow by its key without compiling again.
#
# Prerequisites: Postgres (POSTGRES_PASSWORD or PGPASSWORD; a superuser for
# the copy), docker (Valkey), and the workflow components in
# target/wasm32-wasip2/release (scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-${PGPASSWORD:-}}"

DB="migrate_tenant_e2e_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17910}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18911}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16490}"
TEST_DATA_DIR="$(mktemp -d -t runtara_migrate_tenant_e2e_XXXXXX)"
SERVER_PID=""
VALKEY_CONTAINER=""
MOVED="mig_e2e"
OTHER="mig_e2e_host"
NEAR="mig_e2e_near"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
}
url() { echo "postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${DB}_$1"; }

cleanup() {
    if [ -n "${SERVER_PID}" ] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    [ -n "${VALKEY_CONTAINER}" ] && docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    for db in from_server from_runtime to_server to_runtime; do
        psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${DB}_${db}" >/dev/null 2>&1 || true
    done
    rm -rf "${TEST_DATA_DIR}" 2>/dev/null || true
}
trap cleanup EXIT

API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"
AUTH=()
api() { # METHOD PATH [BODY]
    curl -sS --max-time 900 -X "$1" -H "Content-Type: application/json" ${AUTH[@]+"${AUTH[@]}"} ${3:+-d "$3"} "${API}$2"
}
http_status() { curl -sS -o /dev/null -w '%{http_code}' "$@"; }

# start SIDE TENANT_ID [SERVED]: a server on SIDE's databases (DB_SIDE's,
# when set) and data dir.
start() {
    RUNTARA_SERVER_DATABASE_URL="$(url "${DB_SIDE:-$1}_server")" \
    OBJECT_MODEL_DATABASE_URL="$(url "${DB_SIDE:-$1}_server")" \
    RUNTARA_DATABASE_URL="$(url "${DB_SIDE:-$1}_runtime")" \
    TENANT_ID="$2" RUNTARA_SERVED_TENANTS="${3:-}" \
    SERVER_HOST=127.0.0.1 SERVER_PORT="${TEST_PORT_PUBLIC}" \
    RUNTARA_CORE_PORT="${TEST_CORE_PORT}" RUNTARA_ENVIRONMENT_PORT=$((TEST_CORE_PORT + 1)) \
    RUNTARA_CORE_HTTP_PORT=$((TEST_CORE_PORT + 2)) RUNTARA_ENV_HTTP_PORT=$((TEST_CORE_PORT + 3)) \
    RUNTARA_AGENT_COMPONENTS_DIR="${AGENTS_DIR-${COMPONENTS_DIR}}" \
    RUNTARA_CONNECTIONS_ENCRYPTION_KEY="${KEY}" RUNTARA_CONNECTIONS_ENCRYPTION_KEY_ID=k1 \
    DATA_DIR="${TEST_DATA_DIR}/$1" \
    RUST_LOG="warn,runtara_server=info" AUTH_PROVIDER=local \
    RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS=127.0.0.1 \
    VALKEY_HOST=127.0.0.1 VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true RUNTARA_SDK_BACKEND=http SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}" >>"${TEST_DATA_DIR}/$1.log" 2>&1 &
    SERVER_PID=$!
    for _ in {1..180}; do
        http_status "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2" && return 0
        kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "the $1 server exited during boot"; tail -30 "${TEST_DATA_DIR}/$1.log"; exit 1; }
        sleep 1
    done
    print_error "the $1 server did not come up"; exit 1
}
stop() { kill "${SERVER_PID}"; wait "${SERVER_PID}" 2>/dev/null || true; SERVER_PID=""; }

# compiled NAME GRAPH: a new workflow, saved and compiled; echoes its id.
compiled() {
    local wf version
    wf=$(api POST /workflows/create "{\"name\":\"$1\",\"description\":\"migrate e2e\"}" | jq -r '.data.id // empty')
    [ -n "${wf}" ] || { print_error "$1: create failed"; exit 1; }
    api POST "/workflows/${wf}/update" "{\"executionGraph\": $2}" | jq -e '.success == true' >/dev/null \
        || { print_error "$1: update failed"; exit 1; }
    version=$(api GET "/workflows/${wf}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    api POST "/workflows/${wf}/versions/${version}/compile" '{}' | jq -e '.success == true' >/dev/null \
        || { print_error "$1: compile failed"; exit 1; }
    echo "${wf}"
}
status_of() { api GET "/workflows/instances/$1" | jq -r '.data.status // empty'; }
until_status() { # INSTANCE STATUS SECONDS
    for _ in $(seq 1 $(($3 * 2))); do [ "$(status_of "$1")" = "$2" ] && return 0; sleep 0.5; done
    print_error "run $1 is $(status_of "$1"), not $2"; exit 1
}

[ -n "${POSTGRES_PASSWORD}" ] || { print_error "set POSTGRES_PASSWORD (or PGPASSWORD)"; exit 1; }
[ -x "${RUNTARA_SERVER_BIN}" ] || SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
KEY=$(openssl rand -base64 32)
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > "/dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}") 2>/dev/null && break; sleep 0.5; done
for db in from_server from_runtime to_server to_runtime; do
    psql_quiet -d postgres -c "CREATE DATABASE ${DB}_${db}" >/dev/null
done

ANSWER='{"name":"Answer","steps":{"finish":{"stepType":"Finish","id":"finish","inputMapping":{"answer":{"valueType":"immediate","value":42}}}},"entryPoint":"finish","executionPlan":[],"variables":{},"inputSchema":{},"outputSchema":{}}'
NAP='{"name":"Nap","durable":true,"steps":{"nap":{"stepType":"Delay","id":"nap","durationMs":{"valueType":"immediate","value":20000}},"done":{"stepType":"Finish","id":"done","inputMapping":{"slept":{"valueType":"immediate","value":true}}}},"entryPoint":"nap","executionPlan":[{"fromStep":"nap","toStep":"done"}],"variables":{},"inputSchema":{},"outputSchema":{}}'

#-------------------------------------------------------------------------
print_step "1. The tenant's own server: history, a parked run, a trigger, a connection, a key..."
start from "${MOVED}"
KEY_TOKEN=$(api POST /api-keys '{"name":"moved"}' | jq -r '.key // .data.key // empty')
[ -n "${KEY_TOKEN}" ] || { print_error "api key create failed"; exit 1; }
ANSWER_WF=$(compiled Answer "${ANSWER}")
DONE_RUN=$(api POST "/workflows/${ANSWER_WF}/execute" '{"inputs":{"data":{}}}' | jq -r '.data.instanceId')
until_status "${DONE_RUN}" completed 60
NAP_WF=$(compiled Nap "${NAP}")
NAP_RUN=$(api POST "/workflows/${NAP_WF}/execute" '{"inputs":{"data":{}}}' | jq -r '.data.instanceId')
until_status "${NAP_RUN}" suspended 30
RESP=$(api POST /triggers "{\"workflow_id\": \"${ANSWER_WF}\", \"trigger_type\": \"HTTP\", \"active\": true}")
TRIGGER=$(echo "${RESP}" | jq -r '.data.id // .id // empty')
[ -n "${TRIGGER}" ] || { print_error "trigger create failed: ${RESP:0:300}"; exit 1; }
RESP=$(api POST /connections '{"title":"Moved OAuth","integrationId":"http_oauth2_authorization_code","connectionParameters":{"auth_url":"http://127.0.0.1:9/authorize","token_url":"http://127.0.0.1:9/token","client_id":"moved-client","client_secret":"csec","base_url":"https://api.example.com"}}')
CONN=$(echo "${RESP}" | jq -r '.connection_id // .connectionId // empty')
[ -n "${CONN}" ] || { print_error "connection create failed: ${RESP:0:300}"; exit 1; }
stop
echo "  a completed run, a run parked on a 20 s delay, a webhook trigger, an OAuth connection and an API key; stopped ✓"

#-------------------------------------------------------------------------
print_step "2. The shared server serves another tenant; the tenant is copied in..."
start to "${OTHER}"
OTHER_WF=$(compiled "Host's" "${ANSWER}")
stop
MIGRATE_FROM_SERVER_DATABASE_URL="$(url from_server)" MIGRATE_FROM_RUNTIME_DATABASE_URL="$(url from_runtime)" \
MIGRATE_FROM_DATA_DIR="${TEST_DATA_DIR}/from" \
MIGRATE_TO_SERVER_DATABASE_URL="$(url to_server)" MIGRATE_TO_RUNTIME_DATABASE_URL="$(url to_runtime)" \
DATA_DIR="${TEST_DATA_DIR}/to" SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}" migrate-tenant --tenant-id "${MOVED}" > "${TEST_DATA_DIR}/report.json" 2> "${TEST_DATA_DIR}/migrate.log" \
    || { print_error "migrate-tenant failed: $(tail -3 "${TEST_DATA_DIR}/migrate.log")"; exit 1; }
REPORT=$(grep '^{' "${TEST_DATA_DIR}/report.json" | tail -1)
echo "${REPORT}" | jq -e '.rows.instances >= 2 and .rows.workflows == 2 and .rows.connection_data_entity >= 1 and .rows.api_keys == 1 and .artifacts >= 1' >/dev/null \
    || { print_error "unexpected copy: ${REPORT}"; exit 1; }
echo "  copied: $(echo "${REPORT}" | jq -c '{instances: .rows.instances, checkpoints: .rows.checkpoints, events: .rows.instance_events, workflows: .rows.workflows, artifacts}') ✓"

#-------------------------------------------------------------------------
print_step "3. Serving both, the shared server serves the moved tenant as before..."
start to "${OTHER}" "${MOVED}"
AUTH=(-H "Authorization: Bearer ${KEY_TOKEN}")
NAMES=$(api GET /workflows | jq -r '[(.data.content // .data // [])[] | .name] | sort | join(",")')
[ "${NAMES}" = "Answer,Nap" ] || { print_error "the moved tenant's workflows are '${NAMES}'"; exit 1; }
[ "$(status_of "${DONE_RUN}")" = completed ] || { print_error "the completed run is $(status_of "${DONE_RUN}")"; exit 1; }
until_status "${NAP_RUN}" completed 90
CODE=$(http_status -X POST -H 'Content-Type: application/json' -d '{}' "${API}/${MOVED}/events/http/${TRIGGER}/run")
case "${CODE}" in 2??) ;; *) print_error "the moved webhook answered ${CODE}"; exit 1 ;; esac
for _ in {1..120}; do
    RUNS=$(api GET "/workflows/${ANSWER_WF}/instances?size=50" | jq '[(.data.content // .data // [])[] | select(.status == "completed")] | length')
    [ "${RUNS}" -ge 2 ] && break; sleep 1
done
[ "${RUNS}" -ge 2 ] || { print_error "the webhook's run did not complete"; tail -30 "${TEST_DATA_DIR}/to.log"; exit 1; }
AUTHORIZE=$(api GET "/connections/${CONN}/oauth/authorize" | jq -r '.authorization_url // .authorizationUrl // empty')
[[ "${AUTHORIZE}" == *client_id=moved-client* ]] || { print_error "the moved connection does not open: ${AUTHORIZE}"; exit 1; }
AUTH=()
# Saving a graph names the workflow after it, so compare ids.
IDS=$(api GET /workflows | jq -r '[(.data.content // .data // [])[] | .id] | join(",")')
[ "${IDS}" = "${OTHER_WF}" ] || { print_error "the host tenant sees workflows '${IDS}', not only its own ${OTHER_WF}"; exit 1; }
[ "$(http_status "${API}/workflows/instances/${DONE_RUN}")" = 404 ] || { print_error "the host tenant reads the moved run"; exit 1; }
stop
MIGRATE_FROM_SERVER_DATABASE_URL="$(url from_server)" MIGRATE_FROM_RUNTIME_DATABASE_URL="$(url from_runtime)" \
MIGRATE_FROM_DATA_DIR="${TEST_DATA_DIR}/from" \
MIGRATE_TO_SERVER_DATABASE_URL="$(url to_server)" MIGRATE_TO_RUNTIME_DATABASE_URL="$(url to_runtime)" \
DATA_DIR="${TEST_DATA_DIR}/to" SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}" migrate-tenant --tenant-id "${MOVED}" > /dev/null 2> "${TEST_DATA_DIR}/again.log" \
    && { print_error "a second copy was not refused"; exit 1; }
grep -q "already has" "${TEST_DATA_DIR}/again.log" || { print_error "a second copy failed otherwise: $(tail -2 "${TEST_DATA_DIR}/again.log")"; exit 1; }
echo "  by its old key: its two workflows, its completed run, its parked run woken and completed, its webhook run (compiled again), its connection opened; the host tenant sees none of it; a second copy refused ✓"

#-------------------------------------------------------------------------
print_step "4. A tenant already on the target's databases has only its artifacts copied..."
DB_SIDE=to start near "${NEAR}"
NEAR_KEY=$(api POST /api-keys '{"name":"near"}' | jq -r '.key // .data.key // empty')
[ -n "${NEAR_KEY}" ] || { print_error "api key create failed"; exit 1; }
NEAR_WF=$(compiled Near "${ANSWER}")
stop
MIGRATE_FROM_SERVER_DATABASE_URL="$(url to_server)" MIGRATE_FROM_RUNTIME_DATABASE_URL="$(url to_runtime)" \
MIGRATE_FROM_DATA_DIR="${TEST_DATA_DIR}/near" \
MIGRATE_TO_SERVER_DATABASE_URL="$(url to_server)" MIGRATE_TO_RUNTIME_DATABASE_URL="$(url to_runtime)" \
DATA_DIR="${TEST_DATA_DIR}/to" SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}" migrate-tenant --tenant-id "${NEAR}" > "${TEST_DATA_DIR}/near.json" 2> "${TEST_DATA_DIR}/near-migrate.log" \
    || { print_error "migrate-tenant failed: $(tail -3 "${TEST_DATA_DIR}/near-migrate.log")"; exit 1; }
REPORT=$(grep '^{' "${TEST_DATA_DIR}/near.json" | tail -1)
echo "${REPORT}" | jq -e '.shared == true and .rows == {} and .artifacts >= 1' >/dev/null \
    || { print_error "unexpected copy: ${REPORT}"; exit 1; }
DIGEST=$(psql_quiet -d "${DB}_to_runtime" -c "SELECT binary_digest FROM images WHERE tenant_id = '${NEAR}' AND binary_digest IS NOT NULL LIMIT 1")
DIGEST_HEX=${DIGEST#sha256:}
[ -n "${DIGEST_HEX}" ] && find "${TEST_DATA_DIR}/to" -name "*${DIGEST_HEX}*" | grep -q . \
    || { print_error "the near tenant's artifact ${DIGEST} is not in the target's store"; exit 1; }
start to "${OTHER}" "${MOVED},${NEAR}"
AUTH=(-H "Authorization: Bearer ${NEAR_KEY}")
RUN=$(api POST "/workflows/${NEAR_WF}/execute" '{"inputs":{"data":{}}}' | jq -r '.data.instanceId // empty')
[ -n "${RUN}" ] || { print_error "the near tenant's workflow did not start"; exit 1; }
until_status "${RUN}" completed 60
AUTH=()
stop
echo "  only its artifacts copied ($(echo "${REPORT}" | jq -c '{artifacts, shared}')); the target ran its compiled workflow by its key ✓"

print_success "migrate tenant: a tenant moved from its own server onto a shared one keeps its workflows, runs, triggers, connections and keys"
