#!/bin/bash
# E2E Test: a workflow's memory (MiB) bounds its run, above a floor set at compile.
#
# The memory caps the sum of a run's linear memories. Compile records the
# floor (what the composed workflow allocates when it starts) and refuses a
# memory below it; a run that grows past its memory fails with a memory error.
#
#   1. pad-text, 128 MiB, 1 KB      -> compiles, floor on the version, completes
#   2. pad-text, tier S (8 MiB), 20 MB -> the deprecated preset still maps to
#                                      MiB; the run fails: memory limit
#   3. seven agents, 8 MiB          -> compile refused with the floor named
#      same graph, 16 MiB           -> compiles and completes
#
# Usage:  ./e2e/test_workflow_memory_tier.sh
#
# Prereqs: Postgres + docker (isolated Valkey) and prebuilt components in
# target/wasm32-wasip2/release (scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"

TEST_DB_SERVER="memtier_e2e_server_$$"
TEST_DB_RUNTIME="memtier_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17730}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18731}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18732}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18733}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18734}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16393}"
TEST_DATA_DIR="$(mktemp -d -t runtara_memtier_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="memtier_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

PG_CONTAINER="${PG_CONTAINER:-runtara-dev-postgres}"
if command -v psql >/dev/null 2>&1; then
    psql_quiet() {
        PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
    }
else
    psql_quiet() {
        docker exec -e PGPASSWORD="${POSTGRES_PASSWORD}" -i "${PG_CONTAINER}" \
            psql -U "${POSTGRES_USER}" -tA "$@"
    }
fi
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}

cleanup() {
    local code=$?
    [ -n "${SERVER_PID}" ] && kill "${SERVER_PID}" 2>/dev/null || true
    wait "${SERVER_PID}" 2>/dev/null || true
    [ -n "${VALKEY_CONTAINER}" ] && docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER} WITH (FORCE)" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME} WITH (FORCE)" >/dev/null 2>&1 || true
    [ ${code} -ne 0 ] && [ -f "${TEST_LOG}" ] && { echo "--- server log tail ---"; tail -40 "${TEST_LOG}"; }
    rm -rf "${TEST_DATA_DIR}"
    exit ${code}
}
trap cleanup EXIT

start_server() {
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
    RUST_LOG="${RUST_LOG_OVERRIDE:-warn,runtara_server=info,runtara_environment=info}" \
    AUTH_PROVIDER=local \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true \
    SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}" >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!

    for _ in {1..60}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        sleep 1
        if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
            print_error "Server exited during boot."; exit 1
        fi
    done
    print_error "Server did not become healthy."; exit 1
}

agent_step() {
    jq -n --arg id "$1" --arg agent "$2" --arg cap "$3" --argjson input "$4" '{
      stepType: "Agent", id: $id, agentId: $agent, capabilityId: $cap,
      inputMapping: ($input | with_entries(.value = {valueType: "immediate", value: .value}))
    }'
}

# A chain of Agent steps ending in Finish, as an execution graph.
graph() {
    jq -n --argjson steps "$1" '{
      name: "memory-tier-harness",
      entryPoint: $steps[0].id,
      steps: (($steps | map({key: .id, value: .}) | from_entries) + {
        finish: { stepType: "Finish", id: "finish",
                  inputMapping: { done: { valueType: "immediate", value: true } } }
      }),
      executionPlan: ([range(0; $steps | length)] | map({
        fromStep: $steps[.].id,
        toStep: (if . + 1 < ($steps | length) then $steps[. + 1].id else "finish" end)
      })),
      variables: {}, inputSchema: {}, outputSchema: {}
    }'
}

create_workflow() {
    local resp wf_id
    resp=$(api_post /workflows/create "{\"name\": \"$1\", \"description\": \"memory tier\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || { print_error "Workflow create failed: ${resp}"; exit 1; }
    echo "${wf_id}"
}

# Save a new version with a memory (a MiB number, or a quoted tier preset);
# echo its version number.
update_workflow() {
    local resp field=memoryMb
    case "$3" in \"*) field=memoryTier ;; esac
    resp=$(api_post "/workflows/$1/update" "{\"executionGraph\": $2, \"${field}\": $3}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Update failed: ${resp}"; exit 1; }
    curl -sS "${API}/workflows/$1/versions" | jq -r '[.data[]?.versionNumber] | max'
}

compile() {
    api_post "/workflows/$1/versions/$2/compile" '{}' 900
}

# Run the workflow to a terminal status; echo "<status>|<error>".
run() {
    local resp inst st
    resp=$(api_post "/workflows/$1/execute" '{"inputs": {"data": {}, "variables": {}}}')
    inst=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    [ -n "${inst}" ] || { print_error "Execute failed: ${resp}"; exit 1; }
    for _ in {1..120}; do
        resp=$(curl -sS "${API}/workflows/instances/${inst}")
        st=$(echo "${resp}" | jq -r '.data.status // empty')
        case "${st}" in
            completed|failed|cancelled|timeout)
                echo "${st}|$(echo "${resp}" | jq -c '.data.error // .data.errorMessage // empty')"
                return 0 ;;
        esac
        sleep 1
    done
    print_error "Instance ${inst} never finished (status ${st})"; exit 1
}

echo "==============================================================="
echo "E2E: workflow memory tier and its compile floor"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN} (cargo build -p runtara-server --bin runtara-server)"; exit 1; }
[ -f "${COMPONENTS_DIR}/runtara_agent_text.wasm" ] || { print_error "Missing agent components — run scripts/build-agent-components.sh"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres (psql on PATH, or container '${PG_CONTAINER}')"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
start_server

# macOS bash 3.2 brace-expands JSON quoted inside "$( )", so every step is
# built into a variable first.
pad() {
    local input step
    input=$(jq -n --argjson length "$1" '{text: "x", length: $length, pad_char: "y"}')
    step=$(agent_step pad text pad-text "${input}")
    graph "[${step}]"
}

# ---------------------------------------------------------------------------
print_step "Case 1: a small pad under tier L compiles and completes..."
WF_PAD=$(create_workflow memory-tier-pad)
V=$(update_workflow "${WF_PAD}" "$(pad 1000)" 128)
RESP=$(compile "${WF_PAD}" "${V}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Compile failed: ${RESP}"; exit 1; }
FLOOR=$(curl -sS "${API}/workflows/${WF_PAD}/versions" | jq -r --argjson v "${V}" '.data[] | select(.versionNumber == $v) | .memoryFloorBytes // empty')
echo "  memory floor: ${FLOOR} bytes"
[ -n "${FLOOR}" ] && [ "${FLOOR}" -gt 0 ] || { print_error "The compiled version records no memory floor"; exit 1; }
LIMIT=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c \
    "SELECT metadata->'workflow'->>'memoryLimitBytes' FROM images ORDER BY created_at DESC LIMIT 1" | tr -d '[:space:]')
echo "  image memory limit: ${LIMIT} bytes"
[ "${LIMIT}" = "$((128 * 1024 * 1024))" ] || { print_error "The image records memory limit '${LIMIT}', not 128 MiB"; exit 1; }
MEMORY=$(curl -sS "${API}/workflows/${WF_PAD}" | jq -r '.data.memoryMb // empty')
[ "${MEMORY}" = 128 ] || { print_error "The workflow reports memoryMb '${MEMORY}', not 128"; exit 1; }
OUT=$(run "${WF_PAD}")
[ "${OUT%%|*}" = "completed" ] || { print_error "Expected completed, got ${OUT}"; exit 1; }
print_success "Completed with 128 MiB, floor ${FLOOR} bytes recorded ✓"

# ---------------------------------------------------------------------------
print_step "Case 2: a 20 MB pad under the S preset fails with a memory error..."
V=$(update_workflow "${WF_PAD}" "$(pad 20000000)" '"S"')
RESP=$(compile "${WF_PAD}" "${V}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Compile failed: ${RESP}"; exit 1; }
OUT=$(run "${WF_PAD}")
echo "  ${OUT}"
[ "${OUT%%|*}" = "failed" ] || { print_error "Expected failed, got ${OUT}"; exit 1; }
echo "${OUT}" | grep -q "memory limit exceeded" || { print_error "The failure does not name the memory limit: ${OUT}"; exit 1; }
print_success "Failed on its 8 MiB limit ✓"

# ---------------------------------------------------------------------------
print_step "Case 3: seven agents with 8 MiB are refused at compile..."
STEPS="["
for spec in 'a text case-conversion {"text":"a","format":"uppercase"}' \
            'b transform coalesce {"values":[null,1]}' \
            'c utils do-nothing {}' \
            'd datetime get-current-date {}' \
            'e crypto hash {"data":"a","algorithm":"sha256"}' \
            'f csv get-header {"data":"a,b"}' \
            'g xml from-xml {"data":"<a/>"}'; do
    read -r id agent cap input <<< "${spec}"
    step=$(agent_step "${id}" "${agent}" "${cap}" "${input}")
    [ "${STEPS}" = "[" ] || STEPS="${STEPS},"
    STEPS="${STEPS}${step}"
done
STEPS="${STEPS}]"
WF_WIDE=$(create_workflow memory-tier-wide)
V=$(update_workflow "${WF_WIDE}" "$(graph "${STEPS}")" 8)
RESP=$(compile "${WF_WIDE}" "${V}")
echo "  $(echo "${RESP}" | jq -c '{success, message, error}')"
[ "$(echo "${RESP}" | jq -r '.success // false')" = "false" ] || { print_error "Compile with 8 MiB succeeded: ${RESP}"; exit 1; }
echo "${RESP}" | grep -q "below this workflow's memory floor" || { print_error "The refusal does not name the floor: ${RESP}"; exit 1; }
V=$(update_workflow "${WF_WIDE}" "$(graph "${STEPS}")" 16)
RESP=$(compile "${WF_WIDE}" "${V}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Compile with 16 MiB failed: ${RESP}"; exit 1; }
OUT=$(run "${WF_WIDE}")
[ "${OUT%%|*}" = "completed" ] || { print_error "Expected completed with 16 MiB, got ${OUT}"; exit 1; }
print_success "Refused at 8 MiB with its floor named, ran at 16 MiB ✓"

echo
print_success "A workflow's memory bounds its run, above the floor compile records."
