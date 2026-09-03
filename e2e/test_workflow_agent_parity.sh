#!/bin/bash
# E2E Test: full workflow<>agent parity on a live server (direct-wasm engine).
#
# Proves the whole publish-and-invoke pipeline end to end:
#
#   1. SLUG — created workflows get an auto-derived slug (WIT-safe capability
#      id); an explicit duplicate slug 409s; the dedicated slug endpoint edits
#      it (identity-level, never the graph path).
#   2. PUBLISH — POST /workflows/{id}/publish-agent compiles the child with the
#      AgentCapabilities ABI under its slug and stages
#      runtara_agent_<slug>.wasm + synthesized .meta.json into the tenant's
#      workflow-agent dir.
#   3. PARENT INVOKE — a parent workflow's ordinary Agent step targets
#      `agentId: <slug>, capabilityId: "run"`; save-time validation sees the
#      published child through the catalog overlay, composition finds its
#      .wasm through the extra search dir, and execution returns the child's
#      output through standard agent-output shaping.
#   4. SAFETY GATE — a workflow that can wait, sleep, retry, or pause is
#      refused before an agent artifact is staged. The workflow-agent ABI is
#      synchronous; durable work stays a top-level or embedded workflow.
#
# Prerequisites: Postgres + docker (isolated Valkey) and the agent / shared
# workflow components in target/wasm32-wasip2/release
# (scripts/build-agent-components.sh).

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

TEST_DB_SERVER="wf_agent_e2e_server_$$"
TEST_DB_RUNTIME="wf_agent_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17720}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17721}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18721}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18722}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18723}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18724}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16392}"
TEST_DATA_DIR="$(mktemp -d -t runtara_wf_agent_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="wf_agent_e2e"

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

api_put() {
    curl -sS --max-time 60 -X PUT -H "Content-Type: application/json" \
        -d "$2" "${API}$1"
}

# Create a workflow, save its graph, compile it; echoes the workflow id.
create_and_compile() {
    local name="$1" graph="$2"
    local resp wf_id version
    resp=$(api_post /workflows/create "{\"name\":\"${name}\",\"description\":\"parity e2e\"}")
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

# Execute a workflow and assert the completed output equals the expected JSON.
execute_and_assert() {
    local wf_id="$1" inputs="$2" expected="$3" label="$4"
    local resp instance status outputs
    resp=$(api_post "/workflows/${wf_id}/execute" "{\"inputs\": ${inputs}}")
    instance=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    [ -n "${instance}" ] || { print_error "execute failed (${label}): ${resp}"; exit 1; }
    status=""
    for _ in {1..90}; do
        resp=$(curl -sS "${API}/workflows/instances/${instance}")
        status=$(echo "${resp}" | jq -r '.data.status // .status // empty')
        case "${status}" in completed|failed|crashed|stopped) break ;; esac
        sleep 2
    done
    if [ "${status}" != "completed" ]; then
        print_error "${label}: instance ended '${status}': $(echo "${resp}" | jq -c '.data.error // empty')"
        tail -40 "${TEST_LOG}"
        exit 1
    fi
    outputs=$(echo "${resp}" | jq -cS '.data.outputs')
    local expected_sorted
    expected_sorted=$(echo "${expected}" | jq -cS .)
    if [ "${outputs}" != "${expected_sorted}" ]; then
        print_error "${label}: output mismatch"
        print_error "  expected: ${expected_sorted}"
        print_error "  actual  : ${outputs}"
        exit 1
    fi
    echo "  ${label}: completed with expected output ✓"
}

#-------------------------------------------------------------------------
echo "==============================================================="
echo "E2E Test: workflow<>agent parity (publish + parent invoke)"
echo "==============================================================="

if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi
for f in runtara_workflow_stdlib.wasm runtara_workflow_runtime.wasm; do
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
    kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "Server exited during boot."; tail -30 "${TEST_LOG}"; exit 1; }
done
echo "  Server up (PID ${SERVER_PID})"

#-------------------------------------------------------------------------
print_step "1. Slug lifecycle: auto-derivation, conflict, edit..."
RESP=$(api_post /workflows/create '{"name":"Shout Echo Child","description":"parity e2e"}')
CHILD_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
CHILD_SLUG=$(echo "${RESP}" | jq -r '.data.slug // empty')
[ "${CHILD_SLUG}" = "shout-echo-child" ] \
    || { print_error "expected auto-derived slug 'shout-echo-child', got '${CHILD_SLUG}': ${RESP}"; exit 1; }
echo "  auto-derived slug: ${CHILD_SLUG} ✓"

# Explicit duplicate slug → 409.
CODE=$(curl -sS -o /dev/null -w "%{http_code}" -X POST -H "Content-Type: application/json" \
    -d '{"name":"Copycat","description":"", "slug":"shout-echo-child"}' \
    "${API}/workflows/create")
[ "${CODE}" = "409" ] || { print_error "duplicate slug should 409, got ${CODE}"; exit 1; }
echo "  duplicate explicit slug → 409 ✓"

# Reserved native agent id → 409.
CODE=$(curl -sS -o /dev/null -w "%{http_code}" -X POST -H "Content-Type: application/json" \
    -d '{"name":"Native Clash","description":"", "slug":"http"}' \
    "${API}/workflows/create")
[ "${CODE}" = "409" ] || { print_error "reserved native slug should 409, got ${CODE}"; exit 1; }
echo "  reserved native agent slug → 409 ✓"

# Slug edit through the dedicated endpoint.
RESP=$(api_put "/workflows/${CHILD_ID}/slug" '{"slug":"shout-echo"}')
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] \
    || { print_error "slug edit failed: ${RESP}"; exit 1; }
CHILD_SLUG="shout-echo"
echo "  slug edited to '${CHILD_SLUG}' ✓"

#-------------------------------------------------------------------------
print_step "2. Publish the child workflow as an agent..."
CHILD_GRAPH='{
  "name": "Shout Echo Child",
  "durable": false,
  "steps": {
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "echoed": { "valueType": "reference", "value": "data.text" },
        "marker": { "valueType": "immediate", "value": "from-child" }
      }
    }
  },
  "entryPoint": "finish",
  "executionPlan": [],
  "variables": {},
  "inputSchema": { "text": { "type": "string", "required": true } },
  "outputSchema": {}
}'
RESP=$(api_post "/workflows/${CHILD_ID}/update" "{\"executionGraph\": ${CHILD_GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] \
    || { print_error "child update failed: ${RESP}"; exit 1; }
RESP=$(api_post "/workflows/${CHILD_ID}/publish-agent" "" 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] \
    || { print_error "publish-agent failed: ${RESP}"; tail -40 "${TEST_LOG}"; exit 1; }
AGENT_ID=$(echo "${RESP}" | jq -r '.data.agentId')
CAP_ID=$(echo "${RESP}" | jq -r '.data.capabilityId')
[ "${AGENT_ID}" = "shout-echo" ] && [ "${CAP_ID}" = "run" ] \
    || { print_error "unexpected publish result: ${RESP}"; exit 1; }
STAGED_META="${TEST_DATA_DIR}/workflow-agents/${TENANT}/runtara_agent_shout_echo.meta.json"
[ -f "${STAGED_META}" ] || { print_error "staged meta missing at ${STAGED_META}"; exit 1; }
jq -e '.capabilities[0].inputs[] | select(.name=="text" and .type=="string")' "${STAGED_META}" >/dev/null \
    || { print_error "synthesized meta lacks the 'text' input: $(cat "${STAGED_META}")"; exit 1; }
echo "  published as agentId=${AGENT_ID}, capabilityId=${CAP_ID}; meta synthesized ✓"

#-------------------------------------------------------------------------
print_step "3. Parent workflow invokes the published agent..."
PARENT_GRAPH='{
  "name": "Parent Of Shout Echo",
  "steps": {
    "call": {
      "stepType": "Agent",
      "id": "call",
      "agentId": "shout-echo",
      "capabilityId": "run",
      "inputMapping": { "text": { "valueType": "reference", "value": "data.msg" } }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "childEcho": { "valueType": "reference", "value": "steps.call.outputs.echoed" },
        "childMarker": { "valueType": "reference", "value": "steps.call.outputs.marker" }
      }
    }
  },
  "entryPoint": "call",
  "executionPlan": [{ "fromStep": "call", "toStep": "finish" }],
  "variables": {},
  "inputSchema": { "msg": { "type": "string", "required": true } },
  "outputSchema": {}
}'
PARENT_ID=$(create_and_compile "Parent Of Shout Echo" "${PARENT_GRAPH}")
execute_and_assert "${PARENT_ID}" '{"data":{"msg":"hello-live"}}' \
    '{"childEcho":"hello-live","childMarker":"from-child"}' "parent→child"

#-------------------------------------------------------------------------
print_step "4. Safety gate: a sleeping workflow is refused as an agent..."
RESP=$(api_post /workflows/create '{"name":"Durable Delay Echo","description":"parity e2e","slug":"durable-delay-echo"}')
DURABLE_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${DURABLE_ID}" ] || { print_error "durable child create failed: ${RESP}"; exit 1; }
DURABLE_GRAPH='{
  "name": "Durable Delay Echo",
  "steps": {
    "delay": {
      "stepType": "Delay",
      "id": "delay",
      "durationMs": { "valueType": "immediate", "value": 50 }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": { "echo": { "valueType": "reference", "value": "data.value" } }
    }
  },
  "entryPoint": "delay",
  "executionPlan": [ { "fromStep": "delay", "toStep": "finish" } ],
  "variables": {},
  "inputSchema": { "value": { "type": "string", "required": true } },
  "outputSchema": {}
}'
RESP=$(api_post "/workflows/${DURABLE_ID}/update" "{\"executionGraph\": ${DURABLE_GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] \
    || { print_error "durable child update failed: ${RESP}"; exit 1; }
RESP=$(api_post "/workflows/${DURABLE_ID}/publish-agent" "" 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "false" ] \
    || { print_error "a durable workflow must not publish as an agent: ${RESP}"; exit 1; }
PUBLISH_ERROR=$(echo "${RESP}" | jq -r '[.. | strings] | join(" ")')
echo "${PUBLISH_ERROR}" | grep -q "may suspend or sleep" \
    || { print_error "rejection must explain the synchronous agent boundary: ${RESP}"; exit 1; }
echo "${PUBLISH_ERROR}" | grep -q "Delay/delay" \
    || { print_error "rejection must identify the Delay path: ${RESP}"; exit 1; }
echo "  sleeping workflow refused before agent publication ✓"

#-------------------------------------------------------------------------
print_step "5. Safety gate: a waiting workflow is refused as an agent..."
RESP=$(api_post /workflows/create '{"name":"Waiting Approval","description":"parity e2e","slug":"waiting-approval"}')
WAITING_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${WAITING_ID}" ] || { print_error "waiting workflow create failed: ${RESP}"; exit 1; }
WAITING_GRAPH='{
  "name": "Waiting Approval",
  "steps": {
    "approve": {
      "stepType": "WaitForSignal",
      "id": "approve",
      "pollIntervalMs": 500
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish"
    }
  },
  "entryPoint": "approve",
  "executionPlan": [ { "fromStep": "approve", "toStep": "finish" } ],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}'
RESP=$(api_post "/workflows/${WAITING_ID}/update" "{\"executionGraph\": ${WAITING_GRAPH}}")
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "waiting workflow update failed: ${RESP}"
    exit 1
fi
RESP=$(api_post "/workflows/${WAITING_ID}/publish-agent" "" 900)
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "false" ]; then
    print_error "a waiting workflow must not publish as an agent: ${RESP}"
    exit 1
fi
PUBLISH_ERROR=$(echo "${RESP}" | jq -r '[.. | strings] | join(" ")')
echo "${PUBLISH_ERROR}" | grep -q "may suspend or sleep" || {
    print_error "waiting rejection must explain the synchronous agent boundary: ${RESP}"
    exit 1
}
echo "${PUBLISH_ERROR}" | grep -q "WaitForSignal/wait-for-signal" || {
    print_error "rejection must identify the WaitForSignal path: ${RESP}"
    exit 1
}
echo "  waiting workflow refused before agent publication ✓"
#-------------------------------------------------------------------------
print_step "6. Embedded waits: second wait on the same step id stays discoverable..."
# Two EMBEDS of one wait-child (events ON). After site 1's wait completes, a
# step_debug_end with the bare child step id exists — the pending-input
# listing must still surface site 2's open wait (it matches resolved waits
# by per-site SIGNAL id, not bare step id).
RESP=$(api_post /workflows/create '{"name":"Embed Wait Child","description":"parity e2e"}')
EMB_CHILD_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${EMB_CHILD_ID}" ] || { print_error "embed child create failed: ${RESP}"; exit 1; }
EMB_CHILD_GRAPH='{
  "name": "Embed Wait Child",
  "steps": {
    "approve": {
      "stepType": "WaitForSignal",
      "id": "approve",
      "pollIntervalMs": 500,
      "timeoutMs": { "valueType": "immediate", "value": 120000 }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": { "decision": { "valueType": "reference", "value": "steps.approve.outputs.decision" } }
    }
  },
  "entryPoint": "approve",
  "executionPlan": [ { "fromStep": "approve", "toStep": "finish" } ],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}'
RESP=$(api_post "/workflows/${EMB_CHILD_ID}/update" "{\"executionGraph\": ${EMB_CHILD_GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] \
    || { print_error "embed child update failed: ${RESP}"; exit 1; }

EMB_PARENT_GRAPH='{
  "name": "Embed Wait Parent",
  "steps": {
    "embed1": {
      "stepType": "EmbedWorkflow",
      "id": "embed1",
      "childWorkflowId": "'"${EMB_CHILD_ID}"'",
      "childVersion": "latest",
      "inputMapping": {}
    },
    "embed2": {
      "stepType": "EmbedWorkflow",
      "id": "embed2",
      "childWorkflowId": "'"${EMB_CHILD_ID}"'",
      "childVersion": "latest",
      "inputMapping": {}
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "first": { "valueType": "reference", "value": "steps.embed1.outputs.decision" },
        "second": { "valueType": "reference", "value": "steps.embed2.outputs.decision" }
      }
    }
  },
  "entryPoint": "embed1",
  "executionPlan": [
    { "fromStep": "embed1", "toStep": "embed2" },
    { "fromStep": "embed2", "toStep": "finish" }
  ],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}'
EMB_PARENT_ID=$(create_and_compile "Embed Wait Parent" "${EMB_PARENT_GRAPH}")
RESP=$(api_post "/workflows/${EMB_PARENT_ID}/execute" '{"inputs":{"data":{}}}')
EMB_INSTANCE=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${EMB_INSTANCE}" ] || { print_error "embed parent execute failed: ${RESP}"; exit 1; }

# Discover a site's open signal id (by site marker) via pending-input.
discover_signal() {
    local instance="$1" marker="$2" found=""
    for _ in {1..45}; do
        local resp
        resp=$(curl -sS "${API}/workflows/${EMB_PARENT_ID}/instances/${instance}/pending-input")
        found=$(echo "${resp}" | jq -r --arg m "${marker}" \
            '.data.pendingInputs[]?.signalId // empty | select(contains($m))' | head -1)
        [ -n "${found}" ] && { echo "${found}"; return 0; }
        sleep 2
    done
    print_error "pending-input never surfaced a signal id containing '${marker}'"
    return 1
}

SIG1=$(discover_signal "${EMB_INSTANCE}" "::embed1::approve") || { tail -40 "${TEST_LOG}"; exit 1; }
RESP=$(api_post "/signals/${EMB_INSTANCE}" "{\"signalId\": \"${SIG1}\", \"payload\": {\"decision\": \"first-ok\"}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] \
    || { print_error "site-1 signal submit failed: ${RESP}"; exit 1; }
echo "  site 1 discovered + signaled (${SIG1}) ✓"

# THE regression: after site 1 completed (its step_debug_end recorded under
# the bare step id "approve"), site 2's open wait must STILL be listed.
SIG2=$(discover_signal "${EMB_INSTANCE}" "::embed2::approve") || { tail -40 "${TEST_LOG}"; exit 1; }
RESP=$(api_post "/signals/${EMB_INSTANCE}" "{\"signalId\": \"${SIG2}\", \"payload\": {\"decision\": \"second-ok\"}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] \
    || { print_error "site-2 signal submit failed: ${RESP}"; exit 1; }
echo "  site 2 still discoverable after site 1 completed (${SIG2}) ✓"

EMB_STATUS=""
for _ in {1..45}; do
    RESP=$(curl -sS "${API}/workflows/instances/${EMB_INSTANCE}")
    EMB_STATUS=$(echo "${RESP}" | jq -r '.data.status // .status // empty')
    case "${EMB_STATUS}" in completed|failed|crashed|stopped) break ;; esac
    sleep 2
done
[ "${EMB_STATUS}" = "completed" ] \
    || { print_error "embed parent ended '${EMB_STATUS}': $(echo "${RESP}" | jq -c '.data.error // empty')"; tail -40 "${TEST_LOG}"; exit 1; }
OUT=$(echo "${RESP}" | jq -cS '.data.outputs')
[ "${OUT}" = '{"first":"first-ok","second":"second-ok"}' ] \
    || { print_error "per-site payload routing broken, got: ${OUT}"; exit 1; }
echo "  both sites resolved with their own payloads ✓"

#-------------------------------------------------------------------------
print_step "7. Pause during an embedded child's wait: suspend, resume, signal, complete..."
# Durable work remains supported through EmbedWorkflow; pausing the parent
# must suspend the instance rather than consuming a runner slot.
RESP=$(api_post "/workflows/${EMB_PARENT_ID}/execute" '{"inputs":{"data":{}}}')
PAUSE_INSTANCE=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${PAUSE_INSTANCE}" ] || { print_error "pause-target execute failed: ${RESP}"; exit 1; }

PAUSE_SIG=$(discover_signal "${PAUSE_INSTANCE}" "::embed1::approve") || {
    tail -40 "${TEST_LOG}"
    exit 1
}

RESP=$(api_post "/workflows/instances/${PAUSE_INSTANCE}/pause" "")
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "pause request failed: ${RESP}"
    exit 1
fi
PAUSE_STATUS=""
for _ in {1..45}; do
    RESP=$(curl -sS "${API}/workflows/instances/${PAUSE_INSTANCE}")
    PAUSE_STATUS=$(echo "${RESP}" | jq -r '.data.status // .status // empty')
    case "${PAUSE_STATUS}" in
        paused|suspended) break ;;
        failed|crashed|stopped|completed)
            print_error "pause mid-embedded-wait must suspend, instance ended '${PAUSE_STATUS}': $(echo "${RESP}" | jq -c '.data.error // empty')"
            tail -40 "${TEST_LOG}"
            exit 1 ;;
    esac
    sleep 2
done
case "${PAUSE_STATUS}" in
    paused|suspended) echo "  instance ${PAUSE_STATUS} while embedded child waited ✓" ;;
    *) print_error "instance never paused (last status '${PAUSE_STATUS}')"; tail -40 "${TEST_LOG}"; exit 1 ;;
esac

RESP=$(api_post "/workflows/instances/${PAUSE_INSTANCE}/resume" "")
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "resume request failed: ${RESP}"
    exit 1
fi
RESP=$(api_post "/signals/${PAUSE_INSTANCE}" "{\"signalId\": \"${PAUSE_SIG}\", \"payload\": {\"decision\": \"resumed-live\"}}")
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "post-resume first signal submit failed: ${RESP}"
    exit 1
fi
PAUSE_SIG2=$(discover_signal "${PAUSE_INSTANCE}" "::embed2::approve") || {
    tail -40 "${TEST_LOG}"
    exit 1
}
RESP=$(api_post "/signals/${PAUSE_INSTANCE}" "{\"signalId\": \"${PAUSE_SIG2}\", \"payload\": {\"decision\": \"second-after-resume\"}}")
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "post-resume second signal submit failed: ${RESP}"
    exit 1
fi

PAUSE_STATUS=""
for _ in {1..60}; do
    RESP=$(curl -sS "${API}/workflows/instances/${PAUSE_INSTANCE}")
    PAUSE_STATUS=$(echo "${RESP}" | jq -r '.data.status // .status // empty')
    case "${PAUSE_STATUS}" in completed|failed|crashed|stopped) break ;; esac
    sleep 2
done
[ "${PAUSE_STATUS}" = "completed" ] || {
    print_error "resumed instance ended '${PAUSE_STATUS}': $(echo "${RESP}" | jq -c '.data.error // empty')"
    tail -40 "${TEST_LOG}"
    exit 1
}
OUT=$(echo "${RESP}" | jq -cS '.data.outputs')
[ "${OUT}" = '{"first":"resumed-live","second":"second-after-resume"}' ] || {
    print_error "post-resume payload routing broken, got: ${OUT}"
    exit 1
}
echo "  resumed, signaled twice, and completed with both embedded outputs ✓"

print_success "workflow<>agent parity: slug + synchronous publish + parent invoke + suspend-capable-agent rejection + embedded signal discovery + pause/resume, all green"
