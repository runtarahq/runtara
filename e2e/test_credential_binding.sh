#!/bin/bash
# E2E Test: connection credentials are bound to their connection and the
# key can be rotated (M5.4).
#
#   1. BOUND — a Postgres connection's parameters are stored as a version 2
#      envelope under key k1 and decrypt for it; the same ciphertext copied
#      onto another connection does not decrypt there.
#   2. ROTATE — with k2 current and k1 retired the connection still works;
#      `reencrypt-connections` moves it to k2, and it works with k1 gone.
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

TEST_DB_SERVER="cred_binding_e2e_server_$$"
TEST_DB_RUNTIME="cred_binding_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17923}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18921}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18922}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18923}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18924}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16523}"
TEST_DATA_DIR="$(mktemp -d -t runtara_cred_binding_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
# Test-only keys, made here and never printed.
KEY1="$(openssl rand -base64 32)"
KEY2="$(openssl rand -base64 32)"
KEY_ENV=("RUNTARA_CONNECTIONS_ENCRYPTION_KEY=${KEY1}" "RUNTARA_CONNECTIONS_ENCRYPTION_KEY_ID=k1")
TENANT="cred_binding_e2e"

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
    resp=$(api_post /workflows/create "{\"name\":\"${name}\",\"description\":\"artifact store e2e\"}")
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

#-------------------------------------------------------------------------
echo "==============================================================="
echo "E2E Test: connection credentials are bound and rotatable"
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
    RUNTARA_AGENT_COMPONENTS_DIR="${AGENTS_DIR-${COMPONENTS_DIR}}" \
    MAX_CONCURRENT_EXECUTIONS=40 \
    DATA_DIR="${TEST_DATA_DIR}" \
    RUST_LOG="warn,runtara_server=info" \
    AUTH_PROVIDER=local \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true \
    RUNTARA_SDK_BACKEND=http \
    SQLX_OFFLINE="${SQLX_OFFLINE}" \
    env "${KEY_ENV[@]}" "${RUNTARA_SERVER_BIN}" >>"${TEST_LOG}" 2>&1 &
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


runtime_sql() { psql_quiet -d "${TEST_DB_RUNTIME}" -c "$1"; }

# Run the workflow; fail unless it completes with its output.
run_echo() {
    local resp instance status
    resp=$(api_post "/workflows/${WF_ID}/execute" '{"inputs": {"data": {}}}')
    instance=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    [ -n "${instance}" ] || { print_error "execute failed: ${resp}"; exit 1; }
    status=""
    for _ in {1..120}; do
        status=$(curl -sS "${API}/workflows/instances/${instance}" | jq -r '.data.status // empty')
        case "${status}" in completed|failed|crashed|stopped|cancelled) break ;; esac
        sleep 0.5
    done
    resp=$(curl -sS "${API}/workflows/instances/${instance}")
    [ "${status}" = "completed" ] && echo "${resp}" \
        | jq -e '.data.outputs.answer == 42 and .data.outputs.hash == "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"' >/dev/null \
        || { print_error "the run ended '${status}': $(echo "${resp}" | jq -c '.data.error // .data.outputs')"; tail -40 "${TEST_LOG}"; exit 1; }
}


server_sql() { psql_quiet -d "${TEST_DB_SERVER}" -c "$1"; }
envelope_of() { server_sql "SELECT (connection_parameters::jsonb->>'v') || ':' || (connection_parameters::jsonb->>'kid') FROM connection_data_entity WHERE id = '$1'"; }
schemas_status() { curl -sS -o /dev/null -w '%{http_code}' "${API}/object-model/schemas?connectionId=$1"; }
create_connection() {
    local resp id
    resp=$(api_post /connections "{\"title\": \"$1\", \"integrationId\": \"postgres\", \"connectionParameters\": {\"database_url\": \"$2\"}}")
    id=$(echo "${resp}" | jq -r '.connection_id // .connectionId // empty')
    [ -n "${id}" ] || { print_error "connection create failed"; exit 1; }
    echo "${id}"
}
restart() {
    kill "${SERVER_PID}"; wait "${SERVER_PID}" 2>/dev/null || true
    SERVER_PID=""
    start_server
}

#-------------------------------------------------------------------------
print_step "1. Credentials are bound to their connection..."
CONN=$(create_connection "bound" "${SERVER_DB_URL}")
[ "$(envelope_of "${CONN}")" = "2:k1" ] || { print_error "the stored envelope is $(envelope_of "${CONN}"), not 2:k1"; exit 1; }
[ "$(schemas_status "${CONN}")" = "200" ] || { print_error "the connection does not decrypt for itself"; exit 1; }
OTHER=$(create_connection "other" "postgresql://u:p@db.invalid/x")
server_sql "UPDATE connection_data_entity SET connection_parameters =
              (SELECT connection_parameters FROM connection_data_entity WHERE id = '${CONN}')
            WHERE id = '${OTHER}'" >/dev/null
case "$(schemas_status "${OTHER}")" in
    2??) print_error "a ciphertext copied to another connection decrypted"; exit 1 ;;
esac
grep -q "Failed to decrypt connection parameters" "${TEST_LOG}" \
    || { print_error "the copied ciphertext did not fail to decrypt"; tail -20 "${TEST_LOG}"; exit 1; }
echo "  stored as a version 2 envelope under k1; the same ciphertext on another connection does not decrypt ✓"

#-------------------------------------------------------------------------
print_step "2. Rotate to k2, re-encrypt, retire k1..."
KEY_ENV=("RUNTARA_CONNECTIONS_ENCRYPTION_KEY=${KEY2}" "RUNTARA_CONNECTIONS_ENCRYPTION_KEY_ID=k2" "RUNTARA_CONNECTIONS_RETIRED_KEYS=k1:${KEY1}")
restart
[ "$(schemas_status "${CONN}")" = "200" ] || { print_error "the k1 credential does not decrypt with k1 retired"; exit 1; }
# The copied ciphertext is not that connection's, so re-encryption refuses
# it (and says so) while it moves the real one.
REENCRYPT_LOG="${TEST_DATA_DIR}/reencrypt.log"
RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" SKIP_MIGRATIONS=true \
    env "${KEY_ENV[@]}" "${RUNTARA_SERVER_BIN}" reencrypt-connections --tenant-id "${TENANT}" >"${REENCRYPT_LOG}" 2>&1 || true
grep -q "failed=1" "${REENCRYPT_LOG}" && grep -q "${OTHER}" "${REENCRYPT_LOG}" \
    || { print_error "re-encryption did not refuse exactly the moved ciphertext"; tail -10 "${REENCRYPT_LOG}"; exit 1; }
[ "$(envelope_of "${CONN}")" = "2:k2" ] || { print_error "re-encryption left $(envelope_of "${CONN}"), not 2:k2"; exit 1; }
KEY_ENV=("RUNTARA_CONNECTIONS_ENCRYPTION_KEY=${KEY2}" "RUNTARA_CONNECTIONS_ENCRYPTION_KEY_ID=k2")
restart
[ "$(schemas_status "${CONN}")" = "200" ] || { print_error "the re-encrypted credential does not decrypt with k1 gone"; exit 1; }
echo "  readable with k1 retired, re-encrypted to k2 (the moved ciphertext refused), and readable with k1 gone ✓"

print_success "credentials: bound to their connection, and rotated by re-encryption"
