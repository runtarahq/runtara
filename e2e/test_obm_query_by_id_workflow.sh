#!/bin/bash
# E2E Test: object-model query-instances by id under the direct-wasm engine
#
# Regression test for the silent-empty-result bug: a workflow that creates an
# instance and loads it back with EQ(field "id", <reference to the created
# id>) returned { instances: [], total_count: 0 } under the direct compiler,
# because references nested inside agent condition payloads were never
# resolved (the legacy compiler's resolve_nested_references pass was dropped
# in the direct-wasm migration). The literal path string reached the SQL
# layer as the comparison value, so the query succeeded with zero rows.
#
# Full loop, all via the HTTP API of a self-contained server:
#   1. Create an object schema and a postgres connection
#   2. Create + update + compile a workflow:
#        create-instance → query-instances EQ(id, steps.create.outputs.instance_id) → finish
#   3. Execute it and assert total_count == 1 and the loaded id matches
#
# Prerequisites: Postgres + docker (for an isolated Valkey) and the agent /
# shared workflow components in target/wasm32-wasip2/release (see
# scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"

TEST_DB_SERVER="obm_id_e2e_server_$$"
TEST_DB_RUNTIME="obm_id_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17700}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17701}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18701}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18702}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18703}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18704}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16390}"
TEST_DATA_DIR="$(mktemp -d -t runtara_obm_id_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="obm_id_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
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

api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" \
        -d "$2" "${API}$1"
}

#-------------------------------------------------------------------------
echo "==============================================================="
echo "E2E Test: query-instances by id reference (direct-wasm engine)"
echo "==============================================================="

if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi
for f in runtara_agent_object_model.wasm runtara_workflow_stdlib.wasm runtara_workflow_runtime.wasm; do
    if [ ! -f "${COMPONENTS_DIR}/${f}" ]; then
        print_error "Missing component ${COMPONENTS_DIR}/${f} — run scripts/build-agent-components.sh"
        exit 1
    fi
done

print_step "Pre-flight: Postgres and docker..."
if ! psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1; then
    print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"
    exit 1
fi
if ! docker info >/dev/null 2>&1; then
    print_error "docker is required (isolated Valkey for the trigger stream)"
    exit 1
fi

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for i in {1..20}; do
    if (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null; then break; fi
    sleep 0.5
done

print_step "Creating test databases (${TEST_DB_SERVER}, ${TEST_DB_RUNTIME})..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null
SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC} (env HTTP :${TEST_ENV_HTTP_PORT})..."
RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
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
RUST_LOG="warn,runtara_server=info,runtara_object_store=warn" \
AUTH_PROVIDER=local \
SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2 \
VALKEY_HOST=127.0.0.1 \
VALKEY_PORT="${TEST_VALKEY_PORT}" \
OTEL_SDK_DISABLED=true \
RUNTARA_SDK_BACKEND=http \
SQLX_OFFLINE="${SQLX_OFFLINE}" \
"${RUNTARA_SERVER_BIN}" >"${TEST_LOG}" 2>&1 &
SERVER_PID=$!

for i in {1..60}; do
    if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
        break
    fi
    sleep 1
    if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
        print_error "Server exited during boot."
        tail -30 "${TEST_LOG}"
        exit 1
    fi
done
echo "  Server up (PID ${SERVER_PID})"

#-------------------------------------------------------------------------
print_step "Creating object schema E2eOrder..."
RESP=$(api_post /object-model/schemas '{
  "name": "E2eOrder",
  "tableName": "obm_id_e2e_order",
  "columns": [{"name": "sku", "type": "string"}]
}')
SCHEMA_ID=$(echo "${RESP}" | jq -r '.schemaId // empty')
if [ -z "${SCHEMA_ID}" ]; then
    print_error "Schema create failed: ${RESP}"
    exit 1
fi
echo "  Schema ${SCHEMA_ID} ✓"

print_step "Creating postgres connection for the object-model agent..."
RESP=$(api_post /connections "{
  \"title\": \"obm-id-e2e store\",
  \"integrationId\": \"postgres\",
  \"connectionParameters\": {\"database_url\": \"${SERVER_DB_URL}\"}
}")
CONN_ID=$(echo "${RESP}" | jq -r '.connection_id // .connectionId // empty')
if [ -z "${CONN_ID}" ]; then
    print_error "Connection create failed: ${RESP}"
    exit 1
fi
echo "  Connection ${CONN_ID} ✓"

#-------------------------------------------------------------------------
print_step "Creating workflow..."
RESP=$(api_post /workflows/create '{"name": "obm-id-roundtrip", "description": "create instance then load it back by id"}')
WF_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
if [ -z "${WF_ID}" ]; then
    print_error "Workflow create failed: ${RESP}"
    exit 1
fi
echo "  Workflow ${WF_ID} ✓"

print_step "Pushing workflow definition..."
DEFINITION=$(jq -n --arg conn "${CONN_ID}" '{
  name: "obm-id-roundtrip",
  steps: {
    create: {
      stepType: "Agent", id: "create",
      agentId: "object_model", capabilityId: "create-instance",
      connectionId: $conn,
      inputMapping: {
        schema_name: {valueType: "immediate", value: "E2eOrder"},
        data: {valueType: "immediate", value: {sku: "SKU-123"}}
      }
    },
    load: {
      stepType: "Agent", id: "load",
      agentId: "object_model", capabilityId: "query-instances",
      connectionId: $conn,
      inputMapping: {
        schema_name: {valueType: "immediate", value: "E2eOrder"},
        condition: {valueType: "immediate", value: {
          type: "operation", op: "EQ",
          arguments: [
            {valueType: "reference", value: "id"},
            {valueType: "reference", value: "steps.create.outputs.instance_id"}
          ]
        }}
      }
    },
    finish: {
      stepType: "Finish", id: "finish",
      inputMapping: {
        found_count: {valueType: "reference", value: "steps.load.outputs.total_count"},
        created_id: {valueType: "reference", value: "steps.create.outputs.instance_id"},
        loaded: {valueType: "reference", value: "steps.load.outputs.instances"}
      }
    }
  },
  entryPoint: "create",
  executionPlan: [
    {fromStep: "create", toStep: "load"},
    {fromStep: "load", toStep: "finish"}
  ],
  variables: {},
  inputSchema: {},
  outputSchema: {}
}')
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${DEFINITION}}")
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "Workflow update failed: ${RESP}"
    exit 1
fi
VERSION=$(curl -sS "${API}/workflows/${WF_ID}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
echo "  Definition pushed (version ${VERSION}) ✓"

print_step "Compiling version ${VERSION} (direct-wasm, in-process)..."
RESP=$(api_post "/workflows/${WF_ID}/versions/${VERSION}/compile" '{}' 900)
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "Compile failed: ${RESP}"
    tail -40 "${TEST_LOG}"
    exit 1
fi
echo "  Compiled ✓"

#-------------------------------------------------------------------------
print_step "Executing workflow..."
RESP=$(api_post "/workflows/${WF_ID}/execute" '{"inputs": {"data": {}}}')
INSTANCE_ID=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
if [ -z "${INSTANCE_ID}" ]; then
    print_error "Execute failed: ${RESP}"
    exit 1
fi
echo "  Instance ${INSTANCE_ID}"

STATUS=""
for i in {1..90}; do
    RESP=$(curl -sS "${API}/workflows/instances/${INSTANCE_ID}")
    STATUS=$(echo "${RESP}" | jq -r '.data.status // .status // empty')
    case "${STATUS}" in
        completed|failed|crashed|stopped) break ;;
    esac
    sleep 2
done
if [ "${STATUS}" != "completed" ]; then
    print_error "Instance did not complete (status='${STATUS}'): ${RESP}"
    tail -40 "${TEST_LOG}"
    exit 1
fi
echo "  Instance completed ✓"

#-------------------------------------------------------------------------
print_step "Asserting query-by-id found the created instance..."
OUTPUT=$(echo "${RESP}" | jq '.data.outputs // .outputs')
echo "  Output: ${OUTPUT}"

FOUND_COUNT=$(echo "${OUTPUT}" | jq -r '.found_count // 0')
CREATED_ID=$(echo "${OUTPUT}" | jq -r '.created_id // empty')
LOADED_ID=$(echo "${OUTPUT}" | jq -r '.loaded[0].id // .loaded[0].instance_id // empty')

if [ "${FOUND_COUNT}" != "1" ]; then
    print_error "EQ(id, <created id>) returned ${FOUND_COUNT} rows, expected 1 — query-by-id regression"
    exit 1
fi
if [ -z "${CREATED_ID}" ]; then
    print_error "created_id missing from workflow output"
    exit 1
fi
if [ "${LOADED_ID}" != "${CREATED_ID}" ]; then
    print_error "Loaded instance id '${LOADED_ID}' != created id '${CREATED_ID}'"
    exit 1
fi

print_success "query-instances EQ(id, steps.create.outputs.instance_id) returned the created row (id ${CREATED_ID})"
