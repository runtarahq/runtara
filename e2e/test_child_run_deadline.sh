#!/bin/bash
# E2E Test: a child-run step's timeout is its deadline.
#
#   1. PUBLISH — `nap` sleeps (a durable Delay) for the `ms` its input names.
#      It parks, so a call to it is a child run of its own.
#   2. IN TIME — a parent calls `nap` for 3s under a 60s timeout. It parks on
#      the child and completes with the child's output.
#   3. TIMED OUT — a parent calls `nap` for 60s under a 3s timeout. The
#      parent's wait settles at the deadline, the parent wakes, cancels the
#      child and fails the step with AGENT_TIMEOUT, well before the child
#      would have woken.
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

TEST_DB_SERVER="child_deadline_e2e_server_$$"
TEST_DB_RUNTIME="child_deadline_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17780}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18781}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18782}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18783}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18784}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16398}"
TEST_DATA_DIR="$(mktemp -d -t runtara_child_deadline_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="child_deadline_e2e"

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
    resp=$(api_post /workflows/create "{\"name\":\"${name}\",\"description\":\"deadline e2e\"}")
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
echo "E2E Test: a child-run step times out at its deadline"
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
MAX_CONCURRENT_EXECUTIONS=40 \
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
RESP=$(api_post /workflows/create '{"name":"Nap","description":"deadline e2e","slug":"nap"}')
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

# A parent calling nap for `ms` under `timeoutMs`.
parent_graph() {
    local ms="$1" timeout_ms="$2"
    cat <<JSON
{
  "name": "Call Nap",
  "durable": true,
  "steps": {
    "call": {
      "stepType": "Agent",
      "id": "call",
      "agentId": "nap",
      "capabilityId": "run",
      "maxRetries": 0,
      "timeoutMs": ${timeout_ms},
      "inputMapping": { "ms": { "valueType": "immediate", "value": ${ms} } }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": { "slept": { "valueType": "reference", "value": "steps.call.outputs.slept" } }
    }
  },
  "entryPoint": "call",
  "executionPlan": [{ "fromStep": "call", "toStep": "finish" }],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}
JSON
}

# Run a parent to its end; echoes "<status> <seconds> <instance id>".
run_parent() {
    local wf_id="$1" resp instance started status
    resp=$(api_post "/workflows/${wf_id}/execute" '{"inputs": {"data": {}}}')
    instance=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    [ -n "${instance}" ] || { print_error "execute failed: ${resp}"; exit 1; }
    started=$(date +%s)
    for _ in {1..120}; do
        status=$(curl -sS "${API}/workflows/instances/${instance}" | jq -r '.data.status // empty')
        case "${status}" in completed|failed|crashed|stopped|cancelled) break ;; esac
        sleep 0.5
    done
    echo "${status} $(( $(date +%s) - started )) ${instance}"
}

#-------------------------------------------------------------------------
print_step "2. A child that ends within the timeout..."
IN_TIME_ID=$(create_and_compile "Nap In Time" "$(parent_graph 3000 60000)")
read -r STATUS ELAPSED PARENT <<<"$(run_parent "${IN_TIME_ID}")"
RESP=$(curl -sS "${API}/workflows/instances/${PARENT}")
[ "${STATUS}" = "completed" ] \
    || { print_error "the in-time parent ended '${STATUS}': $(echo "${RESP}" | jq -c '.data.error // empty')"; tail -60 "${TEST_LOG}"; exit 1; }
echo "${RESP}" | jq -e '.data.outputs.slept == 3000' >/dev/null \
    || { print_error "the parent must return the child's output: $(echo "${RESP}" | jq -c '.data.outputs')"; exit 1; }
[ "${ELAPSED}" -ge 3 ] \
    || { print_error "the parent must wait for its child, it ended after ${ELAPSED}s"; exit 1; }
KIDS=$(children_of "${PARENT}")
echo "${KIDS}" | jq -e 'length == 1 and .[0].status == "completed"' >/dev/null \
    || { print_error "one completed child expected: ${KIDS}"; exit 1; }
echo "  completed after ${ELAPSED}s with the child's output; the deadline did not fire early ✓"

#-------------------------------------------------------------------------
print_step "3. A child that outlives the timeout..."
TIMED_OUT_ID=$(create_and_compile "Nap Timed Out" "$(parent_graph 60000 3000)")
read -r STATUS ELAPSED PARENT <<<"$(run_parent "${TIMED_OUT_ID}")"
RESP=$(curl -sS "${API}/workflows/instances/${PARENT}")
[ "${STATUS}" = "failed" ] \
    || { print_error "the timed-out parent ended '${STATUS}' after ${ELAPSED}s: $(echo "${RESP}" | jq -c '.data.error // empty')"; tail -60 "${TEST_LOG}"; exit 1; }
echo "${RESP}" | jq -r '.data.error // empty' | grep -q "AGENT_TIMEOUT" \
    || { print_error "the step must fail with AGENT_TIMEOUT: $(echo "${RESP}" | jq -c '.data.error')"; exit 1; }
[ "${ELAPSED}" -lt 30 ] \
    || { print_error "the parent must time out at its 3s deadline, not after ${ELAPSED}s"; exit 1; }
echo "  failed with AGENT_TIMEOUT after ${ELAPSED}s ✓"
KIDS=""
for _ in {1..20}; do
    KIDS=$(children_of "${PARENT}")
    echo "${KIDS}" | jq -e 'length == 1 and (.[0].status | IN("running", "suspended", "pending", "queued") | not)' >/dev/null && break
    sleep 0.5
done
echo "${KIDS}" | jq -e 'length == 1 and (.[0].status | IN("cancelled", "stopped"))' >/dev/null \
    || { print_error "the timed-out child must be cancelled: ${KIDS}"; exit 1; }
echo "  the child was cancelled ($(echo "${KIDS}" | jq -r '.[0].status')) ✓"

print_success "child-run deadline: a child within the timeout completes the step; one past it is cancelled and the step times out"
