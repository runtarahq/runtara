#!/bin/bash
# E2E Test: seal as a process of its own (RUNTARA_SEAL_MODE).
#
# A server that ran seal in its own process moves to a seal that runs beside
# it, and then to one reached over mutual TLS. Throughout, the same
# connection, whose database password was sealed at the start, keeps working.
#
#   1. IN THE SERVER    a Postgres connection's password is sealed; the
#                       tenant's seal key is in the server's database.
#   2. HANDED OVER      a server does not start on a seal that lacks its
#                       keys. `seal-keys export` writes them and the key that
#                       wraps them; seal is started over them, on a socket.
#   3. ON A SOCKET      the server, told RUNTARA_SEAL_MODE=unix, uses the old
#                       connection and seals a new one, through seal: seal's
#                       audit log has each use, and the server's database is
#                       rid of the keys (`seal-keys forget`).
#   4. REFUSED          a server given a wrapping key of its own, or a seal
#                       nobody listens at, does not start.
#   5. OVER MUTUAL TLS  the same seal, reached by address with certificates;
#                       a certificate seal does not know is refused.
#   6. A TENANT LEAVES  its keys are destroyed, by seal, and its connections
#                       open no more.
#
# Prerequisites: Postgres (POSTGRES_PASSWORD or PGPASSWORD in the
# environment), docker (isolated Valkey), openssl, the shared workflow
# components in target/wasm32-wasip2/release
# (scripts/build-agent-components.sh), and seal's binary: SEAL_BIN, or it is
# built from the revision this repository pins (cargo, a few minutes, once).

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

TEST_DB_SERVER="seal_process_e2e_server_$$"
TEST_DB_RUNTIME="seal_process_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17933}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18931}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18932}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18933}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18934}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16533}"
TEST_SEAL_PORT="${TEST_SEAL_PORT:-17943}"
TEST_DATA_DIR="$(mktemp -d -t runtara_seal_process_e2e_XXXXXX)"
# seal's own directory: short, because a socket's path is.
SEAL_DIR="$(mktemp -d /tmp/rseal.XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SEAL_LOG="${TEST_DATA_DIR}/seal.log"
SERVER_PID=""
SEAL_PID=""
VALKEY_CONTAINER=""
TENANT="seal_process_e2e"
# How the server is told where seal runs; empty is seal in the server.
SEAL_ENV=()

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

stop() {
    local pid="$1"
    if [ -n "${pid}" ] && kill -0 "${pid}" 2>/dev/null; then
        kill "${pid}" 2>/dev/null || true
        wait "${pid}" 2>/dev/null || true
    fi
}

cleanup() {
    local code=$?
    stop "${SERVER_PID}"
    stop "${SEAL_PID}"
    if [ -n "${VALKEY_CONTAINER}" ]; then
        docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    fi
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME}" >/dev/null 2>&1 || true
    if [ ${code} -ne 0 ]; then
        for log in "${TEST_LOG}" "${SEAL_LOG}"; do
            [ -f "${log}" ] && { echo "--- ${log##*/} (tail)" >&2; tail -25 "${log}" >&2; }
        done
    fi
    # The keys never outlive the test, unless it is asked to keep its files.
    if [ -n "${SEAL_PROCESS_KEEP:-}" ]; then
        echo "kept ${TEST_DATA_DIR} ${SEAL_DIR}" >&2
    else
        rm -rf "${TEST_DATA_DIR}" "${SEAL_DIR}" 2>/dev/null || true
    fi
    exit ${code}
}
trap cleanup EXIT

API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

api_post() {
    curl -sS --max-time "${3:-120}" -X POST -H "Content-Type: application/json" \
        ${2:+-d "$2"} "${API}$1"
}

#-------------------------------------------------------------------------
echo "==============================================================="
echo "E2E Test: seal as a process of its own"
echo "==============================================================="

[ -n "${POSTGRES_PASSWORD}" ] \
    || { print_error "set POSTGRES_PASSWORD (or PGPASSWORD) for ${POSTGRES_USER}@${POSTGRES_HOST}"; exit 1; }
if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi
[ -f "${COMPONENTS_DIR}/runtara_workflow_stdlib.wasm" ] \
    || { print_error "Missing component ${COMPONENTS_DIR}/runtara_workflow_stdlib.wasm — run scripts/build-agent-components.sh"; exit 1; }
command -v openssl >/dev/null || { print_error "openssl is required"; exit 1; }

# seal's binary, at the revision the server's own seal is.
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

# Runs a command in the server's environment, with where seal runs. `exec`,
# so that what is started in the background is the server itself, and
# stopping it stops the server; in the foreground it is run in a subshell.
server_env() {
    exec env \
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
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
    RUNTARA_DATABASE_CONNECTION_GUARD=off RUNTARA_SEAL_PRIVATE_CIDRS="${RUNTARA_SEAL_PRIVATE_CIDRS:-127.0.0.0/8,::1/128}" \
    RUNTARA_PROXY_ALLOWED_HOSTS="${POSTGRES_HOST}" \
    RUNTARA_PROXY_ALLOW_HTTP_HOSTS="${POSTGRES_HOST}" \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true \
    RUNTARA_SDK_BACKEND=http \
    SQLX_OFFLINE="${SQLX_OFFLINE}" \
    ${SEAL_ENV[@]+"${SEAL_ENV[@]}"} "$@"
}

start_server() {
    print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
    (server_env "${RUNTARA_SERVER_BIN}") >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!
    for _ in {1..60}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            break
        fi
        sleep 1
        kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "Server exited during boot."; exit 1; }
    done
    echo "  Server up (PID ${SERVER_PID})"
}

stop_server() { stop "${SERVER_PID}"; SERVER_PID=""; }

# A server that must not start: echoes what it said.
refused_start() {
    local out="${TEST_DATA_DIR}/refused.log" pid
    (server_env "${RUNTARA_SERVER_BIN}") >"${out}" 2>&1 &
    pid=$!
    for _ in {1..40}; do
        kill -0 "${pid}" 2>/dev/null || break
        sleep 0.5
    done
    if kill -0 "${pid}" 2>/dev/null; then
        kill "${pid}" 2>/dev/null || true
        wait "${pid}" 2>/dev/null || true
        print_error "the server started"
        return 1
    fi
    cat "${out}"
}

server_sql() { psql_quiet -d "${TEST_DB_SERVER}" -c "$1"; }
keys_in_database() { server_sql "SELECT count(*) FROM seal_wrapped_key"; }
schemas_status() { curl -sS -o /dev/null -w '%{http_code}' "${API}/object-model/schemas?connectionId=$1"; }
create_connection() {
    local resp id
    resp=$(api_post /connections "{\"title\": \"$1\", \"integrationId\": \"postgres\", \"connectionParameters\": {\"database_url\": \"$2\"}}")
    id=$(echo "${resp}" | jq -r '.connection_id // .connectionId // empty')
    [ -n "${id}" ] || { print_error "connection create failed"; exit 1; }
    echo "${id}"
}
seal_mode_logged() { grep -c "mode=\"$1\"" "${TEST_LOG}" || true; }

# seal's configuration: who may call it is the one thing that differs between
# a socket and mutual TLS. The database it logs in to is on this host and has
# no TLS, which seal, like the server, is told it may reach so.
write_seal_config() {
    local listen="$1" caller="$2"
    cat > "${SEAL_DIR}/seal.toml" <<TOML
[listen]
${listen}

# The server: every tenant's keyset, which it makes, reads the public keys
# of, and destroys when a tenant leaves.
[[callers]]
name = "runtara-server"
${caller}
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
}

start_seal() {
    "${SEAL_BIN}" check-config --config "${SEAL_DIR}/seal.toml" >>"${SEAL_LOG}" 2>&1 \
        || { print_error "seal refuses its configuration"; exit 1; }
    "${SEAL_BIN}" serve --config "${SEAL_DIR}/seal.toml" >>"${SEAL_LOG}" 2>&1 &
    SEAL_PID=$!
    for _ in {1..40}; do
        kill -0 "${SEAL_PID}" 2>/dev/null || { print_error "seal exited as it started"; exit 1; }
        if [ -n "${1:-}" ]; then
            (echo > "/dev/tcp/127.0.0.1/$1") 2>/dev/null && break
        else
            [ -S "${SEAL_DIR}/seal.sock" ] && break
        fi
        sleep 0.25
    done
    echo "  seal up (PID ${SEAL_PID})"
}

stop_seal() { stop "${SEAL_PID}"; SEAL_PID=""; }

# How many of seal's audit records are of this operation.
audit_count() {
    local count
    count=$(grep -c "\"op\":\"$1\"" "${SEAL_DIR}/audit.jsonl" 2>/dev/null || true)
    echo "${count:-0}"
}

#-------------------------------------------------------------------------
print_step "1. seal in the server: a connection's password is sealed, its key in the database..."
start_server
[ "$(seal_mode_logged in_process)" -ge 1 ] || { print_error "the server does not say seal runs in it"; exit 1; }
# The URL says how its session is protected: a local database with no TLS,
# which the server is told it may reach so. Its password is sealed.
CONN=$(create_connection "sealed in the server" "${SERVER_DB_URL}?sslmode=disable")
[ "$(schemas_status "${CONN}")" = "200" ] || { print_error "the connection does not work"; exit 1; }
[ "$(keys_in_database)" = "1" ] || { print_error "the database holds $(keys_in_database) seal keys, not the tenant's one"; exit 1; }
echo "  the connection logs in through seal in the server; the tenant's key is in seal_wrapped_key ✓"

#-------------------------------------------------------------------------
print_step "2. The keys are handed over to a seal of its own..."
stop_server
# First, a seal that was not given them: a server that started on it would
# seal to new keys, and what is sealed already would stop opening.
HANDED_OVER_DIR="${SEAL_DIR}"
SEAL_DIR="$(mktemp -d /tmp/rseal.XXXXXX)"
mkdir "${SEAL_DIR}/keys"
(umask 077 && head -c 32 /dev/urandom > "${SEAL_DIR}/kek")
write_seal_config "unix = \"${SEAL_DIR}/seal.sock\"" "unix_uid = $(id -u)"
start_seal
SEAL_ENV=(RUNTARA_SEAL_MODE=unix "RUNTARA_SEAL_SOCKET=${SEAL_DIR}/seal.sock")
SAID=$(refused_start) || exit 1
echo "${SAID}" | grep -q "Hand the keys over first" \
    || { print_error "a server on a seal without its keys said: $(echo "${SAID}" | tail -2)"; exit 1; }
stop_seal
rm -rf "${SEAL_DIR}"
SEAL_DIR="${HANDED_OVER_DIR}"
SEAL_ENV=()
EXPORT_LOG="${TEST_DATA_DIR}/export.log"
(server_env SKIP_MIGRATIONS=true "${RUNTARA_SERVER_BIN}" seal-keys export \
    --out "${SEAL_DIR}/keys" --wrapping-key-out "${SEAL_DIR}/kek") >"${EXPORT_LOG}" 2>&1 \
    || { print_error "the export failed: $(tail -5 "${EXPORT_LOG}")"; exit 1; }
grep -q "Wrote 1 seal key(s)" "${EXPORT_LOG}" || { print_error "the export wrote something else: $(head -3 "${EXPORT_LOG}")"; exit 1; }
[ "$(find "${SEAL_DIR}/keys" -name '*.json' | wc -l | tr -d ' ')" = "1" ] \
    || { print_error "seal's key directory does not hold the one key"; exit 1; }
# The wrapping key file is its owner's alone, and is 32 bytes.
[ "$(wc -c < "${SEAL_DIR}/kek" | tr -d ' ')" = "32" ] || { print_error "the wrapping key is not 32 bytes"; exit 1; }
case "$(ls -l "${SEAL_DIR}/kek")" in -rw-------*) ;; *) print_error "the wrapping key file is readable by others"; exit 1 ;; esac
# Run again, the export replaces nothing, and never the wrapping key.
(server_env SKIP_MIGRATIONS=true "${RUNTARA_SERVER_BIN}" seal-keys export --out "${SEAL_DIR}/keys") 2>&1 \
    | grep -q "Wrote 0 seal key(s).*(1 already there)" || { print_error "a second export did not find the key there"; exit 1; }
if (server_env SKIP_MIGRATIONS=true "${RUNTARA_SERVER_BIN}" seal-keys export \
        --out "${SEAL_DIR}/keys" --wrapping-key-out "${SEAL_DIR}/kek") >/dev/null 2>&1; then
    print_error "a wrapping key file was written over"; exit 1
fi
write_seal_config "unix = \"${SEAL_DIR}/seal.sock\"" "unix_uid = $(id -u)"
start_seal
echo "  a seal without the keys is refused; one key and the key that wraps it are exported, and seal runs over them ✓"

#-------------------------------------------------------------------------
print_step "3. The server uses seal on its socket..."
SEAL_ENV=(RUNTARA_SEAL_MODE=unix "RUNTARA_SEAL_SOCKET=${SEAL_DIR}/seal.sock")
start_server
[ "$(seal_mode_logged unix)" -ge 1 ] || { print_error "the server does not say seal is on a socket"; exit 1; }
[ "$(schemas_status "${CONN}")" = "200" ] \
    || { print_error "the connection sealed in the server does not work through seal"; exit 1; }
NEW=$(create_connection "sealed by seal" "${SERVER_DB_URL}?sslmode=disable")
[ "$(schemas_status "${NEW}")" = "200" ] || { print_error "a connection sealed through seal does not work"; exit 1; }
[ "$(find "${SEAL_DIR}/keys" -name '*.json' | wc -l | tr -d ' ')" = "1" ] \
    || { print_error "a new key was made instead of the one that came over being used"; exit 1; }
# The database's copy of the keys goes once seal answers for each.
FORGET=$( (server_env SKIP_MIGRATIONS=true "${RUNTARA_SERVER_BIN}" seal-keys forget --confirm) 2>&1) \
    || { print_error "the keys were not deleted from the database: ${FORGET}"; exit 1; }
[ "$(keys_in_database)" = "0" ] || { print_error "the database still holds $(keys_in_database) seal keys"; exit 1; }
[ "$(schemas_status "${CONN}")" = "200" ] || { print_error "the connection stopped working once the database's keys were gone"; exit 1; }
# seal records a database session when it ends, which the server's does as
# it stops: each login is there, naming who asked and never the password.
stop_server
[ "$(audit_count connect)" -ge 2 ] \
    || { print_error "seal's audit log has $(audit_count connect) records of the logins it made"; exit 1; }
grep -q '"caller":"runtara-server"' "${SEAL_DIR}/audit.jsonl" || { print_error "the audit log does not name the server"; exit 1; }
PASSWORD_IN_AUDIT=$(grep -c -F "${POSTGRES_PASSWORD}" "${SEAL_DIR}/audit.jsonl" || true)
[ "${PASSWORD_IN_AUDIT:-0}" = "0" ] || { print_error "the audit log holds the database's password"; exit 1; }
echo "  the old connection and a new one work through seal, which logs each use; the database holds no key ✓"

#-------------------------------------------------------------------------
print_step "4. A server that holds a wrapping key, or reaches no seal, does not start..."
SEAL_ENV=(RUNTARA_SEAL_MODE=unix "RUNTARA_SEAL_SOCKET=${SEAL_DIR}/seal.sock" "RUNTARA_SEAL_WRAPPING_KEY=$(openssl rand -base64 32)")
SAID=$(refused_start) || exit 1
echo "${SAID}" | grep -q "RUNTARA_SEAL_WRAPPING_KEY is set" \
    || { print_error "a server with a wrapping key of its own said: $(echo "${SAID}" | tail -2)"; exit 1; }
SEAL_ENV=(RUNTARA_SEAL_MODE=unix "RUNTARA_SEAL_SOCKET=${SEAL_DIR}/nobody.sock")
SAID=$(refused_start) || exit 1
echo "${SAID}" | grep -q "cannot be reached" \
    || { print_error "a server with no seal to reach said: $(echo "${SAID}" | tail -2)"; exit 1; }
SEAL_ENV=(RUNTARA_SEAL_MODE=elsewhere)
SAID=$(refused_start) || exit 1
echo "${SAID}" | grep -q "RUNTARA_SEAL_MODE is \`elsewhere\`" \
    || { print_error "a server told an unknown mode said: $(echo "${SAID}" | tail -2)"; exit 1; }
echo "  each is refused at start, and says why ✓"

#-------------------------------------------------------------------------
print_step "5. The same seal, over mutual TLS..."
stop_seal
PKI="${SEAL_DIR}/pki"
mkdir -p "${PKI}/server" "${PKI}/stranger"
issue() { # issue <name> <subject> [<subjectAltName>]
    openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "${PKI}/$1.key" 2>/dev/null
    openssl req -new -key "${PKI}/$1.key" -subj "$2" -out "${PKI}/$1.csr" 2>/dev/null
    printf 'basicConstraints=CA:FALSE\n%s\n' "${3:+subjectAltName=$3}" > "${PKI}/$1.ext"
    openssl x509 -req -in "${PKI}/$1.csr" -CA "${PKI}/ca.pem" -CAkey "${PKI}/ca.key" \
        -CAcreateserial -days 2 -sha256 -extfile "${PKI}/$1.ext" -out "${PKI}/$1.pem" 2>/dev/null
    chmod 600 "${PKI}/$1.key"
}
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out "${PKI}/ca.key" 2>/dev/null
openssl req -x509 -new -key "${PKI}/ca.key" -subj "/CN=seal e2e CA" -days 2 -sha256 -out "${PKI}/ca.pem" 2>/dev/null
issue seal "/CN=seal" "DNS:seal"
issue runtara "/CN=runtara-server"
issue other "/CN=someone-else"
# What the server is given: the CA, and its own certificate and key.
for who in server stranger; do cp "${PKI}/ca.pem" "${PKI}/${who}/ca.pem"; done
cp "${PKI}/runtara.pem" "${PKI}/server/tls.pem"; cp "${PKI}/runtara.key" "${PKI}/server/tls.key"
cp "${PKI}/other.pem" "${PKI}/stranger/tls.pem"; cp "${PKI}/other.key" "${PKI}/stranger/tls.key"
write_seal_config \
    "tcp = \"127.0.0.1:${TEST_SEAL_PORT}\"
tls = { cert = \"${PKI}/seal.pem\", key = \"${PKI}/seal.key\", client_ca = \"${PKI}/ca.pem\" }" \
    'cert_subject = "CN=runtara-server"'
start_seal "${TEST_SEAL_PORT}"
SEAL_ENV=(RUNTARA_SEAL_MODE=mtls "RUNTARA_SEAL_ADDRESS=127.0.0.1:${TEST_SEAL_PORT}" "RUNTARA_SEAL_TLS_DIR=${PKI}/server")
start_server
[ "$(seal_mode_logged mtls)" -ge 1 ] || { print_error "the server does not say seal is reached over mutual TLS"; exit 1; }
[ "$(schemas_status "${CONN}")" = "200" ] || { print_error "the connection does not work through seal over mutual TLS"; exit 1; }
stop_server
SEAL_ENV=(RUNTARA_SEAL_MODE=mtls "RUNTARA_SEAL_ADDRESS=127.0.0.1:${TEST_SEAL_PORT}" "RUNTARA_SEAL_TLS_DIR=${PKI}/stranger")
SAID=$(refused_start) || exit 1
echo "${SAID}" | grep -q "cannot be reached, or does not take this server's calls" \
    || { print_error "a server with a certificate seal does not know said: $(echo "${SAID}" | tail -2)"; exit 1; }
echo "  the connection works by address and certificate; another certificate from the same CA is refused ✓"

#-------------------------------------------------------------------------
print_step "6. A tenant that leaves has its keys destroyed, by seal..."
SEAL_ENV=(RUNTARA_SEAL_MODE=mtls "RUNTARA_SEAL_ADDRESS=127.0.0.1:${TEST_SEAL_PORT}" "RUNTARA_SEAL_TLS_DIR=${PKI}/server")
(server_env SKIP_MIGRATIONS=true "${RUNTARA_SERVER_BIN}" destroy-seal-keys \
    --tenant-id "${TENANT}" --confirm "${TENANT}") >/dev/null 2>&1 \
    || { print_error "the tenant's keys were not destroyed"; exit 1; }
[ "$(find "${SEAL_DIR}/keys" -name '*.json' | wc -l | tr -d ' ')" = "0" ] \
    || { print_error "seal's key directory still holds the tenant's key"; exit 1; }
[ "$(audit_count destroy_keyset)" -ge 1 ] || { print_error "seal's audit log has no record of the destruction"; exit 1; }
start_server
case "$(schemas_status "${CONN}")" in
    2??) print_error "a connection of the tenant that left still works"; exit 1 ;;
esac
echo "  seal deleted the key and recorded it; the tenant's connection opens no more ✓"

print_success "seal as a process of its own: keys handed over, used on a socket and over mutual TLS, and destroyed, with the server holding none"
