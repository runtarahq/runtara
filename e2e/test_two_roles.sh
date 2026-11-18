#!/bin/bash
# E2E Test: the server's API and its engine as two processes
# (RUNTARA_ROLE, docs/separating-concerns.md).
#
# What takes requests from outside holds nothing of the engine's. This test
# starts, as separate processes: seal, an engine (RUNTARA_ROLE=engine), two
# executors, and an API (RUNTARA_ROLE=api) whose environment is written out
# here in full. Everything is asked of the API.
#
#   1. refusals         an API given the runtime database refuses to start,
#                       and so does an engine with nowhere to listen, or
#                       with seal in itself
#   2. what each holds  the API's environment has no key, no runtime database
#                       and no way to seal; the engine has no public listener
#   3. runs             a workflow is made, compiled and run through the API,
#                       on both executors; what anyone may send is refused
#                       for want of intake, by the API and by the engine; one
#                       is started by a message into a conversation, and
#                       one is published as an agent and called by another:
#                       each compiled on an executor, none in the engine
#   4. a connection     made through the API, sealed by seal, and used
#   5. certificates     the engine's listener takes the API's certificate and
#                       not an executor's; the run channel refuses the API's
#   6. the API dies     runs started before it was killed complete
#   7. the engine away  the API still answers, says the engine is away, and
#                       starts nothing; when the engine is back, it works
#   8. a second API     in front of the same engine
#
# Usage: e2e/test_two_roles.sh
# Prerequisites: Postgres (POSTGRES_PASSWORD or PGPASSWORD in the
# environment; `psql` on PATH or the container PG_CONTAINER), docker (an
# isolated Valkey), the built components (scripts/build-agent-components.sh),
# and seal's binary: SEAL_BIN, or it is built at the revision the server uses.

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# The engine needs a credential encryption key; a throwaway one per run. It
# is kept out of this script's own environment, so that only the engine is
# given it.
# shellcheck source=../scripts/credential-key.sh
source "${PROJECT_ROOT}/scripts/credential-key.sh"
ENGINE_KEY="$(generate_credential_key)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-${PGPASSWORD:-}}"

TEST_DB_SERVER="two_roles_e2e_server_$$"
TEST_DB_RUNTIME="two_roles_e2e_runtime_$$"
API_PORT="${TEST_PORT_PUBLIC:-17860}"
SECOND_API_PORT="${TEST_SECOND_API_PORT:-17861}"
ENGINE_PORT="${TEST_ENGINE_PORT:-17865}"
# Where the engine would serve the public API, if it served one.
ENGINE_PUBLIC_PORT="${TEST_ENGINE_PUBLIC_PORT:-17866}"
# Where the engine says it is up, and nothing else.
ENGINE_HEALTH_PORT="${TEST_ENGINE_HEALTH_PORT:-17867}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18861}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18862}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18863}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18864}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16398}"
RUN_CHANNEL_PORT="${TEST_RUN_CHANNEL_PORT:-17564}"
EXECUTOR_PORT_BASE="${TEST_EXECUTOR_PORT:-17570}"
TENANT="two_roles_e2e"

TARGET_DIR="${CARGO_TARGET_DIR:-${PROJECT_ROOT}/target}"
RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${TARGET_DIR}/debug/runtara-server}"
EXECUTOR_BIN="${TARGET_DIR}/debug/runtara-executor"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"
# The compiler's WASI command, which an engine that is a process of its own
# compiles in: RUNTARA_COMPILER_WASM, or built beside the components.
COMPILER_WASM="${RUNTARA_COMPILER_WASM:-${COMPONENTS_DIR}/runtara-compile-sandboxed.wasm}"

WORK_DIR="$(mktemp -d -t runtara_two_roles_e2e_XXXXXX)"
# A socket's path is short.
SEAL_DIR="$(mktemp -d /tmp/rseal.XXXXXX)"
ENGINE_LOG="${WORK_DIR}/engine.log"
API_LOG="${WORK_DIR}/api.log"
SECOND_API_LOG="${WORK_DIR}/api-2.log"
SEAL_LOG="${WORK_DIR}/seal.log"
ENGINE_PID=""; API_PID=""; SECOND_API_PID=""; SEAL_PID=""
VALKEY_CONTAINER=""
EXECUTOR_PIDS=("" "")
API="http://127.0.0.1:${API_PORT}/api/runtime"

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

stop() {
    [ -n "${1:-}" ] || return 0
    kill "$1" 2>/dev/null || true
    wait "$1" 2>/dev/null || true
}

cleanup() {
    local code=$?
    for pid in "${API_PID}" "${SECOND_API_PID}" "${EXECUTOR_PIDS[@]}" "${ENGINE_PID}" "${SEAL_PID}"; do
        [ -z "${pid}" ] || kill "${pid}" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    [ -z "${VALKEY_CONTAINER}" ] || docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER} WITH (FORCE)" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME} WITH (FORCE)" >/dev/null 2>&1 || true
    if [ ${code} -ne 0 ]; then
        for log in "${API_LOG}" "${ENGINE_LOG}" "${SEAL_LOG}" "${WORK_DIR}"/executor-*.log; do
            [ -f "${log}" ] && { echo "--- ${log##*/} (tail)" >&2; tail -30 "${log}" >&2; }
        done
    fi
    # The certificates' keys and seal's never outlive the test.
    rm -rf "${WORK_DIR}" "${SEAL_DIR}"
    exit ${code}
}
trap cleanup EXIT

api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}
http_status() { curl -sS -o /dev/null --max-time 30 -w '%{http_code}' "$@"; }
healthy() { [ "$(http_status "http://127.0.0.1:$1/health" 2>/dev/null || true)" = "200" ]; }

# --- the engine ------------------------------------------------------------
# Everything the server was given before, and where it listens for the API.
engine_env() {
    exec env \
    RUNTARA_ROLE=engine \
    RUNTARA_ENGINE_LISTEN="127.0.0.1:${ENGINE_PORT}" \
    RUNTARA_ENGINE_HEALTH_LISTEN="127.0.0.1:${ENGINE_HEALTH_PORT}" \
    RUNTARA_COMPILER_WASM="${COMPILER_WASM}" \
    RUNTARA_RUNNER=executor \
    RUNTARA_EXECUTOR_TLS_DIR="${WORK_DIR}/pki-engine" \
    RUNTARA_RUN_CHANNEL_LISTEN="127.0.0.1:${RUN_CHANNEL_PORT}" \
    RUNTARA_RUN_CHANNEL_ENDPOINT="https://127.0.0.1:${RUN_CHANNEL_PORT}" \
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
    RUNTARA_DATABASE_URL="${RUNTIME_DB_URL}" \
    RUNTARA_CONNECTIONS_ENCRYPTION_KEY="${ENGINE_KEY}" \
    RUNTARA_SEAL_MODE=unix \
    RUNTARA_SEAL_SOCKET="${SEAL_DIR}/seal.sock" \
    TENANT_ID="${TENANT}" \
    SERVER_HOST=127.0.0.1 \
    SERVER_PORT="${ENGINE_PUBLIC_PORT}" \
    RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
    RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
    RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
    RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
    RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
    DATA_DIR="${WORK_DIR}/data" \
    RUNTARA_DEV_MODE=false \
    RUST_LOG="warn,runtara_server=info,runtara_environment=info,runtara_runner_executor=info" \
    AUTH_PROVIDER=local \
    RUNTARA_DATABASE_CONNECTION_GUARD=off \
    RUNTARA_PROXY_ALLOWED_HOSTS="${POSTGRES_HOST}" \
    RUNTARA_PROXY_ALLOW_HTTP_HOSTS="${POSTGRES_HOST}" \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true \
    SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "$@"
}

start_engine() {
    (engine_env "${RUNTARA_SERVER_BIN}") >>"${ENGINE_LOG}" 2>&1 &
    ENGINE_PID=$!
    local listening
    listening=$(count_in "${ENGINE_LOG}" "Engine listening for the API role")
    for _ in {1..120}; do
        [ "$(count_in "${ENGINE_LOG}" "engine is ready")" -gt "${1:-0}" ] \
            && [ "$(count_in "${ENGINE_LOG}" "Engine listening for the API role")" -ge "${listening}" ] && return 0
        kill -0 "${ENGINE_PID}" 2>/dev/null || { print_error "the engine exited during boot"; exit 1; }
        sleep 1
    done
    print_error "the engine did not become ready"; exit 1
}

# --- the API ---------------------------------------------------------------
# The whole of the API's environment: nothing is inherited. `$1` is its port.
API_ENV=(
    "PATH=${PATH}"
    "HOME=${HOME}"
    RUNTARA_ROLE=api
    "RUNTARA_ENGINE_URL=https://127.0.0.1:${ENGINE_PORT}"
    "RUNTARA_ENGINE_TLS_DIR=${WORK_DIR}/pki-api"
    "RUNTARA_SERVER_DATABASE_URL=${SERVER_DB_URL}"
    "RUNTARA_AGENT_COMPONENTS_DIR=${WORK_DIR}/agent-descriptions"
    "TENANT_ID=${TENANT}"
    SERVER_HOST=127.0.0.1
    RUNTARA_DEV_MODE=false
    "RUST_LOG=warn,runtara_server=info"
    AUTH_PROVIDER=local
    VALKEY_HOST=127.0.0.1
    "VALKEY_PORT=${TEST_VALKEY_PORT}"
    OTEL_SDK_DISABLED=true
    SKIP_MIGRATIONS=true
)

# Starts an API on port $1, logging to $2; echoes its pid.
start_api() {
    local port="$1" log="$2" pid
    (cd "${WORK_DIR}" && exec env -i "${API_ENV[@]}" "SERVER_PORT=${port}" "${RUNTARA_SERVER_BIN}") >>"${log}" 2>&1 &
    pid=$!
    for _ in {1..60}; do
        healthy "${port}" && { echo "${pid}"; return 0; }
        kill -0 "${pid}" 2>/dev/null || { print_error "the API exited during boot"; exit 1; }
        sleep 1
    done
    print_error "the API did not become healthy"; exit 1
}

# A server that must not start: echoes what it said. `$@` is its command.
refused_start() {
    local out="${WORK_DIR}/refused.log" pid
    ("$@") >"${out}" 2>&1 &
    pid=$!
    for _ in {1..60}; do
        kill -0 "${pid}" 2>/dev/null || break
        sleep 0.5
    done
    if kill -0 "${pid}" 2>/dev/null; then
        stop "${pid}"
        print_error "the server started"
        return 1
    fi
    cat "${out}"
}

# --- seal --------------------------------------------------------------------
start_seal() {
    mkdir "${SEAL_DIR}/keys"
    (umask 077 && head -c 32 /dev/urandom > "${SEAL_DIR}/kek")
    cat > "${SEAL_DIR}/seal.toml" <<TOML
[listen]
unix = "${SEAL_DIR}/seal.sock"

# The engine: every tenant's keyset, which it makes, reads the public keys
# of, and destroys when a tenant leaves.
[[callers]]
name = "runtara-server"
unix_uid = $(id -u)
keysets = ["*"]
admin = true
destroy_keysets = true

[keys]
source = { dir = "${SEAL_DIR}/keys" }
wrap = { file = "${SEAL_DIR}/kek" }
lock_memory = "off"

[network]
allow_private_cidrs = ["127.0.0.0/8", "::1/128"]
honour_private_rules = true
honour_plaintext_postgres_rules = true

[audit]
sink = { file = "${SEAL_DIR}/audit.jsonl" }
TOML
    chmod 600 "${SEAL_DIR}/seal.toml"
    "${SEAL_BIN}" check-config --config "${SEAL_DIR}/seal.toml" >>"${SEAL_LOG}" 2>&1 \
        || { print_error "seal refuses its configuration"; exit 1; }
    "${SEAL_BIN}" serve --config "${SEAL_DIR}/seal.toml" >>"${SEAL_LOG}" 2>&1 &
    SEAL_PID=$!
    for _ in {1..40}; do
        kill -0 "${SEAL_PID}" 2>/dev/null || { print_error "seal exited as it started"; exit 1; }
        [ -S "${SEAL_DIR}/seal.sock" ] && return 0
        sleep 0.25
    done
    print_error "seal did not listen"; exit 1
}

# --- executors -------------------------------------------------------------
# Only Linux has the sandbox; elsewhere the runs are plain processes, which
# still exercises everything between the engine and the guest.
UNSANDBOXED=false
[ "$(uname -s)" = "Linux" ] || UNSANDBOXED=true

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
runs_known() { curl -sS "${API}/executions?size=1" | jq -r '.data.totalElements // .totalElements // empty'; }

# Asks the engine's listener (or any listener of the deployment's) for what
# the engine says of itself, with the certificate in directory $1, at port
# $2; echoes the HTTP status, which is 000 if the connection was refused.
ask_with_certificate() {
    curl -sS -o /dev/null -w '%{http_code}' --http2 --max-time 10 \
        --cacert "$1/ca.pem" --cert "$1/tls.pem" --key "$1/tls.key" \
        --resolve "runtara-server:$2:127.0.0.1" \
        -X POST -H 'Content-Type: application/json' -d '{}' \
        "https://runtara-server:$2/engine/v1/operations/Facts" 2>/dev/null || true
}

echo "==============================================================="
echo "E2E Test: the API and the engine as two processes"
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
SEAL_REV=$(sed -n 's/^seal-client = .*rev = "\([0-9a-f]*\)".*/\1/p' "${PROJECT_ROOT}/crates/runtara-connections/Cargo.toml")
if [ -z "${SEAL_BIN:-}" ]; then
    SEAL_ROOT="${PROJECT_ROOT}/target/seal/${SEAL_REV}"
    SEAL_BIN="${SEAL_ROOT}/bin/seal"
    if [ ! -x "${SEAL_BIN}" ]; then
        print_step "Building seal at ${SEAL_REV:0:12}..."
        cargo install --locked --git https://github.com/runtarahq/seal --rev "${SEAL_REV}" \
            --root "${SEAL_ROOT}" seal >&2
    fi
fi
[ -x "${SEAL_BIN}" ] || { print_error "seal's binary is not at ${SEAL_BIN}"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 \
    || { print_error "cannot reach Postgres (psql on PATH, or the container '${PG_CONTAINER}')"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker is required (an isolated Valkey)"; exit 1; }

# One CA, three certificates: the engine's, the executors', the API's.
"${EXECUTOR_BIN}" init-pki --server-dir "${WORK_DIR}/pki-engine" \
    --executor-dir "${WORK_DIR}/pki-executor" --api-dir "${WORK_DIR}/pki-api" >&2
# The API describes agents from their descriptions, and is given no component.
mkdir "${WORK_DIR}/agent-descriptions"
cp "${COMPONENTS_DIR}"/runtara_agent_*.meta.json "${WORK_DIR}/agent-descriptions/"

print_step "Starting an isolated Valkey, the databases and seal..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > "/dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}") 2>/dev/null && break; sleep 0.5; done
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null
if [ ! -f "${COMPILER_WASM}" ]; then
    print_step "Building the compiler's WASI command..."
    cargo build -p runtara-compile-guest --bin runtara-compile-sandboxed --release --target wasm32-wasip2 >&2
fi
start_seal

# --- 1 ------------------------------------------------------------------
print_step "1. Each role refuses what it cannot be..."
SAID=$(refused_start env -i "${API_ENV[@]}" "SERVER_PORT=${API_PORT}" SKIP_MIGRATIONS=false \
    "RUNTARA_DATABASE_URL=${RUNTIME_DB_URL}" "${RUNTARA_SERVER_BIN}")
echo "${SAID}" | grep -q "RUNTARA_ROLE=api was given RUNTARA_DATABASE_URL" \
    || { print_error "an API given the runtime database did not refuse it: ${SAID}"; exit 1; }
SAID=$(refused_start env -i "${API_ENV[@]}" "SERVER_PORT=${API_PORT}" \
    "RUNTARA_CONNECTIONS_ENCRYPTION_KEY=${ENGINE_KEY}" "RUNTARA_SEAL_SOCKET=${SEAL_DIR}/seal.sock" \
    "${RUNTARA_SERVER_BIN}")
echo "${SAID}" | grep -q "was given RUNTARA_CONNECTIONS_ENCRYPTION_KEY, RUNTARA_SEAL_SOCKET" \
    || { print_error "an API given a key and the way to seal did not refuse them: ${SAID}"; exit 1; }
SAID=$(refused_start env -i "${API_ENV[@]}" "SERVER_PORT=${API_PORT}" \
    "RUNTARA_ENGINE_HEALTH_LISTEN=127.0.0.1:${ENGINE_HEALTH_PORT}" "${RUNTARA_SERVER_BIN}")
echo "${SAID}" | grep -q "was given RUNTARA_ENGINE_HEALTH_LISTEN" \
    || { print_error "an API given the engine's health address did not refuse it: ${SAID}"; exit 1; }
SAID=$(refused_start engine_env env -u RUNTARA_SEAL_MODE -u RUNTARA_SEAL_SOCKET "${RUNTARA_SERVER_BIN}")
echo "${SAID}" | grep -q "RUNTARA_ROLE=engine needs a seal of its own" \
    || { print_error "an engine with seal in itself did not refuse to start: ${SAID}"; exit 1; }
# With executors, workflows are compiled on one, and nowhere else instead: an
# engine that has not the compiler to place there does not start.
SAID=$(refused_start engine_env RUNTARA_COMPILE_GUEST_WASM="${WORK_DIR}/not-a-compiler.wasm" "${RUNTARA_SERVER_BIN}")
echo "${SAID}" | grep -q "RUNTARA_RUNNER=executor needs the compiler that is placed on an executor" \
    || { print_error "an engine with executors and no compiler to place did not refuse to start: ${SAID}"; exit 1; }
SAID=$(refused_start engine_env RUNTARA_VALIDATE_GUEST_WASM="${WORK_DIR}/not-a-validator.wasm" "${RUNTARA_SERVER_BIN}")
echo "${SAID}" | grep -q "RUNTARA_RUNNER=executor needs the validator that is placed on an executor" \
    || { print_error "an engine with executors and no validator to place did not refuse to start: ${SAID}"; exit 1; }
# Without executors it compiles in a sandbox of the compiler's own, or not at all.
SAID=$(refused_start engine_env env -u RUNTARA_RUNNER -u RUNTARA_COMPILER_WASM "${RUNTARA_SERVER_BIN}")
echo "${SAID}" | grep -q "RUNTARA_ROLE=engine needs RUNTARA_COMPILER_WASM" \
    || { print_error "an engine with no sandbox to compile in did not refuse to start: ${SAID}"; exit 1; }
SAID=$(refused_start engine_env env -u RUNTARA_RUNNER RUNTARA_COMPILER_WASM="${WORK_DIR}/not-a-compiler.wasm" "${RUNTARA_SERVER_BIN}")
echo "${SAID}" | grep -q "names a compiler that could not be loaded" \
    || { print_error "an engine whose compiler is not there did not refuse to start: ${SAID}"; exit 1; }
SAID=$(refused_start engine_env env -u RUNTARA_ENGINE_LISTEN "${RUNTARA_SERVER_BIN}")
echo "${SAID}" | grep -q "RUNTARA_ROLE=engine needs RUNTARA_ENGINE_LISTEN" \
    || { print_error "an engine with nowhere to listen did not refuse to start: ${SAID}"; exit 1; }
print_success "an API refuses the engine's holdings; an engine refuses to be unreachable, to hold seal's keys, or to compile in its own process"

# --- 2 ------------------------------------------------------------------
print_step "2. Starting the engine, two executors and the API..."
start_engine
start_executor 0
start_executor 1
wait_for_log "${ENGINE_LOG}" "an executor registered" 30 2
API_PID=$(start_api "${API_PORT}" "${API_LOG}")
for name in "${API_ENV[@]}"; do
    case "${name%%=*}" in
        RUNTARA_DATABASE_URL|RUNTARA_CONNECTIONS_*|RUNTARA_SEAL_*|RUNTARA_EXECUTOR_*|RUNTARA_ARTIFACT_*|RUNTARA_RUN_CHANNEL_*)
            print_error "the API is given ${name%%=*}"; exit 1 ;;
    esac
done
# The engine places its compiles on an executor, and does not load the
# compiler itself, though it is given one to compile in.
grep -q "Workflows are compiled on an executor" "${ENGINE_LOG}" \
    || { print_error "the engine does not say it compiles on an executor"; exit 1; }
grep -q "Workflows are validated on an executor" "${ENGINE_LOG}" \
    || { print_error "the engine does not say it validates on an executor"; exit 1; }
if grep -q "Workflows are compiled in a sandbox" "${ENGINE_LOG}"; then
    print_error "an engine with executors loaded the compiler into its own process"; exit 1
fi
grep -q "Engine at https://127.0.0.1:${ENGINE_PORT} answers" "${API_LOG}" \
    || { print_error "the API does not say the engine answers"; exit 1; }
if grep -Eq "Embedded runtara|Component dispatcher loaded|Connection secrets are held by seal|Execution engine initialized" "${API_LOG}"; then
    print_error "the API started something of the engine's"; exit 1
fi
# The engine's work is the engine's, and the API's the API's.
grep "Background work started" "${ENGINE_LOG}" | tail -1 | grep -Eq 'api=( |$)' \
    || { print_error "the engine started work that is the API's"; exit 1; }
grep "Background work started" "${ENGINE_LOG}" | tail -1 | grep -q "trigger worker" \
    || { print_error "the engine did not start its own work"; exit 1; }
grep "Background work started" "${API_LOG}" | tail -1 | grep -Eq 'engine=( |$)' \
    || { print_error "the API started work that is the engine's"; exit 1; }
grep "Background work started" "${API_LOG}" | tail -1 | grep -q "member control events" \
    || { print_error "the API did not start its own work"; exit 1; }
if healthy "${ENGINE_PUBLIC_PORT}"; then
    print_error "the engine serves the public API"; exit 1
fi
(echo > "/dev/tcp/127.0.0.1/${ENGINE_PUBLIC_PORT}") 2>/dev/null \
    && { print_error "the engine listens on the public port"; exit 1; }
# Where it is told to, the engine says it is up, and answers nothing else.
healthy "${ENGINE_HEALTH_PORT}" || { print_error "the engine does not say it is up"; exit 1; }
for path in /api/runtime/workflows /ready /engine/v1/operations/Facts; do
    [ "$(http_status "http://127.0.0.1:${ENGINE_HEALTH_PORT}${path}")" = "404" ] \
        || { print_error "the engine's health listener answers ${path}"; exit 1; }
done
print_success "the API holds no key, no runtime database and no way to seal; the engine has no public listener, and says it is up where it is told to"

# --- 3 ------------------------------------------------------------------
print_step "3. A workflow is made, compiled and run through the API..."
RESP=$(api_post /workflows/create '{"name":"two roles e2e","description":"conditional"}')
WORKFLOW=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${WORKFLOW}" ] || { print_error "create failed: ${RESP}"; exit 1; }
GRAPH=$(cat "${PROJECT_ROOT}/examples/workflows/01-conditional.json")
RESP=$(api_post "/workflows/${WORKFLOW}/update" "{\"executionGraph\": ${GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "update failed: ${RESP}"; exit 1; }
VERSION=$(curl -sS "${API}/workflows/${WORKFLOW}/versions" \
    | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(api_post "/workflows/${WORKFLOW}/versions/${VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "compile failed: ${RESP}"; exit 1; }
api_post "/workflows/${WORKFLOW}/versions/${VERSION}/set-current" '{}' >/dev/null
run_many 12
for index in 0 1; do
    [ "$(placed_by "${index}")" -gt 0 ] \
        || { print_error "executor ${index} took no run ($(placed_by 0) and $(placed_by 1))"; exit 1; }
done
# What the API reads of a run comes from the engine too.
FIRST=$(execute); all_complete 120 "${FIRST}"
[ "$(curl -sS "${API}/workflows/instances/${FIRST}" | jq -r '.data.workflowId // .data.workflow_id // empty')" = "${WORKFLOW}" ] \
    || { print_error "a run's record does not name its workflow"; exit 1; }
[ "$(http_status "${API}/workflows/${WORKFLOW}/instances/${FIRST}/steps")" = "200" ] \
    || { print_error "a run's steps are not read through the API"; exit 1; }
# Run and waited for: the answer is held until the run is over. The request
# itself is the run's input, so this is a workflow that asks for none.
ANSWER_GRAPH='{
  "name": "Answer",
  "steps": {
    "finish": {
      "stepType": "Finish", "id": "finish",
      "inputMapping": { "answer": { "valueType": "immediate", "value": 42 } }
    }
  },
  "entryPoint": "finish",
  "executionPlan": [],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}'
RESP=$(api_post /workflows/create '{"name":"two roles e2e answer","description":"answers 42"}')
ANSWER=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${ANSWER}" ] || { print_error "create failed: ${RESP}"; exit 1; }
RESP=$(api_post "/workflows/${ANSWER}/update" "{\"executionGraph\": ${ANSWER_GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "update failed: ${RESP}"; exit 1; }
ANSWER_VERSION=$(curl -sS "${API}/workflows/${ANSWER}/versions" \
    | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(api_post "/workflows/${ANSWER}/versions/${ANSWER_VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "compile failed: ${RESP}"; exit 1; }
api_post "/workflows/${ANSWER}/versions/${ANSWER_VERSION}/set-current" '{}' >/dev/null
# What anyone may send (a webhook, a provider's redirect) is not received by
# an engine of its own: intake receives it. This deployment has none, so the
# API has nowhere to hand it, and says so (e2e/test_intake.sh has one).
for anyones in "${API}/events/http-sync/${ANSWER}" "${API}/events/http/no-such-trigger/run"; do
    RESP=$(curl -sS --max-time 30 -w '\n%{http_code}' -X POST -H 'Content-Type: application/json' -d '{}' "${anyones}")
    [ "${RESP##*$'\n'}" = "503" ] && echo "${RESP}" | grep -q "NO_INTAKE" \
        || { print_error "a webhook was not refused for want of intake: ${RESP}"; exit 1; }
done
RESP=$(curl -sS --max-time 30 -w '\n%{http_code}' "http://127.0.0.1:${API_PORT}/api/oauth/${TENANT}/callback?state=s&code=c")
[ "${RESP##*$'\n'}" = "503" ] && echo "${RESP}" | grep -q "NO_INTAKE" \
    || { print_error "a provider's redirect was not refused for want of intake: ${RESP}"; exit 1; }
# And the engine takes neither from the API as it arrived, were it sent one.
for part in "events/api/runtime/events/http/no-such-trigger/run" "o-auth-callbacks/api/oauth/${TENANT}/callback?state=s&code=c"; do
    RESP=$(curl -sS --max-time 30 -w '\n%{http_code}' -X POST \
        --cacert "${WORK_DIR}/pki-api/ca.pem" --cert "${WORK_DIR}/pki-api/tls.pem" --key "${WORK_DIR}/pki-api/tls.key" \
        --resolve "runtara-server:${ENGINE_PORT}:127.0.0.1" \
        "https://runtara-server:${ENGINE_PORT}/engine/v1/served/${part}")
    [ "${RESP##*$'\n'}" = "403" ] && echo "${RESP}" | grep -q "NOT_FOR_THE_ENGINE" \
        || { print_error "the engine took what anyone may send from the API's certificate: ${RESP}"; exit 1; }
done
# A conversation is the engine's to keep and carry, and is asked for through
# the API: a message starts a run, and what became of it is read back.
CONVERSATION="${API}/workflows/${ANSWER}/conversations/two-roles"
RESP=$(curl -sS --max-time 60 -X POST -H 'Content-Type: application/json' \
    -d '{"id":"m-1","text":"hello"}' "${CONVERSATION}/messages")
[ "$(echo "${RESP}" | jq -r '.data.state // empty')" = "started" ] \
    || { print_error "a message did not start a conversation's run through the API: ${RESP}"; exit 1; }
for _ in $(seq 1 120); do
    RESP=$(curl -sS --max-time 30 "${CONVERSATION}/events")
    [ "$(echo "${RESP}" | jq -r '[.data[]? | select(.kind == "run_ended")] | length')" = "1" ] && break
    sleep 0.5
done
echo "${RESP}" | jq -e '[.data[] | select(.kind == "run_ended")][0].body.status == "completed"' >/dev/null \
    || { print_error "a conversation's run was not carried to its end: ${RESP}"; exit 1; }
[ "$(curl -sS "${CONVERSATION}" | jq -r '.data.key')" = "two-roles" ] \
    || { print_error "a conversation is not read through the API"; exit 1; }
# Saving a workflow validates it, on an executor too: the API asks the
# engine, which places the validator. What the validator finds is what the
# save is refused for.
BROKEN_GRAPH=$(echo "${ANSWER_GRAPH}" | jq '.steps.call = {"stepType": "Agent", "id": "call",
    "agentId": "an-agent-that-is-not-there", "capabilityId": "run", "inputMapping": {}}
    | .entryPoint = "call" | .executionPlan = [{"fromStep": "call", "toStep": "finish"}]')
RESP=$(api_post "/workflows/${ANSWER}/update" "{\"executionGraph\": ${BROKEN_GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "false" ] \
    && echo "${RESP}" | grep -q "an-agent-that-is-not-there" \
    || { print_error "a workflow that does not validate was saved, or refused for something else: ${RESP}"; exit 1; }
RESP=$(curl -sS --max-time 60 -X POST -H 'Content-Type: application/json' -d "${BROKEN_GRAPH}" \
    "${API}/workflows/graph/validate")
echo "${RESP}" | grep -q "an-agent-that-is-not-there" \
    || { print_error "the validate route did not say what the validator found: ${RESP}"; exit 1; }

# A workflow published as an agent is compiled on an executor too, and a
# workflow that calls it is compiled with it: the compiler asks the engine
# for the tenant's published agent as it asks for any component.
CHILD_GRAPH='{
  "name": "Shout Echo Child", "durable": false,
  "steps": { "finish": { "stepType": "Finish", "id": "finish", "inputMapping": {
      "echoed": { "valueType": "reference", "value": "data.text" },
      "marker": { "valueType": "immediate", "value": "from-child" } } } },
  "entryPoint": "finish", "executionPlan": [], "variables": {},
  "inputSchema": { "text": { "type": "string", "required": true } }, "outputSchema": {}
}'
RESP=$(api_post /workflows/create '{"name":"Shout Echo Child","description":"published as an agent","slug":"shout-echo"}')
CHILD=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${CHILD}" ] || { print_error "create failed: ${RESP}"; exit 1; }
RESP=$(api_post "/workflows/${CHILD}/update" "{\"executionGraph\": ${CHILD_GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "update failed: ${RESP}"; exit 1; }
RESP=$(curl -sS --max-time 900 -X POST -H 'Content-Type: application/json' "${API}/workflows/${CHILD}/publish-agent")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] && [ "$(echo "${RESP}" | jq -r '.data.agentId')" = "shout-echo" ] \
    || { print_error "a workflow was not published as an agent through an executor: ${RESP}"; exit 1; }
PARENT_GRAPH='{
  "name": "Parent Of Shout Echo",
  "steps": {
    "call": { "stepType": "Agent", "id": "call", "agentId": "shout-echo", "capabilityId": "run",
              "inputMapping": { "text": { "valueType": "reference", "value": "data.msg" } } },
    "finish": { "stepType": "Finish", "id": "finish", "inputMapping": {
        "childEcho": { "valueType": "reference", "value": "steps.call.outputs.echoed" },
        "childMarker": { "valueType": "reference", "value": "steps.call.outputs.marker" } } }
  },
  "entryPoint": "call", "executionPlan": [{ "fromStep": "call", "toStep": "finish" }],
  "variables": {}, "inputSchema": { "msg": { "type": "string", "required": true } }, "outputSchema": {}
}'
RESP=$(api_post /workflows/create '{"name":"Parent Of Shout Echo","description":"calls a published agent"}')
PARENT=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${PARENT}" ] || { print_error "create failed: ${RESP}"; exit 1; }
RESP=$(api_post "/workflows/${PARENT}/update" "{\"executionGraph\": ${PARENT_GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "update failed: ${RESP}"; exit 1; }
PARENT_VERSION=$(curl -sS "${API}/workflows/${PARENT}/versions" \
    | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(api_post "/workflows/${PARENT}/versions/${PARENT_VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] \
    || { print_error "a workflow that calls a published agent did not compile on an executor: ${RESP}"; exit 1; }
api_post "/workflows/${PARENT}/versions/${PARENT_VERSION}/set-current" '{}' >/dev/null
RESP=$(api_post "/workflows/${PARENT}/execute" '{"inputs": {"data": {"msg": "hello-live"}}}')
CALLER=$(echo "${RESP}" | jq -er '.data.instanceId') || { print_error "execute failed: ${RESP}"; exit 1; }
all_complete 120 "${CALLER}"
RESP=$(curl -sS "${API}/workflows/instances/${CALLER}")
echo "${RESP}" | jq -e '[.. | objects | select(.childEcho? == "hello-live" and .childMarker? == "from-child")] | length > 0' >/dev/null \
    || { print_error "a workflow did not run the agent published through an executor: ${RESP}"; exit 1; }
# A test of one capability runs an agent's code with input its caller wrote.
# With executors it is run on one, as a guest with no run behind it.
RESP=$(curl -sS --max-time 180 -X POST -H 'Content-Type: application/json' \
    -d '{"input": {"value": {"seven": 7}}}' "${API}/agents/utils/capabilities/return-input/test")
echo "${RESP}" | jq -e '.success == true and .output == {"seven": 7} and .engine == "executor"' >/dev/null \
    || { print_error "a capability was not tested on an executor: ${RESP}"; exit 1; }
RUNS_BEFORE=$(runs_known)
RESP=$(curl -sS --max-time 180 -X POST -H 'Content-Type: application/json' \
    -d '{"input": {"value": 1}}' "${API}/agents/utils/capabilities/return-input/test")
[ "$(runs_known)" = "${RUNS_BEFORE}" ] \
    || { print_error "a test of a capability left a run behind it"; exit 1; }
# The agents the API describes are the ones the engine runs.
[ "$(curl -sS "${API}/agents" | jq -r '(.agents // .data // .) | length')" -gt 0 ] \
    || { print_error "the engine's agents are not listed through the API"; exit 1; }
# One agent, one of its capabilities and what the engine loaded are asked of
# the engine too: the API loads no component.
AGENT=$(curl -sS "${API}/agents/utils")
CAPABILITY=$(echo "${AGENT}" | jq -r '.capabilities[0].id // empty')
[ "$(echo "${AGENT}" | jq -r '.id')" = "utils" ] && [ -n "${CAPABILITY}" ] \
    || { print_error "an agent is not read through the API: ${AGENT}"; exit 1; }
[ "$(curl -sS "${API}/agents/utils/capabilities/${CAPABILITY}" | jq -r '.id')" = "${CAPABILITY}" ] \
    || { print_error "a capability is not read through the API"; exit 1; }
# One the engine does not have is in no tenant's plan, which is asked first.
case "$(http_status "${API}/agents/no-such-agent")" in 403|404) ;; *)
    print_error "an agent the engine does not have was answered"; exit 1 ;;
esac
[ "$(curl -sS "${API}/_internal/components/status" | jq -r '.loaded')" = "true" ] \
    || { print_error "what the engine loaded is not read through the API"; exit 1; }
print_success "runs went to both executors ($(placed_by 0) and $(placed_by 1)), and one was run and waited for"

# --- 4 ------------------------------------------------------------------
print_step "4. A connection is made through the API, sealed by seal, and used..."
RESP=$(api_post /connections "{\"title\": \"two roles\", \"integrationId\": \"postgres\", \"connectionParameters\": {\"database_url\": \"${SERVER_DB_URL}?sslmode=disable\"}}")
CONN=$(echo "${RESP}" | jq -r '.connection_id // .connectionId // empty')
[ -n "${CONN}" ] || { print_error "connection create failed"; exit 1; }
[ "$(http_status "${API}/object-model/schemas?connectionId=${CONN}")" = "200" ] \
    || { print_error "the connection does not work through the API, the engine and seal"; exit 1; }
[ "$(find "${SEAL_DIR}/keys" -name '*.json' | wc -l | tr -d ' ')" -ge 1 ] \
    || { print_error "seal holds no key for the tenant"; exit 1; }
[ "$(psql_quiet -d "${TEST_DB_SERVER}" -c "SELECT count(*) FROM seal_wrapped_key")" = "0" ] \
    || { print_error "the server database holds a seal key"; exit 1; }
print_success "the secret was sealed by seal, which the API cannot reach, and used by the engine"

# --- 5 ------------------------------------------------------------------
print_step "5. A certificate is good for one channel..."
[ "$(ask_with_certificate "${WORK_DIR}/pki-api" "${ENGINE_PORT}")" = "200" ] \
    || { print_error "the engine's listener refuses the API's certificate"; exit 1; }
[ "$(ask_with_certificate "${WORK_DIR}/pki-executor" "${ENGINE_PORT}")" = "000" ] \
    || { print_error "the engine's listener accepts an executor's certificate"; exit 1; }
[ "$(ask_with_certificate "${WORK_DIR}/pki-api" "${RUN_CHANNEL_PORT}")" = "000" ] \
    || { print_error "the run channel accepts the API's certificate"; exit 1; }
[ "$(ask_with_certificate "${WORK_DIR}/pki-executor" "${RUN_CHANNEL_PORT}")" != "000" ] \
    || { print_error "the run channel refuses an executor's certificate"; exit 1; }
print_success "the engine listens to the API's certificate only, and the run channel to an executor's only"

# --- 6 ------------------------------------------------------------------
print_step "6. The API is killed; the runs it started complete..."
STARTED=()
for _ in 1 2 3 4 5 6; do STARTED+=("$(execute)"); done
kill -9 "${API_PID}"; wait "${API_PID}" 2>/dev/null || true
API_PID=$(start_api "${API_PORT}" "${API_LOG}")
all_complete 120 "${STARTED[@]}"
print_success "six runs started before the API was killed completed without it"

# --- 7 ------------------------------------------------------------------
print_step "7. The engine is away; the API says so and starts nothing..."
KNOWN=$(runs_known)
[ -n "${KNOWN}" ] || { print_error "the runs known could not be counted"; exit 1; }
READY=$(count_in "${ENGINE_LOG}" "engine is ready")
stop "${ENGINE_PID}"; ENGINE_PID=""
healthy "${API_PORT}" || { print_error "the API stopped answering with its engine away"; exit 1; }
# The API's own records are still read.
[ "$(http_status "${API}/workflows/${WORKFLOW}")" = "200" ] \
    || { print_error "the API does not read a workflow with its engine away"; exit 1; }
BODY="${WORK_DIR}/away.json"
CODE=$(curl -sS -o "${BODY}" -w '%{http_code}' --max-time 30 -X POST -H 'Content-Type: application/json' \
    -d '{"inputs": {"data": {"amount": 150}}}' "${API}/workflows/${WORKFLOW}/execute")
[ "${CODE}" = "503" ] || { print_error "a run asked of an absent engine answered ${CODE}: $(cat "${BODY}")"; exit 1; }
grep -q "the engine cannot be reached" "${BODY}" \
    || { print_error "the error does not say the engine is away: $(cat "${BODY}")"; exit 1; }
CODE=$(curl -sS -o "${BODY}" -w '%{http_code}' --max-time 30 "${API}/connections")
[ "${CODE}" = "503" ] && grep -q "ENGINE_UNAVAILABLE" "${BODY}" \
    || { print_error "a request the engine serves answered ${CODE}: $(cat "${BODY}")"; exit 1; }
start_engine "${READY}"
wait_for_log "${ENGINE_LOG}" "an executor registered" 60 3
# The same API process, not started again.
AFTER=$(execute); all_complete 180 "${AFTER}"
[ "$(runs_known)" = "$((KNOWN + 1))" ] \
    || { print_error "$(runs_known) runs are known, not $((KNOWN + 1)): one was started while the engine was away"; exit 1; }
print_success "with the engine away the API answered 503 and started no run; with it back, the same API ran one"

# --- 8 ------------------------------------------------------------------
print_step "8. A second API in front of the same engine..."
SECOND_API_PID=$(start_api "${SECOND_API_PORT}" "${SECOND_API_LOG}")
RESP=$(curl -sS --max-time 60 -X POST -H "Content-Type: application/json" \
    -d '{"inputs": {"data": {"amount": 150}}}' \
    "http://127.0.0.1:${SECOND_API_PORT}/api/runtime/workflows/${WORKFLOW}/execute")
SECOND=$(echo "${RESP}" | jq -er '.data.instanceId') || { print_error "the second API did not start a run: ${RESP}"; exit 1; }
all_complete 120 "${SECOND}"
print_success "a run started through a second API completed, and is read through the first"

echo
print_success "the API and the engine ran as two processes, and the API held nothing of the engine's"
