#!/bin/bash
# E2E Test: a workflow's stateSchema declaration on an isolated live server.
#
#   1. A workflow saved with a stateSchema (labels, a currency number, an
#      enum and a datetime) returns it unchanged from the workflow GET and
#      the version-schemas endpoint.
#   2. The MCP set_state_schema tool replaces it and get_state_schema reads
#      it back; apply_graph_mutations accepts a set_state_schema operation.
#   3. The workflow still compiles: stateSchema is a declaration only.
#   4. A state field with `required: true` draws the W081 warning from the
#      save and from graph validation.
#
# Usage:  ./e2e/test_workflow_state_schema.sh
#
# Prereqs: Postgres + docker (isolated Valkey), a built runtara-server, and
# prebuilt components (scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD-GueUkDKea0CjKP4Rn5Bk0FDV}"

TEST_DB_SERVER="${TEST_DB_SERVER:-state_schema_e2e_server_$$}"
TEST_DB_RUNTIME="${TEST_DB_RUNTIME:-state_schema_e2e_runtime_$$}"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17760}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18761}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18762}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18763}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18764}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16396}"
TEST_DATA_DIR="$(mktemp -d -t runtara_state_schema_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="state_schema_e2e_$$"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

if [ -n "${POSTGRES_PASSWORD}" ]; then
    CREDENTIALS="${POSTGRES_USER}:${POSTGRES_PASSWORD}"
else
    CREDENTIALS="${POSTGRES_USER}"
fi
SERVER_DB_URL="postgresql://${CREDENTIALS}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${CREDENTIALS}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
BASE="http://127.0.0.1:${TEST_PORT_PUBLIC}"
API="${BASE}/api/runtime"

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql -X -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
}
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}
api_get() {
    curl -sS --max-time 60 "${API}$1"
}

stop_server() {
    if [ -n "${SERVER_PID}" ]; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
        SERVER_PID=""
    fi
}

cleanup() {
    local code=$?
    stop_server
    [ -n "${VALKEY_CONTAINER}" ] && docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER} WITH (FORCE)" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME} WITH (FORCE)" >/dev/null 2>&1 || true
    [ ${code} -ne 0 ] && [ -f "${TEST_LOG}" ] && { echo "--- server log tail ---"; tail -60 "${TEST_LOG}"; }
    rm -rf "${TEST_DATA_DIR}"
    exit ${code}
}
trap cleanup EXIT

start_server() {
    (
        # dotenvy walks parent directories; start outside the repository.
        cd "${TEST_DATA_DIR}"
        RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
        OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
        RUNTARA_DATABASE_URL="${RUNTIME_DB_URL}" \
        TENANT_ID="${TENANT}" \
        SERVER_HOST=127.0.0.1 \
        SERVER_PORT="${TEST_PORT_PUBLIC}" \
        RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
        RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
        RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
        RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
        RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
        DATA_DIR="${TEST_DATA_DIR}" \
        RUNTARA_DEV_MODE=true \
        RUST_LOG="${RUST_LOG_OVERRIDE:-warn,runtara_server=info}" \
        AUTH_PROVIDER=local \
        VALKEY_HOST=127.0.0.1 \
        VALKEY_PORT="${TEST_VALKEY_PORT}" \
        OTEL_SDK_DISABLED=true \
        SQLX_OFFLINE="${SQLX_OFFLINE}" \
        exec "${RUNTARA_SERVER_BIN}"
    ) >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!

    for _ in {1..90}; do
        if curl -sS -o /dev/null -w "%{http_code}" "${BASE}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        sleep 1
        if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
            print_error "Server exited during boot."; exit 1
        fi
    done
    print_error "Server did not become healthy."; exit 1
}

fail() { print_error "$1"; exit 1; }

# Assert jq filter $2 over JSON $1 is `true`, or fail with message $3.
expect_true() {
    local got
    got=$(echo "$1" | jq -r "$2" 2>/dev/null || echo "jq-error")
    [ "${got}" = "true" ] || fail "$3 (filter: $2)
$(echo "$1" | head -c 2000)"
}

# ---------------------------------------------------------------------------
# MCP over Streamable HTTP: initialize once, then JSON-RPC tool calls.
# ---------------------------------------------------------------------------
MCP_URL="${BASE}/mcp"
MCP_SESSION=""
MCP_ID=0

MCP_HEADERS="${TEST_DATA_DIR}/mcp_headers"

# POST a JSON-RPC message; echo the JSON-RPC response body (plain JSON or
# the last SSE `data:` line). Response headers land in ${MCP_HEADERS}.
mcp_post() {
    local body="$1" raw
    local session_header=()
    [ -n "${MCP_SESSION}" ] && session_header=(-H "Mcp-Session-Id: ${MCP_SESSION}")
    raw=$(curl -sS --max-time 120 -D "${MCP_HEADERS}" -X POST "${MCP_URL}" \
        -H "Content-Type: application/json" \
        -H "Accept: application/json, text/event-stream" \
        ${session_header[@]+"${session_header[@]}"} \
        -d "${body}")
    if echo "${raw}" | grep -q '^data:'; then
        echo "${raw}" | grep '^data:' | sed 's/^data: \{0,1\}//' | grep -v '^$' | tail -1
    else
        echo "${raw}"
    fi
}

mcp_init() {
    local resp
    resp=$(mcp_post '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"state-schema-e2e","version":"1"}}}')
    expect_true "${resp}" '.result.serverInfo != null' "MCP initialize failed"
    MCP_SESSION=$(grep -i '^mcp-session-id:' "${MCP_HEADERS}" | head -1 | cut -d' ' -f2 | tr -d '\r\n' || true)
    [ -n "${MCP_SESSION}" ] || fail "MCP initialize returned no session id"
    mcp_post '{"jsonrpc":"2.0","method":"notifications/initialized"}' >/dev/null
}

# Call tool $1 with arguments JSON $2; echo the tool's JSON result.
mcp_tool() {
    local resp
    MCP_ID=$((MCP_ID + 1))
    resp=$(mcp_post "$(jq -nc --arg name "$1" --argjson args "$2" --argjson id "${MCP_ID}" \
        '{jsonrpc: "2.0", id: $id, method: "tools/call", params: {name: $name, arguments: $args}}')")
    if [ "$(echo "${resp}" | jq -r '.result.isError // false')" = "true" ] || [ "$(echo "${resp}" | jq -r 'has("error")')" = "true" ]; then
        fail "MCP $1 failed: ${resp}"
    fi
    echo "${resp}" | jq -c '.result.content[0].text | fromjson'
}

echo "==============================================================="
echo "E2E: workflow stateSchema"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || fail "Missing server bin ${RUNTARA_SERVER_BIN} (cargo build -p runtara-server --bin runtara-server)"
[ -f "${COMPONENTS_DIR}/runtara_workflow_stdlib.wasm" ] || fail "Missing components in ${COMPONENTS_DIR} — run scripts/build-agent-components.sh"
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || fail "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"
docker info >/dev/null 2>&1 || fail "docker required (isolated Valkey)"

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases ${TEST_DB_SERVER} and ${TEST_DB_RUNTIME}..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
start_server

STATE_SCHEMA='{
  "order":    { "type": "string", "label": "Order" },
  "customer": { "type": "string", "label": "Customer" },
  "amount":   { "type": "number", "label": "Amount", "format": "currency" },
  "stage":    { "type": "string", "label": "Stage", "enum": ["received", "credit_check", "approval", "fulfilment", "delivered"] },
  "dueAt":    { "type": "string", "format": "datetime", "label": "Due" }
}'

# Assert that the stateSchema in JSON $1 (at jq path $2) matches the owner
# example: labels, the currency and datetime formats and the stage enum.
expect_owner_state_schema() {
    local json="$1" path="$2" where="$3"
    expect_true "${json}" "(${path} | keys | sort) == [\"amount\",\"customer\",\"dueAt\",\"order\",\"stage\"]" "${where}: state fields"
    expect_true "${json}" "${path}.order.label == \"Order\" and ${path}.customer.label == \"Customer\" and ${path}.amount.label == \"Amount\" and ${path}.stage.label == \"Stage\" and ${path}.dueAt.label == \"Due\"" "${where}: labels"
    expect_true "${json}" "${path}.amount.type == \"number\" and ${path}.amount.format == \"currency\"" "${where}: currency amount"
    expect_true "${json}" "${path}.dueAt.format == \"datetime\"" "${where}: datetime"
    expect_true "${json}" "${path}.stage.enum == [\"received\",\"credit_check\",\"approval\",\"fulfilment\",\"delivered\"]" "${where}: stage enum"
    expect_true "${json}" "[${path}[] | has(\"required\") and .required == true] | any | not" "${where}: no required state field"
}

GRAPH=$(jq -n --argjson state "${STATE_SCHEMA}" '{
    name: "state-schema-e2e",
    description: "stateSchema e2e",
    entryPoint: "finish",
    steps: { finish: { id: "finish", stepType: "Finish",
                       inputMapping: { ok: { valueType: "immediate", value: true } } } },
    executionPlan: [],
    inputSchema: { order: { type: "string", required: true } },
    outputSchema: { ok: { type: "boolean" } },
    stateSchema: $state
}')

# ---------------------------------------------------------------------------
# 1. Save and read back over HTTP.
# ---------------------------------------------------------------------------
print_step "1. Create and save a workflow declaring stateSchema..."
RESP=$(api_post /workflows/create '{"name": "state-schema-e2e", "description": "stateSchema e2e"}')
WF_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${WF_ID}" ] || fail "Workflow create failed: ${RESP}"
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${GRAPH}}")
expect_true "${RESP}" '.success == true' "Update with stateSchema failed"
expect_true "${RESP}" '[.warnings[]? | select(contains("[W081]"))] | length == 0' "The clean state schema drew W081"
VERSION=$(echo "${RESP}" | jq -r '.version // empty')
[ -n "${VERSION}" ] || VERSION=$(api_get "/workflows/${WF_ID}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max')
echo "  workflow ${WF_ID} version ${VERSION}"

WF=$(api_get "/workflows/${WF_ID}?versionNumber=${VERSION}")
expect_owner_state_schema "${WF}" '.data.stateSchema' "workflow GET stateSchema"
expect_owner_state_schema "${WF}" '.data.executionGraph.stateSchema' "workflow GET executionGraph.stateSchema"

SCHEMAS=$(api_get "/workflows/${WF_ID}/versions/${VERSION}/schemas")
expect_owner_state_schema "${SCHEMAS}" '.stateSchema' "version schemas"
expect_true "${SCHEMAS}" '.inputSchema.order.required == true and .outputSchema.ok.type == "boolean"' "version schemas input/output"
print_success "stateSchema round-trips through the workflow GET and version schemas"

# ---------------------------------------------------------------------------
# 2. MCP get/set_state_schema and the batch operation.
# ---------------------------------------------------------------------------
print_step "2. MCP get_state_schema / set_state_schema / apply_graph_mutations..."
mcp_init
GOT=$(mcp_tool get_state_schema "$(jq -nc --arg wf "${WF_ID}" '{workflow_id: $wf}')")
expect_true "${GOT}" '.count == 5' "get_state_schema count"
expect_owner_state_schema "${GOT}" '.stateSchema' "MCP get_state_schema"

NEW_STATE=$(echo "${STATE_SCHEMA}" | jq -c '. + {priority: {type: "integer", label: "Priority"}}')
SET=$(mcp_tool set_state_schema "$(jq -nc --arg wf "${WF_ID}" --argjson fields "${NEW_STATE}" '{workflow_id: $wf, fields: $fields}')")
expect_true "${SET}" '.success == true and .count == 6' "set_state_schema result"
GOT=$(mcp_tool get_state_schema "$(jq -nc --arg wf "${WF_ID}" '{workflow_id: $wf}')")
expect_true "${GOT}" '.count == 6 and .stateSchema.priority.label == "Priority" and .stateSchema.amount.format == "currency"' "get_state_schema after set"

BATCH_STATE=$(echo "${STATE_SCHEMA}" | jq -c 'del(.customer)')
BATCH=$(mcp_tool apply_graph_mutations "$(jq -nc --arg wf "${WF_ID}" --argjson fields "${BATCH_STATE}" \
    '{workflow_id: $wf, operations: [{op: "set_state_schema", fields: $fields}]}')")
expect_true "${BATCH}" '.success == true and .operationCount == 1' "apply_graph_mutations set_state_schema"
GOT=$(mcp_tool get_state_schema "$(jq -nc --arg wf "${WF_ID}" '{workflow_id: $wf}')")
expect_true "${GOT}" '.count == 4 and (.stateSchema | has("customer") | not) and .stateSchema.stage.label == "Stage"' "get_state_schema after batch"

LATEST=$(api_get "/workflows/${WF_ID}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max')
SCHEMAS=$(api_get "/workflows/${WF_ID}/versions/${LATEST}/schemas")
expect_true "${SCHEMAS}" '(.stateSchema | keys | sort) == ["amount","dueAt","order","stage"]' "version schemas after MCP edits"
SUMMARY=$(mcp_tool summarize_workflow "$(jq -nc --arg wf "${WF_ID}" '{workflow_id: $wf}')")
expect_true "${SUMMARY}" '.counts.stateFields == 4' "summarize_workflow stateFields"
print_success "MCP reads and replaces stateSchema (latest version ${LATEST})"

# ---------------------------------------------------------------------------
# 3. Compilation is unaffected.
# ---------------------------------------------------------------------------
print_step "3. Compile version ${LATEST} with its stateSchema..."
RESP=$(api_post "/workflows/${WF_ID}/versions/${LATEST}/compile" '{}' 900)
expect_true "${RESP}" '.success == true' "Compile with stateSchema failed"
print_success "The workflow compiles with a stateSchema"

# ---------------------------------------------------------------------------
# 4. W081 for a required state field.
# ---------------------------------------------------------------------------
print_step "4. W081 for a state field with required: true..."
BAD_GRAPH=$(echo "${GRAPH}" | jq -c '.stateSchema.stage.required = true')
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${BAD_GRAPH}}")
expect_true "${RESP}" '.success == true' "Update with a required state field failed (W081 is a warning)"
expect_true "${RESP}" '[.warnings[]? | select(contains("[W081]") and contains("'"'"'stage'"'"'") and contains("required"))] | length == 1' "Save did not warn W081"
echo "  $(echo "${RESP}" | jq -r '.warnings[] | select(contains("[W081]"))')"

RESP=$(api_post /workflows/graph/validate "${BAD_GRAPH}")
expect_true "${RESP}" '.valid == true' "Graph validation rejected a W081-only graph"
expect_true "${RESP}" '[.warnings[]? | select(contains("[W081]"))] | length == 1' "Graph validation did not warn W081"
print_success "W081 surfaces on save and in graph validation"

echo
print_success "All workflow stateSchema checks passed."
