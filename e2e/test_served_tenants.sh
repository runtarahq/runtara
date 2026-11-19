#!/bin/bash
# E2E Test: a server serves exactly its tenants (M5.1, first slice).
#
# The server's primary tenant is TENANT_ID; RUNTARA_SERVED_TENANTS adds a
# second one. Nothing here may authenticate or run work for a third.
#
#   1. WEBHOOKS — the primary tenant's HTTP trigger and sync endpoint run its
#      workflow; a trigger row of an unserved tenant is not found.
#   2. API KEYS — the primary tenant's key works; a key of an unserved tenant
#      is refused; a key of the served second tenant authenticates as that
#      tenant and sees none of the primary tenant's workflows.
#   3. SECOND TENANT'S WORK — as the second tenant: a workflow compiles, its
#      HTTP trigger runs it through the second tenant's trigger stream, and
#      its cron trigger fires; its runs are its own, and the primary tenant
#      sees none of them.
#   4. NO CROSS-TENANT READS — neither tenant reads the other's workflow,
#      versions or runs by id.
#   5. OWN LIMITS — the second tenant is limited to one workflow
#      (RUNTARA_TENANT_ENTITLEMENTS_JSON): its second is refused while the
#      primary tenant still creates more, and each reads its own entitlements.
#   6. OAUTH AND CONNECTION EVENTS — the second tenant's authorize URL
#      redirects to its own callback path; that state is refused at the
#      primary tenant's callback and left alone; its connection.created product
#      event carries the second tenant, and a failed callback's event is not
#      filed under the tenant named in its URL.
#   7. MCP SESSIONS — the second tenant's MCP session is kept under its own
#      tenant; after a restart, restoring it with the primary tenant's
#      credentials fails, while its owner restores it.
#   8. ROW SECURITY (C11, M7.11) — the server reaches the runtime and server
#      databases as a plain role that owns them, so row-level security binds
#      everything above; that role, naming one tenant, sees none of the
#      other's runs, workflows, triggers or API keys.
#   9. PACED CALLS (SYN-917) — the second tenant may make 12 calls a minute
#      that start external work: its run of 15 outbound HTTP calls waits for
#      the last three and completes, while the primary's same run does not.
#  10. CROSS-TENANT SWEEP (M7.16) — every GET route of the server's OpenAPI
#      document, its path filled with the second tenant's workflow, run,
#      trigger, connection and API key ids, asked as the primary tenant:
#      none answers 2xx with the second tenant's object.
#
#  11. THE ENGINE DECIDES (TWO_ROLES only; docs/separating-concerns.md,
#      Phase 4) — the API is a database role with its own records and no
#      more, and a Valkey user that cannot write what starts work
#      (scripts/roles). With all the API holds — its certificate, its
#      database role, its Valkey user — the engine is asked for another
#      tenant and refuses; a key held to less is held to it by the engine,
#      for reading as for running; the outbox, the keys and the runtime database are
#      closed; a definition changed in the server database does not run
#      until a caller who may compiles it; the version that runs is the one
#      chosen through the engine; a version a caller edits through the API
#      still runs as edited.
#
# With SERVED_TENANTS_KEY_ID_PER_TENANT, a host that seals each tenant's
# credentials under its own keys (key id `{tenant}-k1`) is checked to.
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
# runtara-server needs a credential encryption key; a throwaway one per run.
# shellcheck source=../scripts/credential-key.sh
source "${PROJECT_ROOT}/scripts/credential-key.sh"
ensure_credential_key

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-${PGPASSWORD:-}}"

TEST_DB_SERVER="served_tenants_e2e_server_$$"
TEST_DB_RUNTIME="served_tenants_e2e_runtime_$$"
RUNTIME_ROLE="${TEST_DB_RUNTIME}_owner"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17880}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18881}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18882}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18883}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18884}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16480}"
TEST_DATA_DIR="$(mktemp -d -t runtara_served_tenants_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
# TWO_ROLES=true runs the server as two processes (RUNTARA_ROLE): an engine,
# and an API in front of it that every request here goes to. The API is then
# the plain database role with nothing of the engine's, and what the engine
# is asked crosses a wire, so this is the same test of the same tenants with
# the tenant's scope entered on the engine's side.
TWO_ROLES="${TWO_ROLES:-false}"
ENGINE_PID=""
SEAL_PID=""
SEAL_DIR=""
TEST_ENGINE_PORT="${TEST_ENGINE_PORT:-17885}"
# The API's own database role and Valkey user (TWO_ROLES): scripts/roles.
API_ROLE="${TEST_DB_SERVER}_api"
API_PASSWORD=""
API_VALKEY_USER="runtara-api"
API_VALKEY_PASSWORD=""
# Intake (TWO_ROLES): what receives what anyone may send, which an engine of
# its own does not. Its own database role, Valkey user, certificate and seal.
INTAKE_PID=""
INTAKE_SEAL_PID=""
TEST_INTAKE_PORT="${TEST_INTAKE_PORT:-17886}"
TEST_INTAKE_HEALTH_PORT="${TEST_INTAKE_HEALTH_PORT:-17887}"
INTAKE_ROLE="${TEST_DB_SERVER}_intake"
INTAKE_PASSWORD=""
INTAKE_VALKEY_PASSWORD=""
MADE_RUNTARA_SYSTEM=""
VALKEY_CONTAINER=""
TENANT="served_tenants_e2e"
SECOND_TENANT="served_tenants_e2e_b"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"
# The compiler's WASI command, which an engine that is a process of its own
# compiles in: RUNTARA_COMPILER_WASM, or built beside the components.
COMPILER_WASM="${RUNTARA_COMPILER_WASM:-${COMPONENTS_DIR}/runtara-compile-sandboxed.wasm}"

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
    if [ -n "${ENGINE_PID}" ] && kill -0 "${ENGINE_PID}" 2>/dev/null; then
        kill "${ENGINE_PID}" 2>/dev/null || true
        wait "${ENGINE_PID}" 2>/dev/null || true
    fi
    [ -z "${INTAKE_PID}" ] || kill "${INTAKE_PID}" 2>/dev/null || true
    [ -z "${INTAKE_SEAL_PID}" ] || kill "${INTAKE_SEAL_PID}" 2>/dev/null || true
    [ -z "${SEAL_PID}" ] || kill "${SEAL_PID}" 2>/dev/null || true
    [ -z "${SEAL_DIR}" ] || rm -rf "${SEAL_DIR}"
    if [ -n "${VALKEY_CONTAINER}" ]; then
        docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    fi
    [ -n "${MOCK_PID:-}" ] && kill "${MOCK_PID}" 2>/dev/null || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP ROLE IF EXISTS ${RUNTIME_ROLE}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP ROLE IF EXISTS ${API_ROLE}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP ROLE IF EXISTS ${INTAKE_ROLE}" >/dev/null 2>&1 || true
    # `runtara_system` is the cluster's: it is dropped only if this run made it.
    [ -z "${MADE_RUNTARA_SYSTEM}" ] \
        || psql_quiet -d postgres -c "DROP ROLE IF EXISTS runtara_system" >/dev/null 2>&1 || true
    if [ -n "${SERVED_TENANTS_KEEP:-}" ]; then
        echo "kept ${TEST_DATA_DIR}" >&2
    else
        rm -rf "${TEST_DATA_DIR}" 2>/dev/null || true
    fi
}
trap cleanup EXIT

API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

# Requests run as the primary tenant (local auth) unless AUTH carries a key.
AUTH=()
api_post() {
    curl -sS --max-time "${3:-120}" -X POST -H "Content-Type: application/json" ${AUTH[@]+"${AUTH[@]}"} \
        ${2:+-d "$2"} "${API}$1"
}

# Create a workflow, save its graph, compile it; echoes the workflow id.
create_and_compile() {
    local name="$1" graph="$2"
    local resp wf_id version
    resp=$(api_post /workflows/create "{\"name\":\"${name}\",\"description\":\"served tenants e2e\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || { print_error "create failed: ${resp}"; exit 1; }
    resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${graph}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] \
        || { print_error "update/validate failed for ${name}: ${resp}"; exit 1; }
    version=$(curl -sS ${AUTH[@]+"${AUTH[@]}"} "${API}/workflows/${wf_id}/versions" \
        | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf_id}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] \
        || { print_error "compile failed for ${name}: ${resp}"; tail -40 "${TEST_LOG}" >&2; exit 1; }
    echo "${wf_id}"
}

#-------------------------------------------------------------------------
echo "==============================================================="
echo "E2E Test: a server serves exactly its tenants"
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
# Neither a superuser nor BYPASSRLS: row-level security binds the server, on
# both databases it owns. The extensions its migrations ask for are made
# beforehand, as an administrator makes them on a deployment.
RUNTIME_PASSWORD=$(openssl rand -hex 16)
psql_quiet -d postgres -c "CREATE ROLE ${RUNTIME_ROLE} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD '${RUNTIME_PASSWORD}'" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER} OWNER ${RUNTIME_ROLE}" >/dev/null
for extension in uuid-ossp vector fuzzystrmatch pg_trgm; do
    psql_quiet -d "${TEST_DB_SERVER}" -c "CREATE EXTENSION IF NOT EXISTS \"${extension}\"" >/dev/null
done
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME} OWNER ${RUNTIME_ROLE}" >/dev/null
SERVER_DB_URL="postgresql://${RUNTIME_ROLE}:${RUNTIME_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
# The API in front of an engine is a role of its own, which owns nothing: it
# is granted its own records once the engine has made the tables.
API_DB_URL="${SERVER_DB_URL}"
if [ "${TWO_ROLES}" = "true" ]; then
    API_PASSWORD=$(openssl rand -hex 16)
    API_VALKEY_PASSWORD=$(openssl rand -hex 16)
    psql_quiet -d postgres -c "CREATE ROLE ${API_ROLE} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD '${API_PASSWORD}'" >/dev/null
    API_DB_URL="postgresql://${API_ROLE}:${API_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
    INTAKE_PASSWORD=$(openssl rand -hex 16)
    INTAKE_VALKEY_PASSWORD=$(openssl rand -hex 16)
    psql_quiet -d postgres -c "CREATE ROLE ${INTAKE_ROLE} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD '${INTAKE_PASSWORD}'" >/dev/null
    [ "$(psql_quiet -d postgres -c "SELECT count(*) FROM pg_roles WHERE rolname = 'runtara_system'")" = "1" ] \
        || MADE_RUNTARA_SYSTEM=yes
fi

# What the API role is given of the database and of Valkey, as a deployment
# gives it: the two files under scripts/roles.
grant_the_api_its_own() {
    psql_quiet -d "${TEST_DB_SERVER}" -v api_role="${API_ROLE}" -v engine_role="${RUNTIME_ROLE}" \
        < "${PROJECT_ROOT}/scripts/roles/api-database-role.sql" >/dev/null
    # shellcheck disable=SC2046
    docker exec "${VALKEY_CONTAINER}" valkey-cli ACL SETUSER "${API_VALKEY_USER}" on ">${API_VALKEY_PASSWORD}" \
        $(grep -v '^#' "${PROJECT_ROOT}/scripts/roles/api-valkey-user.acl") >/dev/null
}
# No platform object-model database: tenants sharing a process take none
# (M7.16).

# An engine that is a process of its own has a seal of its own: SEAL_BIN, or
# seal built at the revision the server uses.
start_seal() {
    local rev
    rev=$(sed -n 's/^seal-client = .*rev = "\([0-9a-f]*\)".*/\1/p' "${PROJECT_ROOT}/crates/runtara-connections/Cargo.toml")
    if [ -z "${SEAL_BIN:-}" ]; then
        SEAL_BIN="${PROJECT_ROOT}/target/seal/${rev}/bin/seal"
        [ -x "${SEAL_BIN}" ] || cargo install --locked --git https://github.com/runtarahq/seal \
            --rev "${rev}" --root "${PROJECT_ROOT}/target/seal/${rev}" seal >&2
    fi
    # A socket's path is short.
    SEAL_DIR="$(mktemp -d /tmp/rseal.XXXXXX)"
    mkdir "${SEAL_DIR}/keys"
    (umask 077 && head -c 32 /dev/urandom > "${SEAL_DIR}/kek")
    cat > "${SEAL_DIR}/seal.toml" <<TOML
[listen]
unix = "${SEAL_DIR}/seal.sock"

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
private_rule_cidrs = ["127.0.0.0/8", "::1/128"]
honour_plaintext_postgres_rules = true

[audit]
sink = { file = "${SEAL_DIR}/audit.jsonl" }
TOML
    chmod 600 "${SEAL_DIR}/seal.toml"
    "${SEAL_BIN}" serve --config "${SEAL_DIR}/seal.toml" >>"${TEST_DATA_DIR}/seal.log" 2>&1 &
    SEAL_PID=$!
    for _ in {1..40}; do
        kill -0 "${SEAL_PID}" 2>/dev/null || { print_error "seal exited as it started"; tail -20 "${TEST_DATA_DIR}/seal.log"; exit 1; }
        [ -S "${SEAL_DIR}/seal.sock" ] && return 0
        sleep 0.25
    done
    print_error "seal did not listen"; exit 1
}

# Intake's seal: the same keys, for a caller that administers nothing and
# may only verify.
start_intakes_seal() {
    sed -e "s|${SEAL_DIR}/seal.sock|${SEAL_DIR}/seal-intake.sock|" \
        -e 's|name = "runtara-server"|name = "runtara-intake"|' \
        -e 's|^admin = true|admin = false|' \
        -e 's|^destroy_keysets = true|destroy_keysets = false\noperations = ["verify"]|' \
        -e "s|${SEAL_DIR}/audit.jsonl|${SEAL_DIR}/audit-intake.jsonl|" \
        "${SEAL_DIR}/seal.toml" > "${SEAL_DIR}/seal-intake.toml"
    chmod 600 "${SEAL_DIR}/seal-intake.toml"
    "${SEAL_BIN}" serve --config "${SEAL_DIR}/seal-intake.toml" >>"${TEST_DATA_DIR}/seal-intake.log" 2>&1 &
    INTAKE_SEAL_PID=$!
    for _ in {1..40}; do
        kill -0 "${INTAKE_SEAL_PID}" 2>/dev/null || { print_error "intake's seal exited as it started"; tail -20 "${TEST_DATA_DIR}/seal-intake.log"; exit 1; }
        [ -S "${SEAL_DIR}/seal-intake.sock" ] && return 0
        sleep 0.25
    done
    print_error "intake's seal did not listen"; exit 1
}

# Intake: what it is given, in full. It receives the webhooks and the OAuth
# redirects the API hands it, which an engine of its own does not.
start_intake() {
    psql_quiet -d "${TEST_DB_SERVER}" -v intake_role="${INTAKE_ROLE}" \
        < "${PROJECT_ROOT}/scripts/roles/intake-database-role.sql" >/dev/null
    # shellcheck disable=SC2046
    docker exec "${VALKEY_CONTAINER}" valkey-cli ACL SETUSER runtara-intake on ">${INTAKE_VALKEY_PASSWORD}" \
        $(grep -v '^#' "${PROJECT_ROOT}/scripts/roles/intake-valkey-user.acl") >/dev/null
    start_intakes_seal
    (cd "${TEST_DATA_DIR}" && exec env -i \
    PATH="${PATH}" HOME="${HOME}" \
    RUNTARA_ROLE=intake \
    RUNTARA_INTAKE_LISTEN="127.0.0.1:${TEST_INTAKE_PORT}" \
    RUNTARA_INTAKE_HEALTH_LISTEN="127.0.0.1:${TEST_INTAKE_HEALTH_PORT}" \
    RUNTARA_INTAKE_TLS_DIR="${TEST_DATA_DIR}/pki-intake" \
    RUNTARA_ENGINE_URL="https://127.0.0.1:${TEST_ENGINE_PORT}" \
    RUNTARA_SERVER_DATABASE_URL="postgresql://${INTAKE_ROLE}:${INTAKE_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}" \
    RUNTARA_REQUIRE_ROW_SECURITY=true \
    RUNTARA_SEAL_MODE=unix \
    RUNTARA_SEAL_SOCKET="${SEAL_DIR}/seal-intake.sock" \
    TENANT_ID="${TENANT}" \
    RUNTARA_SERVED_TENANTS="${SECOND_TENANT}" \
    RUNTARA_OAUTH_TENANTLESS_CALLBACK=false \
    RUST_LOG="warn,runtara_server=info" \
    AUTH_PROVIDER=local \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    VALKEY_USER=runtara-intake \
    VALKEY_PASSWORD="${INTAKE_VALKEY_PASSWORD}" \
    OTEL_SDK_DISABLED=true \
    SKIP_MIGRATIONS=true \
    "${RUNTARA_SERVER_BIN}") >>"${TEST_LOG}" 2>&1 &
    INTAKE_PID=$!
    for _ in {1..60}; do
        curl -sS -o /dev/null "http://127.0.0.1:${TEST_INTAKE_HEALTH_PORT}/health" 2>/dev/null && return 0
        kill -0 "${INTAKE_PID}" 2>/dev/null || { print_error "Intake exited during boot."; tail -30 "${TEST_LOG}"; exit 1; }
        sleep 1
    done
    print_error "Intake did not become ready."; exit 1
}

# The API in front of an engine: what it is given, in full. Stopping "the
# server" then stops the API; the engine keeps running, as its runs do.
start_api() {
    (cd "${TEST_DATA_DIR}" && exec env -i \
    PATH="${PATH}" HOME="${HOME}" \
    RUNTARA_ROLE=api \
    RUNTARA_ENGINE_URL="https://127.0.0.1:${TEST_ENGINE_PORT}" \
    RUNTARA_INTAKE_URL="https://127.0.0.1:${TEST_INTAKE_PORT}" \
    RUNTARA_ENGINE_TLS_DIR="${TEST_DATA_DIR}/pki-api" \
    RUNTARA_SERVER_DATABASE_URL="${API_DB_URL}" \
    TENANT_ID="${TENANT}" \
    RUNTARA_SERVED_TENANTS="${SECOND_TENANT}" \
    RUNTARA_REQUIRE_ROW_SECURITY=true \
    RUNTARA_TENANT_ENTITLEMENTS_JSON="{\"${SECOND_TENANT}\": {\"overrides\": {\"limits\": {\"maxWorkflows\": 1, \"workflowCallsPerMinute\": 12}}}}" \
    SERVER_HOST=127.0.0.1 \
    SERVER_PORT="${TEST_PORT_PUBLIC}" \
    RUNTARA_AGENT_COMPONENTS_DIR="${TEST_DATA_DIR}/agent-descriptions" \
    MAX_CONCURRENT_EXECUTIONS=40 \
    RUST_LOG="warn,runtara_server=info" \
    AUTH_PROVIDER=local \
    RUNTARA_OAUTH_TENANTLESS_CALLBACK=false \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    VALKEY_USER="${API_VALKEY_USER}" \
    VALKEY_PASSWORD="${API_VALKEY_PASSWORD}" \
    OTEL_SDK_DISABLED=true \
    SKIP_MIGRATIONS=true \
    "${RUNTARA_SERVER_BIN}") >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!
}

start_server() {
    print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
    if [ "${TWO_ROLES}" = "true" ]; then
        if [ -z "${ENGINE_PID}" ]; then
            "$(dirname "${RUNTARA_SERVER_BIN}")/runtara-executor" init-pki \
                --server-dir "${TEST_DATA_DIR}/pki-engine" \
                --executor-dir "${TEST_DATA_DIR}/pki-executor" \
                --api-dir "${TEST_DATA_DIR}/pki-api" \
                --intake-dir "${TEST_DATA_DIR}/pki-intake" >/dev/null
            mkdir "${TEST_DATA_DIR}/agent-descriptions"
            cp "${COMPONENTS_DIR}"/runtara_agent_*.meta.json "${TEST_DATA_DIR}/agent-descriptions/"
            if [ ! -f "${COMPILER_WASM}" ]; then
                cargo build -p runtara-compile-guest --bin runtara-compile-sandboxed --release --target wasm32-wasip2 >&2
            fi
            start_seal
            RUNTARA_ROLE=engine \
            RUNTARA_COMPILER_WASM="${COMPILER_WASM}" \
            RUNTARA_ENGINE_LISTEN="127.0.0.1:${TEST_ENGINE_PORT}" \
            RUNTARA_ENGINE_TLS_DIR="${TEST_DATA_DIR}/pki-engine" \
            RUNTARA_SEAL_MODE=unix \
            RUNTARA_SEAL_SOCKET="${SEAL_DIR}/seal.sock" \
                start_one_server
            ENGINE_PID="${SERVER_PID}"
            for _ in {1..120}; do
                grep -q "engine is ready" "${TEST_LOG}" && break
                sleep 1
                kill -0 "${ENGINE_PID}" 2>/dev/null || { print_error "Engine exited during boot."; tail -30 "${TEST_LOG}"; exit 1; }
            done
            grant_the_api_its_own
            start_intake
        fi
        start_api
    else
        start_one_server
    fi

    for _ in {1..60}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            break
        fi
        sleep 1
        kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "Server exited during boot."; tail -30 "${TEST_LOG}"; exit 1; }
    done
    echo "  Server up (PID ${SERVER_PID})"
}

# One server process, in whatever role its environment names.
start_one_server() {
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
    RUNTARA_DATABASE_CONNECTION_GUARD=off RUNTARA_SEAL_PRIVATE_CIDRS="${RUNTARA_SEAL_PRIVATE_CIDRS:-127.0.0.0/8,::1/128}" \
    RUNTARA_DATABASE_URL="postgresql://${RUNTIME_ROLE}:${RUNTIME_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
    TENANT_ID="${TENANT}" \
    RUNTARA_SERVED_TENANTS="${SECOND_TENANT}" \
    RUNTARA_REQUIRE_ROW_SECURITY=true \
    RUNTARA_TENANT_ENTITLEMENTS_JSON="{\"${SECOND_TENANT}\": {\"overrides\": {\"limits\": {\"maxWorkflows\": 1, \"workflowCallsPerMinute\": 12}}}}" \
    RUNTARA_PROXY_ALLOWED_HOSTS=127.0.0.1 \
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
    RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS=127.0.0.1 \
    RUNTARA_PROXY_ALLOW_HTTP_HOSTS=127.0.0.1 \
    RUNTARA_OAUTH_TENANTLESS_CALLBACK=false \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true \
    RUNTARA_SDK_BACKEND=http \
    SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}" >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!
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
http_status() { curl -sS -o /dev/null -w '%{http_code}' "$@"; }

GRAPH='{
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

#-------------------------------------------------------------------------
print_step "1. Webhooks run the primary tenant's work, and only a served tenant's..."
WF_ID=$(create_and_compile "Answer" "${GRAPH}")
RESP=$(api_post /triggers "{\"workflow_id\": \"${WF_ID}\", \"trigger_type\": \"HTTP\", \"active\": true}")
TRIGGER=$(echo "${RESP}" | jq -r '.data.id // .id // empty' 2>/dev/null || true)
[ -n "${TRIGGER}" ] || { print_error "trigger create failed: ${RESP}"; exit 1; }
CODE=$(http_status -X POST -H 'Content-Type: application/json' -d '{"n": 1}' "${API}/${TENANT}/events/http/${TRIGGER}/run")
case "${CODE}" in 2??) ;; *) print_error "the primary tenant's webhook answered ${CODE}"; exit 1 ;; esac
RESP=$(curl -sS -X POST -H 'Content-Type: application/json' -d '{}' "${API}/${TENANT}/events/http-sync/${WF_ID}")
echo "${RESP}" | jq -e '.success == true and .outputs.answer == 42' >/dev/null \
    || { print_error "the sync webhook did not run the workflow: ${RESP}"; exit 1; }
server_sql "CREATE TEMP TABLE t AS SELECT * FROM invocation_trigger WHERE id = '${TRIGGER}';
            UPDATE t SET id = 'outsider-trigger', tenant_id = 'outsider';
            INSERT INTO invocation_trigger SELECT * FROM t;" >/dev/null
CODE=$(http_status -X POST -H 'Content-Type: application/json' -d '{}' "${API}/${TENANT}/events/http/outsider-trigger/run")
[ "${CODE}" = "404" ] || { print_error "an unserved tenant's trigger answered ${CODE}, not 404"; exit 1; }
# Tenants share this process: a webhook names its tenant (M7.16).
CODE=$(http_status -X POST -H 'Content-Type: application/json' -d '{}' "${API}/events/http/${TRIGGER}/run")
[ "${CODE}" = "404" ] || { print_error "a webhook naming no tenant answered ${CODE}, not 404"; exit 1; }
echo "  the primary tenant's webhook and sync run; an unserved tenant's trigger is not found ✓"

#-------------------------------------------------------------------------
print_step "2. API keys authenticate only served tenants, each as itself..."
RESP=$(api_post /api-keys '{"name": "e2e"}')
KEY=$(echo "${RESP}" | jq -r '.key // .data.key // empty')
KEY_ID=$(echo "${RESP}" | jq -r '.id // .data.id // empty')
[ -n "${KEY}" ] && [ -n "${KEY_ID}" ] || { print_error "api key create failed: $(echo "${RESP}" | jq -c 'del(.key)')"; exit 1; }
[ "$(http_status -H "Authorization: Bearer ${KEY}" "${API}/workflows")" = "200" ] \
    || { print_error "the primary tenant's key was refused"; exit 1; }
# Keys cloned from that row for other tenants, with tokens this test knows.
clone_key() {
    local org="$1" token="$2" hash
    hash=$(printf '%s' "${token}" | shasum -a 256 | cut -d' ' -f1)
    server_sql "CREATE TEMP TABLE k AS SELECT * FROM api_keys WHERE id = '${KEY_ID}';
                UPDATE k SET id = gen_random_uuid(), org_id = '${org}', key_hash = '${hash}',
                             jti = gen_random_uuid()::text;
                INSERT INTO api_keys SELECT * FROM k;" >/dev/null
}
OUTSIDER_KEY="rt_outsider_$(openssl rand -hex 24)"
clone_key outsider "${OUTSIDER_KEY}"
CODE=$(http_status -H "Authorization: Bearer ${OUTSIDER_KEY}" "${API}/workflows")
[ "${CODE}" = "403" ] || { print_error "an unserved tenant's key answered ${CODE}, not 403"; exit 1; }
SECOND_KEY="rt_${SECOND_TENANT}_$(openssl rand -hex 24)"
clone_key "${SECOND_TENANT}" "${SECOND_KEY}"
RESP=$(curl -sS -H "Authorization: Bearer ${SECOND_KEY}" "${API}/workflows")
echo "${RESP}" | jq -e '(.data.content // .data // []) | length == 0' >/dev/null \
    || { print_error "the second tenant's key saw the primary tenant's workflows: $(echo "${RESP}" | head -c 300)"; exit 1; }
[ "$(curl -sS -H "Authorization: Bearer ${KEY}" "${API}/workflows" | jq '(.data.content // .data // []) | length')" -ge 1 ] \
    || { print_error "the primary tenant's key no longer sees its workflow"; exit 1; }
echo "  primary key works; an unserved tenant's key is refused (403); the served second tenant sees only its own workflows ✓"

#-------------------------------------------------------------------------
print_step "3. The second tenant's work runs, as the second tenant..."
AUTH=(-H "Authorization: Bearer ${SECOND_KEY}")
B_WF=$(create_and_compile "Answer B" "${GRAPH/\"Answer\"/\"Answer B\"}")
RESP=$(api_post /triggers "{\"workflow_id\": \"${B_WF}\", \"trigger_type\": \"HTTP\", \"active\": true}")
B_TRIGGER=$(echo "${RESP}" | jq -r '.data.id // .id // empty' 2>/dev/null || true)
[ -n "${B_TRIGGER}" ] || { print_error "the second tenant's trigger create failed: ${RESP}"; exit 1; }
RESP=$(api_post /triggers "{\"workflow_id\": \"${B_WF}\", \"trigger_type\": \"CRON\", \"active\": true, \"configuration\": {\"expression\": \"* * * * *\", \"timezone\": \"UTC\"}}")
echo "${RESP}" | jq -e '.data.id // .id' >/dev/null 2>&1 || { print_error "the second tenant's cron create failed: ${RESP}"; exit 1; }
AUTH=()
CODE=$(http_status -X POST -H 'Content-Type: application/json' -d '{}' "${API}/${SECOND_TENANT}/events/http/${B_TRIGGER}/run")
case "${CODE}" in 2??) ;; *) print_error "the second tenant's webhook answered ${CODE}"; exit 1 ;; esac
# Runs land under the second tenant: the webhook's, then the cron's.
b_runs() {
    curl -sS -H "Authorization: Bearer ${SECOND_KEY}" "${API}/workflows/${B_WF}/instances?size=50" \
        | jq '[(.data.content // .data // [])[] | select((.status // "") == "completed")] | length'
}
for _ in {1..60}; do [ "$(b_runs)" -ge 1 ] && break; sleep 1; done
[ "$(b_runs)" -ge 1 ] || { print_error "the second tenant's webhook run did not complete"; tail -30 "${TEST_LOG}"; exit 1; }
for _ in {1..150}; do [ "$(b_runs)" -ge 2 ] && break; sleep 1; done
[ "$(b_runs)" -ge 2 ] || { print_error "the second tenant's cron trigger did not fire"; tail -30 "${TEST_LOG}"; exit 1; }
[ "$(http_status "${API}/workflows/${B_WF}/instances")" = "404" ] \
    || [ "$(curl -sS "${API}/workflows/${B_WF}/instances" | jq '(.data.content // .data // []) | length')" = "0" ] \
    || { print_error "the primary tenant sees the second tenant's runs"; exit 1; }
echo "  the second tenant's workflow compiled, its webhook and cron ran it as the second tenant; the primary sees none ✓"
# A conversation is one tenant's: the same key, under each tenant's own
# workflow, is two conversations, each carried in its tenant's scope by a
# worker that finds them across tenants.
talk() { # <workflow> [curl args: who asks]
    local workflow="$1" resp; shift
    resp=$(curl -sS --max-time 60 "$@" -X POST -H 'Content-Type: application/json' \
        -d '{"id":"m-1","text":"hello"}' "${API}/workflows/${workflow}/conversations/shared/messages")
    [ "$(echo "${resp}" | jq -r '.data.state // empty')" = "started" ] \
        || { print_error "a message did not start a conversation's run: ${resp}"; exit 1; }
    for _ in {1..120}; do
        resp=$(curl -sS --max-time 30 "$@" "${API}/workflows/${workflow}/conversations/shared/events")
        [ "$(echo "${resp}" | jq -r '[.data[]? | select(.kind == "run_ended" and .body.status == "completed")] | length')" = "1" ] && return 0
        sleep 0.5
    done
    print_error "a conversation's run was not carried to its end: ${resp}"; tail -30 "${TEST_LOG}"; exit 1
}
talk "${B_WF}" -H "Authorization: Bearer ${SECOND_KEY}"
talk "${WF_ID}"
[ "$(http_status "${API}/workflows/${B_WF}/conversations/shared")" = "404" ] \
    || { print_error "the primary tenant reads the second tenant's conversation"; exit 1; }
echo "  a conversation with the same key ran in each tenant, and the primary does not find the second's ✓"

#-------------------------------------------------------------------------
print_step "4. Neither tenant reads the other's resources by id..."
for path in "/workflows/${WF_ID}" "/workflows/${WF_ID}/versions" "/workflows/${WF_ID}/instances"; do
    RESP=$(curl -sS -H "Authorization: Bearer ${SECOND_KEY}" "${API}${path}")
    if echo "${RESP}" | jq -e '(.data | type) == "object" and (.data.id? // .data.workflowId? // "") != ""' >/dev/null 2>&1 \
        || echo "${RESP}" | jq -e '((.data.content // .data // []) | type) == "array" and ((.data.content // .data // []) | length) > 0' >/dev/null 2>&1; then
        print_error "the second tenant read the primary's ${path}: $(echo "${RESP}" | head -c 200)"; exit 1
    fi
done
for path in "/workflows/${B_WF}" "/workflows/${B_WF}/versions"; do
    RESP=$(curl -sS "${API}${path}")
    if echo "${RESP}" | jq -e '(.data | type) == "object" and (.data.id? // .data.workflowId? // "") != ""' >/dev/null 2>&1 \
        || echo "${RESP}" | jq -e '((.data.content // .data // []) | type) == "array" and ((.data.content // .data // []) | length) > 0' >/dev/null 2>&1; then
        print_error "the primary read the second tenant's ${path}: $(echo "${RESP}" | head -c 200)"; exit 1
    fi
done
echo "  neither tenant reads the other's workflow, versions or runs by id ✓"

#-------------------------------------------------------------------------
print_step "5. Each tenant enforces its own limits..."
curl -sS -H "Authorization: Bearer ${SECOND_KEY}" "${API}/entitlements" | jq -e '.. | objects | select(has("maxWorkflows")) | .maxWorkflows == 1' >/dev/null \
    || { print_error "the second tenant does not read its own entitlements"; exit 1; }
curl -sS "${API}/entitlements" | jq -e '[.. | objects | select(has("maxWorkflows")) | .maxWorkflows] | all(. != 1)' >/dev/null \
    || { print_error "the primary tenant reads the second tenant's limits"; exit 1; }
CODE=$(curl -sS -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer ${SECOND_KEY}" \
    -H "Content-Type: application/json" -d '{"name":"One too many","description":"limit"}' "${API}/workflows/create")
[ "${CODE}" = "403" ] || { print_error "the second tenant's second workflow answered ${CODE}, not 403"; exit 1; }
RESP=$(api_post /workflows/create '{"name":"Primary again","description":"limit"}')
echo "${RESP}" | jq -e '.data.id' >/dev/null || { print_error "the primary tenant could not create another workflow: ${RESP}"; exit 1; }
echo "  the second tenant is held to one workflow; the primary is not; each reads its own entitlements ✓"

#-------------------------------------------------------------------------
print_step "6. OAuth callbacks and connection events belong to the connection's tenant..."
AUTH=(-H "Authorization: Bearer ${SECOND_KEY}")
RESP=$(api_post /connections '{"title":"B OAuth","integrationId":"http_oauth2_authorization_code","connectionParameters":{"auth_url":"http://127.0.0.1:9/authorize","token_url":"http://127.0.0.1:9/token","client_id":"cid","client_secret":"csec","base_url":"https://api.example.com"}}')
AUTH=()
B_CONN=$(echo "${RESP}" | jq -r '.connection_id // .connectionId // empty')
[ -n "${B_CONN}" ] || { print_error "the second tenant could not create an OAuth connection: ${RESP}"; exit 1; }
# A host that seals each tenant under its own keys names them by tenant
# (SERVED_TENANTS_KEY_ID_PER_TENANT): the connection is under the second's.
if [ -n "${SERVED_TENANTS_KEY_ID_PER_TENANT:-}" ]; then
    KID=$(server_sql "SELECT connection_parameters::jsonb->>'kid' FROM connection_data_entity WHERE id = '${B_CONN}'")
    [ "${KID}" = "${SECOND_TENANT}-k1" ] \
        || { print_error "the second tenant's connection is sealed under '${KID}', not its own key"; exit 1; }
    echo "  the second tenant's connection is sealed under its own key (${KID}) ✓"
fi
AUTHORIZE=$(curl -sS -H "Authorization: Bearer ${SECOND_KEY}" "${API}/connections/${B_CONN}/oauth/authorize" | jq -r '.authorization_url // .authorizationUrl // empty')
case "${AUTHORIZE}" in
    *"%2Fapi%2Foauth%2F${SECOND_TENANT}%2Fcallback"*) ;;
    *) print_error "the authorize URL does not redirect to the second tenant's callback: ${AUTHORIZE}"; exit 1 ;;
esac
STATE=$(python3 -c 'import sys,urllib.parse as u; print(u.parse_qs(u.urlparse(sys.argv[1]).query)["state"][0])' "${AUTHORIZE}")
CALLBACK="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/oauth"
# With a console to go to, the callback redirects there with the error code;
# without one, it answers in plain text.
curl -sS -D - "${CALLBACK}/${TENANT}/callback?state=${STATE}&code=c1" \
    | grep -qiE "Invalid or expired authorization|^location: .*[?&]error=invalid_state" \
    || { print_error "the primary tenant's callback accepted the second tenant's state"; exit 1; }
# Inside the suite's Valkey container: on its published port's own 6379, or,
# where docker runs it on the host network, on the suite's port.
valkey_cli() {
    docker exec "${VALKEY_CONTAINER}" valkey-cli -p "${TEST_VALKEY_PORT}" --raw "$@" 2>/dev/null \
        || docker exec "${VALKEY_CONTAINER}" valkey-cli --raw "$@"
}
# A flow is kept under its tenant, by its state's hash: refused at the primary
# tenant's callback, the second tenant's is neither seen nor spent, so another
# tenant cannot burn a flow it does not own.
STATE_HASH=$(python3 -c 'import sys,hashlib; print(hashlib.sha256(sys.argv[1].encode()).hexdigest())' "${STATE}")
[ "$(valkey_cli EXISTS "${SECOND_TENANT}:oauth_state:${STATE_HASH}")" = "1" ] \
    || { print_error "the primary tenant's callback reached the second tenant's state"; exit 1; }
echo "  the primary tenant's callback refuses the second tenant's state and leaves it alone ✓"
# Its own callback redeems its flow in its own scope: with the browser's
# cookie it gets as far as the provider, which is not there. (An engine in a
# process of its own once answered this in no scope, and row-level security
# refused it its rows.)
JAR="${TEST_DATA_DIR}/oauth.jar"
OWN_AUTHORIZE=$(curl -sS -c "${JAR}" -H "Authorization: Bearer ${SECOND_KEY}" "${API}/connections/${B_CONN}/oauth/authorize" | jq -r '.authorization_url // .authorizationUrl // empty')
OWN_STATE=$(python3 -c 'import sys,urllib.parse as u; print(u.parse_qs(u.urlparse(sys.argv[1]).query)["state"][0])' "${OWN_AUTHORIZE}")
OWN=$(curl -sS -D - -b "${JAR}" "${CALLBACK}/${SECOND_TENANT}/callback?state=${OWN_STATE}&code=c1")
echo "${OWN}" | grep -qE "token_exchange_failed" \
    || { print_error "the second tenant's own callback did not reach the provider: $(echo "${OWN}" | tail -1)"; exit 1; }
echo "  the second tenant's own callback redeems its flow in its scope, as far as the provider ✓"
# A failed callback names no tenant it could prove: its event is not filed
# under the tenant in its path.
curl -sS -o /dev/null "${CALLBACK}/not-a-served-tenant/callback?error=access_denied"
EVENT_TENANT=""
for _ in {1..10}; do
    EVENT_TENANT=$(valkey_cli XRANGE runtara:product_events:stream - + \
        | grep '^{' | jq -r --arg c "${B_CONN}" 'select(.event_type == "connection.created" and .resource_id == $c) | .tenant_id' | head -1)
    [ -n "${EVENT_TENANT}" ] && break
    sleep 1
done
[ "${EVENT_TENANT}" = "${SECOND_TENANT}" ] \
    || { print_error "the second tenant's connection.created event names '${EVENT_TENANT}'"; exit 1; }
FAILED_TENANT=""
for _ in {1..10}; do
    FAILED_TENANT=$(valkey_cli XRANGE runtara:product_events:stream - + \
        | grep '^{' | jq -r 'select(.event_type == "connection.oauth_failed" and (.properties.reason | startswith("access_denied"))) | .tenant_id' | head -1)
    [ -n "${FAILED_TENANT}" ] && break
    sleep 1
done
[ "${FAILED_TENANT}" = "${TENANT}" ] \
    || { print_error "a failed callback's event was filed under '${FAILED_TENANT}', not the primary tenant"; exit 1; }
echo "  the redirect names the second tenant; its state is refused at the primary's callback and left alone; its connection event is its own; a failed callback's path tenant is not trusted ✓"

#-------------------------------------------------------------------------
print_step "7. An MCP session is kept, and restored, under its own tenant..."
MCP_URL="http://127.0.0.1:${TEST_PORT_PUBLIC}/mcp"
MCP_HEADERS="${TEST_DATA_DIR}/mcp_headers"
# POST a JSON-RPC message as AUTH (empty = the primary tenant); echo the HTTP
# status and keep the response headers.
mcp_status() {
    curl -sS -o "${TEST_DATA_DIR}/mcp_body" -w '%{http_code}' --max-time 60 -D "${MCP_HEADERS}" \
        -X POST "${MCP_URL}" -H "Content-Type: application/json" \
        -H "Accept: application/json, text/event-stream" ${AUTH[@]+"${AUTH[@]}"} \
        ${MCP_SESSION:+-H "Mcp-Session-Id: ${MCP_SESSION}"} -d "$1"
}
MCP_SESSION=""
AUTH=(-H "Authorization: Bearer ${SECOND_KEY}")
CODE=$(mcp_status '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"served-tenants-e2e","version":"1"}}}')
MCP_SESSION=$(grep -i '^mcp-session-id:' "${MCP_HEADERS}" | head -1 | cut -d' ' -f2 | tr -d '\r\n' || true)
[ "${CODE}" = "200" ] && [ -n "${MCP_SESSION}" ] \
    || { print_error "the second tenant's MCP initialize answered ${CODE}: $(head -c 300 "${TEST_DATA_DIR}/mcp_body")"; exit 1; }
mcp_status '{"jsonrpc":"2.0","method":"notifications/initialized"}' >/dev/null
[ "$(valkey_cli EXISTS "runtara:mcp:sessions:${SECOND_TENANT}:${MCP_SESSION}")" = "1" ] \
    || { print_error "the session is not kept under the second tenant"; exit 1; }
[ "$(valkey_cli EXISTS "runtara:mcp:sessions:${TENANT}:${MCP_SESSION}")" = "0" ] \
    || { print_error "the session is kept under the primary tenant"; exit 1; }

# A restart drops the in-memory session, so the next request must restore it
# from Valkey, under the requesting tenant. The API keys made above survive
# the restart: in this suite's auth mode nothing records who is a member, and
# the server does not take that for their user having left.
kill "${SERVER_PID}"; wait "${SERVER_PID}" 2>/dev/null || true
start_server
LIST='{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
AUTH=()
CODE=$(mcp_status "${LIST}")
case "${CODE}" in 2??) print_error "the primary tenant restored the second tenant's MCP session (${CODE})"; exit 1 ;; esac
AUTH=(-H "Authorization: Bearer ${SECOND_KEY}")
CODE=$(mcp_status "${LIST}")
[ "${CODE}" = "200" ] && grep -q '"tools"' "${TEST_DATA_DIR}/mcp_body" \
    || { print_error "the second tenant could not restore its own MCP session (${CODE}): $(head -c 300 "${TEST_DATA_DIR}/mcp_body")"; exit 1; }
# A tool that runs a workflow and waits is the caller's own operation: the
# second tenant runs its workflow through its session, and not the primary's.
CALL="{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"execute_workflow_sync\",\"arguments\":{\"workflow_id\":\"${B_WF}\",\"body\":{}}}}"
CODE=$(mcp_status "${CALL}")
[ "${CODE}" = "200" ] && grep -q 'answer' "${TEST_DATA_DIR}/mcp_body" && grep -q '42' "${TEST_DATA_DIR}/mcp_body" \
    || { print_error "the second tenant's MCP tool did not run its workflow (${CODE}): $(head -c 400 "${TEST_DATA_DIR}/mcp_body")"; exit 1; }
CALL="${CALL/${B_WF}/${WF_ID}}"
CODE=$(mcp_status "${CALL}")
if grep -q '\\"answer\\":42\|"answer":42' "${TEST_DATA_DIR}/mcp_body" && ! grep -qi 'not found\|isError.*true' "${TEST_DATA_DIR}/mcp_body"; then
    print_error "the second tenant's MCP tool ran the primary tenant's workflow: $(head -c 400 "${TEST_DATA_DIR}/mcp_body")"; exit 1
fi
AUTH=()
echo "  the session is kept under the second tenant; after a restart the primary is refused it and its owner restores it; its tool runs its own workflow and not the primary's ✓"

#-------------------------------------------------------------------------
print_step "8. The runtime and server databases bind the server to each request's tenant..."
as_runtime_role() {
    PGPASSWORD="${RUNTIME_PASSWORD}" psql -qtA -U "${RUNTIME_ROLE}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" \
        -d "${2:-${TEST_DB_RUNTIME}}" -c "$1"
}
[ "$(as_runtime_role "SELECT rolsuper OR rolbypassrls FROM pg_roles WHERE rolname = current_user")" = "f" ] \
    || { print_error "the runtime role escapes row-level security"; exit 1; }
SECOND_RUNS=$(runtime_sql "SELECT count(*) FROM instances WHERE tenant_id = '${SECOND_TENANT}'")
[ "${SECOND_RUNS}" -gt 0 ] || { print_error "the second tenant has no runs to hide"; exit 1; }
SEEN=$(as_runtime_role "SELECT set_config('runtara.tenant_id', '${TENANT}', false); SELECT count(*) FROM instances WHERE tenant_id = '${SECOND_TENANT}'" | tail -1)
[ "${SEEN}" = "0" ] || { print_error "as ${TENANT} the runtime role saw ${SEEN} of the second tenant's runs"; exit 1; }
echo "  every stage ran on a runtime role bound by row-level security; naming ${TENANT}, it sees none of the second tenant's ${SECOND_RUNS} runs ✓"
for table in "workflows tenant_id" "invocation_trigger tenant_id" "api_keys org_id" \
    "conversations tenant_id" "conversation_messages tenant_id" "conversation_events tenant_id"; do
    set -- ${table}
    OWN=$(server_sql "SELECT count(*) FROM $1 WHERE $2 = '${SECOND_TENANT}'")
    [ "${OWN}" -gt 0 ] || { print_error "the second tenant has no $1 rows to hide"; exit 1; }
    SEEN=$(as_runtime_role "SELECT set_config('runtara.tenant_id', '${TENANT}', false); SELECT count(*) FROM $1 WHERE $2 = '${SECOND_TENANT}'" "${TEST_DB_SERVER}" | tail -1)
    [ "${SEEN}" = "0" ] || { print_error "as ${TENANT} the server role saw ${SEEN} of the second tenant's $1"; exit 1; }
done
echo "  on the server database, naming ${TENANT}, it sees none of the second tenant's workflows, triggers, API keys or conversations ✓"

#-------------------------------------------------------------------------
print_step "9. A run's outbound calls wait for its tenant's limit..."
MOCK_PORT="${TEST_MOCK_PORT:-17889}"
mkdir -p "${TEST_DATA_DIR}/mock" && echo ok > "${TEST_DATA_DIR}/mock/index.html"
python3 -m http.server "${MOCK_PORT}" --bind 127.0.0.1 --directory "${TEST_DATA_DIR}/mock" >/dev/null 2>&1 &
MOCK_PID=$!
CALLS_GRAPH=$(python3 - "${MOCK_PORT}" <<'PY'
import json, sys
url = f"http://127.0.0.1:{sys.argv[1]}/"
n = 15
steps = {f"call{i}": {"stepType": "Agent", "id": f"call{i}", "agentId": "http", "capabilityId": "http-request",
                      "inputMapping": {"url": {"valueType": "immediate", "value": url},
                                       "method": {"valueType": "immediate", "value": "GET"}}}
         for i in range(1, n + 1)}
steps["finish"] = {"stepType": "Finish", "id": "finish",
                   "inputMapping": {"calls": {"valueType": "immediate", "value": n}}}
plan = [{"fromStep": f"call{i}", "toStep": f"call{i + 1}"} for i in range(1, n)]
plan.append({"fromStep": f"call{n}", "toStep": "finish"})
print(json.dumps({"name": "Calls", "steps": steps, "entryPoint": "call1", "executionPlan": plan,
                  "variables": {}, "inputSchema": {}, "outputSchema": {}}))
PY
)
# timed_run WF: seconds the run took to complete; fails unless it completes.
timed_run() {
    local resp instance status started
    started=$(date +%s)
    resp=$(api_post "/workflows/$1/execute" '{"inputs": {"data": {}}}')
    instance=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    [ -n "${instance}" ] || { print_error "execute failed: ${resp}"; exit 1; }
    for _ in {1..240}; do
        status=$(curl -sS ${AUTH[@]+"${AUTH[@]}"} "${API}/workflows/instances/${instance}" | jq -r '.data.status // empty')
        case "${status}" in completed|failed|crashed|stopped|cancelled) break ;; esac
        sleep 0.5
    done
    [ "${status}" = completed ] || { print_error "the run ended '${status}': $(curl -sS ${AUTH[@]+"${AUTH[@]}"} "${API}/workflows/instances/${instance}" | jq -c '.data.error // .data')"; exit 1; }
    echo $(( $(date +%s) - started ))
}
AUTH=()
A_CALLS=$(create_and_compile "Calls" "${CALLS_GRAPH}")
AUTH=(-H "Authorization: Bearer ${SECOND_KEY}")
RESP=$(api_post "/workflows/${B_WF}/update" "{\"executionGraph\": ${CALLS_GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = true ] || { print_error "the second tenant's update failed: ${RESP}"; exit 1; }
VERSION=$(curl -sS ${AUTH[@]+"${AUTH[@]}"} "${API}/workflows/${B_WF}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(api_post "/workflows/${B_WF}/versions/${VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = true ] || { print_error "the second tenant's compile failed: ${RESP}"; exit 1; }
B_SECONDS=$(timed_run "${B_WF}")
AUTH=()
A_SECONDS=$(timed_run "${A_CALLS}")
# At 12 a minute 15 calls cannot finish sooner than three tokens' refill,
# (15 - 12) x 5 s, however fast each call is.
[ "${B_SECONDS}" -ge 15 ] && [ "${A_SECONDS}" -lt "${B_SECONDS}" ] \
    || { print_error "the second tenant's 15 calls took ${B_SECONDS} s against the primary's ${A_SECONDS} s: not paced"; exit 1; }
echo "  15 outbound calls: the second tenant's run, at 12 a minute, completed in ${B_SECONDS} s (at least 15), the primary's in ${A_SECONDS} s ✓"

#-------------------------------------------------------------------------
print_step "10. Every GET route, asked as the primary for the second tenant's objects..."
DUMP_BIN="$(dirname "${RUNTARA_SERVER_BIN}")/dump_openapi"
[ -x "${DUMP_BIN}" ] || SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -q -p runtara-server --bin dump_openapi >&2
"${DUMP_BIN}" > "${TEST_DATA_DIR}/openapi.json"
B_RUN=$(runtime_sql "SELECT instance_id FROM instances WHERE tenant_id = '${SECOND_TENANT}' LIMIT 1")
B_KEY_ID=$(server_sql "SELECT id FROM api_keys WHERE org_id = '${SECOND_TENANT}' LIMIT 1")
python3 - "${TEST_DATA_DIR}/openapi.json" "http://127.0.0.1:${TEST_PORT_PUBLIC}" \
    "${B_WF}" "${B_RUN}" "${B_TRIGGER}" "${B_CONN}" "${B_KEY_ID}" <<'PY' || exit 1
import json, re, sys, urllib.request, urllib.error, uuid
def normal(body, value):
    body = body.replace(value, "<id>")
    return re.sub(r"\d{4}-\d\d-\d\dT[0-9:.]+(Z|[+-]\d\d:?\d\d)?", "<time>", body)
spec, base, workflow, run, trigger, connection, key = sys.argv[1:]
ids = {"workflow": workflow, "instance": run, "execution": run, "run": run,
       "trigger": trigger, "connection": connection, "key": key}
def values(param):
    name = param.lower()
    for word, value in ids.items():
        if word in name:
            return [value]
    return list(ids.values())
asked = leaked = 0
for path, item in json.load(open(spec))["paths"].items():
    if "get" not in item or "{" not in path:
        continue
    params = re.findall(r"{([^}]+)}", path)
    first = values(params[0])
    for value in first:
        url = path
        for param in params:
            url = url.replace("{" + param + "}", value if param == params[0] else values(param)[0])
        asked += 1
        try:
            with urllib.request.urlopen(base + url, timeout=30) as response:
                status, body = response.status, response.read().decode(errors="replace")
        except urllib.error.HTTPError as error:
            status, body = error.code, ""
        except Exception:
            continue
        if 200 <= status < 300 and value in body:
            # A route may echo the id it was asked for: it leaks only if it
            # answers otherwise for an id that names nothing.
            unknown = str(uuid.uuid4())
            try:
                with urllib.request.urlopen(base + url.replace(value, unknown), timeout=30) as response:
                    other = response.read().decode(errors="replace")
            except urllib.error.HTTPError as error:
                other = f"HTTP {error.code}"
            if normal(body, value) != normal(other, unknown):
                leaked += 1
                print(f"  GET {url} answered {status} with the second tenant's {value}", file=sys.stderr)
if leaked:
    sys.exit(f"{leaked} of {asked} cross-tenant reads answered with the second tenant's object")
if asked < 20:
    sys.exit(f"only {asked} routes were asked; the OpenAPI document lists too few")
print(f"  {asked} GET routes asked as the primary for the second tenant's objects: none answered with one ✓")
PY

#-------------------------------------------------------------------------
if [ "${TWO_ROLES}" = "true" ]; then
print_step "11. The engine decides, with all the API holds turned against it..."
AUTH=()

# a. The API's certificate lets it ask the engine; it does not let it say
# for whom. What a caller presented is what the engine goes by.
presented() {  # presented [bearer token]: the wire's header for that caller
    python3 - "$@" <<'PY'
import base64, json, sys
headers = [["authorization", f"Bearer {sys.argv[1]}"]] if len(sys.argv) > 1 else []
print(base64.b64encode(json.dumps(
    {"door": "api", "headers": headers, "requested_tenant": None, "as_a_member": True}).encode()).decode())
PY
}
ask_engine() {  # ask_engine OPERATION BODY [presented]: the status the engine answers
    curl -sS -o "${TEST_DATA_DIR}/engine_body" -w '%{http_code}' --max-time 30 \
        --cacert "${TEST_DATA_DIR}/pki-api/ca.pem" \
        --cert "${TEST_DATA_DIR}/pki-api/tls.pem" --key "${TEST_DATA_DIR}/pki-api/tls.key" \
        --resolve "runtara-server:${TEST_ENGINE_PORT}:127.0.0.1" \
        -X POST -H 'Content-Type: application/json' ${3:+-H "x-runtara-presented: $3"} -d "$2" \
        "https://runtara-server:${TEST_ENGINE_PORT}/engine/v1/operations/$1"
}
B_LIST="{\"tenant_id\": \"${SECOND_TENANT}\", \"workflow_id\": \"${B_WF}\", \"page\": 0, \"size\": 5}"
B_QUEUE="{\"run_label\": null, \"tenant_id\": \"${SECOND_TENANT}\", \"workflow_id\": \"${B_WF}\", \"version\": null, \"inputs\": {\"data\": {}}, \"debug\": false, \"correlation_id\": null, \"idempotency_key\": null, \"trigger_source\": \"HttpApi\", \"instance_id\": null}"
B_RUNS_BEFORE=$(runtime_sql "SELECT count(*) FROM instances WHERE tenant_id = '${SECOND_TENANT}'")
# Nothing presented: in this suite's auth mode that is the primary tenant's
# operator, who is not the second tenant's.
for operation in "ListExecutions ${B_LIST}" "Queue ${B_QUEUE}"; do
    CODE=$(ask_engine "${operation%% *}" "${operation#* }" "$(presented)")
    [ "${CODE}" = "403" ] || { print_error "the engine answered ${operation%% *} for the second tenant to the primary's operator with ${CODE}: $(head -c 300 "${TEST_DATA_DIR}/engine_body")"; exit 1; }
    CODE=$(ask_engine "${operation%% *}" "${operation#* }")
    [ "${CODE}" = "401" ] || { print_error "the engine answered ${operation%% *} with no caller with ${CODE}"; exit 1; }
    CODE=$(ask_engine "${operation%% *}" "${operation#* }" "$(presented "${KEY}")")
    [ "${CODE}" = "403" ] || { print_error "the engine answered ${operation%% *} for the second tenant to the primary's key with ${CODE}"; exit 1; }
    CODE=$(ask_engine "${operation%% *}" "${operation#* }" "$(presented "rt_not_a_key_$(openssl rand -hex 12)")")
    [ "${CODE}" = "401" ] || { print_error "the engine answered ${operation%% *} to a key that is nobody's with ${CODE}"; exit 1; }
done
[ "$(runtime_sql "SELECT count(*) FROM instances WHERE tenant_id = '${SECOND_TENANT}'")" = "${B_RUNS_BEFORE}" ] \
    || { print_error "a refused operation started a run for the second tenant"; exit 1; }
# The second tenant's own key is answered.
CODE=$(ask_engine ListExecutions "${B_LIST}" "$(presented "${SECOND_KEY}")")
[ "${CODE}" = "200" ] || { print_error "the engine refused the second tenant's own key (${CODE}): $(head -c 300 "${TEST_DATA_DIR}/engine_body")"; exit 1; }
# A part of the API that has a caller is served to nobody without one.
CODE=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 30 \
    --cacert "${TEST_DATA_DIR}/pki-api/ca.pem" --cert "${TEST_DATA_DIR}/pki-api/tls.pem" --key "${TEST_DATA_DIR}/pki-api/tls.key" \
    --resolve "runtara-server:${TEST_ENGINE_PORT}:127.0.0.1" \
    -H "x-runtara-caller: $(printf '{"org_id":"%s","user_id":"mallory","auth_method":"Jwt","role":"owner","jti":null,"email":null,"name":null,"tenant_slug":null}' "${SECOND_TENANT}" | base64 | tr -d '\n')" \
    "https://runtara-server:${TEST_ENGINE_PORT}/engine/v1/served/connections/api/runtime/connections")
[ "${CODE}" = "401" ] || { print_error "the engine served connections to a caller the API named (${CODE})"; exit 1; }
echo "  with the API's certificate: no caller 401, another tenant's caller 403, a key that is nobody's 401, no run started; the tenant's own key is answered ✓"

# A key held to less is held to it by the engine too, for what reads as for
# what changes something: through the API, and when the API is not asked.
scoped_key() {  # scoped_key NAME SCOPE-JSON: a key of the primary tenant
    local resp key
    resp=$(api_post /api-keys "{\"name\": \"$1\", \"scope\": $2}")
    key=$(echo "${resp}" | jq -r '.key // .data.key // empty')
    [ -n "${key}" ] || { print_error "the $1 key was not made: $(echo "${resp}" | jq -c 'del(.key)')"; exit 1; }
    echo "${key}"
}
READ_KEY=$(scoped_key reader '"read_only"')
CONN_KEY=$(scoped_key connections '["connection:read"]')
RUN_KEY=$(scoped_key runner '["workflow:execute"]')
A_LIST="{\"tenant_id\": \"${TENANT}\", \"workflow_id\": \"${WF_ID}\", \"page\": 0, \"size\": 5}"
A_QUEUE="${B_QUEUE//${SECOND_TENANT}/${TENANT}}"; A_QUEUE="${A_QUEUE//${B_WF}/${WF_ID}}"
expect_engine() {  # expect_engine STATUS OPERATION BODY KEY WHAT
    local code
    code=$(ask_engine "$2" "$3" "$(presented "$4")")
    [ "${code}" = "$1" ] || { print_error "the engine answered $2 to $5 with ${code}, not $1: $(head -c 300 "${TEST_DATA_DIR}/engine_body")"; exit 1; }
}
expect_engine 200 ListExecutions "${A_LIST}" "${READ_KEY}" "a key that only reads"
expect_engine 403 Queue "${A_QUEUE}" "${READ_KEY}" "a key that only reads"
expect_engine 403 ListExecutions "${A_LIST}" "${CONN_KEY}" "a key held to connections"
expect_engine 403 Queue "${A_QUEUE}" "${CONN_KEY}" "a key held to connections"
expect_engine 403 ListExecutions "${A_LIST}" "${RUN_KEY}" "a key held to running workflows"
A_RUNS_BEFORE=$(runtime_sql "SELECT count(*) FROM instances WHERE tenant_id = '${TENANT}'")
expect_engine 200 Queue "${A_QUEUE}" "${RUN_KEY}" "a key held to running workflows"
for _ in {1..40}; do
    [ "$(runtime_sql "SELECT count(*) FROM instances WHERE tenant_id = '${TENANT}'")" -gt "${A_RUNS_BEFORE}" ] && break
    sleep 0.5
done
[ "$(runtime_sql "SELECT count(*) FROM instances WHERE tenant_id = '${TENANT}'")" -gt "${A_RUNS_BEFORE}" ] \
    || { print_error "the run a key held to running workflows asked for did not start"; exit 1; }
# And through the API, which the engine agrees with.
[ "$(http_status -H "Authorization: Bearer ${READ_KEY}" "${API}/workflows/${WF_ID}/instances")" = "200" ] \
    || { print_error "a key that only reads was refused a workflow's runs through the API"; exit 1; }
[ "$(http_status -H "Authorization: Bearer ${CONN_KEY}" "${API}/workflows/${WF_ID}/instances")" = "403" ] \
    || { print_error "a key held to connections read a workflow's runs through the API"; exit 1; }
[ "$(http_status -H "Authorization: Bearer ${CONN_KEY}" "${API}/connections")" = "200" ] \
    || { print_error "a key held to connections was refused them"; exit 1; }
CODE=$(http_status -X POST -H "Authorization: Bearer ${RUN_KEY}" -H 'Content-Type: application/json' \
    -d '{"inputs": {"data": {}}}' "${API}/workflows/${WF_ID}/execute")
case "${CODE}" in 2??) ;; *) print_error "a key held to running workflows could not run one through the API (${CODE})"; exit 1 ;; esac
echo "  a key that only reads is answered a run's history and refused a run; one held to connections is refused both; one held to running starts a run and lists none ✓"

# b. The API's database role holds its own records and no more.
as_api_role() {  # as_api_role SQL [database]: its output, or the error
    PGPASSWORD="${API_PASSWORD}" psql -qtA -U "${API_ROLE}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" \
        -d "${2:-${TEST_DB_SERVER}}" -c "$1" 2>&1 || true
}
SCOPED="SELECT set_config('runtara.tenant_id', '${TENANT}', false);"
[ "$(as_api_role "${SCOPED} SELECT count(*) > 0 FROM workflows" | tail -1)" = "t" ] \
    || { print_error "the API's role cannot read its own workflows"; exit 1; }
for table in execution_outbox execution_requests execution_admission_reservations \
             connection_data_entity seal_wrapped_key platform_oauth_app oauth_state; do
    OUT=$(as_api_role "${SCOPED} SELECT count(*) FROM ${table}")
    echo "${OUT}" | grep -q "permission denied" \
        || { print_error "the API's role read ${table}: ${OUT}"; exit 1; }
done
OUT=$(as_api_role "${SCOPED} DELETE FROM workflow_compilations")
echo "${OUT}" | grep -q "permission denied" || { print_error "the API's role wrote workflow_compilations: ${OUT}"; exit 1; }
OUT=$(as_api_role "SELECT set_config('runtara.all_tenants', 'on', false); SELECT count(*) FROM workflows")
echo "${OUT}" | grep -q "may not work across tenants" \
    || { print_error "the API's role worked across tenants: ${OUT}"; exit 1; }
OUT=$(as_api_role "SELECT count(*) FROM instances" "${TEST_DB_RUNTIME}")
echo "${OUT}" | grep -q "permission denied" || { print_error "the API's role read the runtime database: ${OUT}"; exit 1; }
OUT=$(as_api_role "SELECT count(*) FROM workflow_approvals" "${TEST_DB_RUNTIME}")
echo "${OUT}" | grep -q "permission denied" || { print_error "the API's role read what the engine approved: ${OUT}"; exit 1; }
echo "  the API's database role reads its workflows; the outbox, admission, connections, keys, compilations (to write), every tenant at once and the runtime database are closed to it ✓"

# c. Its Valkey user cannot write what starts work.
as_api_valkey() {
    docker exec "${VALKEY_CONTAINER}" valkey-cli --user "${API_VALKEY_USER}" --pass "${API_VALKEY_PASSWORD}" --no-auth-warning "$@" 2>&1 || true
}
for command in "XADD runtara:triggers:${SECOND_TENANT} * event x" \
               "RPUSH ${SECOND_TENANT}:compilation:queue x" "RPUSH runtara:compilation:queue x" \
               "XADD ${SECOND_TENANT}:control * type x" "INCR ${SECOND_TENANT}:admission:reserved" \
               "SET ${SECOND_TENANT}:member:mallory owner"; do
    # shellcheck disable=SC2086
    OUT=$(as_api_valkey ${command})
    echo "${OUT}" | grep -q "NOPERM" || { print_error "the API's Valkey user ran '${command}': ${OUT}"; exit 1; }
done
[ "$(as_api_valkey SET "${TENANT}:token:revoked:e2e-probe" 1 EX 5)" = "OK" ] \
    || { print_error "the API's Valkey user cannot record a revoked token"; exit 1; }
echo "  the API's Valkey user cannot write a trigger stream, a compilation queue, the control stream, an admission count or a membership ✓"

# d. A definition changed in the server database, which the API's role may
# write, does not run: nobody who may run it asked for it as it now is.
sync_answer() { curl -sS --max-time 120 -X POST -H 'Content-Type: application/json' -d '{}' "${API}/${TENANT}/events/http-sync/${WF_ID}"; }
RESP=$(sync_answer)
echo "${RESP}" | jq -e '.outputs.answer == 42' >/dev/null || { print_error "the workflow did not run before it was changed: ${RESP}"; exit 1; }
WF_VERSION=$(server_sql "SELECT COALESCE(current_version, latest_version) FROM workflows WHERE workflow_id = '${WF_ID}'")
OUT=$(as_api_role "${SCOPED} UPDATE workflow_definitions SET definition = jsonb_set(definition, '{steps,finish,inputMapping,answer,value}', '666') WHERE workflow_id = '${WF_ID}' AND version = ${WF_VERSION} RETURNING 1")
[ "${OUT##*$'\n'}" = "1" ] || { print_error "the definition was not changed as the API's role: ${OUT}"; exit 1; }
RESP=$(sync_answer)
if echo "${RESP}" | jq -e '.outputs.answer == 666' >/dev/null 2>&1; then
    print_error "a definition changed behind the engine's back ran: ${RESP}"; exit 1
fi
echo "${RESP}" | grep -q "approved to run" \
    || { print_error "the changed definition was not refused as unapproved: $(echo "${RESP}" | head -c 400)"; exit 1; }
CODE=$(http_status -X POST -H 'Content-Type: application/json' -d '{"n": 1}' "${API}/${TENANT}/events/http/${TRIGGER}/run")
sleep 5
[ "$(runtime_sql "SELECT count(*) FROM instances WHERE tenant_id = '${TENANT}' AND convert_from(output, 'UTF8') LIKE '%666%'")" = "0" ] \
    || { print_error "a webhook ran the changed definition"; exit 1; }
# A caller who may compile it approves it as it now is, and it runs.
RESP=$(api_post "/workflows/${WF_ID}/versions/${WF_VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "compiling the changed definition failed: ${RESP}"; exit 1; }
RESP=$(sync_answer)
echo "${RESP}" | jq -e '.outputs.answer == 666' >/dev/null \
    || { print_error "the definition a caller compiled did not run: $(echo "${RESP}" | head -c 400)"; exit 1; }
echo "  a definition changed in the server database was refused (webhook and sync) until a caller compiled it, and then ran ✓"

# e. The version that runs is the one chosen through the engine.
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${GRAPH/\"value\": 42/\"value\": 43}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "a new version was not saved: ${RESP}"; exit 1; }
NEW_VERSION=$(server_sql "SELECT latest_version FROM workflows WHERE workflow_id = '${WF_ID}'")
[ "${NEW_VERSION}" -gt "${WF_VERSION}" ] || { print_error "no new version (${NEW_VERSION})"; exit 1; }
RESP=$(api_post "/workflows/${WF_ID}/versions/${NEW_VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "compiling the new version failed: ${RESP}"; exit 1; }
RESP=$(api_post "/workflows/${WF_ID}/versions/${WF_VERSION}/set-current" '{}')
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "set-current failed: ${RESP}"; exit 1; }
[ "$(runtime_sql "SELECT version FROM workflow_current_versions WHERE workflow_id = '${WF_ID}'")" = "${WF_VERSION}" ] \
    || { print_error "the engine did not record the chosen version"; exit 1; }
echo "$(sync_answer)" | jq -e '.outputs.answer == 666' >/dev/null || { print_error "the chosen version did not run"; exit 1; }
OUT=$(as_api_role "${SCOPED} UPDATE workflows SET current_version = ${NEW_VERSION} WHERE workflow_id = '${WF_ID}' RETURNING 1")
[ "${OUT##*$'\n'}" = "1" ] || { print_error "the current version was not changed as the API's role: ${OUT}"; exit 1; }
RESP=$(sync_answer)
echo "${RESP}" | jq -e '.outputs.answer == 666' >/dev/null \
    || { print_error "a current version written to the server database chose what ran: $(echo "${RESP}" | head -c 300)"; exit 1; }
RESP=$(api_post "/workflows/${WF_ID}/versions/${NEW_VERSION}/set-current" '{}')
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "set-current failed: ${RESP}"; exit 1; }
echo "$(sync_answer)" | jq -e '.outputs.answer == 43' >/dev/null || { print_error "the version chosen through the engine did not run"; exit 1; }
echo "  the version chosen through the engine ran; one written to the server database behind it did not ✓"

# f. A caller who may edit a version in place approves it as they leave it:
# it is compiled when it next runs, without anyone compiling it first.
RESP=$(curl -sS --max-time 120 -X PUT -H 'Content-Type: application/json' \
    -d "{\"executionGraph\": ${GRAPH/\"value\": 42/\"value\": 44}}" \
    "${API}/workflows/${WF_ID}/versions/${NEW_VERSION}/graph")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "the version was not edited in place: ${RESP}"; exit 1; }
RESP=$(sync_answer)
echo "${RESP}" | jq -e '.outputs.answer == 44' >/dev/null \
    || { print_error "a version a caller edited in place did not run: $(echo "${RESP}" | head -c 400)"; exit 1; }
echo "  a version a caller edited in place through the API ran as edited, compiled as it ran ✓"
fi

print_success "served tenants: webhooks, API keys, triggers, cron, OAuth callbacks and connection events reach exactly the tenants this server serves, each its own, under its own limits"
