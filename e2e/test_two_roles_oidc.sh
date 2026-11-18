#!/bin/bash
# E2E Test: the engine verifies OIDC tokens and membership itself
# (docs/separating-concerns.md, Phase 4; docs/roles.md).
#
# The server runs as two processes (RUNTARA_ROLE): an engine with a seal of
# its own, and an API in front of it. Both sign users in with OIDC (a mock
# JWKS) and require membership (RUNTARA_AUTH_MEMBERSHIP_POLICY=required),
# with one audience for the API and another for MCP.
#
#   1. THROUGH THE API  an owner's token saves, compiles and runs a workflow:
#                       every call the API makes for it, the engine verifies
#                       again and answers.
#   2. NOT ON THE API'S WORD  with the API's certificate and no API: nothing
#                       presented, a token that is not one, one signed by
#                       another key, an expired one and one whose tenant
#                       this server does not serve are each refused.
#   3. MEMBERSHIP       the engine reads who is a member itself: a signed-in
#                       user who is not one is refused, a revoked token is
#                       refused, and a member who is removed is refused from
#                       then on.
#   4. ROLE             a viewer's token reads a workflow's runs and is
#                       refused a run, by the engine as by the API.
#   5. WHICH WAY IN     a token for the API's audience is not one for MCP's:
#                       the engine verifies it for the way in it came by.
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
# The engine needs a credential encryption key; a throwaway one per run,
# kept out of this script's own environment so only the engine is given it.
# shellcheck source=../scripts/credential-key.sh
source "${PROJECT_ROOT}/scripts/credential-key.sh"
ENGINE_KEY="$(generate_credential_key)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-${PGPASSWORD:-}}"

TEST_DB_SERVER="two_roles_oidc_e2e_server_$$"
TEST_DB_RUNTIME="two_roles_oidc_e2e_runtime_$$"
API_PORT="${TEST_PORT_PUBLIC:-17870}"
ENGINE_PORT="${TEST_ENGINE_PORT:-17875}"
ENGINE_PUBLIC_PORT="${TEST_ENGINE_PUBLIC_PORT:-17876}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18871}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18872}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18873}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18874}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16399}"
MOCK_PORT="${MOCK_PORT:-17879}"
TENANT="two_roles_oidc_e2e"
ISSUER="http://127.0.0.1:${MOCK_PORT}/"
KID="two-roles-oidc-e2e"
API_AUDIENCE="runtara-api"
MCP_AUDIENCE="runtara-mcp"

TARGET_DIR="${CARGO_TARGET_DIR:-${PROJECT_ROOT}/target}"
RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${TARGET_DIR}/debug/runtara-server}"
EXECUTOR_BIN="${TARGET_DIR}/debug/runtara-executor"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

WORK_DIR="$(mktemp -d -t runtara_two_roles_oidc_e2e_XXXXXX)"
# A socket's path is short.
SEAL_DIR="$(mktemp -d /tmp/rseal.XXXXXX)"
ENGINE_LOG="${WORK_DIR}/engine.log"
API_LOG="${WORK_DIR}/api.log"
ENGINE_PID=""; API_PID=""; SEAL_PID=""; MOCK_PID=""
VALKEY_CONTAINER=""
API="http://127.0.0.1:${API_PORT}/api/runtime"

SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"

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
    for pid in "${API_PID}" "${ENGINE_PID}" "${SEAL_PID}" "${MOCK_PID}"; do
        [ -z "${pid}" ] || kill "${pid}" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    [ -z "${VALKEY_CONTAINER}" ] || docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER} WITH (FORCE)" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME} WITH (FORCE)" >/dev/null 2>&1 || true
    if [ ${code} -ne 0 ]; then
        for log in "${API_LOG}" "${ENGINE_LOG}" "${WORK_DIR}/seal.log"; do
            [ -f "${log}" ] && { echo "--- ${log##*/} (tail)" >&2; tail -30 "${log}" >&2; }
        done
    fi
    # The certificates' keys, the signing key and seal's never outlive the test.
    rm -rf "${WORK_DIR}" "${SEAL_DIR}"
    exit ${code}
}
trap cleanup EXIT

valkey() { docker exec "${VALKEY_CONTAINER}" valkey-cli --raw "$@"; }
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }

# token SUB JTI [AUDIENCE] [TENANT] [LIFETIME SECONDS] [KEY FILE]: a signed
# OIDC token.
token() {
    local sub="$1" jti="$2" aud="${3:-${API_AUDIENCE}}" tenant="${4:-${TENANT}}" life="${5:-3600}"
    local key="${6:-${WORK_DIR}/key.pem}" now header claims signing sig
    now=$(date +%s)
    header='{"alg":"RS256","typ":"JWT","kid":"'"${KID}"'"}'
    claims='{"iss":"'"${ISSUER}"'","aud":"'"${aud}"'","sub":"'"${sub}"'","org_id":"'"${tenant}"'","jti":"'"${jti}"'","iat":'"$((now - 7200))"',"exp":'"$((now + life))"'}'
    signing="$(printf '%s' "${header}" | b64url).$(printf '%s' "${claims}" | b64url)"
    sig=$(printf '%s' "${signing}" | openssl dgst -sha256 -sign "${key}" -binary | b64url)
    printf '%s.%s' "${signing}" "${sig}"
}
member() {  # member SUB ROLE: makes SUB a member of the tenant
    valkey SET "${TENANT}:member:$1" "{\"role\":\"$2\",\"updated_at\":\"2026-11-18T00:00:00Z\"}" >/dev/null
}

# call TOKEN METHOD PATH [BODY]: through the API.
call() {
    curl -sS --max-time 900 -X "$2" -H "Authorization: Bearer $1" -H "Content-Type: application/json" \
        ${4:+-d "$4"} "${API}$3"
}
status_of() {  # status_of TOKEN METHOD PATH [BODY]
    curl -sS -o /dev/null -w '%{http_code}' --max-time 120 -X "$2" -H "Authorization: Bearer $1" \
        -H "Content-Type: application/json" ${4:+-d "$4"} "${API}$3"
}

# What a caller presented, as the API sends it to the engine.
# presented DOOR [TOKEN]
presented() {
    python3 - "$@" <<'PY'
import base64, json, sys
headers = [["authorization", f"Bearer {sys.argv[2]}"]] if len(sys.argv) > 2 else []
print(base64.b64encode(json.dumps(
    {"door": sys.argv[1], "headers": headers, "requested_tenant": None, "as_a_member": True}).encode()).decode())
PY
}
# ask_engine OPERATION BODY [PRESENTED]: the status the engine answers to
# whoever holds the API's certificate.
ask_engine() {
    curl -sS -o "${WORK_DIR}/engine_body" -w '%{http_code}' --max-time 30 \
        --cacert "${WORK_DIR}/pki-api/ca.pem" \
        --cert "${WORK_DIR}/pki-api/tls.pem" --key "${WORK_DIR}/pki-api/tls.key" \
        --resolve "runtara-server:${ENGINE_PORT}:127.0.0.1" \
        -X POST -H 'Content-Type: application/json' ${3:+-H "x-runtara-presented: $3"} -d "$2" \
        "https://runtara-server:${ENGINE_PORT}/engine/v1/operations/$1"
}
# expect STATUS OPERATION BODY PRESENTED WHO
expect() {
    local code
    code=$(ask_engine "$2" "$3" "$4")
    [ "${code}" = "$1" ] || { print_error "the engine answered $2 to $5 with ${code}, not $1: $(head -c 300 "${WORK_DIR}/engine_body")"; exit 1; }
}

echo "==============================================================="
echo "E2E Test: the engine verifies OIDC tokens and membership itself"
echo "==============================================================="

[ -n "${POSTGRES_PASSWORD}" ] \
    || { print_error "set POSTGRES_PASSWORD (or PGPASSWORD) for ${POSTGRES_USER}@${POSTGRES_HOST}"; exit 1; }
if [ ! -x "${RUNTARA_SERVER_BIN}" ] || [ ! -x "${EXECUTOR_BIN}" ]; then
    print_step "Building runtara-server and runtara-executor (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server -p runtara-executor >&2
fi
[ -f "${COMPONENTS_DIR}/runtara_workflow_stdlib.wasm" ] \
    || { print_error "Missing ${COMPONENTS_DIR}/runtara_workflow_stdlib.wasm — run scripts/build-agent-components.sh"; exit 1; }

print_step "Starting an isolated Valkey, a mock JWKS, the databases and seal..."
docker info >/dev/null 2>&1 || { print_error "docker is required"; exit 1; }
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "${WORK_DIR}/key.pem" 2>/dev/null
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "${WORK_DIR}/other-key.pem" 2>/dev/null
RSA_N=$(openssl rsa -in "${WORK_DIR}/key.pem" -noout -modulus 2>/dev/null | cut -d= -f2 \
    | python3 -c 'import sys,base64; print(base64.urlsafe_b64encode(bytes.fromhex(sys.stdin.read().strip())).decode().rstrip("="))')
cat > "${WORK_DIR}/jwks.py" <<'PY'
import json, os, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
BODY = json.dumps({"keys": [{"kty": "RSA", "use": "sig", "alg": "RS256",
    "kid": os.environ["KID"], "n": os.environ["RSA_N"], "e": "AQAB"}]}).encode()
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(BODY)))
        self.end_headers()
        self.wfile.write(BODY)
ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
PY
KID="${KID}" RSA_N="${RSA_N}" python3 "${WORK_DIR}/jwks.py" "${MOCK_PORT}" >/dev/null 2>&1 &
MOCK_PID=$!
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

"${EXECUTOR_BIN}" init-pki --server-dir "${WORK_DIR}/pki-engine" \
    --executor-dir "${WORK_DIR}/pki-executor" --api-dir "${WORK_DIR}/pki-api" >/dev/null
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
honour_private_rules = true

[audit]
sink = { file = "${SEAL_DIR}/audit.jsonl" }
TOML
chmod 600 "${SEAL_DIR}/seal.toml"
"${SEAL_BIN}" serve --config "${SEAL_DIR}/seal.toml" >>"${WORK_DIR}/seal.log" 2>&1 &
SEAL_PID=$!
for _ in {1..40}; do
    kill -0 "${SEAL_PID}" 2>/dev/null || { print_error "seal exited as it started"; exit 1; }
    [ -S "${SEAL_DIR}/seal.sock" ] && break
    sleep 0.25
done
[ -S "${SEAL_DIR}/seal.sock" ] || { print_error "seal did not listen"; exit 1; }

# How both processes sign users in. They are given the same.
SIGN_IN=(
    AUTH_PROVIDER=oidc
    "OAUTH2_ISSUER=${ISSUER}"
    "OAUTH2_JWKS_URI=http://127.0.0.1:${MOCK_PORT}/keys"
    "OAUTH2_AUDIENCE=${API_AUDIENCE}"
    "OAUTH2_MCP_AUDIENCE=${MCP_AUDIENCE}"
    RUNTARA_AUTH_MEMBERSHIP_POLICY=required
    RUNTARA_AUTH_LEGACY_MEMBER_KEYS=false
    VALKEY_HOST=127.0.0.1
    "VALKEY_PORT=${TEST_VALKEY_PORT}"
    "TENANT_ID=${TENANT}"
    OTEL_SDK_DISABLED=true
    RUNTARA_DEV_MODE=false
)

print_step "Starting the engine and the API..."
(exec env "${SIGN_IN[@]}" \
    RUNTARA_ROLE=engine \
    RUNTARA_ENGINE_LISTEN="127.0.0.1:${ENGINE_PORT}" \
    RUNTARA_ENGINE_TLS_DIR="${WORK_DIR}/pki-engine" \
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
    RUNTARA_DATABASE_URL="${RUNTIME_DB_URL}" \
    RUNTARA_CONNECTIONS_ENCRYPTION_KEY="${ENGINE_KEY}" \
    RUNTARA_SEAL_MODE=unix \
    RUNTARA_SEAL_SOCKET="${SEAL_DIR}/seal.sock" \
    SERVER_HOST=127.0.0.1 \
    SERVER_PORT="${ENGINE_PUBLIC_PORT}" \
    RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
    RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
    RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
    RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
    RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
    DATA_DIR="${WORK_DIR}/data" \
    RUNTARA_DATABASE_CONNECTION_GUARD=off \
    RUST_LOG="warn,runtara_server=info" \
    SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}") >>"${ENGINE_LOG}" 2>&1 &
ENGINE_PID=$!
for _ in {1..120}; do
    grep -q "engine is ready" "${ENGINE_LOG}" 2>/dev/null && break
    kill -0 "${ENGINE_PID}" 2>/dev/null || { print_error "the engine exited during boot"; exit 1; }
    sleep 1
done
grep -q "engine is ready" "${ENGINE_LOG}" || { print_error "the engine did not become ready"; exit 1; }

# The whole of the API's environment: nothing is inherited.
(cd "${WORK_DIR}" && exec env -i "PATH=${PATH}" "HOME=${HOME}" "${SIGN_IN[@]}" \
    RUNTARA_ROLE=api \
    "RUNTARA_ENGINE_URL=https://127.0.0.1:${ENGINE_PORT}" \
    "RUNTARA_ENGINE_TLS_DIR=${WORK_DIR}/pki-api" \
    "RUNTARA_SERVER_DATABASE_URL=${SERVER_DB_URL}" \
    "RUNTARA_AGENT_COMPONENTS_DIR=${WORK_DIR}/agent-descriptions" \
    SERVER_HOST=127.0.0.1 "SERVER_PORT=${API_PORT}" \
    "RUST_LOG=warn,runtara_server=info" \
    SKIP_MIGRATIONS=true \
    "${RUNTARA_SERVER_BIN}") >>"${API_LOG}" 2>&1 &
API_PID=$!
for _ in {1..60}; do
    [ "$(curl -sS -o /dev/null -w '%{http_code}' "http://127.0.0.1:${API_PORT}/health" 2>/dev/null || true)" = "200" ] && break
    kill -0 "${API_PID}" 2>/dev/null || { print_error "the API exited during boot"; exit 1; }
    sleep 1
done

#-------------------------------------------------------------------------
print_step "1. An owner's token works through the API, verified again by the engine..."
member owner-1 owner
OWNER=$(token owner-1 jti-owner-1)
GRAPH='{"name":"Answer","steps":{"finish":{"stepType":"Finish","id":"finish","inputMapping":{"answer":{"valueType":"immediate","value":42}}}},"entryPoint":"finish","executionPlan":[],"variables":{},"inputSchema":{},"outputSchema":{}}'
RESP=$(call "${OWNER}" POST /workflows/create '{"name":"Answer","description":"two roles oidc"}')
WF=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${WF}" ] || { print_error "create failed: ${RESP}"; exit 1; }
RESP=$(call "${OWNER}" POST "/workflows/${WF}/update" "{\"executionGraph\": ${GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "update failed: ${RESP}"; exit 1; }
VERSION=$(call "${OWNER}" GET "/workflows/${WF}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(call "${OWNER}" POST "/workflows/${WF}/versions/${VERSION}/compile" '{}')
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "compile failed: ${RESP}"; exit 1; }
RESP=$(call "${OWNER}" POST "/workflows/${WF}/execute" '{"inputs": {"data": {}}}')
INSTANCE=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${INSTANCE}" ] || { print_error "execute failed: ${RESP}"; exit 1; }
STATUS=""
for _ in {1..240}; do
    STATUS=$(call "${OWNER}" GET "/workflows/instances/${INSTANCE}" | jq -r '.data.status // empty')
    case "${STATUS}" in completed|failed|crashed|stopped|cancelled) break ;; esac
    sleep 0.5
done
[ "${STATUS}" = "completed" ] || { print_error "the run ended '${STATUS}'"; exit 1; }
# A connection is a part of the API the engine serves: to the caller it found.
[ "$(status_of "${OWNER}" GET /connections)" = "200" ] \
    || { print_error "the owner was refused the connections the engine serves"; exit 1; }
echo "  saved, compiled, run to completion and read back with a token; connections served to its caller ✓"

LIST="{\"tenant_id\": \"${TENANT}\", \"workflow_id\": \"${WF}\", \"page\": 0, \"size\": 5}"
QUEUE="{\"run_label\": null, \"tenant_id\": \"${TENANT}\", \"workflow_id\": \"${WF}\", \"version\": null, \"inputs\": {\"data\": {}}, \"debug\": false, \"correlation_id\": null, \"idempotency_key\": null, \"trigger_source\": \"HttpApi\", \"instance_id\": null}"
runs() { psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT count(*) FROM instances WHERE tenant_id = '${TENANT}'"; }

#-------------------------------------------------------------------------
print_step "2. The API's certificate does not say who is calling..."
RUNS_BEFORE=$(runs)
expect 200 ListExecutions "${LIST}" "$(presented api "${OWNER}")" "the owner"
CODE=$(ask_engine ListExecutions "${LIST}")
[ "${CODE}" = "401" ] || { print_error "the engine answered with nothing presented: ${CODE}"; exit 1; }
for operation in ListExecutions Queue; do
    BODY="${LIST}"; [ "${operation}" = Queue ] && BODY="${QUEUE}"
    expect 401 "${operation}" "${BODY}" "$(presented api)" "a caller with no token"
    expect 401 "${operation}" "${BODY}" "$(presented api "not.a.token")" "a token that is not one"
    expect 401 "${operation}" "${BODY}" "$(presented api "$(token owner-1 jti-forged "${API_AUDIENCE}" "${TENANT}" 3600 "${WORK_DIR}/other-key.pem")")" \
        "a token signed by another key"
    expect 401 "${operation}" "${BODY}" "$(presented api "$(token owner-1 jti-old "${API_AUDIENCE}" "${TENANT}" -3600)")" "an expired token"
    expect 403 "${operation}" "${BODY}" "$(presented api "$(token owner-1 jti-other "${API_AUDIENCE}" "another_tenant")")" \
        "a token for a tenant this server does not serve"
done
[ "$(runs)" = "${RUNS_BEFORE}" ] || { print_error "a refused operation started a run"; exit 1; }
echo "  nothing presented, a token that is not one, another key's, an expired one: 401; another tenant's: 403; no run started ✓"

#-------------------------------------------------------------------------
print_step "3. The engine reads who is a member itself..."
STRANGER=$(token stranger-1 jti-stranger-1)
expect 403 ListExecutions "${LIST}" "$(presented api "${STRANGER}")" "a signed-in user who is not a member"
[ "$(status_of "${STRANGER}" GET "/workflows/${WF}/instances")" = "403" ] \
    || { print_error "the API answered a user who is not a member"; exit 1; }
REVOKED=$(token owner-1 jti-revoked)
expect 200 ListExecutions "${LIST}" "$(presented api "${REVOKED}")" "the owner's second token"
valkey SET "${TENANT}:token:revoked:jti-revoked" '{"revoked_at":"2026-11-18T00:00:00Z"}' >/dev/null
expect 401 ListExecutions "${LIST}" "$(presented api "${REVOKED}")" "a revoked token"
expect 200 ListExecutions "${LIST}" "$(presented api "${OWNER}")" "the owner's other token"
member leaver-1 member
LEAVER=$(token leaver-1 jti-leaver-1)
expect 200 Queue "${QUEUE}" "$(presented api "${LEAVER}")" "a member"
valkey DEL "${TENANT}:member:leaver-1" >/dev/null
RUNS_BEFORE=$(runs)
expect 403 Queue "${QUEUE}" "$(presented api "${LEAVER}")" "a member who was removed"
sleep 2
[ "$(runs)" -le "$((RUNS_BEFORE + 1))" ] || { print_error "a removed member's run started"; exit 1; }
echo "  a user who is not a member: 403; a revoked token: 401; a member who is removed: 403 from then on ✓"

#-------------------------------------------------------------------------
print_step "4. A viewer reads and does not run..."
member viewer-1 viewer
VIEWER=$(token viewer-1 jti-viewer-1)
expect 200 ListExecutions "${LIST}" "$(presented api "${VIEWER}")" "a viewer"
expect 403 Queue "${QUEUE}" "$(presented api "${VIEWER}")" "a viewer"
[ "$(status_of "${VIEWER}" GET "/workflows/${WF}/instances")" = "200" ] \
    || { print_error "the API refused a viewer a workflow's runs"; exit 1; }
[ "$(status_of "${VIEWER}" POST "/workflows/${WF}/execute" '{"inputs": {"data": {}}}')" = "403" ] \
    || { print_error "the API let a viewer run a workflow"; exit 1; }
echo "  a viewer's token reads a workflow's runs and is refused a run, by the engine as by the API ✓"

#-------------------------------------------------------------------------
print_step "5. A token is verified for the way in it came by..."
MCP_TOKEN=$(token owner-1 jti-owner-mcp "${MCP_AUDIENCE}")
expect 200 ListExecutions "${LIST}" "$(presented mcp "${MCP_TOKEN}")" "an MCP token by MCP"
expect 401 ListExecutions "${LIST}" "$(presented api "${MCP_TOKEN}")" "an MCP token by the API"
expect 401 ListExecutions "${LIST}" "$(presented mcp "${OWNER}")" "an API token by MCP"
[ "$(status_of "${MCP_TOKEN}" GET "/workflows/${WF}/instances")" = "401" ] \
    || { print_error "the API took a token for MCP's audience"; exit 1; }
echo "  a token for the API's audience is refused by MCP's way in, and one for MCP's by the API's ✓"

echo
print_success "the engine verified every token, its membership and its role itself, and took none on the API's word"
