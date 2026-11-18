#!/bin/bash
# E2E Test: a server's executors come and go without the server being told
# (docs/executor.md).
#
# A server with RUNTARA_RUNNER=executor is configured with no executor: each
# one tells the server it is here, and fetches the workflows it runs. This
# test starts the server alone and then, against real `runtara-executor`
# processes:
#
#   0. compiling       with no executor a workflow is not compiled; with one,
#                      it is compiled there, by a compiler placed as a guest
#   1. no executor     a run is not failed; it waits to be offered again
#   2. one joins       the waiting run completes
#   3. a second joins  runs go to both, and each fetched the workflow itself
#   4. a cache is lost the executor fetches the workflow again
#   5. one is killed   its runs are over, and new runs go to the one left
#   6. server restarts it learns the executor from the executor's next call
#   7. one stops       it tells the server, which forgets it at once
#
# Usage: e2e/test_scalable_executors.sh
# Prerequisites: Postgres (POSTGRES_PASSWORD or PGPASSWORD in the
# environment; `psql` on PATH or the container PG_CONTAINER), docker (an
# isolated Valkey) and the built components
# (scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# runtara-server needs a credential encryption key; a throwaway one per run.
# shellcheck source=../scripts/credential-key.sh
source "${PROJECT_ROOT}/scripts/credential-key.sh"
ensure_credential_key

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-${PGPASSWORD:-}}"

TEST_DB_SERVER="scalable_executors_e2e_server_$$"
TEST_DB_RUNTIME="scalable_executors_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17760}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18761}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18762}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18763}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18764}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16399}"
RUN_CHANNEL_PORT="${TEST_RUN_CHANNEL_PORT:-17464}"
EXECUTOR_PORT_BASE="${TEST_EXECUTOR_PORT:-17470}"
TENANT="scalable_executors_e2e"

TARGET_DIR="${CARGO_TARGET_DIR:-${PROJECT_ROOT}/target}"
RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${TARGET_DIR}/debug/runtara-server}"
EXECUTOR_BIN="${TARGET_DIR}/debug/runtara-executor"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

WORK_DIR="$(mktemp -d -t runtara_scalable_executors_e2e_XXXXXX)"
SERVER_LOG="${WORK_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
EXECUTOR_PIDS=("" "")
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"

# `psql` is not always on PATH where Postgres runs in a container; then it is
# run inside that container.
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

cleanup() {
    local code=$?
    for pid in "${EXECUTOR_PIDS[@]}" "${SERVER_PID}"; do
        [ -z "${pid}" ] || kill "${pid}" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    [ -z "${VALKEY_CONTAINER}" ] || docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER} WITH (FORCE)" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME} WITH (FORCE)" >/dev/null 2>&1 || true
    if [ ${code} -ne 0 ]; then
        for log in "${SERVER_LOG}" "${WORK_DIR}"/executor-*.log; do
            [ -f "${log}" ] && { echo "--- ${log##*/} (tail)" >&2; tail -30 "${log}" >&2; }
        done
    fi
    # The certificates' keys never outlive the test.
    rm -rf "${WORK_DIR}"
    exit ${code}
}
trap cleanup EXIT

api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}

start_server() {
    RUNTARA_RUNNER=executor \
    RUNTARA_EXECUTOR_TLS_DIR="${WORK_DIR}/pki-server" \
    RUNTARA_RUN_CHANNEL_LISTEN="127.0.0.1:${RUN_CHANNEL_PORT}" \
    RUNTARA_RUN_CHANNEL_ENDPOINT="https://127.0.0.1:${RUN_CHANNEL_PORT}" \
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
    RUNTARA_DATABASE_URL="${RUNTIME_DB_URL}" \
    TENANT_ID="${TENANT}" \
    SERVER_HOST=127.0.0.1 \
    SERVER_PORT="${TEST_PORT_PUBLIC}" \
    RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
    RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
    RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
    RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
    RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
    DATA_DIR="${WORK_DIR}/data" \
    RUNTARA_DEV_MODE=false \
    RUST_LOG="warn,runtara_server=info,runtara_environment=info,runtara_runner_executor=info" \
    AUTH_PROVIDER=local \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true \
    SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}" >>"${SERVER_LOG}" 2>&1 &
    SERVER_PID=$!
    for _ in {1..90}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "the server exited during boot"; exit 1; }
        sleep 1
    done
    print_error "the server did not become healthy"; exit 1
}

# Only Linux has the sandbox; elsewhere the runs are plain processes, which
# still exercises everything between the server and the guest.
UNSANDBOXED=false
[ "$(uname -s)" = "Linux" ] || UNSANDBOXED=true

# Starts executor $1 (0 or 1) and waits until it listens.
start_executor() {
    local index="$1" log="${WORK_DIR}/executor-$1.log"
    RUNTARA_EXECUTOR_LISTEN="127.0.0.1:$((EXECUTOR_PORT_BASE + index))" \
    RUNTARA_EXECUTOR_SERVER_URL="https://127.0.0.1:${RUN_CHANNEL_PORT}" \
    RUNTARA_EXECUTOR_TLS_DIR="${WORK_DIR}/pki-executor" \
    RUNTARA_EXECUTOR_STATE_DIR="${WORK_DIR}/executor-state-${index}" \
    RUNTARA_EXECUTOR_MAX_RUNS=4 \
    RUNTARA_EXECUTOR_UNSANDBOXED="${UNSANDBOXED}" \
    RUNTARA_EXECUTOR_NO_LANDLOCK="${RUNTARA_EXECUTOR_NO_LANDLOCK:-false}" \
        "${EXECUTOR_BIN}" >>"${log}" 2>&1 &
    EXECUTOR_PIDS[index]=$!
    for _ in $(seq 1 50); do
        grep -q "executor listening" "${log}" && return 0
        kill -0 "${EXECUTOR_PIDS[index]}" 2>/dev/null || break
        sleep 0.2
    done
    print_error "executor ${index} did not start"; exit 1
}

# How many lines of `$1` hold `$2`.
count_in() { grep -c -- "$2" "$1" 2>/dev/null || true; }

# Waits up to $3 seconds until `$1` holds `$2` at least `$4` (default 1) times.
wait_for_log() {
    local log="$1" text="$2" seconds="$3" times="${4:-1}"
    for _ in $(seq 1 "$((seconds * 5))"); do
        [ "$(count_in "${log}" "${text}")" -ge "${times}" ] && return 0
        sleep 0.2
    done
    print_error "\"${text}\" did not appear ${times} time(s) in ${log##*/} within ${seconds}s"; exit 1
}

instance_status() {
    curl -sS "${API}/workflows/instances/$1" | jq -r '.data.status // .status // empty'
}

# Starts a run; echoes its instance id.
execute() {
    local resp
    resp=$(api_post "/workflows/${WORKFLOW}/execute" '{"inputs": {"data": {"amount": 150}}}')
    echo "${resp}" | jq -er '.data.instanceId' || { print_error "execute failed: ${resp}"; exit 1; }
}

# Waits up to $1 seconds for every instance after it to complete.
all_complete() {
    local seconds="$1" status; shift
    for instance in "$@"; do
        for _ in $(seq 1 "$((seconds * 2))"); do
            status="$(instance_status "${instance}")"
            [ "${status}" = "completed" ] && continue 2
            [ "${status}" = "failed" ] || [ "${status}" = "cancelled" ] \
                && { print_error "instance ${instance} ended ${status}"; exit 1; }
            sleep 0.5
        done
        print_error "instance ${instance} is still ${status} after ${seconds}s"; exit 1
    done
}

# Runs $1 instances and waits for all of them to complete.
run_many() {
    local instances=()
    for _ in $(seq 1 "$1"); do instances+=("$(execute)"); done
    all_complete 120 "${instances[@]}"
}

placed_by() { count_in "${WORK_DIR}/executor-$1.log" "run placed"; }

echo "==============================================================="
echo "E2E Test: executors register themselves and fetch their workflows"
echo "==============================================================="

[ -n "${POSTGRES_PASSWORD}" ] \
    || { print_error "set POSTGRES_PASSWORD (or PGPASSWORD) for ${POSTGRES_USER}@${POSTGRES_HOST}"; exit 1; }
if [ ! -x "${RUNTARA_SERVER_BIN}" ] || [ ! -x "${EXECUTOR_BIN}" ] || [ ! -x "${TARGET_DIR}/debug/runtara-run" ]; then
    print_step "Building runtara-server, runtara-executor and runtara-run..."
    (cd "${PROJECT_ROOT}" && SQLX_OFFLINE="${SQLX_OFFLINE}" \
        cargo build -p runtara-server -p runtara-executor -p runtara-run >&2)
fi
[ -f "${COMPONENTS_DIR}/runtara_workflow_stdlib.wasm" ] \
    || { print_error "missing runtara_workflow_stdlib.wasm: run scripts/build-agent-components.sh"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 \
    || { print_error "cannot reach Postgres (psql on PATH, or the container '${PG_CONTAINER}')"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker is required (an isolated Valkey)"; exit 1; }

"${EXECUTOR_BIN}" init-pki \
    --server-dir "${WORK_DIR}/pki-server" --executor-dir "${WORK_DIR}/pki-executor" >&2

print_step "Starting an isolated Valkey and the databases..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > "/dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}") 2>/dev/null && break; sleep 0.5; done
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting the server, told of no executor..."
start_server

RESP=$(api_post /workflows/create '{"name":"scalable executors e2e","description":"conditional"}')
WORKFLOW=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${WORKFLOW}" ] || { print_error "create failed: ${RESP}"; exit 1; }
GRAPH=$(cat "${PROJECT_ROOT}/examples/workflows/01-conditional.json")
RESP=$(api_post "/workflows/${WORKFLOW}/update" "{\"executionGraph\": ${GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "update failed: ${RESP}"; exit 1; }
VERSION=$(curl -sS "${API}/workflows/${WORKFLOW}/versions" \
    | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')

# --- 0 ------------------------------------------------------------------
# A workflow is compiled where workflows run: on an executor, by a compiler
# placed there as a guest (docs/compiler-on-an-executor.md). The server does
# not compile it itself instead.
print_step "0. With no executor, a workflow is not compiled, in the server or anywhere..."
grep -q "Workflows are compiled on an executor" "${SERVER_LOG}" \
    || { print_error "the server does not say it compiles on an executor"; exit 1; }
RESP=$(api_post "/workflows/${WORKFLOW}/versions/${VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "false" ] \
    && echo "${RESP}" | grep -q "no executor is here" \
    || { print_error "with no executor a workflow was compiled, or failed otherwise: ${RESP}"; exit 1; }
start_executor 0
wait_for_log "${SERVER_LOG}" "an executor registered" 20
RESP=$(api_post "/workflows/${WORKFLOW}/versions/${VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "compile failed: ${RESP}"; exit 1; }
api_post "/workflows/${WORKFLOW}/versions/${VERSION}/set-current" '{}' >/dev/null
# The compiler is kept by the executor under the platform's name, once.
ls "${WORK_DIR}/executor-state-0/compiled/runtara-compiler/"*.cwasm >/dev/null 2>&1 \
    || { print_error "the executor does not hold the compiler it ran"; exit 1; }
kill "${EXECUTOR_PIDS[0]}"; wait "${EXECUTOR_PIDS[0]}" 2>/dev/null || true
EXECUTOR_PIDS[0]=""
wait_for_log "${SERVER_LOG}" "an executor left" 15
print_success "the workflow was compiled on an executor, and not at all without one"

# --- 1 and 2 ------------------------------------------------------------
print_step "1. With no executor, a run is not failed..."
EARLY=$(execute)
sleep 8
STATUS="$(instance_status "${EARLY}")"
[ "${STATUS}" != "failed" ] && [ "${STATUS}" != "completed" ] \
    || { print_error "with no executor the run is ${STATUS}"; exit 1; }
echo "  the run is ${STATUS}"

print_step "2. An executor joins, and the waiting run completes..."
start_executor 0
wait_for_log "${SERVER_LOG}" "an executor registered" 20 2
all_complete 180 "${EARLY}"
print_success "the run waited for an executor and completed on it"

# --- 3 ------------------------------------------------------------------
print_step "3. A second executor joins, and runs go to both..."
start_executor 1
wait_for_log "${SERVER_LOG}" "an executor registered" 20 3
run_many 12
for index in 0 1; do
    [ "$(placed_by "${index}")" -gt 0 ] \
        || { print_error "executor ${index} took no run ($(placed_by 0) and $(placed_by 1))"; exit 1; }
    ls "${WORK_DIR}/executor-state-${index}/compiled/"*/*.cwasm >/dev/null 2>&1 \
        || { print_error "executor ${index} holds no workflow"; exit 1; }
done
print_success "executor 0 took $(placed_by 0) runs and executor 1 took $(placed_by 1); each fetched the workflow"

# --- 4 ------------------------------------------------------------------
print_step "4. An executor that lost its cache fetches the workflow again..."
rm -rf "${WORK_DIR}/executor-state-0/compiled" "${WORK_DIR}/executor-state-1/compiled"
run_many 6
ls "${WORK_DIR}"/executor-state-*/compiled/*/*.cwasm >/dev/null 2>&1 \
    || { print_error "no executor fetched the workflow again"; exit 1; }
print_success "runs completed on executors that had lost the workflow"

# --- 5 ------------------------------------------------------------------
print_step "5. An executor is killed; new runs go to the one left..."
kill -9 "${EXECUTOR_PIDS[1]}"; wait "${EXECUTOR_PIDS[1]}" 2>/dev/null || true
EXECUTOR_PIDS[1]=""
BEFORE=$(placed_by 0)
run_many 8
[ "$(( $(placed_by 0) - BEFORE ))" -ge 8 ] \
    || { print_error "executor 0 took $(( $(placed_by 0) - BEFORE )) of 8 runs after the other died"; exit 1; }
print_success "every run completed on the executor that was left"

# --- 6 ------------------------------------------------------------------
print_step "6. The server starts again and learns its executor from it..."
kill "${SERVER_PID}"; wait "${SERVER_PID}" 2>/dev/null || true
REGISTERED=$(count_in "${SERVER_LOG}" "an executor registered")
start_server
wait_for_log "${SERVER_LOG}" "an executor registered" 30 "$((REGISTERED + 1))"
run_many 4
print_success "the restarted server placed runs with an executor it was never told of"

# --- 7 ------------------------------------------------------------------
print_step "7. An executor that stops says so..."
kill "${EXECUTOR_PIDS[0]}"; wait "${EXECUTOR_PIDS[0]}" 2>/dev/null || true
EXECUTOR_PIDS[0]=""
wait_for_log "${SERVER_LOG}" "an executor left" 15 2
print_success "the server forgot the executor as it stopped"

echo
print_success "executors came, went and came back; the server was told of none of them"
