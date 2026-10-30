#!/bin/bash
# E2E Test: a server without a platform object model database (M3.5).
#
#   1. NO DEFAULT — booted without OBJECT_MODEL_DATABASE_URL, the object model
#      API asked for its default database answers what to do instead; no
#      default connection exists.
#   2. GUARD — a tenant Postgres connection to the cloud metadata address or
#      to localhost is refused before anything connects.
#   3. TENANT DATABASE — with the test host on the operator's allowlist
#      (RUNTARA_PROXY_ALLOWED_HOSTS), a schema is created and an object-model
#      + SQL workflow runs against the tenant's own connection.
#   4. POOL LIMIT — with OBJECT_MODEL_TENANT_POOL_MAX=1, a second database
#      is refused while the first pool is held.
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

TEST_DB_SERVER="optional_db_e2e_server_$$"
TEST_DB_RUNTIME="optional_db_e2e_runtime_$$"
TEST_DB_TENANT="optional_db_e2e_tenant_$$"
TEST_DB_TENANT2="optional_db_e2e_tenant2_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17860}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18861}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18862}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18863}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18864}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16460}"
TEST_DATA_DIR="$(mktemp -d -t runtara_optional_db_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
# Extra environment for the server (the operator allowlist in step 3).
EXTRA_ENV=()
TENANT="optional_db_e2e"

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
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_TENANT}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_TENANT2}" >/dev/null 2>&1 || true
    rm -rf "${TEST_DATA_DIR}" 2>/dev/null || true
}
trap cleanup EXIT

API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

api_post() {
    curl -sS --max-time "${3:-120}" -X POST -H "Content-Type: application/json" \
        ${2:+-d "$2"} "${API}$1"
}

# Create a workflow, save its graph, compile it; echoes the workflow id.
create_and_compile() {
    local name="$1" graph="$2"
    local resp wf_id version
    resp=$(api_post /workflows/create "{\"name\":\"${name}\",\"description\":\"optional platform db e2e\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || { print_error "create failed: ${resp}"; exit 1; }
    resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${graph}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] \
        || { print_error "update/validate failed for ${name}: ${resp}"; exit 1; }
    version=$(curl -sS "${API}/workflows/${wf_id}/versions" \
        | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf_id}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] \
        || { print_error "compile failed for ${name}: ${resp}"; tail -40 "${TEST_LOG}" >&2; exit 1; }
    echo "${wf_id}"
}

#-------------------------------------------------------------------------
echo "==============================================================="
echo "E2E Test: a server without a platform object model database"
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
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_TENANT}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_TENANT2}" >/dev/null
TENANT_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@localhost:${POSTGRES_PORT}/${TEST_DB_TENANT}"
TENANT2_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@localhost:${POSTGRES_PORT}/${TEST_DB_TENANT2}"
SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"

start_server() {
    # From the test's own dir: no .env elsewhere may supply a platform database.
    cd "${TEST_DATA_DIR}"
    print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
    OBJECT_MODEL_TENANT_POOL_MAX=1 \
    RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
    TENANT_ID="${TENANT}" \
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
    env ${EXTRA_ENV[@]+"${EXTRA_ENV[@]}"} "${RUNTARA_SERVER_BIN}" >>"${TEST_LOG}" 2>&1 &
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


# POST with status: echoes "<http status> <body>".
api_post_status() {
    curl -sS --max-time 120 -o /dev/stdout -w ' %{http_code}' -X POST \
        -H "Content-Type: application/json" ${2:+-d "$2"} "${API}$1" \
        | awk '{code=$NF; $NF=""; print code" "$0}'
}
create_connection() {
    local resp id
    resp=$(api_post /connections "{\"title\": \"$1\", \"integrationId\": \"postgres\", \"connectionParameters\": {\"database_url\": \"$2\"}}")
    id=$(echo "${resp}" | jq -r '.connection_id // .connectionId // empty')
    [ -n "${id}" ] || { print_error "connection create failed: ${resp}"; exit 1; }
    echo "${id}"
}
SCHEMA='{"name": "Item", "tableName": "optional_db_items", "columns": [{"name": "sku", "type": "string"}, {"name": "qty", "type": "integer"}]}'

#-------------------------------------------------------------------------
print_step "1. No platform database: the default is refused with what to do..."
grep -q "No platform object model database" "${TEST_LOG}" \
    || { print_error "the server did not boot without a platform database"; exit 1; }
OUT=$(api_post_status /object-model/schemas "${SCHEMA}")
case "${OUT}" in
    4??\ *"add a Postgres connection"*) ;;
    *) print_error "the default database was not refused helpfully: ${OUT}"; exit 1 ;;
esac
[ "$(curl -sS "${API}/connections" | jq '[.. | objects | select(.integrationId? == "postgres" and ((.defaultFor? // []) | index("object_model")))] | length')" = "0" ] \
    || { print_error "a default object_model connection was created"; exit 1; }
echo "  no default database: refused with 'add a Postgres connection'; no default connection ✓"

#-------------------------------------------------------------------------
print_step "2. Connections to metadata and loopback addresses are refused..."
METADATA=$(create_connection "metadata" "postgresql://u:p@169.254.169.254:5432/db")
OUT=$(api_post_status "/object-model/schemas?connectionId=${METADATA}" "${SCHEMA}")
case "${OUT}" in 4??\ *"is refused"*"169.254.169.254"*) ;; *) print_error "metadata address not refused: ${OUT}"; exit 1 ;; esac
LOCAL=$(create_connection "local" "${TENANT_DB_URL}")
OUT=$(api_post_status "/object-model/schemas?connectionId=${LOCAL}" "${SCHEMA}")
case "${OUT}" in 4??\ *"is refused"*) ;; *) print_error "loopback address not refused: ${OUT}"; exit 1 ;; esac
echo "  metadata and loopback connections refused before connecting ✓"

#-------------------------------------------------------------------------
print_step "3. With the test host allowlisted, workflows run on the tenant's database..."
kill "${SERVER_PID}"; wait "${SERVER_PID}" 2>/dev/null || true
SERVER_PID=""
EXTRA_ENV=("RUNTARA_PROXY_ALLOWED_HOSTS=localhost")
start_server
OUT=$(api_post_status "/object-model/schemas?connectionId=${LOCAL}" "${SCHEMA}")
case "${OUT}" in 2??\ *) ;; *) print_error "schema create on the tenant connection failed: ${OUT}"; exit 1 ;; esac
DEFINITION=$(jq -n --arg conn "${LOCAL}" '{
  name: "Tenant SQL",
  steps: {
    add: {
      stepType: "Agent", id: "add", agentId: "object_model", capabilityId: "create-instance",
      connectionId: $conn,
      inputMapping: {
        schema_name: {valueType: "immediate", value: "Item"},
        data: {valueType: "immediate", value: {sku: "A", qty: 5}}
      }
    },
    total: {
      stepType: "Agent", id: "total", agentId: "object_model", capabilityId: "query-sql",
      connectionId: $conn,
      inputMapping: {
        sql: {valueType: "immediate", value: "SELECT COALESCE(SUM(qty), 0)::bigint AS total FROM optional_db_items"}
      }
    },
    finish: {
      stepType: "Finish", id: "finish",
      inputMapping: { rows: {valueType: "reference", value: "steps.total.outputs.rows"} }
    }
  },
  entryPoint: "add",
  executionPlan: [{fromStep: "add", toStep: "total"}, {fromStep: "total", toStep: "finish"}],
  variables: {}, inputSchema: {}, outputSchema: {}
}')
WF_ID=$(create_and_compile "Tenant SQL" "${DEFINITION}")
RESP=$(api_post "/workflows/${WF_ID}/execute" '{"inputs": {"data": {}}}')
INSTANCE=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${INSTANCE}" ] || { print_error "execute failed: ${RESP}"; exit 1; }
STATUS=""
for _ in {1..120}; do
    STATUS=$(curl -sS "${API}/workflows/instances/${INSTANCE}" | jq -r '.data.status // empty')
    case "${STATUS}" in completed|failed|crashed|stopped|cancelled) break ;; esac
    sleep 0.5
done
RESP=$(curl -sS "${API}/workflows/instances/${INSTANCE}")
[ "${STATUS}" = "completed" ] && echo "${RESP}" | jq -e '.. | objects | select(has("total")) | .total == 5' >/dev/null \
    || { print_error "the tenant SQL workflow ended '${STATUS}': $(echo "${RESP}" | jq -c '.data.error // .data.outputs')"; tail -30 "${TEST_LOG}"; exit 1; }
[ "$(psql_quiet -d "${TEST_DB_TENANT}" -c "SELECT count(*) FROM optional_db_items")" = "1" ] \
    || { print_error "the row is not in the tenant's database"; exit 1; }
echo "  schema created and an object-model + SQL workflow ran on the tenant's database ✓"

#-------------------------------------------------------------------------
print_step "4. The per-tenant pool limit holds..."
SECOND=$(create_connection "second" "${TENANT2_DB_URL}")
OUT=$(api_post_status "/object-model/schemas?connectionId=${SECOND}" "${SCHEMA}")
case "${OUT}" in *"OBJECT_MODEL_TENANT_POOL_MAX"*) ;; *) print_error "a second pool was not refused: ${OUT}"; exit 1 ;; esac
echo "  a second database refused at OBJECT_MODEL_TENANT_POOL_MAX=1 ✓"

print_success "optional platform database: no default without it, tenant connections guarded and pool-limited, and workflows run on the tenant's own database"
