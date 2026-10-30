#!/bin/bash
# E2E Test: a child-run start refused for capacity parks and starts again.
#
# The tenant's limit is 3, so control's share (4/5, at least 1) is 2 runs:
#
#   1. PUBLISH — `nap` sleeps (a durable Delay) for the `ms` its input names.
#   2. SPLIT — a parallel Split over four naps starts its window's children
#      faster than they launch, so starts beyond the share are refused for
#      capacity. The parent parks on the retry hint (and on its children's
#      waits), starts the refused ones again, and completes with four
#      results from four children.
#
# A plain Agent step takes the same `capacity` answer and retry park; no
# workflow here can hold a slot without parking, so that path is covered
# by the emitter's tests only.
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

TEST_DB_SERVER="child_capacity_e2e_server_$$"
TEST_DB_RUNTIME="child_capacity_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17790}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18791}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18792}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18793}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18794}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16399}"
TEST_DATA_DIR="$(mktemp -d -t runtara_child_capacity_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="child_capacity_e2e"

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

api_post() {
    curl -sS --max-time "${3:-120}" -X POST -H "Content-Type: application/json" \
        ${2:+-d "$2"} "${API}$1"
}

# Create a workflow, save its graph, compile it; echoes the workflow id.
create_and_compile() {
    local name="$1" graph="$2"
    local resp wf_id version
    resp=$(api_post /workflows/create "{\"name\":\"${name}\",\"description\":\"capacity e2e\"}")
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

# The parent's child runs, as `[{id, status}]`.
children_of() {
    curl -sS "${API}/executions?parentInstanceId=$1&size=100" \
        | jq -c '[.data.content[]? | {id: (.instanceId // .id), status: .status}]'
}

#-------------------------------------------------------------------------
echo "==============================================================="
echo "E2E Test: a child-run start refused for capacity starts again"
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

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
TENANT_ID="${TENANT}" \
SERVER_HOST=127.0.0.1 \
SERVER_PORT="${TEST_PORT_PUBLIC}" \
RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
MAX_CONCURRENT_EXECUTIONS=3 \
DATA_DIR="${TEST_DATA_DIR}" \
RUST_LOG="warn,runtara_server=info" \
AUTH_PROVIDER=local \
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
    kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "Server exited during boot."; tail -30 "${TEST_LOG}"; exit 1; }
done
echo "  Server up (PID ${SERVER_PID})"

#-------------------------------------------------------------------------
print_step "1. Publish nap, a workflow-agent that parks..."
RESP=$(api_post /workflows/create '{"name":"Nap","description":"capacity e2e","slug":"nap"}')
CHILD_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${CHILD_ID}" ] || { print_error "child create failed: ${RESP}"; exit 1; }
CHILD_GRAPH='{
  "name": "Nap",
  "durable": true,
  "steps": {
    "nap": {
      "stepType": "Delay",
      "id": "nap",
      "durationMs": { "valueType": "reference", "value": "data.ms" }
    },
    "done": {
      "stepType": "Finish",
      "id": "done",
      "inputMapping": { "slept": { "valueType": "reference", "value": "data.ms" } }
    }
  },
  "entryPoint": "nap",
  "executionPlan": [{ "fromStep": "nap", "toStep": "done" }],
  "variables": {},
  "inputSchema": { "ms": { "type": "integer", "required": true } },
  "outputSchema": {}
}'
RESP=$(api_post "/workflows/${CHILD_ID}/update" "{\"executionGraph\": ${CHILD_GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] \
    || { print_error "child update failed: ${RESP}"; exit 1; }
RESP=$(api_post "/workflows/${CHILD_ID}/publish-agent" "" 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] \
    || { print_error "publish-agent failed: ${RESP}"; tail -40 "${TEST_LOG}"; exit 1; }
META=$(find "${TEST_DATA_DIR}" -name 'runtara_agent_nap.meta.json' | head -1)
[ -n "${META}" ] && [ -f "${META}" ] || { print_error "published sidecar not staged"; exit 1; }
jq -e '.capabilities[0].workflowRun.parks == true' "${META}" >/dev/null \
    || { print_error "nap must be bound to a version that parks: $(jq -c '.capabilities[0].workflowRun' "${META}")"; exit 1; }
echo "  nap published; its version parks ✓"


# Poll an instance until it ends; echoes its status.
wait_for_end() {
    local instance="$1" status=""
    for _ in {1..180}; do
        status=$(curl -sS "${API}/workflows/instances/${instance}" | jq -r '.data.status // empty')
        case "${status}" in completed|failed|crashed|stopped|cancelled) break ;; esac
        sleep 0.5
    done
    echo "${status}"
}

# Whether the server refused a child-run start of `instance` for capacity.
refused_for_capacity() {
    grep -c "child run start refused instance_id=$1 .*code=Capacity" "${TEST_LOG}" || true
}

#-------------------------------------------------------------------------
print_step "2. A parallel Split over four naps, with room for two..."
SPLIT_GRAPH='{
  "name": "Split Nap",
  "durable": true,
  "steps": {
    "split": {
      "stepType": "Split",
      "id": "split",
      "config": {
        "value": { "valueType": "reference", "value": "data.items" },
        "parallelism": 4,
        "maxRetries": 0
      },
      "subgraph": {
        "name": "Call Nap",
        "steps": {
          "call": {
            "stepType": "Agent", "id": "call", "agentId": "nap", "capabilityId": "run",
            "maxRetries": 0,
            "inputMapping": { "ms": { "valueType": "reference", "value": "data.ms" } }
          },
          "done": {
            "stepType": "Finish", "id": "done",
            "inputMapping": { "slept": { "valueType": "reference", "value": "steps.call.outputs.slept" } }
          }
        },
        "entryPoint": "call",
        "executionPlan": [{ "fromStep": "call", "toStep": "done" }]
      }
    },
    "finish": {
      "stepType": "Finish", "id": "finish",
      "inputMapping": { "results": { "valueType": "reference", "value": "steps.split.outputs" } }
    }
  },
  "entryPoint": "split",
  "executionPlan": [{ "fromStep": "split", "toStep": "finish" }],
  "variables": {},
  "inputSchema": { "items": { "type": "array", "required": true } },
  "outputSchema": {}
}'
SPLIT_ID=$(create_and_compile "Split Nap" "${SPLIT_GRAPH}")
RESP=$(api_post "/workflows/${SPLIT_ID}/execute" \
    '{"inputs": {"data": {"items": [{"ms": 8000}, {"ms": 8000}, {"ms": 8000}, {"ms": 8000}]}}}')
SPLIT_PARENT=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${SPLIT_PARENT}" ] || { print_error "split execute failed: ${RESP}"; exit 1; }
STATUS=$(wait_for_end "${SPLIT_PARENT}")
RESP=$(curl -sS "${API}/workflows/instances/${SPLIT_PARENT}")
[ "${STATUS}" = "completed" ] \
    || { print_error "the split parent ended '${STATUS}': $(echo "${RESP}" | jq -c '.data.error // empty')"; tail -60 "${TEST_LOG}"; exit 1; }
REFUSED=$(refused_for_capacity "${SPLIT_PARENT}")
[ "${REFUSED}" -ge 1 ] \
    || { print_error "the window's starts must have met the full share"; exit 1; }
RESULTS=$(echo "${RESP}" | jq -c '.data.outputs.results | if type == "object" and has("data") then .data else . end')
echo "${RESULTS}" | jq -e 'length == 4 and all(.[]; .slept == 8000)' >/dev/null \
    || { print_error "the split must return four results: ${RESULTS}"; exit 1; }
KIDS=$(children_of "${SPLIT_PARENT}")
echo "${KIDS}" | jq -e 'length == 4 and all(.[]; .status == "completed")' >/dev/null \
    || { print_error "the split must start each child once: ${KIDS}"; exit 1; }
echo "  ${REFUSED} start(s) refused for capacity were started again; four results from four children ✓"

print_success "child-run capacity: starts refused for capacity park on the retry hint and start again"
