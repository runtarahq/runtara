#!/bin/bash
# E2E Test: workflow saves enforce one finite execution-timeout policy.
#
# This starts a local server with a deliberately smaller deployment maximum
# (300 seconds) and exercises the public save route. It proves invalid JSON
# never becomes a stored workflow that could later reach the async start,
# resume, or wake paths through a narrowing conversion.
#
# Prereqs: a reachable Postgres (host psql or runtara-dev-postgres container)
# and a built runtara-server binary. No runtime/Valkey is required: this is a
# server-side persistence-boundary test, and embedded Environment is disabled.

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
step() { echo -e "${GREEN}[STEP]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
fail() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"
PG_CONTAINER="${PG_CONTAINER:-runtara-dev-postgres}"

TEST_DB="execution_timeout_e2e_${$}"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17790}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17791}"
TEST_LOG="$(mktemp -t runtara_execution_timeout_e2e_XXXXXX)"
SERVER_PID=""
TENANT="execution_timeout_e2e"
RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
RUNTARA_AGENT_COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

if command -v psql >/dev/null 2>&1 && PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -d postgres -c 'SELECT 1' >/dev/null 2>&1; then
    psql_quiet() { PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"; }
elif docker exec "${PG_CONTAINER}" true >/dev/null 2>&1; then
    warn "host psql unavailable — using docker exec ${PG_CONTAINER}"
    psql_quiet() { docker exec -e PGPASSWORD="${POSTGRES_PASSWORD}" "${PG_CONTAINER}" psql -U "${POSTGRES_USER}" -tA "$@"; }
else
    fail "no reachable psql (host psql or ${PG_CONTAINER} required)"
fi

cleanup() {
    local result=$?
    if [ -n "${SERVER_PID}" ] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB} WITH (FORCE)" >/dev/null 2>&1 || true
    if [ "${result}" -ne 0 ]; then
        echo "--- server log tail ---"
        tail -40 "${TEST_LOG}" || true
    fi
    rm -f "${TEST_LOG}"
    exit "${result}"
}
trap cleanup EXIT

start_server() {
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
    OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
    TENANT_ID="${TENANT}" \
    SERVER_HOST=127.0.0.1 \
    SERVER_PORT="${TEST_PORT_PUBLIC}" \
    INTERNAL_PORT="${TEST_PORT_INTERNAL}" \
    RUNTARA_EMBEDDED=false \
    RUNTARA_AGENT_COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR}" \
    RUNTARA_MCP_SESSION_STORE=local \
    RUNTARA_DEFAULT_EXECUTION_TIMEOUT_SECS=120 \
    RUNTARA_MAX_EXECUTION_TIMEOUT_SECS=300 \
    AUTH_PROVIDER=local \
    SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2 \
    OTEL_SDK_DISABLED=true \
    SQLX_OFFLINE="${SQLX_OFFLINE:-true}" \
    "${RUNTARA_SERVER_BIN}" >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!

    for _ in {1..60}; do
        if curl -sS -o /dev/null -w '%{http_code}' "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q '^2'; then
            return 0
        fi
        sleep 1
        kill -0 "${SERVER_PID}" 2>/dev/null || fail "server exited during boot"
    done
    fail "server did not become healthy"
}

post_status() {
    local path="$1" body="$2" response_file="$3"
    curl -sS --max-time 30 -o "${response_file}" -w '%{http_code}' \
        -X POST "${API}${path}" -H 'Content-Type: application/json' -d "${body}"
}

expect_rejected() {
    local label="$1" graph="$2" body status
    body="$(jq -nc --argjson graph "${graph}" '{executionGraph: $graph}')"
    status="$(post_status "/workflows/${WORKFLOW_ID}/update" "${body}" "${TEST_LOG}.${label}.json")"
    [ "${status}" = "400" ] || fail "${label}: expected HTTP 400, got ${status}: $(cat "${TEST_LOG}.${label}.json")"
    jq -e '.. | strings | select(test("executionTimeoutSeconds"))' "${TEST_LOG}.${label}.json" >/dev/null \
        || fail "${label}: response did not identify executionTimeoutSeconds: $(cat "${TEST_LOG}.${label}.json")"
    echo "  ✓ ${label} rejected at save boundary"
}

expect_step_timeout_rejected() {
    local label="$1" step_id="$2" graph="$3" body status response
    response="$(mktemp -t runtara_execution_timeout_step_XXXXXX)"
    body="$(jq -nc --argjson graph "${graph}" '{executionGraph: $graph}')"
    status="$(post_status "/workflows/${WORKFLOW_ID}/update" "${body}" "${response}")"
    [ "${status}" = "400" ] || fail "${label} step timeout: expected HTTP 400, got ${status}: $(cat "${response}")"
    jq -e --arg step_id "${step_id}" '.validationErrors[] | select(.code == "E128" and .stepId == $step_id and .fieldName == "timeout")' "${response}" >/dev/null \
        || fail "${label} step timeout: response did not expose E128 on ${step_id}.timeout: $(cat "${response}")"
    rm -f "${response}"
    echo "  ✓ unsupported ${label} timeout rejected at save boundary"
}

echo '==============================================================='
echo 'E2E: typed execution-timeout save policy'
echo '==============================================================='

[ -x "${RUNTARA_SERVER_BIN}" ] || fail "missing ${RUNTARA_SERVER_BIN}; run cargo build -p runtara-server --bin runtara-server"
[ -d "${RUNTARA_AGENT_COMPONENTS_DIR}" ] || fail "missing ${RUNTARA_AGENT_COMPONENTS_DIR}; run scripts/build-agent-components.sh"
command -v jq >/dev/null 2>&1 || fail 'jq is required'

step 'Creating isolated server database...'
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB}" >/dev/null

step 'Starting server with default=120s and maximum=300s...'
start_server

step 'Creating a workflow...'
CREATE_RESPONSE="$(mktemp -t runtara_execution_timeout_create_XXXXXX)"
CREATE_STATUS="$(post_status /workflows/create '{"name":"execution-timeout-policy","description":"timeout validation harness"}' "${CREATE_RESPONSE}")"
[ "${CREATE_STATUS}" = '200' ] || fail "workflow create failed: $(cat "${CREATE_RESPONSE}")"
WORKFLOW_ID="$(jq -r '.data.id // empty' "${CREATE_RESPONSE}")"
rm -f "${CREATE_RESPONSE}"
[ -n "${WORKFLOW_ID}" ] || fail 'workflow create returned no id'

BASE_GRAPH='{
  "name": "execution-timeout-policy",
  "entryPoint": "finish",
  "steps": {
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": { "ok": { "valueType": "immediate", "value": true } }
    }
  },
  "executionPlan": [],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}'

step 'Rejecting invalid timeout representations through the live API...'
expect_rejected zero "$(jq -c '. + {executionTimeoutSeconds: 0}' <<<"${BASE_GRAPH}")"
expect_rejected negative "$(jq -c '. + {executionTimeoutSeconds: -1}' <<<"${BASE_GRAPH}")"
expect_rejected string "$(jq -c '. + {executionTimeoutSeconds: "120"}' <<<"${BASE_GRAPH}")"
expect_rejected above_deployment_cap "$(jq -c '. + {executionTimeoutSeconds: 301}' <<<"${BASE_GRAPH}")"

AGENT_STEP_TIMEOUT_GRAPH='{
  "name": "execution-timeout-policy",
  "entryPoint": "agent",
  "steps": {
    "agent": {
      "stepType": "Agent",
      "id": "agent",
      "agentId": "utils",
      "capabilityId": "get-current-iso-datetime",
      "inputMapping": {},
      "timeout": 1000
    },
    "finish": {"stepType": "Finish", "id": "finish"}
  },
  "executionPlan": [{"fromStep": "agent", "toStep": "finish"}],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}'

step 'Rejecting unsupported Agent step timeouts through the live API...'
expect_step_timeout_rejected Agent agent "${AGENT_STEP_TIMEOUT_GRAPH}"

EMBED_WORKFLOW_STEP_TIMEOUT_GRAPH='{
  "name": "execution-timeout-policy",
  "entryPoint": "embed",
  "steps": {
    "embed": {
      "stepType": "EmbedWorkflow",
      "id": "embed",
      "childWorkflowId": "00000000-0000-0000-0000-000000000000",
      "childVersion": "latest",
      "inputMapping": {},
      "timeout": 1000
    },
    "finish": {"stepType": "Finish", "id": "finish"}
  },
  "executionPlan": [{"fromStep": "embed", "toStep": "finish"}],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}'

step 'Rejecting unsupported EmbedWorkflow step timeouts through the live API...'
expect_step_timeout_rejected EmbedWorkflow embed "${EMBED_WORKFLOW_STEP_TIMEOUT_GRAPH}"

step 'Accepting a timeout within the deployment policy...'
VALID_GRAPH="$(jq -c '. + {executionTimeoutSeconds: 300}' <<<"${BASE_GRAPH}")"
VALID_BODY="$(jq -nc --argjson graph "${VALID_GRAPH}" '{executionGraph: $graph}')"
VALID_RESPONSE="$(mktemp -t runtara_execution_timeout_valid_XXXXXX)"
VALID_STATUS="$(post_status "/workflows/${WORKFLOW_ID}/update" "${VALID_BODY}" "${VALID_RESPONSE}")"
[ "${VALID_STATUS}" = '200' ] || fail "valid timeout save failed: $(cat "${VALID_RESPONSE}")"
jq -e '.success == true' "${VALID_RESPONSE}" >/dev/null || fail "valid timeout response was not successful: $(cat "${VALID_RESPONSE}")"
rm -f "${VALID_RESPONSE}"

echo -e "${GREEN}[SUCCESS]${NC} live server rejects unsafe timeouts and accepts bounded ones"
