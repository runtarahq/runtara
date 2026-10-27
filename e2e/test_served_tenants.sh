#!/bin/bash
# E2E Test: a server serves exactly its tenants (M5.1, first slice).
#
# The server's primary tenant is TENANT_ID; RUNTARA_SERVED_TENANTS adds a
# second one. Nothing here may authenticate or run work for a third.
#
#   1. WEBHOOKS — the primary tenant's HTTP trigger and sync endpoint run its
#      workflow; a trigger row of an unserved tenant is not found.
#   2. API KEYS — the primary tenant's key works; a key of an unserved tenant
#      is refused; a key of the served second tenant authenticates as that
#      tenant and sees none of the primary tenant's workflows.
#   3. SECOND TENANT'S WORK — as the second tenant: a workflow compiles, its
#      HTTP trigger runs it through the second tenant's trigger stream, and
#      its cron trigger fires; its runs are its own, and the primary tenant
#      sees none of them.
#   4. NO CROSS-TENANT READS — neither tenant reads the other's workflow,
#      versions or runs by id.
#
# Prerequisites: Postgres (POSTGRES_PASSWORD or PGPASSWORD in the
# environment) + docker (isolated Valkey) and the shared workflow components
# in target/wasm32-wasip2/release (scripts/build-agent-components.sh).

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

TEST_DB_SERVER="served_tenants_e2e_server_$$"
TEST_DB_RUNTIME="served_tenants_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17880}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18881}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18882}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18883}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18884}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16480}"
TEST_DATA_DIR="$(mktemp -d -t runtara_served_tenants_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="served_tenants_e2e"
SECOND_TENANT="served_tenants_e2e_b"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
# stderr so failures inside $(command substitution) helpers stay visible.
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql \
        -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" \
        -tA "$@"
}

cleanup() {
    if [ -n "${SERVER_PID}" ] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    if [ -n "${VALKEY_CONTAINER}" ]; then
        docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    fi
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME}" >/dev/null 2>&1 || true
    rm -rf "${TEST_DATA_DIR}" 2>/dev/null || true
}
trap cleanup EXIT

API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

# Requests run as the primary tenant (local auth) unless AUTH carries a key.
AUTH=()
api_post() {
    curl -sS --max-time "${3:-120}" -X POST -H "Content-Type: application/json" ${AUTH[@]+"${AUTH[@]}"} \
        ${2:+-d "$2"} "${API}$1"
}

# Create a workflow, save its graph, compile it; echoes the workflow id.
create_and_compile() {
    local name="$1" graph="$2"
    local resp wf_id version
    resp=$(api_post /workflows/create "{\"name\":\"${name}\",\"description\":\"served tenants e2e\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || { print_error "create failed: ${resp}"; exit 1; }
    resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${graph}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] \
        || { print_error "update/validate failed for ${name}: ${resp}"; exit 1; }
    version=$(curl -sS ${AUTH[@]+"${AUTH[@]}"} "${API}/workflows/${wf_id}/versions" \
        | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf_id}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] \
        || { print_error "compile failed for ${name}: ${resp}"; tail -40 "${TEST_LOG}" >&2; exit 1; }
    echo "${wf_id}"
}

#-------------------------------------------------------------------------
echo "==============================================================="
echo "E2E Test: a server serves exactly its tenants"
echo "==============================================================="

[ -n "${POSTGRES_PASSWORD}" ] \
    || { print_error "set POSTGRES_PASSWORD (or PGPASSWORD) for ${POSTGRES_USER}@${POSTGRES_HOST}"; exit 1; }
if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi
for f in runtara_workflow_stdlib.wasm; do
    if [ ! -f "${COMPONENTS_DIR}/${f}" ]; then
        print_error "Missing component ${COMPONENTS_DIR}/${f} — run scripts/build-agent-components.sh"
        exit 1
    fi
done

print_step "Pre-flight: Postgres and docker..."
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 \
    || { print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker is required"; exit 1; }

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do
    if (echo > "/dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}") 2>/dev/null; then break; fi
    sleep 0.5
done

print_step "Creating test databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null
SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"

start_server() {
    print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
    OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
    RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
    TENANT_ID="${TENANT}" \
    RUNTARA_SERVED_TENANTS="${SECOND_TENANT}" \
    SERVER_HOST=127.0.0.1 \
    SERVER_PORT="${TEST_PORT_PUBLIC}" \
    RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
    RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
    RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
    RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
    RUNTARA_AGENT_COMPONENTS_DIR="${AGENTS_DIR-${COMPONENTS_DIR}}" \
    MAX_CONCURRENT_EXECUTIONS=40 \
    DATA_DIR="${TEST_DATA_DIR}" \
    RUST_LOG="warn,runtara_server=info" \
    AUTH_PROVIDER=local \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true \
    RUNTARA_SDK_BACKEND=http \
    SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}" >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!

    for _ in {1..60}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            break
        fi
        sleep 1
        kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "Server exited during boot."; tail -30 "${TEST_LOG}"; exit 1; }
    done
    echo "  Server up (PID ${SERVER_PID})"
}

start_server


runtime_sql() { psql_quiet -d "${TEST_DB_RUNTIME}" -c "$1"; }

# Run the workflow; fail unless it completes with its output.
run_echo() {
    local resp instance status
    resp=$(api_post "/workflows/${WF_ID}/execute" '{"inputs": {"data": {}}}')
    instance=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    [ -n "${instance}" ] || { print_error "execute failed: ${resp}"; exit 1; }
    status=""
    for _ in {1..120}; do
        status=$(curl -sS "${API}/workflows/instances/${instance}" | jq -r '.data.status // empty')
        case "${status}" in completed|failed|crashed|stopped|cancelled) break ;; esac
        sleep 0.5
    done
    resp=$(curl -sS "${API}/workflows/instances/${instance}")
    [ "${status}" = "completed" ] && echo "${resp}" \
        | jq -e '.data.outputs.answer == 42 and .data.outputs.hash == "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"' >/dev/null \
        || { print_error "the run ended '${status}': $(echo "${resp}" | jq -c '.data.error // .data.outputs')"; tail -40 "${TEST_LOG}"; exit 1; }
}


server_sql() { psql_quiet -d "${TEST_DB_SERVER}" -c "$1"; }
http_status() { curl -sS -o /dev/null -w '%{http_code}' "$@"; }

GRAPH='{
  "name": "Answer",
  "steps": {
    "finish": {
      "stepType": "Finish", "id": "finish",
      "inputMapping": { "answer": { "valueType": "immediate", "value": 42 } }
    }
  },
  "entryPoint": "finish",
  "executionPlan": [],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}'

#-------------------------------------------------------------------------
print_step "1. Webhooks run the primary tenant's work, and only a served tenant's..."
WF_ID=$(create_and_compile "Answer" "${GRAPH}")
RESP=$(api_post /triggers "{\"workflow_id\": \"${WF_ID}\", \"trigger_type\": \"HTTP\", \"active\": true}")
TRIGGER=$(echo "${RESP}" | jq -r '.data.id // .id // empty' 2>/dev/null || true)
[ -n "${TRIGGER}" ] || { print_error "trigger create failed: ${RESP}"; exit 1; }
CODE=$(http_status -X POST -H 'Content-Type: application/json' -d '{"n": 1}' "${API}/events/http/${TRIGGER}/run")
case "${CODE}" in 2??) ;; *) print_error "the primary tenant's webhook answered ${CODE}"; exit 1 ;; esac
RESP=$(curl -sS -X POST -H 'Content-Type: application/json' -d '{}' "${API}/events/http-sync/${WF_ID}")
echo "${RESP}" | jq -e '.success == true and .outputs.answer == 42' >/dev/null \
    || { print_error "the sync webhook did not run the workflow: ${RESP}"; exit 1; }
server_sql "CREATE TEMP TABLE t AS SELECT * FROM invocation_trigger WHERE id = '${TRIGGER}';
            UPDATE t SET id = 'outsider-trigger', tenant_id = 'outsider';
            INSERT INTO invocation_trigger SELECT * FROM t;" >/dev/null
CODE=$(http_status -X POST -H 'Content-Type: application/json' -d '{}' "${API}/events/http/outsider-trigger/run")
[ "${CODE}" = "404" ] || { print_error "an unserved tenant's trigger answered ${CODE}, not 404"; exit 1; }
echo "  the primary tenant's webhook and sync run; an unserved tenant's trigger is not found ✓"

#-------------------------------------------------------------------------
print_step "2. API keys authenticate only served tenants, each as itself..."
RESP=$(api_post /api-keys '{"name": "e2e"}')
KEY=$(echo "${RESP}" | jq -r '.key // .data.key // empty')
KEY_ID=$(echo "${RESP}" | jq -r '.id // .data.id // empty')
[ -n "${KEY}" ] && [ -n "${KEY_ID}" ] || { print_error "api key create failed: $(echo "${RESP}" | jq -c 'del(.key)')"; exit 1; }
[ "$(http_status -H "Authorization: Bearer ${KEY}" "${API}/workflows")" = "200" ] \
    || { print_error "the primary tenant's key was refused"; exit 1; }
# Keys cloned from that row for other tenants, with tokens this test knows.
clone_key() {
    local org="$1" token="$2" hash
    hash=$(printf '%s' "${token}" | shasum -a 256 | cut -d' ' -f1)
    server_sql "CREATE TEMP TABLE k AS SELECT * FROM api_keys WHERE id = '${KEY_ID}';
                UPDATE k SET id = gen_random_uuid(), org_id = '${org}', key_hash = '${hash}',
                             jti = gen_random_uuid()::text;
                INSERT INTO api_keys SELECT * FROM k;" >/dev/null
}
OUTSIDER_KEY="rt_outsider_$(openssl rand -hex 24)"
clone_key outsider "${OUTSIDER_KEY}"
CODE=$(http_status -H "Authorization: Bearer ${OUTSIDER_KEY}" "${API}/workflows")
[ "${CODE}" = "403" ] || { print_error "an unserved tenant's key answered ${CODE}, not 403"; exit 1; }
SECOND_KEY="rt_${SECOND_TENANT}_$(openssl rand -hex 24)"
clone_key "${SECOND_TENANT}" "${SECOND_KEY}"
RESP=$(curl -sS -H "Authorization: Bearer ${SECOND_KEY}" "${API}/workflows")
echo "${RESP}" | jq -e '(.data.content // .data // []) | length == 0' >/dev/null \
    || { print_error "the second tenant's key saw the primary tenant's workflows: $(echo "${RESP}" | head -c 300)"; exit 1; }
[ "$(curl -sS -H "Authorization: Bearer ${KEY}" "${API}/workflows" | jq '(.data.content // .data // []) | length')" -ge 1 ] \
    || { print_error "the primary tenant's key no longer sees its workflow"; exit 1; }
echo "  primary key works; an unserved tenant's key is refused (403); the served second tenant sees only its own workflows ✓"

#-------------------------------------------------------------------------
print_step "3. The second tenant's work runs, as the second tenant..."
AUTH=(-H "Authorization: Bearer ${SECOND_KEY}")
B_WF=$(create_and_compile "Answer B" "${GRAPH/\"Answer\"/\"Answer B\"}")
RESP=$(api_post /triggers "{\"workflow_id\": \"${B_WF}\", \"trigger_type\": \"HTTP\", \"active\": true}")
B_TRIGGER=$(echo "${RESP}" | jq -r '.data.id // .id // empty' 2>/dev/null || true)
[ -n "${B_TRIGGER}" ] || { print_error "the second tenant's trigger create failed: ${RESP}"; exit 1; }
RESP=$(api_post /triggers "{\"workflow_id\": \"${B_WF}\", \"trigger_type\": \"CRON\", \"active\": true, \"configuration\": {\"expression\": \"* * * * *\", \"timezone\": \"UTC\"}}")
echo "${RESP}" | jq -e '.data.id // .id' >/dev/null 2>&1 || { print_error "the second tenant's cron create failed: ${RESP}"; exit 1; }
AUTH=()
CODE=$(http_status -X POST -H 'Content-Type: application/json' -d '{}' "${API}/events/http/${B_TRIGGER}/run")
case "${CODE}" in 2??) ;; *) print_error "the second tenant's webhook answered ${CODE}"; exit 1 ;; esac
# Runs land under the second tenant: the webhook's, then the cron's.
b_runs() {
    curl -sS -H "Authorization: Bearer ${SECOND_KEY}" "${API}/workflows/${B_WF}/instances?size=50" \
        | jq '[(.data.content // .data // [])[] | select((.status // "") == "completed")] | length'
}
for _ in {1..60}; do [ "$(b_runs)" -ge 1 ] && break; sleep 1; done
[ "$(b_runs)" -ge 1 ] || { print_error "the second tenant's webhook run did not complete"; tail -30 "${TEST_LOG}"; exit 1; }
for _ in {1..150}; do [ "$(b_runs)" -ge 2 ] && break; sleep 1; done
[ "$(b_runs)" -ge 2 ] || { print_error "the second tenant's cron trigger did not fire"; tail -30 "${TEST_LOG}"; exit 1; }
[ "$(http_status "${API}/workflows/${B_WF}/instances")" = "404" ] \
    || [ "$(curl -sS "${API}/workflows/${B_WF}/instances" | jq '(.data.content // .data // []) | length')" = "0" ] \
    || { print_error "the primary tenant sees the second tenant's runs"; exit 1; }
echo "  the second tenant's workflow compiled, its webhook and cron ran it as the second tenant; the primary sees none ✓"

#-------------------------------------------------------------------------
print_step "4. Neither tenant reads the other's resources by id..."
for path in "/workflows/${WF_ID}" "/workflows/${WF_ID}/versions" "/workflows/${WF_ID}/instances"; do
    RESP=$(curl -sS -H "Authorization: Bearer ${SECOND_KEY}" "${API}${path}")
    if echo "${RESP}" | jq -e '(.data | type) == "object" and (.data.id? // .data.workflowId? // "") != ""' >/dev/null 2>&1 \
        || echo "${RESP}" | jq -e '((.data.content // .data // []) | type) == "array" and ((.data.content // .data // []) | length) > 0' >/dev/null 2>&1; then
        print_error "the second tenant read the primary's ${path}: $(echo "${RESP}" | head -c 200)"; exit 1
    fi
done
for path in "/workflows/${B_WF}" "/workflows/${B_WF}/versions"; do
    RESP=$(curl -sS "${API}${path}")
    if echo "${RESP}" | jq -e '(.data | type) == "object" and (.data.id? // .data.workflowId? // "") != ""' >/dev/null 2>&1 \
        || echo "${RESP}" | jq -e '((.data.content // .data // []) | type) == "array" and ((.data.content // .data // []) | length) > 0' >/dev/null 2>&1; then
        print_error "the primary read the second tenant's ${path}: $(echo "${RESP}" | head -c 200)"; exit 1
    fi
done
echo "  neither tenant reads the other's workflow, versions or runs by id ✓"

print_success "served tenants: webhooks, API keys, triggers and cron reach exactly the tenants this server serves, each its own"
