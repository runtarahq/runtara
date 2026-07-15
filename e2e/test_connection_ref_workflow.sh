#!/bin/bash
# E2E Test: a step binds its connection through a resolvable `connectionRef`
# (direct-wasm engine).
#
# Proves the workflow-as-agent connection design end to end (see
# docs/workflow-agent-connections.md): a workflow declares a `connection`-typed
# INPUT and an Agent step binds to it with
#   "connectionRef": { "valueType": "reference", "value": "data.conn" }
# instead of a compile-time-pinned `connectionId`. The concrete id is supplied
# by the caller in `data` at execute time and resolved at runtime.
#
# The observable is the internal proxy's own error: the `http` agent forwards
# `_connection.connection_id` as `X-Runtara-Connection-Id`, and the proxy
# fail-closes an unknown id with `Connection '<id>' not found`. Feeding a
# DISTINCTIVE id in `data.conn` and asserting the proxy error names EXACTLY
# that id proves the ref resolved from the runtime input and threaded through
# the whole pipeline (save+validate → compile → resolve → agent → proxy). A
# second run with a different id confirms the id tracks the input, not a baked
# value.
#
# Full loop, all via the HTTP API of a self-contained server:
#   1. Create + update (save+validate) + compile a workflow:
#        http-request bound via connectionRef=data.conn → finish
#      (a `connection`-typed input + a ref-bound step must save clean — C4 —
#       and compile through the emitter — C3.)
#   2. Execute twice with two distinct connection ids in `data.conn`.
#   3. Assert each run's proxy error names EXACTLY the id that run supplied.
#
# Part 2 (nested scope): a Split iterates a list of items and its subgraph's
# http step binds connectionRef=data.conn — where `data` is the CURRENT ITEM,
# not the top level (which has no `conn`). Feeding items whose `conn` id exists
# ONLY inside the item proves the ref resolves against the iteration scope: if
# it wrongly used the top-level source, `data.conn` would be absent and the
# proxy would never name that id. This exercises the uniform resolution's
# per-scope `source` threading.
#
# Prerequisites: Postgres + docker (for an isolated Valkey) and the agent /
# shared workflow components in target/wasm32-wasip2/release (see
# scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"

TEST_DB_SERVER="conn_ref_e2e_server_$$"
TEST_DB_RUNTIME="conn_ref_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17710}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17711}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18711}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18712}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18713}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18714}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16391}"
TEST_DATA_DIR="$(mktemp -d -t runtara_conn_ref_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="conn_ref_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
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

# Run the workflow with a distinctive connection id in data.conn and assert the
# proxy fail-closes on EXACTLY that id — i.e. the ref resolved from the input.
assert_ref_resolves_to() {
    local probe_id="$1"
    print_step "Executing with data.conn='${probe_id}'..."
    local resp instance status err resolved
    resp=$(api_post "/workflows/${WF_ID}/execute" "{\"inputs\": {\"data\": {\"conn\": \"${probe_id}\"}}}")
    instance=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    if [ -z "${instance}" ]; then
        print_error "Execute failed: ${resp}"
        exit 1
    fi
    status=""
    for _ in {1..90}; do
        resp=$(curl -sS "${API}/workflows/instances/${instance}")
        status=$(echo "${resp}" | jq -r '.data.status // .status // empty')
        case "${status}" in completed|failed|crashed|stopped) break ;; esac
        sleep 2
    done
    err=$(echo "${resp}" | jq -r '.data.error // .error // empty')
    # The proxy error is `Connection '<id>' not found`; pull the named id back
    # out and require it to equal the id this run supplied.
    resolved=$(printf '%s' "${err}" | sed -n "s/.*Connection '\([^']*\)' not found.*/\1/p")
    if [ "${resolved}" != "${probe_id}" ]; then
        print_error "Ref did not resolve to the supplied id."
        print_error "  supplied : ${probe_id}"
        print_error "  in error : ${resolved:-<none>}"
        print_error "  status   : ${status}"
        print_error "  error    : ${err}"
        tail -40 "${TEST_LOG}"
        exit 1
    fi
    echo "  Proxy saw connection id '${resolved}' — matches data.conn ✓"
}

#-------------------------------------------------------------------------
echo "==============================================================="
echo "E2E Test: resolvable connectionRef (direct-wasm engine)"
echo "==============================================================="

if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi
for f in runtara_agent_http.wasm runtara_workflow_stdlib.wasm runtara_workflow_runtime.wasm; do
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
for _ in {1..20}; do
    if (echo > "/dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}") 2>/dev/null; then break; fi
    sleep 0.5
done

print_step "Creating test databases (${TEST_DB_SERVER}, ${TEST_DB_RUNTIME})..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null
SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
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
RUST_LOG="warn,runtara_server=info" \
AUTH_PROVIDER=local \
SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2 \
VALKEY_HOST=127.0.0.1 \
VALKEY_PORT="${TEST_VALKEY_PORT}" \
OTEL_SDK_DISABLED=true \
RUNTARA_SDK_BACKEND=http \
SQLX_OFFLINE="${SQLX_OFFLINE}" \
"${RUNTARA_SERVER_BIN}" >"${TEST_LOG}" 2>&1 &
SERVER_PID=$!

for _ in {1..60}; do
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
print_step "Creating workflow..."
RESP=$(api_post /workflows/create '{"name":"conn-ref-e2e","description":"resolvable connectionRef"}')
WF_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
if [ -z "${WF_ID}" ]; then
    print_error "Workflow create failed: ${RESP}"
    exit 1
fi
echo "  Workflow ${WF_ID} ✓"

print_step "Saving graph: http step bound via connectionRef=data.conn (save+validate)..."
GRAPH='{
  "name": "conn-ref-e2e",
  "steps": {
    "call": {
      "stepType": "Agent",
      "id": "call",
      "agentId": "http",
      "capabilityId": "http-request",
      "connectionRef": { "valueType": "reference", "value": "data.conn" },
      "inputMapping": {
        "url": { "valueType": "immediate", "value": "https://example.com/probe" },
        "method": { "valueType": "immediate", "value": "GET" }
      }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "boundConnection": { "valueType": "reference", "value": "data.conn" }
      }
    }
  },
  "entryPoint": "call",
  "executionPlan": [{ "fromStep": "call", "toStep": "finish" }],
  "variables": {},
  "inputSchema": {
    "conn": { "type": "connection", "integration": "api_key", "required": true }
  },
  "outputSchema": {}
}'
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${GRAPH}}")
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "Update/validate failed (a connection input + connectionRef step must save clean): ${RESP}"
    exit 1
fi
echo "  Saved + validated clean (C4: connection input + ref-bound step) ✓"

VERSION=$(curl -sS "${API}/workflows/${WF_ID}/versions" \
    | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')

print_step "Compiling version ${VERSION} (direct-wasm, in-process)..."
RESP=$(api_post "/workflows/${WF_ID}/versions/${VERSION}/compile" '{}' 900)
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "Compile failed (C3: connectionRef lowering): ${RESP}"
    tail -40 "${TEST_LOG}"
    exit 1
fi
echo "  Compiled ✓"

#-------------------------------------------------------------------------
# Two runs with two distinct ids: each proxy error must name its own id, so the
# resolved connection tracks the runtime input rather than a baked value.
assert_ref_resolves_to "conn-ref-probe-ALPHA-$$"
assert_ref_resolves_to "conn-ref-probe-BETA-$$"

#-------------------------------------------------------------------------
# Part 2 — nested (Split) scope: the ref must resolve against the ITERATION
# item, not the top-level source.
print_step "Creating Split workflow: subgraph http step binds connectionRef=data.conn (item)..."
RESP=$(api_post /workflows/create '{"name":"conn-ref-split-e2e","description":"nested connectionRef scope"}')
SPLIT_WF_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
if [ -z "${SPLIT_WF_ID}" ]; then
    print_error "Split workflow create failed: ${RESP}"
    exit 1
fi
SPLIT_GRAPH='{
  "name": "conn-ref-split-e2e",
  "steps": {
    "split": {
      "stepType": "Split", "id": "split",
      "config": { "value": { "valueType": "reference", "value": "data.items" }, "sequential": true },
      "inputSchema": { "conn": { "type": "connection", "integration": "api_key", "required": true } },
      "subgraph": {
        "steps": {
          "call": {
            "stepType": "Agent", "id": "call", "agentId": "http", "capabilityId": "http-request",
            "connectionRef": { "valueType": "reference", "value": "data.conn" },
            "inputMapping": {
              "url": { "valueType": "immediate", "value": "https://example.com/probe" },
              "method": { "valueType": "immediate", "value": "GET" }
            }
          },
          "sfinish": { "stepType": "Finish", "id": "sfinish",
            "inputMapping": { "out": { "valueType": "reference", "value": "steps.call.outputs" } } }
        },
        "entryPoint": "call",
        "executionPlan": [{ "fromStep": "call", "toStep": "sfinish" }]
      }
    },
    "finish": { "stepType": "Finish", "id": "finish",
      "inputMapping": { "results": { "valueType": "reference", "value": "steps.split.outputs" } } }
  },
  "entryPoint": "split",
  "executionPlan": [{ "fromStep": "split", "toStep": "finish" }],
  "variables": {},
  "inputSchema": { "items": { "type": "array", "required": true } },
  "outputSchema": {}
}'
RESP=$(api_post "/workflows/${SPLIT_WF_ID}/update" "{\"executionGraph\": ${SPLIT_GRAPH}}")
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "Split update/validate failed: ${RESP}"
    exit 1
fi
SPLIT_VERSION=$(curl -sS "${API}/workflows/${SPLIT_WF_ID}/versions" \
    | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(api_post "/workflows/${SPLIT_WF_ID}/versions/${SPLIT_VERSION}/compile" '{}' 900)
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "Split compile failed: ${RESP}"
    tail -40 "${TEST_LOG}"
    exit 1
fi
echo "  Split compiled ✓"

# The first item's `conn` id exists ONLY inside data.items[0].conn — never at
# top-level `data.conn`. A sequential Split fails-fast on item 0, so the proxy
# error must name that item-scoped id.
ITEM_ID="split-item-conn-${$}"
print_step "Executing Split with items[0].conn='${ITEM_ID}'..."
RESP=$(api_post "/workflows/${SPLIT_WF_ID}/execute" \
    "{\"inputs\": {\"data\": {\"items\": [{\"conn\": \"${ITEM_ID}\"}, {\"conn\": \"split-item-2\"}]}}}")
SPLIT_INSTANCE=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
SPLIT_STATUS=""
for _ in {1..90}; do
    RESP=$(curl -sS "${API}/workflows/instances/${SPLIT_INSTANCE}")
    SPLIT_STATUS=$(echo "${RESP}" | jq -r '.data.status // .status // empty')
    case "${SPLIT_STATUS}" in completed|failed|crashed|stopped) break ;; esac
    sleep 2
done
SPLIT_ERR=$(echo "${RESP}" | jq -r '.data.error // .error // empty')
SPLIT_RESOLVED=$(printf '%s' "${SPLIT_ERR}" | sed -n "s/.*Connection '\([^']*\)' not found.*/\1/p")
if [ "${SPLIT_RESOLVED}" != "${ITEM_ID}" ]; then
    print_error "Nested ref did not resolve against the iteration item."
    print_error "  item[0].conn : ${ITEM_ID}"
    print_error "  in error     : ${SPLIT_RESOLVED:-<none>}"
    print_error "  error        : ${SPLIT_ERR}"
    tail -40 "${TEST_LOG}"
    exit 1
fi
echo "  Proxy saw item-scoped id '${SPLIT_RESOLVED}' — resolved against the Split iteration ✓"

print_success "connectionRef resolves at runtime — top-level input and per-iteration Split scope"
