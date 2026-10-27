#!/bin/bash
# E2E Test: an upgrade restages workflow-agents built by an older lowering.
#
#   1. PUBLISH — `nap` (a durable Delay) is published as a workflow-agent; its
#      staged artifact records the lowering it was built with.
#   2. UPGRADE — the server stops, and nap's record in the artifact store is
#      rewritten as an older lowering's (what a new server binary sees after
#      an upgrade).
#   3. RESTAGE — on boot the server republishes `nap` from its version: the
#      record is this lowering's again.
#   4. CALL — a parent calling `nap` compiles and completes.
#   5. RESTORE — with the staging dir deleted, a restart rebuilds it from the
#      store and a parent compiles against nap again.
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

TEST_DB_SERVER="agent_restage_e2e_server_$$"
TEST_DB_RUNTIME="agent_restage_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17800}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18801}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18802}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18803}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18804}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16400}"
TEST_DATA_DIR="$(mktemp -d -t runtara_agent_restage_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="agent_restage_e2e"

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
    resp=$(api_post /workflows/create "{\"name\":\"${name}\",\"description\":\"restage e2e\"}")
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
echo "E2E Test: an upgrade restages stale workflow-agents"
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

#-------------------------------------------------------------------------
print_step "1. Publish nap, a workflow-agent that parks..."
RESP=$(api_post /workflows/create '{"name":"Nap","description":"restage e2e","slug":"nap"}')
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

LOWERING=$(find "${TEST_DATA_DIR}" -name 'runtara_agent_nap.lowering' | head -1)
[ -n "${LOWERING}" ] || { print_error "the staged agent records no lowering"; exit 1; }
CURRENT_LOWERING=$(cat "${LOWERING}")
RECORD=$(find "${TEST_DATA_DIR}/artifacts/manifests/tenants" -path '*/workflow-agents/nap.json' | head -1)
[ -n "${RECORD}" ] || { print_error "nap has no record in the artifact store"; exit 1; }
jq -e --arg l "${CURRENT_LOWERING}" '.metadata.lowering == $l and .metadata.meta.id == "nap"' "${RECORD}" >/dev/null \
    || { print_error "nap's record is not its lowering and metadata: $(head -c 300 "${RECORD}")"; exit 1; }
echo "  the published agent is a store record with its lowering, staged from it ✓"

#-------------------------------------------------------------------------
print_step "2. Stop the server and age the staged artifact..."
kill "${SERVER_PID}"; wait "${SERVER_PID}" 2>/dev/null || true
SERVER_PID=""
jq -c '.metadata.lowering = "an-older-lowering"' "${RECORD}" > "${RECORD}.tmp" && mv "${RECORD}.tmp" "${RECORD}"
echo "  recorded lowering rewritten as an older one"

#-------------------------------------------------------------------------
print_step "3. Restart: the server restages nap..."
start_server
for _ in {1..120}; do
    [ "$(jq -r .metadata.lowering "${RECORD}")" = "${CURRENT_LOWERING}" ] && break
    sleep 1
done
[ "$(jq -r .metadata.lowering "${RECORD}")" = "${CURRENT_LOWERING}" ] \
    && [ "$(cat "${LOWERING}")" = "${CURRENT_LOWERING}" ] \
    || { print_error "nap was not restaged: $(grep -i restage "${TEST_LOG}" | tail -5)"; exit 1; }
grep -q "restaged workflow-agent for this lowering" "${TEST_LOG}" \
    || { print_error "the server did not log the restage"; exit 1; }
echo "  nap republished at boot; its record is this lowering's again ✓"

#-------------------------------------------------------------------------
print_step "4. A parent calling nap compiles and completes..."
PARENT_GRAPH='{
  "name": "Call Nap",
  "durable": true,
  "steps": {
    "call": {
      "stepType": "Agent", "id": "call", "agentId": "nap", "capabilityId": "run",
      "maxRetries": 0,
      "inputMapping": { "ms": { "valueType": "immediate", "value": 1000 } }
    },
    "finish": {
      "stepType": "Finish", "id": "finish",
      "inputMapping": { "slept": { "valueType": "reference", "value": "steps.call.outputs.slept" } }
    }
  },
  "entryPoint": "call",
  "executionPlan": [{ "fromStep": "call", "toStep": "finish" }],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}'
PARENT_ID=$(create_and_compile "Call Nap" "${PARENT_GRAPH}")
RESP=$(api_post "/workflows/${PARENT_ID}/execute" '{"inputs": {"data": {}}}')
PARENT=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${PARENT}" ] || { print_error "execute failed: ${RESP}"; exit 1; }
STATUS=""
for _ in {1..120}; do
    STATUS=$(curl -sS "${API}/workflows/instances/${PARENT}" | jq -r '.data.status // empty')
    case "${STATUS}" in completed|failed|crashed|stopped|cancelled) break ;; esac
    sleep 0.5
done
RESP=$(curl -sS "${API}/workflows/instances/${PARENT}")
[ "${STATUS}" = "completed" ] && echo "${RESP}" | jq -e '.data.outputs.slept == 1000' >/dev/null \
    || { print_error "the parent ended '${STATUS}': $(echo "${RESP}" | jq -c '.data.error // .data.outputs')"; tail -40 "${TEST_LOG}"; exit 1; }
echo "  the parent completed with nap's output ✓"

#-------------------------------------------------------------------------
print_step "5. Delete the staging dir; a restart restores it from the store..."
kill "${SERVER_PID}"; wait "${SERVER_PID}" 2>/dev/null || true
SERVER_PID=""
rm -rf "${TEST_DATA_DIR}/workflow-agents"
start_server
[ -f "${LOWERING}" ] && [ -f "${LOWERING%.lowering}.wasm" ] \
    || { print_error "nap was not restored from the store"; exit 1; }
AGAIN_GRAPH="${PARENT_GRAPH/\"Call Nap\"/\"Call Nap Again\"}"
AGAIN_ID=$(create_and_compile "Call Nap Again" "${AGAIN_GRAPH}")
RESP=$(api_post "/workflows/${AGAIN_ID}/execute" '{"inputs": {"data": {}}}')
AGAIN=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${AGAIN}" ] || { print_error "execute failed: ${RESP}"; exit 1; }
STATUS=""
for _ in {1..120}; do
    STATUS=$(curl -sS "${API}/workflows/instances/${AGAIN}" | jq -r '.data.status // empty')
    case "${STATUS}" in completed|failed|crashed|stopped|cancelled) break ;; esac
    sleep 0.5
done
[ "${STATUS}" = "completed" ] \
    || { print_error "the second parent ended '${STATUS}'"; tail -40 "${TEST_LOG}"; exit 1; }
echo "  restored from the store; a new parent compiles against nap and completes ✓"

print_success "workflow-agent restage: an agent staged by an older lowering is republished at boot and callable, and a lost staging dir is restored from the store"
