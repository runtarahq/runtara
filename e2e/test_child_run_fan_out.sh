#!/bin/bash
# E2E Test: a parallel Split fans a parking workflow-agent out as child runs.
#
# One parent, twenty children, half of them asleep:
#
#   1. PUBLISH — `maybe-sleep` sleeps (a durable Delay) when its input says
#      so, and otherwise finishes at once. It may park, so its staged meta
#      binds it to its version with `workflowRun.parks`.
#   2. FAN OUT — a durable parent Splits twenty items over `maybe-sleep` with
#      parallelism 20. The window starts all twenty children before any of
#      them parks, and parks on one wait over all of them; the ten that do not
#      sleep complete while the ten sleepers
#      sleep, and the parent parks on a wait per sleeping child.
#   3. JOIN — every wake replays the Split, which finds the same twenty
#      children (no duplicates), and the parent completes once all have ended,
#      with each item's result in order.
#   4. BRANCHES — a fan-out of two branches, each calling `maybe-sleep` to
#      sleep, starts both children before parking on the first, and finds
#      the same two when it completes.
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

TEST_DB_SERVER="child_fan_out_e2e_server_$$"
TEST_DB_RUNTIME="child_fan_out_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17770}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18771}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18772}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18773}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18774}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16397}"
TEST_DATA_DIR="$(mktemp -d -t runtara_child_fan_out_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="child_fan_out_e2e"
# How long a sleeping child sleeps. Long enough that the snapshot below sees
# every non-sleeper done while every sleeper still sleeps.
SLEEP_MS="${SLEEP_MS:-15000}"

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
    resp=$(api_post /workflows/create "{\"name\":\"${name}\",\"description\":\"fan-out e2e\"}")
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
echo "E2E Test: a parallel Split fans a parking workflow-agent out"
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

# Twenty children and their parent fit the tenant's limit and its control share
# with room to spare, so no start is refused for capacity.
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
print_step "1. Publish maybe-sleep, a workflow-agent that parks..."
RESP=$(api_post /workflows/create '{"name":"Maybe Sleep","description":"fan-out e2e","slug":"maybe-sleep"}')
CHILD_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${CHILD_ID}" ] || { print_error "child create failed: ${RESP}"; exit 1; }
CHILD_GRAPH='{
  "name": "Maybe Sleep",
  "durable": true,
  "steps": {
    "check": {
      "stepType": "Conditional",
      "id": "check",
      "condition": {
        "type": "operation",
        "op": "EQ",
        "arguments": [
          { "valueType": "reference", "value": "data.sleep" },
          { "valueType": "immediate", "value": true }
        ]
      }
    },
    "nap": {
      "stepType": "Delay",
      "id": "nap",
      "durationMs": { "valueType": "immediate", "value": '"${SLEEP_MS}"' }
    },
    "slept": {
      "stepType": "Finish",
      "id": "slept",
      "inputMapping": {
        "slept": { "valueType": "immediate", "value": true },
        "item": { "valueType": "reference", "value": "data.item" }
      }
    },
    "awake": {
      "stepType": "Finish",
      "id": "awake",
      "inputMapping": {
        "slept": { "valueType": "immediate", "value": false },
        "item": { "valueType": "reference", "value": "data.item" }
      }
    }
  },
  "entryPoint": "check",
  "executionPlan": [
    { "fromStep": "check", "toStep": "nap", "label": "true" },
    { "fromStep": "check", "toStep": "awake", "label": "false" },
    { "fromStep": "nap", "toStep": "slept" }
  ],
  "variables": {},
  "inputSchema": {
    "item": { "type": "integer", "required": true },
    "sleep": { "type": "boolean", "required": true }
  },
  "outputSchema": {}
}'
RESP=$(api_post "/workflows/${CHILD_ID}/update" "{\"executionGraph\": ${CHILD_GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] \
    || { print_error "child update failed: ${RESP}"; exit 1; }
RESP=$(api_post "/workflows/${CHILD_ID}/publish-agent" "" 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] \
    || { print_error "publish-agent failed: ${RESP}"; tail -40 "${TEST_LOG}"; exit 1; }
META="${TEST_DATA_DIR}/workflow-agents/${TENANT}/runtara_agent_maybe_sleep.meta.json"
[ -f "${META}" ] || META=$(find "${TEST_DATA_DIR}" -name 'runtara_agent_maybe_sleep.meta.json' | head -1)
[ -n "${META}" ] && [ -f "${META}" ] || { print_error "published sidecar not staged"; exit 1; }
jq -e --arg id "${CHILD_ID}" \
    '.capabilities[0].workflowRun | .parks == true and .workflowId == $id' "${META}" >/dev/null \
    || { print_error "maybe-sleep must be bound to a version that parks: $(jq -c '.capabilities[0].workflowRun' "${META}")"; exit 1; }
echo "  maybe-sleep published; its version parks (workflowRun.parks) ✓"

#-------------------------------------------------------------------------
print_step "2. A parallel Split starts twenty children at once..."
PARENT_GRAPH='{
  "name": "Fan Out Maybe Sleep",
  "durable": true,
  "steps": {
    "split": {
      "stepType": "Split",
      "id": "split",
      "config": {
        "value": { "valueType": "reference", "value": "data.items" },
        "parallelism": 20,
        "maxRetries": 0
      },
      "subgraph": {
        "name": "Call Maybe Sleep",
        "steps": {
          "call": {
            "stepType": "Agent",
            "id": "call",
            "agentId": "maybe-sleep",
            "capabilityId": "run",
            "maxRetries": 0,
            "inputMapping": {
              "item": { "valueType": "reference", "value": "data.item" },
              "sleep": { "valueType": "reference", "value": "data.sleep" }
            }
          },
          "done": {
            "stepType": "Finish",
            "id": "done",
            "inputMapping": {
              "item": { "valueType": "reference", "value": "steps.call.outputs.item" },
              "slept": { "valueType": "reference", "value": "steps.call.outputs.slept" }
            }
          }
        },
        "entryPoint": "call",
        "executionPlan": [{ "fromStep": "call", "toStep": "done" }]
      }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": { "results": { "valueType": "reference", "value": "steps.split.outputs" } }
    }
  },
  "entryPoint": "split",
  "executionPlan": [{ "fromStep": "split", "toStep": "finish" }],
  "variables": {},
  "inputSchema": { "items": { "type": "array", "required": true } },
  "outputSchema": {}
}'
PARENT_ID=$(create_and_compile "Fan Out Maybe Sleep" "${PARENT_GRAPH}")
ITEMS=$(jq -nc '[range(20) | {item: ., sleep: (. % 2 == 1)}]')
RESP=$(api_post "/workflows/${PARENT_ID}/execute" "{\"inputs\": {\"data\": {\"items\": ${ITEMS}}}}")
PARENT=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${PARENT}" ] || { print_error "parent execute failed: ${RESP}"; exit 1; }
STARTED_AT=$(date +%s)

# Snapshot the children while the sleepers sleep: all twenty exist, the ten
# that do not sleep have completed, and the five sleepers have not.
SNAPSHOT=""
PARENT_STATUS=""
for _ in {1..60}; do
    SNAPSHOT=$(children_of "${PARENT}")
    COMPLETED=$(echo "${SNAPSHOT}" | jq '[.[] | select(.status == "completed")] | length')
    TOTAL=$(echo "${SNAPSHOT}" | jq 'length')
    PARENT_STATUS=$(curl -sS "${API}/workflows/instances/${PARENT}" | jq -r '.data.status // empty')
    case "${PARENT_STATUS}" in completed|failed|crashed|stopped) break ;; esac
    [ "${TOTAL}" = "20" ] && [ "${COMPLETED}" -ge 10 ] && break
    sleep 0.5
done
ELAPSED=$(( $(date +%s) - STARTED_AT ))
TOTAL=$(echo "${SNAPSHOT}" | jq 'length')
COMPLETED=$(echo "${SNAPSHOT}" | jq '[.[] | select(.status == "completed")] | length')
[ "${TOTAL}" = "20" ] \
    || { print_error "the window must start all twenty children, saw ${TOTAL}: ${SNAPSHOT}"; tail -60 "${TEST_LOG}"; exit 1; }
[ "${COMPLETED}" = "10" ] \
    || { print_error "ten children must complete while ten sleep, saw ${COMPLETED} completed after ${ELAPSED}s: ${SNAPSHOT}"; tail -60 "${TEST_LOG}"; exit 1; }
[ "${PARENT_STATUS}" = "suspended" ] \
    || { print_error "the parent must park while its children sleep, it is '${PARENT_STATUS}'"; tail -60 "${TEST_LOG}"; exit 1; }
echo "  twenty children started in one window; ten completed, ten still sleeping after ${ELAPSED}s ✓"
echo "  the parent parked (${PARENT_STATUS}) while its children sleep ✓"

# The ten that completed are the ten that did not sleep.
for CHILD in $(echo "${SNAPSHOT}" | jq -r '.[] | select(.status == "completed") | .id'); do
    OUT=$(curl -sS "${API}/workflows/instances/${CHILD}" | jq -c '.data.outputs')
    echo "${OUT}" | jq -e '.slept == false and (.item % 2 == 0)' >/dev/null \
        || { print_error "a child that completed early must be a non-sleeper: ${CHILD} → ${OUT}"; exit 1; }
done
echo "  the early ten are the even (non-sleeping) items ✓"

#-------------------------------------------------------------------------
print_step "3. The parent completes once all twenty children have ended..."
for _ in {1..90}; do
    RESP=$(curl -sS "${API}/workflows/instances/${PARENT}")
    PARENT_STATUS=$(echo "${RESP}" | jq -r '.data.status // empty')
    case "${PARENT_STATUS}" in completed|failed|crashed|stopped) break ;; esac
    sleep 1
done
[ "${PARENT_STATUS}" = "completed" ] \
    || { print_error "the parent ended '${PARENT_STATUS}': $(echo "${RESP}" | jq -c '.data.error // empty')"; tail -60 "${TEST_LOG}"; exit 1; }
RESULTS=$(echo "${RESP}" | jq -c '.data.outputs.results | if type == "object" and has("data") then .data else . end')
echo "${RESULTS}" | jq -e '
    length == 20
    and ([.[] | .item] == [range(20)])
    and all(.[]; .slept == (.item % 2 == 1))' >/dev/null \
    || { print_error "the parent must return twenty results in order, slept = odd: ${RESULTS}"; exit 1; }
echo "  twenty results in order, each slept exactly when its item is odd ✓"

FINAL=$(children_of "${PARENT}")
echo "${FINAL}" | jq -e 'length == 20 and all(.[]; .status == "completed")' >/dev/null \
    || { print_error "replays must find the same twenty children, all completed: ${FINAL}"; exit 1; }
echo "  replays found the same twenty children (no duplicates), all completed ✓"

#-------------------------------------------------------------------------
print_step "4. Parallel branches start their children together..."
BRANCH_GRAPH='{
  "name": "Branch Maybe Sleep",
  "durable": true,
  "steps": {
    "start": { "stepType": "Log", "id": "start", "message": "fan out" },
    "first": {
      "stepType": "Agent", "id": "first", "agentId": "maybe-sleep", "capabilityId": "run",
      "maxRetries": 0,
      "inputMapping": {
        "item": { "valueType": "immediate", "value": 1 },
        "sleep": { "valueType": "immediate", "value": true }
      }
    },
    "second": {
      "stepType": "Agent", "id": "second", "agentId": "maybe-sleep", "capabilityId": "run",
      "maxRetries": 0,
      "inputMapping": {
        "item": { "valueType": "immediate", "value": 2 },
        "sleep": { "valueType": "immediate", "value": true }
      }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "first": { "valueType": "reference", "value": "steps.first.outputs.item" },
        "second": { "valueType": "reference", "value": "steps.second.outputs.item" }
      }
    }
  },
  "entryPoint": "start",
  "executionPlan": [
    { "fromStep": "start", "toStep": "first" },
    { "fromStep": "start", "toStep": "second" },
    { "fromStep": "first", "toStep": "finish" },
    { "fromStep": "second", "toStep": "finish" }
  ],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}'
BRANCH_ID=$(create_and_compile "Branch Maybe Sleep" "${BRANCH_GRAPH}")
RESP=$(api_post "/workflows/${BRANCH_ID}/execute" '{"inputs": {"data": {}}}')
PARENT=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${PARENT}" ] || { print_error "branch parent execute failed: ${RESP}"; exit 1; }
# Both children exist while the first still sleeps: the second branch did not
# wait for the first one's child to end before starting its own.
SNAPSHOT=""
for _ in {1..20}; do
    SNAPSHOT=$(children_of "${PARENT}")
    [ "$(echo "${SNAPSHOT}" | jq 'length')" = "2" ] && break
    sleep 0.5
done
echo "${SNAPSHOT}" | jq -e 'length == 2 and all(.[]; .status != "completed")' >/dev/null \
    || { print_error "both branch children must run at once: ${SNAPSHOT}"; tail -60 "${TEST_LOG}"; exit 1; }
echo "  both branch children started before either ended: $(echo "${SNAPSHOT}" | jq -c '[.[].status]') ✓"
for _ in {1..90}; do
    RESP=$(curl -sS "${API}/workflows/instances/${PARENT}")
    PARENT_STATUS=$(echo "${RESP}" | jq -r '.data.status // empty')
    case "${PARENT_STATUS}" in completed|failed|crashed|stopped) break ;; esac
    sleep 1
done
[ "${PARENT_STATUS}" = "completed" ] \
    || { print_error "the branch parent ended '${PARENT_STATUS}': $(echo "${RESP}" | jq -c '.data.error // empty')"; tail -60 "${TEST_LOG}"; exit 1; }
echo "${RESP}" | jq -e '.data.outputs.first == 1 and .data.outputs.second == 2' >/dev/null \
    || { print_error "the branch parent must return both results: $(echo "${RESP}" | jq -c '.data.outputs')"; exit 1; }
FINAL=$(children_of "${PARENT}")
echo "${FINAL}" | jq -e 'length == 2 and all(.[]; .status == "completed")' >/dev/null \
    || { print_error "the branches must find the two children they started, both completed: ${FINAL}"; exit 1; }
echo "  completed with both results; still exactly two children ✓"

print_success "child-run fan-out: twenty children started in one window, ten finished while ten slept, the parent parked and completed with every result; parallel branches started their children together"
