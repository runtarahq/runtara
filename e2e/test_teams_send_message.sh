#!/bin/bash
# E2E Test: Microsoft Teams `send-message` capability, end to end, through the
# REAL credential proxy — against a MOCK Bot Connector and MOCK Azure token
# endpoint (no real Teams tenant, no real app registration).
#
# Proves the full outbound path the MVP ships:
#   workflow teams.send-message step
#     → runtara-agent-teams (relative Bot Connector path + X-Runtara-Endpoint-Ref
#       + X-Runtara-Connection-Id, percent-encoded conversation id)
#     → internal proxy:
#         · resolve_connection_auth (teams_bot arm) mints the Bot Connector
#           token from the MOCK Azure token endpoint via the shared token cache
#         · apply_endpoint_ref_override verifies the signed ref, enforces
#           tenant + connection match and the conversation-id-in-path check,
#           and pins base_url to the ref's serviceUrl (the mock Bot Connector)
#         · pin_url_to_base joins the relative path UNDER the serviceUrl
#         · hardened client POSTs, injecting Authorization: Bearer <token>
#     → MOCK Bot Connector returns ResourceResponse {"id": ...}
#
# Assertions:
#   1. A valid ref → workflow completes; the mock Bot Connector received a POST
#      to /v3/conversations/<encoded-conv>/activities carrying the minted bearer
#      token, and the step output carries the returned activity id.
#   2. A ref for a DIFFERENT connection → proxy fail-closes (nothing reaches the
#      mock; the run does not report a successful send).
#   3. A ref whose conversation id ≠ the request path → proxy fail-closes.
#
# The signed endpoint ref is minted in-test with the same HMAC secret the server
# is given (RUNTARA_ENDPOINT_REF_SECRET), matching api::services::endpoint_ref's
# wire format exactly.
#
# Prerequisites: Postgres + docker (isolated Valkey), python3, and the agent /
# shared workflow components in target/wasm32-wasip2/release
# (scripts/build-agent-components.sh — must include runtara_agent_teams.wasm).

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"

TEST_DB_SERVER="teams_e2e_server_$$"
TEST_DB_RUNTIME="teams_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17720}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17721}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18721}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18722}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18723}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18724}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16392}"
MOCK_PORT="${MOCK_PORT:-17725}"
TEST_DATA_DIR="$(mktemp -d -t runtara_teams_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
MOCK_LOG="${TEST_DATA_DIR}/mock.log"
MOCK_RECORD="${TEST_DATA_DIR}/botconnector_requests.jsonl"
SERVER_PID=""
MOCK_PID=""
VALKEY_CONTAINER=""
TENANT="teams_e2e"
ENDPOINT_REF_SECRET="teams-e2e-endpoint-ref-secret-$$"
MOCK_BASE="http://127.0.0.1:${MOCK_PORT}"
CONV_ID='19:abcDEF@thread.tacv2;messageid=1700000000001'
# Exported for the python heredocs that mint refs / build payloads.
export TENANT ENDPOINT_REF_SECRET MOCK_BASE

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql \
        -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" \
        -tA "$@"
}

cleanup() {
    [ -n "${SERVER_PID}" ] && kill "${SERVER_PID}" 2>/dev/null && wait "${SERVER_PID}" 2>/dev/null || true
    [ -n "${MOCK_PID}" ] && kill "${MOCK_PID}" 2>/dev/null && wait "${MOCK_PID}" 2>/dev/null || true
    [ -n "${VALKEY_CONTAINER}" ] && docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME}" >/dev/null 2>&1 || true
    rm -rf "${TEST_DATA_DIR}" 2>/dev/null || true
}
trap cleanup EXIT

API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" \
        -d "$2" "${API}$1"
}

# Mint a signed endpoint ref matching api::services::endpoint_ref's wire format:
#   base64url(kid).base64url(compact-json).base64url(hmac_sha256(kid.payload))
# Field order MUST match the EndpointBinding struct (serde declaration order).
mint_ref() {
    local connection_id="$1" base_url="$2" conversation_id="$3"
    python3 - "$connection_id" "$base_url" "$conversation_id" <<PY
import base64, hashlib, hmac, json, sys, os
connection_id, base_url, conversation_id = sys.argv[1], sys.argv[2], sys.argv[3]
secret = os.environ["ENDPOINT_REF_SECRET"].encode()
tenant = os.environ["TENANT"]
kid = "1"
# Exact serde field order, compact separators, no escaped slashes (matches serde_json).
payload = (
    '{"v":1,'
    f'"tenant_id":{json.dumps(tenant)},'
    f'"connection_id":{json.dumps(connection_id)},'
    f'"base_url":{json.dumps(base_url)},'
    f'"conversation_id":{json.dumps(conversation_id)},'
    '"conversation_type":"channel",'
    f'"ms_tenant_id":{json.dumps(tenant)},'
    '"iat":1700000000}'
)
def b64(b): return base64.urlsafe_b64encode(b).rstrip(b"=").decode()
enc_payload = b64(payload.encode())
signing_input = f"{kid}.{enc_payload}"
sig = hmac.new(secret, signing_input.encode(), hashlib.sha256).digest()
print(f"{signing_input}.{b64(sig)}")
PY
}

echo "==============================================================="
echo "E2E Test: Teams send-message via the real proxy (mock connector)"
echo "==============================================================="

if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi
for f in runtara_agent_teams.wasm runtara_workflow_stdlib.wasm runtara_workflow_runtime.wasm; do
    if [ ! -f "${COMPONENTS_DIR}/${f}" ]; then
        print_error "Missing component ${COMPONENTS_DIR}/${f} — run scripts/build-agent-components.sh"
        exit 1
    fi
done

print_step "Pre-flight: Postgres, docker, python3..."
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }
command -v python3 >/dev/null 2>&1 || { print_error "python3 required (mock server)"; exit 1; }

# --- Mock server: Azure token endpoint + Bot Connector --------------------
print_step "Starting mock Bot Connector + token endpoint on :${MOCK_PORT}..."
cat > "${TEST_DATA_DIR}/mock.py" <<'PY'
import json, os, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

RECORD = os.environ["MOCK_RECORD"]
BOT_TOKEN = "MOCK_BOT_TOKEN"

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _send(self, code, body):
        b = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def do_POST(self):
        length = int(self.headers.get("Content-Length", 0))
        _ = self.rfile.read(length) if length else b""
        path = self.path
        if path.endswith("/oauth2/v2.0/token"):
            # Azure AD client-credentials token mint.
            self._send(200, {"token_type": "Bearer", "expires_in": 3600,
                             "ext_expires_in": 3600, "access_token": BOT_TOKEN})
            return
        if "/v3/conversations/" in path and path.endswith("/activities"):
            with open(RECORD, "a") as f:
                f.write(json.dumps({
                    "path": path,
                    "authorization": self.headers.get("Authorization", ""),
                }) + "\n")
            self._send(201, {"id": "mock-activity-123"})
            return
        self._send(404, {"error": "not found", "path": path})

port = int(sys.argv[1])
ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()
PY
MOCK_RECORD="${MOCK_RECORD}" python3 "${TEST_DATA_DIR}/mock.py" "${MOCK_PORT}" >"${MOCK_LOG}" 2>&1 &
MOCK_PID=$!
for _ in {1..20}; do
    if (echo > "/dev/tcp/127.0.0.1/${MOCK_PORT}") 2>/dev/null; then break; fi
    sleep 0.3
done

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

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
TENANT_ID="${TENANT}" \
SERVER_HOST=127.0.0.1 \
SERVER_PORT="${TEST_PORT_PUBLIC}" \
INTERNAL_PORT="${TEST_PORT_INTERNAL}" \
RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
DATA_DIR="${TEST_DATA_DIR}" \
RUST_LOG="warn,runtara_server=info" \
AUTH_PROVIDER=local \
SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2 \
RUNTARA_ENDPOINT_REF_SECRET="${ENDPOINT_REF_SECRET}" \
RUNTARA_CONNECTION_SERVICE_URL="http://127.0.0.1:${TEST_PORT_INTERNAL}/api/connections" \
RUNTARA_PROXY_ALLOWED_HOSTS=127.0.0.1 \
RUNTARA_PROXY_ALLOW_HTTP_HOSTS=127.0.0.1 \
VALKEY_HOST=127.0.0.1 \
VALKEY_PORT="${TEST_VALKEY_PORT}" \
OTEL_SDK_DISABLED=true \
RUNTARA_SDK_BACKEND=http \
SQLX_OFFLINE="${SQLX_OFFLINE}" \
"${RUNTARA_SERVER_BIN}" >"${TEST_LOG}" 2>&1 &
SERVER_PID=$!

for _ in {1..60}; do
    if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
        break
    fi
    sleep 1
    if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
        print_error "Server exited during boot."; tail -30 "${TEST_LOG}"; exit 1
    fi
done
echo "  Server up (PID ${SERVER_PID})"

# --- Create the teams_bot connection --------------------------------------
print_step "Creating teams_bot connection (authority_host = mock)..."
CONN_PAYLOAD=$(python3 - <<PY
import json, os
print(json.dumps({
  "title": "teams-e2e",
  "integrationId": "teams_bot",
  "connectionParameters": {
    "app_id": "00000000-0000-0000-0000-000000000001",
    "app_password": "mock-app-secret",
    "azure_tenant_id": "11111111-1111-1111-1111-111111111111",
    "app_type": "single_tenant",
    "authority_host": os.environ["MOCK_BASE"],
  },
}))
PY
)
RESP=$(api_post /connections "${CONN_PAYLOAD}")
CONN_ID=$(echo "${RESP}" | jq -r '.connectionId // .data.id // .id // empty')
if [ -z "${CONN_ID}" ]; then
    print_error "Connection create failed: ${RESP}"; tail -30 "${TEST_LOG}"; exit 1
fi
echo "  Connection ${CONN_ID} ✓"

# --- Create + compile the send-message workflow ---------------------------
print_step "Creating teams.send-message workflow..."
RESP=$(api_post /workflows/create '{"name":"teams-send-e2e","description":"teams send-message"}')
WF_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -z "${WF_ID}" ] && { print_error "Workflow create failed: ${RESP}"; exit 1; }

GRAPH=$(python3 - "${CONN_ID}" <<'PY'
import json, sys
conn = sys.argv[1]
print(json.dumps({
  "name": "teams-send-e2e",
  "steps": {
    "send": {
      "stepType": "Agent", "id": "send", "agentId": "teams", "capabilityId": "send-message",
      "connectionId": conn,
      "inputMapping": {
        "target": {"valueType": "reference", "value": "data.target"},
        "conversation_id": {"valueType": "reference", "value": "data.conversationId"},
        "text": {"valueType": "reference", "value": "data.text"},
      },
    },
    "finish": {"stepType": "Finish", "id": "finish",
      "inputMapping": {"result": {"valueType": "reference", "value": "steps.send.outputs"}}},
  },
  "entryPoint": "send",
  "executionPlan": [{"fromStep": "send", "toStep": "finish"}],
  "variables": {},
  "inputSchema": {
    "target": {"type": "string", "required": True},
    "conversationId": {"type": "string", "required": True},
    "text": {"type": "string", "required": True},
  },
  "outputSchema": {},
}))
PY
)
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${GRAPH}}")
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "Update/validate failed: ${RESP}"; exit 1
fi
VERSION=$(curl -sS "${API}/workflows/${WF_ID}/versions" \
    | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(api_post "/workflows/${WF_ID}/versions/${VERSION}/compile" '{}' 900)
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "Compile failed: ${RESP}"; tail -40 "${TEST_LOG}"; exit 1
fi
echo "  Workflow ${WF_ID} compiled ✓"

# Run the workflow and return the terminal status + error into globals.
run_workflow() {
    local target="$1" conversation_id="$2"
    local inputs
    inputs=$(python3 - "$target" "$conversation_id" <<'PY'
import json, sys
print(json.dumps({"inputs": {"data": {
    "target": sys.argv[1], "conversationId": sys.argv[2], "text": "hello from e2e"}}}))
PY
)
    local resp instance
    resp=$(api_post "/workflows/${WF_ID}/execute" "${inputs}")
    instance=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    [ -z "${instance}" ] && { print_error "Execute failed: ${resp}"; exit 1; }
    RUN_STATUS=""; RUN_RESP=""
    for _ in {1..90}; do
        RUN_RESP=$(curl -sS "${API}/workflows/instances/${instance}")
        RUN_STATUS=$(echo "${RUN_RESP}" | jq -r '.data.status // .status // empty')
        case "${RUN_STATUS}" in completed|failed|crashed|stopped) break ;; esac
        sleep 2
    done
}

# --- Assertion 1: valid ref → send reaches the mock Bot Connector ---------
print_step "Case 1: valid ref → send-message reaches the Bot Connector..."
: > "${MOCK_RECORD}"
REF_VALID=$(mint_ref "${CONN_ID}" "${MOCK_BASE}" "${CONV_ID}")
run_workflow "${REF_VALID}" "${CONV_ID}"
if [ "${RUN_STATUS}" != "completed" ]; then
    print_error "Expected completed, got '${RUN_STATUS}'"
    echo "${RUN_RESP}" | jq -r '.data.error // .error // empty'
    tail -60 "${TEST_LOG}"; exit 1
fi
# The mock recorded exactly one POST with the minted bearer token and the
# percent-encoded conversation id in the path.
RECORDED=$(cat "${MOCK_RECORD}" 2>/dev/null || true)
if [ -z "${RECORDED}" ]; then
    print_error "Bot Connector received no request"; tail -60 "${TEST_LOG}"; exit 1
fi
REC_AUTH=$(echo "${RECORDED}" | jq -r '.authorization' | head -1)
REC_PATH=$(echo "${RECORDED}" | jq -r '.path' | head -1)
if [ "${REC_AUTH}" != "Bearer MOCK_BOT_TOKEN" ]; then
    print_error "Bot Connector auth header wrong: '${REC_AUTH}'"; exit 1
fi
case "${REC_PATH}" in
    /v3/conversations/19%3AabcDEF*/activities) : ;;
    *) print_error "Bot Connector path not percent-encoded/joined as expected: ${REC_PATH}"; exit 1 ;;
esac
# The returned Bot Connector activity id (mock-activity-123) surfaces somewhere
# in the instance output; find it recursively so the assertion is robust to the
# exact output-envelope shape.
ACTIVITY_ID=$(echo "${RUN_RESP}" | jq -r '[.. | .activity_id? // empty] | map(select(. != null)) | first // empty')
if [ "${ACTIVITY_ID}" != "mock-activity-123" ]; then
    print_error "Expected returned activity id 'mock-activity-123', got '${ACTIVITY_ID:-<none>}'"
    echo "${RUN_RESP}" | jq '.data // .' | head -40
    exit 1
fi
echo "  Send reached Bot Connector: path=${REC_PATH}, auth ok, activityId=${ACTIVITY_ID} ✓"

# --- Assertion 2: ref for a DIFFERENT connection → fail closed ------------
print_step "Case 2: ref bound to a different connection → proxy fail-closes..."
: > "${MOCK_RECORD}"
REF_FOREIGN=$(mint_ref "some-other-connection" "${MOCK_BASE}" "${CONV_ID}")
run_workflow "${REF_FOREIGN}" "${CONV_ID}"
if [ "${RUN_STATUS}" = "completed" ]; then
    print_error "Foreign-connection ref must NOT succeed"; exit 1
fi
if [ -s "${MOCK_RECORD}" ]; then
    print_error "Foreign-connection ref reached the Bot Connector (should be blocked)"; exit 1
fi
echo "  Foreign-connection ref blocked before egress (status=${RUN_STATUS}) ✓"

# --- Assertion 3: ref conversation id ≠ request path → fail closed --------
print_step "Case 3: ref conversation id ≠ request path → proxy fail-closes..."
: > "${MOCK_RECORD}"
REF_OTHERCONV=$(mint_ref "${CONN_ID}" "${MOCK_BASE}" "19:DIFFERENT@thread.tacv2")
run_workflow "${REF_OTHERCONV}" "${CONV_ID}"
if [ "${RUN_STATUS}" = "completed" ]; then
    print_error "Conversation-id mismatch must NOT succeed"; exit 1
fi
if [ -s "${MOCK_RECORD}" ]; then
    print_error "Conversation-mismatch ref reached the Bot Connector (should be blocked)"; exit 1
fi
echo "  Conversation-id mismatch blocked before egress (status=${RUN_STATUS}) ✓"

print_success "Teams send-message: valid ref sends through the real proxy; foreign-connection and conversation-mismatch refs fail closed"
