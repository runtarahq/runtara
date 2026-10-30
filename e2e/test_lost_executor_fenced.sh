#!/bin/bash
# E2E Test: a lost executor is fenced and its run finishes once (M3.3).
#
# The server runs on `runtara-server-delegating`, told to lose the first
# launch 2.5 s in: its runner reports the executor lost (`RunExitReport::Lost`)
# while the guest keeps running, as a node that dropped off the network would.
#
#   1. RUN — a durable workflow of six 1 s in-process delays (each step's
#      result is checkpointed) starts on launch 1.
#   2. LOSE — 2.5 s in, launch 1 is reported lost. Recovery suspends the run,
#      which revokes its root lease, and relaunches it (launch 2, next epoch).
#   3. FENCE — launch 1's guest finishes its delay and writes its checkpoint:
#      the store refuses it (superseded) and the guest is stopped.
#   4. ONCE — launch 2 replays the checkpointed steps and completes; the run
#      has one outcome, from launch 2.
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

TEST_DB_SERVER="lost_executor_e2e_server_$$"
TEST_DB_RUNTIME="lost_executor_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17810}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18811}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18812}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18813}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18814}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16401}"
TEST_DATA_DIR="$(mktemp -d -t runtara_lost_executor_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="lost_executor_e2e"

RUNTARA_SERVER_BIN="${PROJECT_ROOT}/target/debug/runtara-server-delegating"
export RUNTARA_DELEGATE_LOSE_LAUNCH=1
export RUNTARA_DELEGATE_LOSE_AFTER_MS=2500
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
    resp=$(api_post /workflows/create "{\"name\":\"${name}\",\"description\":\"lost executor e2e\"}")
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
echo "E2E Test: a lost executor is fenced and its run finishes once"
echo "==============================================================="

[ -n "${POSTGRES_PASSWORD}" ] \
    || { print_error "set POSTGRES_PASSWORD (or PGPASSWORD) for ${POSTGRES_USER}@${POSTGRES_HOST}"; exit 1; }
print_step "Building runtara-server-delegating..."
SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-runner-delegate --bin runtara-server-delegating >&2
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
print_step "1. Run six 1 s steps; launch 1 is lost 2.5 s in..."
DELAY='{ "stepType": "Agent", "agentId": "utils", "capabilityId": "delay-in-ms", "maxRetries": 0,
  "inputMapping": { "delay_value": { "valueType": "immediate", "value": 1000 } } }'
GRAPH=$(jq -n --argjson d "${DELAY}" '{
  name: "Six Delays", durable: true,
  steps: ({finish: {stepType: "Finish", id: "finish",
             inputMapping: {last: {valueType: "reference", value: "steps.d6.outputs"}}}}
          + ([range(1;7)] | map({key: "d\(.)", value: ($d + {id: "d\(.)"})}) | from_entries)),
  entryPoint: "d1",
  executionPlan: (([range(1;6)] | map({fromStep: "d\(.)", toStep: "d\(. + 1)"}))
                  + [{fromStep: "d6", toStep: "finish"}]),
  variables: {}, inputSchema: {}, outputSchema: {}
}')
WF_ID=$(create_and_compile "Six Delays" "${GRAPH}")
RESP=$(api_post "/workflows/${WF_ID}/execute" '{"inputs": {"data": {}}}')
INSTANCE=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${INSTANCE}" ] || { print_error "execute failed: ${RESP}"; exit 1; }
STARTED=$(date +%s)

STATUS=""
for _ in {1..120}; do
    STATUS=$(curl -sS "${API}/workflows/instances/${INSTANCE}" | jq -r '.data.status // empty')
    case "${STATUS}" in completed|failed|crashed|stopped|cancelled) break ;; esac
    sleep 0.5
done
ELAPSED=$(( $(date +%s) - STARTED ))
RESP=$(curl -sS "${API}/workflows/instances/${INSTANCE}")

#-------------------------------------------------------------------------
print_step "2. Launch 1 was lost and the run relaunched..."
grep -q "delegating runner: lost launch .* of ${INSTANCE}" "${TEST_LOG}" \
    || { print_error "launch 1 was not lost"; tail -40 "${TEST_LOG}"; exit 1; }
grep -q "lost the run's executor; handed to recovery" "${TEST_LOG}" \
    || { print_error "the lost executor was not handed to recovery"; tail -40 "${TEST_LOG}"; exit 1; }
LAUNCHES=$(grep -c "delegating runner: launch .* of ${INSTANCE}" "${TEST_LOG}" || true)
[ "${LAUNCHES}" -ge 2 ] \
    || { print_error "the run must be relaunched, saw ${LAUNCHES} launch(es)"; tail -40 "${TEST_LOG}"; exit 1; }
echo "  launch 1 lost, handed to recovery, relaunched (${LAUNCHES} launches) ✓"

#-------------------------------------------------------------------------
print_step "3. Launch 1's next write was fenced..."
grep -q "Execution superseded; stopping the guest" "${TEST_LOG}" \
    || { print_error "the lost executor's write was not refused"; tail -60 "${TEST_LOG}"; exit 1; }
echo "  its checkpoint was refused (superseded) and its guest stopped ✓"

#-------------------------------------------------------------------------
print_step "4. The run finished once..."
[ "${STATUS}" = "completed" ] \
    || { print_error "the run ended '${STATUS}': $(echo "${RESP}" | jq -c '.data.error // empty')"; tail -60 "${TEST_LOG}"; exit 1; }
echo "${RESP}" | jq -e '.data.outputs.last == 1000' >/dev/null \
    || { print_error "unexpected outputs: $(echo "${RESP}" | jq -c '.data.outputs')"; exit 1; }
echo "  completed after ${ELAPSED}s with d6's output ✓"

print_success "lost executor: recovery fenced it, relaunched the run, and the run finished once"
