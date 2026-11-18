#!/bin/bash
# E2E Test: what anyone may send is received by a process of its own
# (docs/splitting-the-engine.md, Phase 7; docs/roles.md).
#
# The server runs as three processes (RUNTARA_ROLE): an engine with a seal
# of its own, intake, and an API in front of both. Intake is a database
# role and a Valkey user of its own (scripts/roles), and a caller of seal
# that may only verify.
#
#   1. WHAT INTAKE REFUSES   the runtime database, the key that opens
#                            connections, a seal in itself.
#   2. A WEBHOOK             a trigger's webhook, sent to the API, is handed
#                            to intake, queued, and run by the engine.
#   3. THE SYNCHRONOUS TRIGGER  intake asks the engine to run and wait.
#   4. A SIGNED WEBHOOK      a trigger bound to a Mailgun connection is
#                            checked by intake from the trigger's row, with
#                            seal: a good signature is queued, a bad one is
#                            refused.
#   5. WHAT INTAKE HOLDS     no runtime database, no connection, no workflow
#                            definition; its seal caller may not send; its
#                            Valkey user cannot write a trigger stream.
#   6. EACH CERTIFICATE      intake's asks the engine only to run and wait,
#                            and the API's does not ask that.
#   7. WITHOUT THE ENGINE    a webhook is still accepted, and runs when the
#                            engine is back; the synchronous trigger says
#                            the engine is away.
#   8. WITHOUT INTAKE        webhooks answer 503; the rest of the API works.
#
# Prerequisites: Postgres (POSTGRES_PASSWORD or PGPASSWORD in the
# environment; `psql` on PATH or the container PG_CONTAINER), docker (an
# isolated Valkey), python3, openssl, seal (SEAL_BIN, or built at the
# revision the server uses), and the workflow components in
# target/wasm32-wasip2/release (scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
# shellcheck source=../scripts/credential-key.sh
source "${PROJECT_ROOT}/scripts/credential-key.sh"
ENGINE_KEY="$(generate_credential_key)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-${PGPASSWORD:-}}"

TEST_DB_SERVER="intake_e2e_server_$$"
TEST_DB_RUNTIME="intake_e2e_runtime_$$"
ENGINE_ROLE="${TEST_DB_SERVER}_engine"
INTAKE_ROLE="${TEST_DB_SERVER}_intake"
API_ROLE="${TEST_DB_SERVER}_api"
ROLE_PASSWORD="$(openssl rand -hex 16)"
INTAKE_VALKEY_PASSWORD="$(openssl rand -hex 16)"
API_PORT="${TEST_PORT_PUBLIC:-17890}"
ENGINE_PORT="${TEST_ENGINE_PORT:-17895}"
ENGINE_PUBLIC_PORT="${TEST_ENGINE_PUBLIC_PORT:-17896}"
ENGINE_HEALTH_PORT="${TEST_ENGINE_HEALTH_PORT:-17897}"
INTAKE_PORT="${TEST_INTAKE_PORT:-17898}"
INTAKE_HEALTH_PORT="${TEST_INTAKE_HEALTH_PORT:-17899}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18891}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18892}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18893}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18894}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16397}"
TENANT="intake_e2e"

TARGET_DIR="${CARGO_TARGET_DIR:-${PROJECT_ROOT}/target}"
RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${TARGET_DIR}/debug/runtara-server}"
EXECUTOR_BIN="${TARGET_DIR}/debug/runtara-executor"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
COMPILER_WASM="${RUNTARA_COMPILER_WASM:-${COMPONENTS_DIR}/runtara-compile-sandboxed.wasm}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

WORK_DIR="$(mktemp -d -t runtara_intake_e2e_XXXXXX)"
# A socket's path is short.
SEAL_DIR="$(mktemp -d /tmp/rseal.XXXXXX)"
ENGINE_LOG="${WORK_DIR}/engine.log"
INTAKE_LOG="${WORK_DIR}/intake.log"
API_LOG="${WORK_DIR}/api.log"
ENGINE_PID=""; INTAKE_PID=""; API_PID=""; SEAL_PID=""; INTAKE_SEAL_PID=""
VALKEY_CONTAINER=""
MADE_RUNTARA_SYSTEM=""
API="http://127.0.0.1:${API_PORT}/api/runtime"

db_url() { echo "postgresql://$1:${ROLE_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/$2"; }

PG_CONTAINER="${PG_CONTAINER:-runtara-dev-postgres}"
if command -v psql >/dev/null 2>&1; then
    psql_as() {  # psql_as USER PASSWORD args...
        local user="$1" password="$2"; shift 2
        PGPASSWORD="${password}" psql -U "${user}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
    }
else
    psql_as() {
        local user="$1" password="$2"; shift 2
        docker exec -e PGPASSWORD="${password}" -i "${PG_CONTAINER}" psql -U "${user}" -tA "$@"
    }
fi
psql_quiet() { psql_as "${POSTGRES_USER}" "${POSTGRES_PASSWORD}" "$@"; }

stop() {
    [ -n "${1:-}" ] || return 0
    kill "$1" 2>/dev/null || true
    wait "$1" 2>/dev/null || true
}

cleanup() {
    local code=$?
    for pid in "${API_PID}" "${INTAKE_PID}" "${ENGINE_PID}" "${SEAL_PID}" "${INTAKE_SEAL_PID}"; do
        [ -z "${pid}" ] || kill "${pid}" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    [ -z "${VALKEY_CONTAINER}" ] || docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER} WITH (FORCE)" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME} WITH (FORCE)" >/dev/null 2>&1 || true
    for role in "${ENGINE_ROLE}" "${INTAKE_ROLE}" "${API_ROLE}"; do
        psql_quiet -d postgres -c "DROP ROLE IF EXISTS ${role}" >/dev/null 2>&1 || true
    done
    [ -z "${MADE_RUNTARA_SYSTEM}" ] \
        || psql_quiet -d postgres -c "DROP ROLE IF EXISTS runtara_system" >/dev/null 2>&1 || true
    if [ ${code} -ne 0 ]; then
        for log in "${API_LOG}" "${INTAKE_LOG}" "${ENGINE_LOG}" "${WORK_DIR}/seal.log" "${WORK_DIR}/seal-intake.log"; do
            [ -f "${log}" ] && { echo "--- ${log##*/} (tail)" >&2; tail -25 "${log}" | cut -c1-400 >&2; }
        done
    fi
    rm -rf "${WORK_DIR}" "${SEAL_DIR}"
    exit ${code}
}
trap cleanup EXIT

api_post() {
    curl -sS --max-time "${3:-120}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}
http_status() { curl -sS -o /dev/null --max-time 60 -w '%{http_code}' "$@"; }
healthy() { [ "$(http_status "http://127.0.0.1:$1/health" 2>/dev/null || true)" = "200" ]; }
valkey() { docker exec "${VALKEY_CONTAINER}" valkey-cli --raw "$@"; }
runtime_sql() { psql_quiet -d "${TEST_DB_RUNTIME}" -c "$1"; }
server_sql() { psql_quiet -d "${TEST_DB_SERVER}" -c "$1"; }

# A seal: `seal CONFIG-NAME SOCKET CALLER-NAME ADMIN`. Both share the keys.
start_a_seal() {
    local name="$1" socket="$2" caller="$3" admin="$4"
    cat > "${SEAL_DIR}/${name}.toml" <<TOML
[listen]
unix = "${socket}"

[[callers]]
name = "${caller}"
unix_uid = $(id -u)
keysets = ["*"]
admin = ${admin}
destroy_keysets = ${admin}

[keys]
source = { dir = "${SEAL_DIR}/keys" }
wrap = { file = "${SEAL_DIR}/kek" }
lock_memory = "off"

[network]
allow_private_cidrs = ["127.0.0.0/8", "::1/128"]
honour_private_rules = true

[audit]
sink = { file = "${SEAL_DIR}/${name}-audit.jsonl" }
TOML
    chmod 600 "${SEAL_DIR}/${name}.toml"
    "${SEAL_BIN}" serve --config "${SEAL_DIR}/${name}.toml" >>"${WORK_DIR}/${name}.log" 2>&1 &
    local pid=$!
    for _ in {1..40}; do
        kill -0 "${pid}" 2>/dev/null || { print_error "${name} exited as it started"; exit 1; }
        [ -S "${socket}" ] && { echo "${pid}"; return 0; }
        sleep 0.25
    done
    print_error "${name} did not listen"; exit 1
}

start_engine() {
    (exec env \
        RUNTARA_ROLE=engine \
        RUNTARA_ENGINE_LISTEN="127.0.0.1:${ENGINE_PORT}" \
        RUNTARA_ENGINE_TLS_DIR="${WORK_DIR}/pki-engine" \
        RUNTARA_ENGINE_HEALTH_LISTEN="127.0.0.1:${ENGINE_HEALTH_PORT}" \
        RUNTARA_COMPILER_WASM="${COMPILER_WASM}" \
        RUNTARA_SERVER_DATABASE_URL="$(db_url "${ENGINE_ROLE}" "${TEST_DB_SERVER}")" \
        RUNTARA_DATABASE_URL="$(db_url "${ENGINE_ROLE}" "${TEST_DB_RUNTIME}")" \
        RUNTARA_REQUIRE_ROW_SECURITY=true \
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
        RUNTARA_DATABASE_CONNECTION_GUARD=off \
        RUST_LOG="warn,runtara_server=info" \
        AUTH_PROVIDER=local \
        VALKEY_HOST=127.0.0.1 \
        VALKEY_PORT="${TEST_VALKEY_PORT}" \
        OTEL_SDK_DISABLED=true \
        SQLX_OFFLINE="${SQLX_OFFLINE}" \
        "${RUNTARA_SERVER_BIN}") >>"${ENGINE_LOG}" 2>&1 &
    ENGINE_PID=$!
    for _ in {1..120}; do
        healthy "${ENGINE_HEALTH_PORT}" && return 0
        kill -0 "${ENGINE_PID}" 2>/dev/null || { print_error "the engine exited during boot"; exit 1; }
        sleep 1
    done
    print_error "the engine did not become ready"; exit 1
}

# The whole of intake's environment: nothing is inherited.
INTAKE_ENV=(
    "PATH=${PATH}"
    "HOME=${HOME}"
    RUNTARA_ROLE=intake
    "RUNTARA_INTAKE_LISTEN=127.0.0.1:${INTAKE_PORT}"
    "RUNTARA_INTAKE_HEALTH_LISTEN=127.0.0.1:${INTAKE_HEALTH_PORT}"
    "RUNTARA_INTAKE_TLS_DIR=${WORK_DIR}/pki-intake"
    "RUNTARA_ENGINE_URL=https://127.0.0.1:${ENGINE_PORT}"
    "RUNTARA_SERVER_DATABASE_URL=$(db_url "${INTAKE_ROLE}" "${TEST_DB_SERVER}")"
    RUNTARA_REQUIRE_ROW_SECURITY=true
    RUNTARA_SEAL_MODE=unix
    "RUNTARA_SEAL_SOCKET=${SEAL_DIR}/seal-intake.sock"
    "TENANT_ID=${TENANT}"
    RUNTARA_DEV_MODE=false
    "RUST_LOG=warn,runtara_server=info"
    AUTH_PROVIDER=local
    VALKEY_HOST=127.0.0.1
    "VALKEY_PORT=${TEST_VALKEY_PORT}"
    VALKEY_USER=runtara-intake
    "VALKEY_PASSWORD=${INTAKE_VALKEY_PASSWORD}"
    OTEL_SDK_DISABLED=true
    SKIP_MIGRATIONS=true
)

start_intake() {
    (cd "${WORK_DIR}" && exec env -i "${INTAKE_ENV[@]}" "${RUNTARA_SERVER_BIN}") >>"${INTAKE_LOG}" 2>&1 &
    INTAKE_PID=$!
    for _ in {1..60}; do
        healthy "${INTAKE_HEALTH_PORT}" && return 0
        kill -0 "${INTAKE_PID}" 2>/dev/null || { print_error "intake exited during boot"; exit 1; }
        sleep 1
    done
    print_error "intake did not become ready"; exit 1
}

start_api() {
    (cd "${WORK_DIR}" && exec env -i "PATH=${PATH}" "HOME=${HOME}" \
        RUNTARA_ROLE=api \
        "RUNTARA_ENGINE_URL=https://127.0.0.1:${ENGINE_PORT}" \
        "RUNTARA_INTAKE_URL=https://127.0.0.1:${INTAKE_PORT}" \
        "RUNTARA_ENGINE_TLS_DIR=${WORK_DIR}/pki-api" \
        "RUNTARA_SERVER_DATABASE_URL=$(db_url "${API_ROLE}" "${TEST_DB_SERVER}")" \
        RUNTARA_REQUIRE_ROW_SECURITY=true \
        "RUNTARA_AGENT_COMPONENTS_DIR=${WORK_DIR}/agent-descriptions" \
        "TENANT_ID=${TENANT}" \
        SERVER_HOST=127.0.0.1 "SERVER_PORT=${API_PORT}" \
        RUNTARA_DEV_MODE=false \
        "RUST_LOG=warn,runtara_server=info" \
        AUTH_PROVIDER=local \
        VALKEY_HOST=127.0.0.1 "VALKEY_PORT=${TEST_VALKEY_PORT}" \
        OTEL_SDK_DISABLED=true \
        SKIP_MIGRATIONS=true \
        "${RUNTARA_SERVER_BIN}") >>"${API_LOG}" 2>&1 &
    API_PID=$!
    for _ in {1..60}; do
        healthy "${API_PORT}" && return 0
        kill -0 "${API_PID}" 2>/dev/null || { print_error "the API exited during boot"; exit 1; }
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

completed_runs() {
    runtime_sql "SELECT count(*) FROM instances WHERE tenant_id = '${TENANT}' AND status = 'completed'"
}
# Waits until more than $1 runs have completed.
wait_for_a_run() {
    for _ in {1..120}; do
        [ "$(completed_runs)" -gt "$1" ] && return 0
        sleep 0.5
    done
    return 1
}

echo "==============================================================="
echo "E2E Test: what anyone may send is received by a process of its own"
echo "==============================================================="

[ -n "${POSTGRES_PASSWORD}" ] \
    || { print_error "set POSTGRES_PASSWORD (or PGPASSWORD) for ${POSTGRES_USER}@${POSTGRES_HOST}"; exit 1; }
if [ ! -x "${RUNTARA_SERVER_BIN}" ] || [ ! -x "${EXECUTOR_BIN}" ]; then
    print_step "Building runtara-server and runtara-executor (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server -p runtara-executor >&2
fi
[ -f "${COMPONENTS_DIR}/runtara_workflow_stdlib.wasm" ] \
    || { print_error "Missing ${COMPONENTS_DIR}/runtara_workflow_stdlib.wasm — run scripts/build-agent-components.sh"; exit 1; }
if [ ! -f "${COMPILER_WASM}" ]; then
    print_step "Building the compiler's WASI command..."
    cargo build -p runtara-compile-guest --bin runtara-compile-sandboxed --release --target wasm32-wasip2 >&2
fi

print_step "Starting an isolated Valkey, the databases and their roles, and seal..."
docker info >/dev/null 2>&1 || { print_error "docker is required"; exit 1; }
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
[ "$(psql_quiet -d postgres -c "SELECT count(*) FROM pg_roles WHERE rolname = 'runtara_system'")" = "1" ] \
    || MADE_RUNTARA_SYSTEM=yes
for role in "${ENGINE_ROLE}" "${INTAKE_ROLE}" "${API_ROLE}"; do
    psql_quiet -d postgres -c "CREATE ROLE ${role} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD '${ROLE_PASSWORD}'" >/dev/null
done
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER} OWNER ${ENGINE_ROLE}" >/dev/null
for extension in uuid-ossp vector fuzzystrmatch pg_trgm; do
    psql_quiet -d "${TEST_DB_SERVER}" -c "CREATE EXTENSION IF NOT EXISTS \"${extension}\"" >/dev/null
done
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME} OWNER ${ENGINE_ROLE}" >/dev/null

"${EXECUTOR_BIN}" init-pki --server-dir "${WORK_DIR}/pki-engine" \
    --executor-dir "${WORK_DIR}/pki-executor" --api-dir "${WORK_DIR}/pki-api" \
    --intake-dir "${WORK_DIR}/pki-intake" >/dev/null
mkdir "${WORK_DIR}/agent-descriptions"
cp "${COMPONENTS_DIR}"/runtara_agent_*.meta.json "${WORK_DIR}/agent-descriptions/"

SEAL_REV=$(sed -n 's/^seal-client = .*rev = "\([0-9a-f]*\)".*/\1/p' "${PROJECT_ROOT}/crates/runtara-connections/Cargo.toml")
if [ -z "${SEAL_BIN:-}" ]; then
    SEAL_BIN="${PROJECT_ROOT}/target/seal/${SEAL_REV}/bin/seal"
    [ -x "${SEAL_BIN}" ] || cargo install --locked --git https://github.com/runtarahq/seal \
        --rev "${SEAL_REV}" --root "${PROJECT_ROOT}/target/seal/${SEAL_REV}" seal >&2
fi
mkdir "${SEAL_DIR}/keys"
(umask 077 && head -c 32 /dev/urandom > "${SEAL_DIR}/kek")
# The engine's seal administers tenants' keys. Intake's is the same keys
# behind a caller that may only use them, and here is asked only to verify.
SEAL_PID=$(start_a_seal seal "${SEAL_DIR}/seal.sock" runtara-server true)
INTAKE_SEAL_PID=$(start_a_seal seal-intake "${SEAL_DIR}/seal-intake.sock" runtara-intake false)

#-------------------------------------------------------------------------
print_step "1. Intake refuses what a run is made of..."
SAID=$(refused_start env -i "${INTAKE_ENV[@]}" "RUNTARA_DATABASE_URL=$(db_url "${ENGINE_ROLE}" "${TEST_DB_RUNTIME}")" "${RUNTARA_SERVER_BIN}")
echo "${SAID}" | grep -q "RUNTARA_ROLE=intake was given RUNTARA_DATABASE_URL" \
    || { print_error "an intake given the runtime database did not refuse it: ${SAID}"; exit 1; }
SAID=$(refused_start env -i "${INTAKE_ENV[@]}" "RUNTARA_CONNECTIONS_ENCRYPTION_KEY=${ENGINE_KEY}" "${RUNTARA_SERVER_BIN}")
echo "${SAID}" | grep -q "was given RUNTARA_CONNECTIONS_ENCRYPTION_KEY" \
    || { print_error "an intake given the key that opens connections did not refuse it: ${SAID}"; exit 1; }
SAID=$(refused_start env -i "${INTAKE_ENV[@]}" RUNTARA_SEAL_MODE=in_process "${RUNTARA_SERVER_BIN}")
echo "${SAID}" | grep -q "RUNTARA_ROLE=intake needs a seal of its own" \
    || { print_error "an intake with seal in itself did not refuse to start: ${SAID}"; exit 1; }
print_success "intake refuses the runtime database, the key that opens connections, and a seal in itself"

print_step "Starting the engine, intake and the API..."
start_engine
psql_quiet -d "${TEST_DB_SERVER}" -v api_role="${API_ROLE}" -v engine_role="${ENGINE_ROLE}" \
    < "${PROJECT_ROOT}/scripts/roles/api-database-role.sql" >/dev/null
psql_quiet -d "${TEST_DB_SERVER}" -v intake_role="${INTAKE_ROLE}" \
    < "${PROJECT_ROOT}/scripts/roles/intake-database-role.sql" >/dev/null
# shellcheck disable=SC2046
docker exec "${VALKEY_CONTAINER}" valkey-cli ACL SETUSER runtara-intake on ">${INTAKE_VALKEY_PASSWORD}" \
    $(grep -v '^#' "${PROJECT_ROOT}/scripts/roles/intake-valkey-user.acl") >/dev/null
start_intake
start_api
grep -q "Webhooks are handed to intake" "${API_LOG}" \
    || { print_error "the API does not say it hands webhooks to intake"; exit 1; }

GRAPH='{"name":"Answer","steps":{"finish":{"stepType":"Finish","id":"finish","inputMapping":{"answer":{"valueType":"immediate","value":42}}}},"entryPoint":"finish","executionPlan":[],"variables":{},"inputSchema":{},"outputSchema":{}}'
RESP=$(api_post /workflows/create '{"name":"Answer","description":"intake e2e"}')
WF=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${WF}" ] || { print_error "create failed: ${RESP}"; exit 1; }
RESP=$(api_post "/workflows/${WF}/update" "{\"executionGraph\": ${GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "update failed: ${RESP}"; exit 1; }
VERSION=$(curl -sS "${API}/workflows/${WF}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(api_post "/workflows/${WF}/versions/${VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "compile failed: ${RESP}"; exit 1; }
RESP=$(api_post /triggers "{\"workflow_id\": \"${WF}\", \"trigger_type\": \"HTTP\", \"active\": true}")
TRIGGER=$(echo "${RESP}" | jq -r '.data.id // .id // empty' 2>/dev/null || true)
[ -n "${TRIGGER}" ] || { print_error "trigger create failed: ${RESP}"; exit 1; }
WEBHOOK="${API}/events/http/${TRIGGER}/run"
SYNC="${API}/events/http-sync/${WF}"

#-------------------------------------------------------------------------
print_step "2. A webhook is received by intake, queued, and run by the engine..."
BEFORE=$(completed_runs)
RESP=$(curl -sS --max-time 60 -X POST -H 'Content-Type: application/json' -d '{"n": 1}' "${WEBHOOK}")
[ "$(echo "${RESP}" | jq -r '.status // empty')" = "queued" ] \
    || { print_error "the webhook was not queued: ${RESP}"; exit 1; }
wait_for_a_run "${BEFORE}" || { print_error "the webhook's run did not complete"; exit 1; }
# It was intake that queued it: the row is the work of intake's database role
# only if intake wrote it, and the engine's log has no request for it.
[ "$(curl -sS -o /dev/null -w '%{http_code}' "http://127.0.0.1:${INTAKE_HEALTH_PORT}/api/runtime/events/http/${TRIGGER}/run")" = "404" ] \
    || { print_error "intake's health listener answers a webhook"; exit 1; }
print_success "a webhook sent to the API was queued by intake and its run completed"

#-------------------------------------------------------------------------
print_step "3. The synchronous trigger: intake asks the engine to run and wait..."
RESP=$(curl -sS --max-time 120 -X POST -H 'Content-Type: application/json' -d '{}' "${SYNC}")
echo "${RESP}" | jq -e '.success == true and .outputs.answer == 42' >/dev/null \
    || { print_error "the synchronous trigger did not run the workflow: $(echo "${RESP}" | head -c 400)"; exit 1; }
[ "$(http_status -X POST -H 'Content-Type: application/json' -d '{}' "${API}/events/http-sync/00000000-0000-0000-0000-000000000000")" = "404" ] \
    || { print_error "a workflow that is not there was not 404"; exit 1; }
print_success "the synchronous trigger ran the workflow through intake and answered with its output"

#-------------------------------------------------------------------------
print_step "4. A signed webhook is checked by intake from the trigger's row..."
SIGNING_KEY="key-$(openssl rand -hex 16)"
RESP=$(api_post /connections "{\"title\":\"Mail\",\"integrationId\":\"mailgun\",\"connectionParameters\":{\"api_key\":\"key-$(openssl rand -hex 8)\",\"domain\":\"mg.example.com\",\"region\":\"us\",\"webhook_signing_key\":\"${SIGNING_KEY}\"}}")
CONN=$(echo "${RESP}" | jq -r '.connection_id // .connectionId // empty')
[ -n "${CONN}" ] || { print_error "the Mailgun connection was not made: ${RESP}"; exit 1; }
RESP=$(api_post /triggers "{\"workflow_id\": \"${WF}\", \"trigger_type\": \"HTTP\", \"active\": true, \"configuration\": {\"connection_id\": \"${CONN}\"}}")
SIGNED_TRIGGER=$(echo "${RESP}" | jq -r '.data.id // .id // empty' 2>/dev/null || true)
[ -n "${SIGNED_TRIGGER}" ] || { print_error "the signed trigger was not made: ${RESP}"; exit 1; }
KEPT=$(server_sql "SELECT webhook_verification->>'integration' FROM invocation_trigger WHERE id = '${SIGNED_TRIGGER}'")
[ "${KEPT}" = "mailgun" ] || { print_error "what checks the trigger's webhook was not kept with it ('${KEPT}')"; exit 1; }
# What is kept is sealed: the signing key itself is nowhere in the row.
server_sql "SELECT webhook_verification::text FROM invocation_trigger WHERE id = '${SIGNED_TRIGGER}'" | grep -q "${SIGNING_KEY}" \
    && { print_error "the signing key is kept readable with the trigger"; exit 1; }
signed_body() {  # signed_body KEY: a Mailgun body signed with KEY
    local timestamp token signature
    timestamp=$(date +%s); token=$(openssl rand -hex 16)
    signature=$(printf '%s%s' "${timestamp}" "${token}" | openssl dgst -sha256 -hmac "$1" | sed 's/^.*= //')
    printf '{"signature":{"timestamp":"%s","token":"%s","signature":"%s"},"event-data":{"event":"delivered"}}' \
        "${timestamp}" "${token}" "${signature}"
}
SIGNED_WEBHOOK="${API}/events/http/${SIGNED_TRIGGER}/run"
BEFORE=$(completed_runs)
RESP=$(curl -sS --max-time 60 -X POST -H 'Content-Type: application/json' -d "$(signed_body "${SIGNING_KEY}")" "${SIGNED_WEBHOOK}")
[ "$(echo "${RESP}" | jq -r '.status // empty')" = "queued" ] \
    || { print_error "a webhook with a good signature was not queued: ${RESP}"; exit 1; }
wait_for_a_run "${BEFORE}" || { print_error "the signed webhook's run did not complete"; exit 1; }
QUEUED=$(server_sql "SELECT count(*) FROM execution_requests")
CODE=$(http_status -X POST -H 'Content-Type: application/json' -d "$(signed_body "another-key")" "${SIGNED_WEBHOOK}")
[ "${CODE}" = "401" ] || { print_error "a webhook signed with another key answered ${CODE}"; exit 1; }
CODE=$(http_status -X POST -H 'Content-Type: application/json' -d '{"event-data":{}}' "${SIGNED_WEBHOOK}")
[ "${CODE}" = "401" ] || { print_error "a webhook with no signature answered ${CODE}"; exit 1; }
[ "$(server_sql "SELECT count(*) FROM execution_requests")" = "${QUEUED}" ] \
    || { print_error "a refused webhook queued a run"; exit 1; }
print_success "a webhook signed with the connection's key was queued; one signed with another, or unsigned, was refused and queued nothing"

#-------------------------------------------------------------------------
print_step "5. Intake holds nothing a run is made of..."
for name in "${INTAKE_ENV[@]}"; do
    case "${name%%=*}" in
        RUNTARA_DATABASE_URL|RUNTARA_CONNECTIONS_*|RUNTARA_SEAL_WRAPPING_KEY|RUNTARA_EXECUTOR_*|RUNTARA_ARTIFACT_*|RUNTARA_COMPILER_*)
            print_error "intake is given ${name%%=*}"; exit 1 ;;
    esac
done
as_intake_role() {  # as_intake_role SQL [database]: its output, or the error
    psql_as "${INTAKE_ROLE}" "${ROLE_PASSWORD}" -q -d "${2:-${TEST_DB_SERVER}}" -c "$1" 2>&1 || true
}
SCOPED="SELECT set_config('runtara.tenant_id', '${TENANT}', false);"
[ "$(as_intake_role "${SCOPED} SELECT count(*) > 0 FROM invocation_trigger" | tail -1)" = "t" ] \
    || { print_error "intake's role cannot read triggers"; exit 1; }
for table in connection_data_entity workflow_definitions workflow_compilations api_keys seal_wrapped_key; do
    OUT=$(as_intake_role "${SCOPED} SELECT count(*) FROM ${table}")
    echo "${OUT}" | grep -q "permission denied" || { print_error "intake's role read ${table}: ${OUT}"; exit 1; }
done
OUT=$(as_intake_role "${SCOPED} UPDATE invocation_trigger SET active = false")
echo "${OUT}" | grep -q "permission denied" || { print_error "intake's role changed a trigger: ${OUT}"; exit 1; }
OUT=$(as_intake_role "SELECT set_config('runtara.all_tenants', 'on', false); SELECT count(*) FROM invocation_trigger")
echo "${OUT}" | grep -q "may not work across tenants" || { print_error "intake's role worked across tenants: ${OUT}"; exit 1; }
OUT=$(as_intake_role "SELECT count(*) FROM instances" "${TEST_DB_RUNTIME}")
echo "${OUT}" | grep -q "permission denied" || { print_error "intake's role read the runtime database: ${OUT}"; exit 1; }
as_intake_valkey() {
    docker exec "${VALKEY_CONTAINER}" valkey-cli --user runtara-intake --pass "${INTAKE_VALKEY_PASSWORD}" --no-auth-warning "$@" 2>&1 || true
}
for command in "XADD runtara:triggers:${TENANT} * event x" "RPUSH ${TENANT}:compilation:queue x" \
               "SET ${TENANT}:member:mallory owner" "SET ${TENANT}:token:revoked:x 1" "GET ${TENANT}:member:someone"; do
    # shellcheck disable=SC2086
    OUT=$(as_intake_valkey ${command})
    echo "${OUT}" | grep -q "NOPERM" || { print_error "intake's Valkey user ran '${command}': ${OUT}"; exit 1; }
done
# Its seal knows it as a caller that administers nothing.
grep -q 'admin = false' "${SEAL_DIR}/seal-intake.toml" && grep -q 'destroy_keysets = false' "${SEAL_DIR}/seal-intake.toml" \
    || { print_error "intake's seal caller is an administrator"; exit 1; }
print_success "intake's environment, database role and Valkey user hold no run, connection, definition or key, and cannot work across tenants"

#-------------------------------------------------------------------------
print_step "6. Each certificate asks the engine only what is its to ask..."
ask_engine() {  # ask_engine PKI-DIR OPERATION BODY: the status the engine answers
    curl -sS -o "${WORK_DIR}/engine_body" -w '%{http_code}' --max-time 120 \
        --cacert "$1/ca.pem" --cert "$1/tls.pem" --key "$1/tls.key" \
        --resolve "runtara-server:${ENGINE_PORT}:127.0.0.1" \
        -X POST -H 'Content-Type: application/json' -d "$3" \
        "https://runtara-server:${ENGINE_PORT}/engine/v1/operations/$2"
}
SYNC_BODY="{\"run_label\": null, \"tenant_id\": \"${TENANT}\", \"workflow_id\": \"${WF}\", \"inputs\": {}}"
CODE=$(ask_engine "${WORK_DIR}/pki-intake" RunSyncForAnyone "${SYNC_BODY}")
[ "${CODE}" = "200" ] || { print_error "the engine refused intake a run for anyone (${CODE}): $(head -c 300 "${WORK_DIR}/engine_body")"; exit 1; }
for operation in "Stop {\"tenant_id\": \"${TENANT}\", \"instance_id\": \"x\"}" \
                 "ListExecutions {\"tenant_id\": \"${TENANT}\", \"workflow_id\": \"${WF}\", \"page\": 0, \"size\": 5}" \
                 "Facts {}"; do
    CODE=$(ask_engine "${WORK_DIR}/pki-intake" "${operation%% *}" "${operation#* }")
    [ "${CODE}" = "403" ] || { print_error "the engine answered ${operation%% *} to intake's certificate with ${CODE}"; exit 1; }
done
CODE=$(ask_engine "${WORK_DIR}/pki-api" RunSyncForAnyone "${SYNC_BODY}")
[ "${CODE}" = "403" ] || { print_error "the engine answered a run for anyone to the API's certificate with ${CODE}"; exit 1; }
CODE=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 30 \
    --cacert "${WORK_DIR}/pki-intake/ca.pem" --cert "${WORK_DIR}/pki-intake/tls.pem" --key "${WORK_DIR}/pki-intake/tls.key" \
    --resolve "runtara-server:${ENGINE_PORT}:127.0.0.1" \
    "https://runtara-server:${ENGINE_PORT}/engine/v1/served/connections/api/runtime/connections")
[ "${CODE}" = "403" ] || { print_error "the engine served a part of the API to intake's certificate (${CODE})"; exit 1; }
# Intake's listener takes the API's certificate and not the engine's or its own.
CODE=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 10 \
    --cacert "${WORK_DIR}/pki-engine/ca.pem" --cert "${WORK_DIR}/pki-engine/tls.pem" --key "${WORK_DIR}/pki-engine/tls.key" \
    --resolve "runtara-intake:${INTAKE_PORT}:127.0.0.1" \
    "https://runtara-intake:${INTAKE_PORT}/engine/v1/served/events/api/runtime/events/http/${TRIGGER}/run" 2>/dev/null || true)
case "${CODE}" in 2??) print_error "intake answered the engine's certificate (${CODE})"; exit 1 ;; esac
print_success "intake's certificate asks the engine to run and wait, and nothing else; the API's does not ask that"

#-------------------------------------------------------------------------
print_step "7. Without the engine a webhook is still accepted..."
stop "${ENGINE_PID}"; ENGINE_PID=""
BEFORE=$(completed_runs)
RESP=$(curl -sS --max-time 60 -X POST -H 'Content-Type: application/json' -d '{"n": 2}' "${WEBHOOK}")
[ "$(echo "${RESP}" | jq -r '.status // empty')" = "queued" ] \
    || { print_error "with the engine away the webhook was not queued: ${RESP}"; exit 1; }
CODE=$(http_status -X POST -H 'Content-Type: application/json' -d '{}' "${SYNC}")
[ "${CODE}" = "503" ] || { print_error "with the engine away the synchronous trigger answered ${CODE}, not 503"; exit 1; }
start_engine
wait_for_a_run "${BEFORE}" || { print_error "the webhook queued while the engine was away did not run"; exit 1; }
print_success "a webhook queued while the engine was away ran when it was back; the synchronous trigger said the engine was away"

#-------------------------------------------------------------------------
print_step "8. Without intake webhooks say so, and the rest of the API works..."
stop "${INTAKE_PID}"; INTAKE_PID=""
CODE=$(http_status -X POST -H 'Content-Type: application/json' -d '{"n": 3}' "${WEBHOOK}")
[ "${CODE}" = "503" ] || { print_error "with intake away a webhook answered ${CODE}, not 503"; exit 1; }
[ "$(http_status "${API}/workflows")" = "200" ] || { print_error "with intake away the API does not list workflows"; exit 1; }
RESP=$(api_post "/workflows/${WF}/execute" '{"inputs": {"data": {}}}')
[ -n "$(echo "${RESP}" | jq -r '.data.instanceId // empty')" ] \
    || { print_error "with intake away a caller could not run a workflow: ${RESP}"; exit 1; }
start_intake
RESP=$(curl -sS --max-time 60 -X POST -H 'Content-Type: application/json' -d '{"n": 4}' "${WEBHOOK}")
[ "$(echo "${RESP}" | jq -r '.status // empty')" = "queued" ] \
    || { print_error "with intake back the webhook was not queued: ${RESP}"; exit 1; }
print_success "with intake away webhooks answered 503 and callers' work went on; with it back, webhooks were queued"

echo
print_success "intake received what anyone may send, and held nothing a run is made of"
