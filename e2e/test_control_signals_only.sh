#!/bin/bash
# E2E Test (control-agent S0.1): parallel approvals on signals alone.
#
# A parent waits on two sequential WaitForSignal steps, `finance` then
# `legal`, as if two approval runs were answering it. While the parent is
# parked on `finance`, `legal` is answered first. That answer is refused with
# INPUT_NOT_FOUND: a managed signal is accepted only for an open request, and
# `legal` has not registered one yet. Nothing is buffered: after `finance` is
# answered the parent parks on `legal` with no answer, and only a second
# submission (the same operation id is still free) lets it finish.
#
# This is why signals alone cannot express "wait for whichever approval
# finishes first" and the WaitForInstances step exists. See docs/control-agent.md
# ("Signals-only parallel approvals (S0.1)").
#
# Usage:  POSTGRES_PORT=55432 POSTGRES_USER=postgres ./e2e/test_control_signals_only.sh
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
POSTGRES_USER="${POSTGRES_USER:-postgres}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD-}"

TEST_DB_SERVER="${TEST_DB_SERVER:-control_s01_server_$$}"
TEST_DB_RUNTIME="${TEST_DB_RUNTIME:-control_s01_runtime_$$}"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17960}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18961}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18962}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18963}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18964}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16410}"
TEST_DATA_DIR="$(mktemp -d -t runtara_control_s01_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="${TENANT_ID_OVERRIDE:-control_s01_$$}"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"

if [ -n "${POSTGRES_PASSWORD}" ]; then
    CREDENTIALS="${POSTGRES_USER}:${POSTGRES_PASSWORD}"
else
    CREDENTIALS="${POSTGRES_USER}"
fi
SERVER_DB_URL="postgresql://${CREDENTIALS}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${CREDENTIALS}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql -X -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
}
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}
# POST and echo "<http status> <body>".
api_post_status() {
    local body status
    body=$(curl -sS --max-time 60 -w '\n%{http_code}' -X POST -H "Content-Type: application/json" -d "$2" "${API}$1")
    status="${body##*$'\n'}"
    echo "${status} ${body%$'\n'*}"
}

cleanup() {
    local code=$?
    if [ -n "${SERVER_PID}" ]; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
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
        RUNTARA_DEV_MODE=false \
        RUST_LOG="warn,runtara_server=info" \
        AUTH_PROVIDER=local \
        VALKEY_HOST=127.0.0.1 \
        VALKEY_PORT="${TEST_VALKEY_PORT}" \
        OTEL_SDK_DISABLED=true \
        SQLX_OFFLINE=true \
        exec "${RUNTARA_SERVER_BIN}"
    ) >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!
    for _ in {1..90}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        sleep 1
        kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "Server exited during boot."; exit 1; }
    done
    print_error "Server did not become healthy."; exit 1
}

instance_status() { curl -sS "${API}/workflows/instances/$1" | jq -r '.data.status // .status // empty'; }
instance_output() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT COALESCE(convert_from(output, 'UTF8'),'null') FROM instances WHERE instance_id = '$1'"
}
wait_status() {
    local id="$1" want="$2" limit="$3" st deadline
    deadline=$(( $(date +%s) + limit ))
    while [ "$(date +%s)" -lt "${deadline}" ]; do
        st=$(instance_status "${id}")
        case "${st}" in "${want}"|completed|failed|cancelled) echo "${st}"; return 0 ;; esac
        sleep 1
    done
    echo "timeout"
}
# Open managed requests of run $2 (workflow $1), one compact JSON object per line.
open_actions() {
    curl -sS "${API}/workflows/$1/instances/$2/actions" \
        | jq -c '.data.actions[] | {requestId, signalId, label, status}'
}
# Wait until run $2 has exactly one open request, and echo it.
wait_single_action() {
    local actions
    for _ in {1..60}; do
        actions=$(open_actions "$1" "$2")
        if [ -n "${actions}" ] && [ "$(echo "${actions}" | wc -l | tr -d ' ')" = "1" ]; then
            echo "${actions}"; return 0
        fi
        sleep 1
    done
    print_error "Run $2 never had exactly one open request: '${actions}'"; exit 1
}
sha256() { printf '%s' "$1" | shasum -a 256 | cut -d' ' -f1; }

echo "==============================================================="
echo "E2E: control S0.1, parallel approvals on signals alone"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN}"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "127.0.0.1:${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases ${TEST_DB_SERVER}, ${TEST_DB_RUNTIME}..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC} (tenant ${TENANT})..."
start_server

print_step "Creating the parent: WaitForSignal finance, then WaitForSignal legal..."
GRAPH=$(jq -n '
    def approval($id; $name): { id: $id, stepType: "WaitForSignal", name: $name,
        pollIntervalMs: 500,
        responseSchema: { approved: { type: "boolean", required: true } } };
    {
        name: "s01-parent", durable: true, entryPoint: "finance",
        steps: { finance: approval("finance"; "Finance"), legal: approval("legal"; "Legal"),
                 finish: { id: "finish", stepType: "Finish", inputMapping: {
                     finance: { valueType: "reference", value: "steps.finance.outputs" },
                     legal: { valueType: "reference", value: "steps.legal.outputs" } } } },
        executionPlan: [ { fromStep: "finance", toStep: "legal" }, { fromStep: "legal", toStep: "finish" } ],
        variables: {}, inputSchema: {}, outputSchema: {}
    }')
RESP=$(api_post /workflows/create '{"name": "s01-parent", "description": "control S0.1"}')
WF=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${WF}" ] || { print_error "Workflow create failed: ${RESP}"; exit 1; }
RESP=$(api_post "/workflows/${WF}/update" "{\"executionGraph\": ${GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Update failed: ${RESP}"; exit 1; }
VERSION=$(curl -sS "${API}/workflows/${WF}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(api_post "/workflows/${WF}/versions/${VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Compile failed: ${RESP}"; exit 1; }

PARENT=""
for _ in {1..90}; do
    RESP=$(api_post "/workflows/${WF}/execute" '{"inputs": {"data": {}, "variables": {}}}')
    PARENT=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
    [ -n "${PARENT}" ] && break
    sleep 2
done
[ -n "${PARENT}" ] || { print_error "Execute never launched: ${RESP}"; exit 1; }
[ "$(wait_status "${PARENT}" suspended 120)" = "suspended" ] || { print_error "Parent did not park on finance"; exit 1; }

FINANCE=$(wait_single_action "${WF}" "${PARENT}")
FINANCE_REQUEST=$(echo "${FINANCE}" | jq -r '.requestId')
FINANCE_SIGNAL=$(echo "${FINANCE}" | jq -r '.signalId')
echo "  parked on: ${FINANCE}"
echo "${FINANCE_SIGNAL}" | grep -q "finance" || { print_error "Expected the finance request first: ${FINANCE}"; exit 1; }
[ "$(sha256 "${FINANCE_SIGNAL}")" = "${FINANCE_REQUEST}" ] || { print_error "requestId is not sha256(signalId)"; exit 1; }

# The legal wait's identity differs from finance's only by its step id, so a
# sender can compute legal's request id before legal registers it.
LEGAL_SIGNAL_PREDICTED="${FINANCE_SIGNAL//finance/legal}"
LEGAL_REQUEST_PREDICTED=$(sha256 "${LEGAL_SIGNAL_PREDICTED}")

print_step "Answering legal while the parent is parked on finance..."
EARLY_OP="s01-legal-${PARENT}"
LEGAL_ANSWER=$(jq -nc --arg r "${LEGAL_REQUEST_PREDICTED}" --arg o "${EARLY_OP}" '{requestId: $r, operationId: $o, payload: {approved: false}}')
read -r STATUS BODY <<< "$(api_post_status "/signals/${PARENT}" "${LEGAL_ANSWER}")"
echo "  legal (predicted request id) -> HTTP ${STATUS} ${BODY}"
[ "${STATUS}" = "404" ] && [ "$(echo "${BODY}" | jq -r '.code')" = "INPUT_NOT_FOUND" ] \
    || { print_error "Expected 404 INPUT_NOT_FOUND for an early legal answer"; exit 1; }
# A sender addressing the step id instead of a request id fares no better.
read -r STATUS BODY <<< "$(api_post_status "/signals/${PARENT}" \
    "$(jq -nc --arg o "${EARLY_OP}-by-step" '{requestId: "legal", operationId: $o, payload: {approved: false}}')")"
echo "  legal (step id as request id) -> HTTP ${STATUS} ${BODY}"
[ "${STATUS}" = "404" ] && [ "$(echo "${BODY}" | jq -r '.code')" = "INPUT_NOT_FOUND" ] \
    || { print_error "Expected 404 INPUT_NOT_FOUND for a step-id answer"; exit 1; }
[ "$(instance_status "${PARENT}")" = "suspended" ] || { print_error "The early answer moved the parent"; exit 1; }
[ "$(open_actions "${WF}" "${PARENT}" | jq -r '.requestId')" = "${FINANCE_REQUEST}" ] \
    || { print_error "The parent should still wait on finance alone"; exit 1; }
print_success "An answer to a wait that has not registered is refused (INPUT_NOT_FOUND), not buffered ✓"

print_step "Answering finance..."
read -r STATUS BODY <<< "$(api_post_status "/signals/${PARENT}" \
    "$(jq -nc --arg r "${FINANCE_REQUEST}" --arg o "s01-finance-${PARENT}" '{requestId: $r, operationId: $o, payload: {approved: true}}')")"
echo "  finance -> HTTP ${STATUS}"
[ "${STATUS}" = "200" ] || { print_error "Answering finance failed: ${BODY}"; exit 1; }

LEGAL=""
for _ in {1..60}; do
    LEGAL=$(open_actions "${WF}" "${PARENT}")
    [ -n "${LEGAL}" ] && [ "$(echo "${LEGAL}" | jq -r '.signalId | contains("legal")')" = "true" ] \
        && [ "$(instance_status "${PARENT}")" = "suspended" ] && break
    [ "$(instance_status "${PARENT}")" = "completed" ] && break
    sleep 1
done
echo "  now parked on: ${LEGAL}"
[ "$(instance_status "${PARENT}")" = "suspended" ] || { print_error "The early legal answer must not complete the parent"; exit 1; }
[ "$(echo "${LEGAL}" | jq -r '.requestId')" = "${LEGAL_REQUEST_PREDICTED}" ] \
    || { print_error "The early answer did not target legal's real request id"; exit 1; }
print_success "After finance, the parent parks on legal with no answer: the early one was dropped ✓"

print_step "Answering legal again, with the operation id of the refused attempt..."
read -r STATUS BODY <<< "$(api_post_status "/signals/${PARENT}" "${LEGAL_ANSWER}")"
echo "  legal -> HTTP ${STATUS}"
[ "${STATUS}" = "200" ] || { print_error "Answering legal failed: ${BODY}"; exit 1; }
[ "$(wait_status "${PARENT}" completed 120)" = "completed" ] || { print_error "Parent did not complete"; exit 1; }
OUT=$(instance_output "${PARENT}")
echo "  output: ${OUT}"
[ "$(echo "${OUT}" | jq -c '{finance: .finance.approved, legal: .legal.approved}')" = '{"finance":true,"legal":false}' ] \
    || { print_error "Unexpected output"; exit 1; }
print_success "Legal can be answered once its wait opens; the refused attempt left no receipt ✓"

echo ""
print_success "S0.1: signals-only approvals are strictly sequential; out-of-order answers are refused ✓"
