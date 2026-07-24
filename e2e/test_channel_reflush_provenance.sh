#!/bin/bash
# E2E Test: channel-session re-flush provenance guard
# (docs/channel-session-reflush-provenance-plan.md).
#
# Proves the fix: a DUPLICATE session that lands on a foreign-owned instance
# (a redelivered activity whose Valkey dedup key was lost) does NOT re-dispatch
# the owning session's reply transcript.
#
# Observable: a deterministically-FAILING workflow makes session_loop send the
# "Sorry, something went wrong" text to the channel. That reply egresses through
# the Teams adapter to a MOCK Bot Connector we record. The owning session sends
# it once; a foreign (redelivery) session must send it ZERO times.
#
# Path exercised: inbound Teams webhook (RS256 JWT vs a mock authority) -> ack ->
# reserve_activity (Layer-1 dedup) -> session_loop -> deterministic instance id ->
# queue()/trigger stream -> Environment start-or-attach (the backstop) -> the
# teams.send-message step fails permanently (empty target) -> instance Failed ->
# session_loop terminal branch sends "Sorry" via TeamsChannel -> mock connector.
#
# Cases:
#   A. First delivery of activity A  -> 1 instance, 1 "Sorry" reply (owner).
#   B. RESIDUAL WINDOW: delete the Valkey dedup key, redeliver the SAME activity A
#      -> still 1 instance (Environment dedups the deterministic id) AND still
#      1 "Sorry" reply total (the foreign session SUPPRESSED — the fix).
#   C. A different activity B -> 2 instances, 2 "Sorry" replies (owned sessions
#      always reply; suppression is provenance-scoped, not a blanket drop).
#
# The owner-died-before-flush corner (Layer-1 loss AND the owning session dead)
# is an accepted, documented v1 limitation (see the plan) — not simulated here.
#
# Prereqs: Postgres + docker (isolated Valkey), python3, openssl; the teams +
# shared workflow components in target/wasm32-wasip2/release.

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"

TEST_DB_SERVER="reflush_e2e_server_$$"
TEST_DB_RUNTIME="reflush_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17740}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17741}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18741}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18742}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18743}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18744}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16394}"
MOCK_PORT="${MOCK_PORT:-17745}"
TEST_DATA_DIR="$(mktemp -d -t runtara_reflush_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
MOCK_LOG="${TEST_DATA_DIR}/mock.log"
REPLY_RECORD="${TEST_DATA_DIR}/replies.jsonl"
SERVER_PID=""; MOCK_PID=""; VALKEY_CONTAINER=""
TENANT="reflush_e2e"

APP_ID="11111111-2222-3333-4444-555555555555"
AZURE_TENANT="aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
BF_ISSUER="https://api.botframework.com"
KID="test-kid-1"
CONV_ID='19:reflush-conv@thread.tacv2'
MOCK_BASE="http://127.0.0.1:${MOCK_PORT}"

RSA_N="xnDLCAOM3SV5h0tgactRl97eSimuypTrGuwhTv8lWFTEJ0M4XUrjcUmUosc6nDHxEKnBFun7VuFuWUzYqarjU6WvRCzgh1HXfK9ZzercPUYyeK-Guiu46iRE-RwWW1hY_bUjy_blZMHjgieLpPL64ccXoWgvfE3yQCijhTvTS-VAKx83VoPMcgfhJbg31FMV5c6ElSoxoNThO5JWW4Kwe9YjcHZnvTcXiq89WEuXQKFpJ5iRMVmLql0LAnVNnP67VLa1gqblrcVWRgyQHTvPiVBr5fw2qbubcsuBkBUpUIqsEW8eHDqxHVWSU_IhIfMpIEo6_PnLXFFELXKspEgVtw"
RSA_E="AQAB"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" \
        -p "${POSTGRES_PORT}" -tA "$@"
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
WEBHOOK="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime/events/webhook/teams"
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }
sign_jwt() {
    local key="$1" claims="$2" header b64h b64p signing sig
    header='{"alg":"RS256","typ":"JWT","kid":"'"${KID}"'"}'
    b64h=$(printf '%s' "${header}" | b64url)
    b64p=$(printf '%s' "${claims}" | b64url)
    signing="${b64h}.${b64p}"
    sig=$(printf '%s' "${signing}" | openssl dgst -sha256 -sign "${key}" -binary | b64url)
    printf '%s.%s' "${signing}" "${sig}"
}

echo "==============================================================="
echo "E2E Test: channel-session re-flush provenance guard"
echo "==============================================================="

if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi
for f in runtara_agent_teams.wasm runtara_workflow_stdlib.wasm runtara_workflow_runtime.wasm; do
    [ -f "${COMPONENTS_DIR}/${f}" ] || { print_error "Missing ${COMPONENTS_DIR}/${f} — run scripts/build-agent-components.sh"; exit 1; }
done

print_step "Pre-flight: Postgres, docker, python3, openssl..."
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required"; exit 1; }
command -v python3 >/dev/null 2>&1 || { print_error "python3 required"; exit 1; }
command -v openssl >/dev/null 2>&1 || { print_error "openssl required"; exit 1; }

print_step "Writing test signing key..."
cat > "${TEST_DATA_DIR}/signing_key.pem" <<'PEM'
-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQDGcMsIA4zdJXmH
S2Bpy1GX3t5KKa7KlOsa7CFO/yVYVMQnQzhdSuNxSZSixzqcMfEQqcEW6ftW4W5Z
TNipquNTpa9ELOCHUdd8r1nN6tw9RjJ4r4a6K7jqJET5HBZbWFj9tSPL9uVkweOC
J4uk8vrhxxehaC98TfJAKKOFO9NL5UArHzdWg8xyB+EluDfUUxXlzoSVKjGg1OE7
klZbgrB71iNwdme9NxeKrz1YS5dAoWknmJExWYuqXQsCdU2c/rtUtrWCpuWtxVZG
DJAdO8+JUGvl/Dapu5tyy4GQFSlQiqwRbx4cOrEdVZJT8iEh8ykgSjr8+ctcUUQt
cqykSBW3AgMBAAECggEAIQ3ju+N/gMy/uAoVtrmfzzjX9SmJTIROvy7LA5IbgeGo
xNN9HYkeZp33jL+74w2slnZ4S91QuPGXBHf49RYahLHqBmSlR9UZnFLHFjZDVk+N
k63FNtiWliXReV802CVYuXYFTvHC1yw2vdThfWnd4WLc7E1i74U6T3aVellzQkZT
hl5hSbIJCe/Ss+3ryBbE8mkhv678irPsTAXok3dpOyDbwjEVt7Xf6Pfe6qihL4Fd
MPyF/nfOtSQR23ypUoDullkBZ+5dgdyynz7dyv+zHPKjxiQ8QK75jAvMB5Rn6YzJ
jVpedW8i1o1JzjvFMxiJ9bvoWgotYA5Mx6b+aAAxzQKBgQD7gmDcXFKDe4S1GLXp
eAYu//jBy390yuAONR6BqB4xlarAGCp+PeDJj5U5U751lDtdiy4bpx4pOhXnjc6V
art6X1K9sU0OWjcRSSPC62BwHIxjIzglFl99JURPH9JjZbF8ZxTbvBLyInliN6c6
Zgz+EDW+Bb9g4wdGZjCuvO1/UwKBgQDJ+9fvvD1kTqG8oWQCNzw9Rj04yNQ0ehzC
zG42Sz+FXt5L4ZqybZswNr3zZvkUNaSVeDzxFCOB7yPPXCgRCRzH2jV7k8ZQX6/K
7Zuo0GhVIGR6nojGpAQpNPqVqVdo8q0erw5piv9egsoTWQE4fISTn44FBDUeN958
OdHoWkWXjQKBgC3DooZWUjlUf2hIb8lkqpNgxk3VDoMc6zoKllt3UM8q8Z/0hb7k
2YMzmi6NO2m/qDG0QpaLiSRtSlEQ75cmjaiNscuMeH31EnIVwekU1T5xI2ZioTO2
Z3epEU3od2rYtTvyscvt4/ClLzsc71Pj/9c28eB6wUEK7mbz70XMYNa7AoGASVRe
XBH6M918SJBLT6af/xruBRycNgUTRgGUDbAZ+qCrkd7xG9BBJCrroV+EFDs5am6B
qYCHN5gLZy/s9+pYAZKOEjRfLjTfDIxhE9O93RHqiL3fqEZJoHA0fXtCWb6o7Vfe
oqCs/7H6DTYmBEzokPO/SsDxS+w6oN0ZAQMs+s0CgYAInh9i3y5U4EzGV5IJtiL0
ub5Px952cG03BmZUDr2yyP8JmcWsG/I7rWetu+KRr6gOD6+IgEi3N4ja3SgGp5lq
SzPGw0VwiPkNiu9FBu4mfbT9ouJT+4ux6xN/lSP2gkJfYWpBkKODlnRSMOoa6WIG
sup34c5zDvmwEupkUwyybA==
-----END PRIVATE KEY-----
PEM

print_step "Starting mock authority + token endpoint + Bot Connector on :${MOCK_PORT}..."
cat > "${TEST_DATA_DIR}/mock.py" <<'PY'
import json, os, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
REPLY_RECORD = os.environ["REPLY_RECORD"]
JWKS = {"keys": [{
    "kty": "RSA", "use": "sig", "kid": os.environ["KID"],
    "n": os.environ["RSA_N"], "e": os.environ["RSA_E"], "endorsements": ["msteams"],
}]}
OPENID = {"issuer": os.environ["BF_ISSUER"], "jwks_uri": f"http://127.0.0.1:{PORT}/keys"}

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _send(self, code, body):
        b = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)
    def do_GET(self):
        if self.path.startswith("/bf/openid"): self._send(200, OPENID)
        elif self.path.startswith("/keys"):    self._send(200, JWKS)
        else: self._send(404, {"error": "not found", "path": self.path})
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n) if n else b""
        if self.path.endswith("/oauth2/v2.0/token"):
            self._send(200, {"token_type": "Bearer", "expires_in": 3600,
                             "ext_expires_in": 3600, "access_token": "MOCK_BOT_TOKEN"})
        elif "/v3/conversations/" in self.path and self.path.endswith("/activities"):
            try: text = json.loads(body.decode()).get("text", "")
            except Exception: text = ""
            with open(REPLY_RECORD, "a") as f:
                f.write(json.dumps({"path": self.path, "text": text}) + "\n")
            self._send(201, {"id": "mock-reply-activity"})
        else:
            self._send(404, {"error": "not found", "path": self.path})

ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
PY
KID="${KID}" RSA_N="${RSA_N}" RSA_E="${RSA_E}" BF_ISSUER="${BF_ISSUER}" REPLY_RECORD="${REPLY_RECORD}" \
    python3 "${TEST_DATA_DIR}/mock.py" "${MOCK_PORT}" >"${MOCK_LOG}" 2>&1 &
MOCK_PID=$!
for _ in {1..20}; do (echo > "/dev/tcp/127.0.0.1/${MOCK_PORT}") 2>/dev/null && break; sleep 0.3; done
: > "${REPLY_RECORD}"

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > "/dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}") 2>/dev/null && break; sleep 0.5; done

print_step "Creating test databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null
SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
TENANT_ID="${TENANT}" SERVER_HOST=127.0.0.1 SERVER_PORT="${TEST_PORT_PUBLIC}" \
INTERNAL_PORT="${TEST_PORT_INTERNAL}" RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
DATA_DIR="${TEST_DATA_DIR}" RUST_LOG="warn,runtara_server=info" AUTH_PROVIDER=local \
SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2 \
RUNTARA_ENDPOINT_REF_SECRET="reflush-e2e-secret-$$" \
RUNTARA_CONNECTION_SERVICE_URL="http://127.0.0.1:${TEST_PORT_INTERNAL}/api/connections" \
RUNTARA_TEAMS_OPENID_CONFIG_URL="${MOCK_BASE}/bf/openid" \
RUNTARA_TEAMS_ALLOW_INSECURE_SERVICE_URL=1 \
RUNTARA_PROXY_ALLOWED_HOSTS=127.0.0.1 \
RUNTARA_PROXY_ALLOW_HTTP_HOSTS=127.0.0.1 \
RUNTARA_CONNECTION_ALLOW_HTTP_HOSTS=127.0.0.1 \
VALKEY_HOST=127.0.0.1 VALKEY_PORT="${TEST_VALKEY_PORT}" \
OTEL_SDK_DISABLED=true RUNTARA_SDK_BACKEND=http SQLX_OFFLINE="${SQLX_OFFLINE}" \
"${RUNTARA_SERVER_BIN}" >"${TEST_LOG}" 2>&1 &
SERVER_PID=$!
for _ in {1..60}; do
    curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2" && break
    sleep 1
    kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "Server exited during boot."; tail -30 "${TEST_LOG}"; exit 1; }
done
echo "  Server up (PID ${SERVER_PID})"

print_step "Creating teams_bot connection (authority_host = mock)..."
CONN_PAYLOAD=$(python3 - "${APP_ID}" "${AZURE_TENANT}" "${MOCK_BASE}" <<'PY'
import json, sys
print(json.dumps({
  "title": "reflush-e2e", "integrationId": "teams_bot",
  "connectionParameters": {
    "app_id": sys.argv[1], "app_password": "mock-app-secret",
    "azure_tenant_id": sys.argv[2], "app_type": "single_tenant",
    "authority_host": sys.argv[3],
  }}))
PY
)
RESP=$(api_post /connections "${CONN_PAYLOAD}")
CONN_ID=$(echo "${RESP}" | jq -r '.connectionId // .data.id // .id // empty')
[ -z "${CONN_ID}" ] && { print_error "Connection create failed: ${RESP}"; tail -30 "${TEST_LOG}"; exit 1; }
echo "  Connection ${CONN_ID} ✓"

print_step "Creating a deterministically-FAILING workflow (teams.send-message, empty target)..."
RESP=$(api_post /workflows/create '{"name":"reflush-e2e","description":"fails so session emits one reply"}')
WF_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -z "${WF_ID}" ] && { print_error "Workflow create failed: ${RESP}"; exit 1; }
GRAPH=$(python3 - "${CONN_ID}" <<'PY'
import json, sys
conn = sys.argv[1]
print(json.dumps({
  "name": "reflush-e2e",
  "steps": {
    "fail": {
      "stepType": "Agent", "id": "fail", "agentId": "teams", "capabilityId": "send-message",
      "connectionId": conn,
      "inputMapping": {
        "target": {"valueType": "immediate", "value": ""},
        "conversation_id": {"valueType": "immediate", "value": ""},
        "text": {"valueType": "immediate", "value": ""},
      },
    },
    "finish": {"stepType": "Finish", "id": "finish",
      "inputMapping": {"result": {"valueType": "reference", "value": "steps.fail.outputs"}}},
  },
  "entryPoint": "fail",
  "executionPlan": [{"fromStep": "fail", "toStep": "finish"}],
  "variables": {}, "inputSchema": {}, "outputSchema": {},
}))
PY
)
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ] && { print_error "Update failed: ${RESP}"; exit 1; }
VERSION=$(curl -sS "${API}/workflows/${WF_ID}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(api_post "/workflows/${WF_ID}/versions/${VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ] && { print_error "Compile failed: ${RESP}"; tail -40 "${TEST_LOG}"; exit 1; }
echo "  Workflow ${WF_ID} compiled ✓"

print_step "Creating Channel trigger (per_message)..."
TRIG_PAYLOAD=$(python3 - "${WF_ID}" "${CONN_ID}" <<'PY'
import json, sys
print(json.dumps({"workflow_id": sys.argv[1], "trigger_type": "CHANNEL", "active": True,
  "configuration": {"connection_id": sys.argv[2], "session_mode": "per_message"}}))
PY
)
RESP=$(curl -sS -X POST -H "Content-Type: application/json" -d "${TRIG_PAYLOAD}" "${API}/triggers")
TRIG_ID=$(echo "${RESP}" | jq -r '.data.id // .id // empty')
[ -z "${TRIG_ID}" ] && { print_error "Trigger create failed: ${RESP}"; tail -30 "${TEST_LOG}"; exit 1; }
echo "  Trigger ${TRIG_ID} ✓"

NOW=$(date +%s)
claims() {
    printf '{"iss":"%s","aud":"%s","serviceurl":"%s","iat":%s,"nbf":%s,"exp":%s}' \
        "${BF_ISSUER}" "${APP_ID}" "${MOCK_BASE}" "$((NOW-60))" "$((NOW-60))" "$((NOW+3600))"
}
activity() {  # $1 = activity id
    python3 - "$1" "${CONV_ID}" "${MOCK_BASE}" "${AZURE_TENANT}" "${APP_ID}" <<'PY'
import json, sys
aid, conv, svc, tenant, app = sys.argv[1:6]
print(json.dumps({
  "type": "message", "id": aid, "text": "trigger the failing workflow",
  "serviceUrl": svc,
  "conversation": {"id": conv, "conversationType": "channel", "tenantId": tenant},
  "channelData": {"tenant": {"id": tenant}},
  "from": {"id": "29:user"}, "recipient": {"id": "28:" + app},
}))
PY
}
JWT=$(sign_jwt "${TEST_DATA_DIR}/signing_key.pem" "$(claims)")
post_activity() {  # $1 = activity id -> echoes HTTP code
    curl -sS -o /dev/null -w "%{http_code}" -X POST -H "Content-Type: application/json" \
        -H "Authorization: Bearer ${JWT}" -d "$(activity "$1")" "${WEBHOOK}/${CONN_ID}"
}
instance_count() {
    curl -sS "${API}/workflows/${WF_ID}/instances?size=100" \
        | jq -r '.data.totalElements // (.data.content | length) // 0' 2>/dev/null || echo 0
}
reply_count() { grep -c "Sorry" "${REPLY_RECORD}" 2>/dev/null || echo 0; }
wait_for_instances() {  # $1 = want ; echoes final
    local c; for _ in {1..40}; do c=$(instance_count); [ "${c}" -ge "$1" ] && break; sleep 0.5; done; echo "${c}"
}
wait_for_replies() {    # $1 = want ; echoes final
    local c; for _ in {1..40}; do c=$(reply_count); [ "${c}" -ge "$1" ] && break; sleep 0.5; done; echo "${c}"
}

# --- Case A: first delivery → 1 instance, 1 reply -------------------------
print_step "Case A: first delivery of activity A → 1 instance + 1 reply (owner)..."
CODE=$(post_activity "activity-A")
[ "${CODE}" != "200" ] && { print_error "Expected 200, got ${CODE}"; tail -60 "${TEST_LOG}"; exit 1; }
IC=$(wait_for_instances 1); RC=$(wait_for_replies 1)
[ "${IC}" -lt 1 ] && { print_error "No instance started"; tail -80 "${TEST_LOG}"; exit 1; }
[ "${RC}" -lt 1 ] && { print_error "Owner did not emit the failure reply (replies=${RC})"; tail -80 "${TEST_LOG}"; exit 1; }
# Give any stray duplicate a chance to appear, then pin exact counts.
sleep 2
[ "$(instance_count)" -eq 1 ] || { print_error "Expected exactly 1 instance, got $(instance_count)"; exit 1; }
[ "$(reply_count)" -eq 1 ] || { print_error "Expected exactly 1 reply, got $(reply_count)"; exit 1; }
echo "  1 instance, 1 'Sorry' reply ✓"

# --- Case B: RESIDUAL WINDOW — DEL dedup key, redeliver same activity -----
print_step "Case B: delete Valkey dedup key + redeliver SAME activity A → foreign session suppresses..."
DEDUP_KEY="channel_activity_dedup:${TENANT}:${CONN_ID}:activity-A"
docker exec "${VALKEY_CONTAINER}" valkey-cli DEL "${DEDUP_KEY}" >/dev/null 2>&1 \
    || { print_error "Failed to DEL dedup key ${DEDUP_KEY}"; exit 1; }
CODE=$(post_activity "activity-A")
[ "${CODE}" != "200" ] && { print_error "Expected 200 on redelivery, got ${CODE}"; exit 1; }
# Let the redelivery's session spawn, poll, classify foreign, and (correctly) do nothing.
sleep 5
IC=$(instance_count); RC=$(reply_count)
[ "${IC}" -ne 1 ] && { print_error "Redelivery created a 2nd instance (count=${IC}, expected 1)"; tail -80 "${TEST_LOG}"; exit 1; }
[ "${RC}" -ne 1 ] && { print_error "RE-FLUSH BUG: foreign session re-sent the reply (replies=${RC}, expected 1)"; tail -80 "${TEST_LOG}"; exit 1; }
echo "  Still 1 instance, still 1 reply — foreign redelivery suppressed ✓"

# --- Case C: a different activity → owned session still replies ------------
print_step "Case C: a DIFFERENT activity B → a new owned instance + its own reply..."
CODE=$(post_activity "activity-B")
[ "${CODE}" != "200" ] && { print_error "Expected 200, got ${CODE}"; exit 1; }
IC=$(wait_for_instances 2); RC=$(wait_for_replies 2)
[ "${IC}" -lt 2 ] && { print_error "Distinct activity did not start a new instance (count=${IC})"; tail -80 "${TEST_LOG}"; exit 1; }
[ "${RC}" -lt 2 ] && { print_error "Owned new instance did not reply (replies=${RC}) — suppression must be provenance-scoped, not blanket"; tail -80 "${TEST_LOG}"; exit 1; }
echo "  2 instances, 2 replies — suppression is provenance-scoped ✓"

print_success "Re-flush guard: owner replies once; foreign redelivery suppressed (no duplicate); distinct activity still replies"
