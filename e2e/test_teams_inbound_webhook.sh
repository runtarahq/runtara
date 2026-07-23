#!/bin/bash
# E2E Test: Microsoft Teams INBOUND webhook, end to end, against a MOCK Bot
# Framework OpenID/JWKS authority (no real Teams tenant).
#
# Proves the inbound perimeter the gap fixes hardened:
#   POST /api/runtime/events/webhook/teams/{connection_id}
#     → teams_auth::validate_teams_request
#         · RS256 signature verified against the mock JWKS (kid → key)
#         · msteams channel endorsement enforced on the signing key
#         · aud == connection app_id, iss == Bot Framework, nbf/exp enforced
#         · serviceurl claim == activity serviceUrl
#     → single-tenant activity-tenant gate (channelData.tenant.id)
#     → ack-fast 200, then (in a spawned task) reserve dedup + route to the
#       Channel trigger's workflow via the session machinery.
#
# Assertions:
#   A. A validly-signed activity → 200 AND exactly one workflow instance starts.
#   B. A byte-identical redelivery (same activity id) → 200 but NO second
#      instance (dedup: reserve_activity + deterministic instance id).
#   C. A DIFFERENT activity id → 200 AND a second instance (proves B is real
#      dedup, not a blanket drop).
#   D. A forged JWT (signed by a different key) → 403 AND no new instance.
#   E. A missing Authorization header → 403 AND no new instance.
#
# The signing key is the fixed test RSA keypair from teams_auth.rs's unit tests;
# the mock JWKS publishes its public half. Forged tokens are signed with a
# throwaway key so the signature fails against the published JWKS.
#
# Prerequisites: Postgres + docker (isolated Valkey), python3, openssl, and the
# datetime + shared workflow components in target/wasm32-wasip2/release
# (scripts/build-agent-components.sh).

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

TEST_DB_SERVER="teams_in_e2e_server_$$"
TEST_DB_RUNTIME="teams_in_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17730}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17731}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18731}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18732}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18733}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18734}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16393}"
MOCK_PORT="${MOCK_PORT:-17735}"
TEST_DATA_DIR="$(mktemp -d -t runtara_teams_in_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
MOCK_LOG="${TEST_DATA_DIR}/mock.log"
SERVER_PID=""
MOCK_PID=""
VALKEY_CONTAINER=""
TENANT="teams_in_e2e"

APP_ID="11111111-2222-3333-4444-555555555555"
AZURE_TENANT="aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
SERVICE_URL="https://smba.trafficmanager.net/amer/"
BF_ISSUER="https://api.botframework.com"
KID="test-kid-1"
CONV_ID='19:inbound-conv@thread.tacv2'

# Public half of teams_auth.rs's fixed test RSA keypair (JWK n/e).
RSA_N="xnDLCAOM3SV5h0tgactRl97eSimuypTrGuwhTv8lWFTEJ0M4XUrjcUmUosc6nDHxEKnBFun7VuFuWUzYqarjU6WvRCzgh1HXfK9ZzercPUYyeK-Guiu46iRE-RwWW1hY_bUjy_blZMHjgieLpPL64ccXoWgvfE3yQCijhTvTS-VAKx83VoPMcgfhJbg31FMV5c6ElSoxoNThO5JWW4Kwe9YjcHZnvTcXiq89WEuXQKFpJ5iRMVmLql0LAnVNnP67VLa1gqblrcVWRgyQHTvPiVBr5fw2qbubcsuBkBUpUIqsEW8eHDqxHVWSU_IhIfMpIEo6_PnLXFFELXKspEgVtw"
RSA_E="AQAB"

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
WEBHOOK="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime/events/webhook/teams"
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" \
        -d "$2" "${API}$1"
}

b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }

# Sign an RS256 JWT. $1 = private key PEM path, $2 = compact claims JSON.
sign_jwt() {
    local key="$1" claims="$2"
    local header b64h b64p signing sig
    header='{"alg":"RS256","typ":"JWT","kid":"'"${KID}"'"}'
    b64h=$(printf '%s' "${header}" | b64url)
    b64p=$(printf '%s' "${claims}" | b64url)
    signing="${b64h}.${b64p}"
    sig=$(printf '%s' "${signing}" | openssl dgst -sha256 -sign "${key}" -binary | b64url)
    printf '%s.%s' "${signing}" "${sig}"
}

echo "==============================================================="
echo "E2E Test: Teams INBOUND webhook JWT validation + dedup (mock authority)"
echo "==============================================================="

if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi
for f in runtara_agent_datetime.wasm runtara_workflow_stdlib.wasm runtara_workflow_runtime.wasm; do
    if [ ! -f "${COMPONENTS_DIR}/${f}" ]; then
        print_error "Missing component ${COMPONENTS_DIR}/${f} — run scripts/build-agent-components.sh"
        exit 1
    fi
done

print_step "Pre-flight: Postgres, docker, python3, openssl..."
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }
command -v python3 >/dev/null 2>&1 || { print_error "python3 required (mock server)"; exit 1; }
command -v openssl >/dev/null 2>&1 || { print_error "openssl required (JWT signing)"; exit 1; }

# --- Test signing keys ----------------------------------------------------
print_step "Writing test signing keys..."
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
# Throwaway key for the forged-token case (wrong signature vs the published JWKS).
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 \
    -out "${TEST_DATA_DIR}/forged_key.pem" 2>/dev/null

# --- Mock OpenID/JWKS authority -------------------------------------------
print_step "Starting mock OpenID/JWKS authority on :${MOCK_PORT}..."
cat > "${TEST_DATA_DIR}/mock.py" <<'PY'
import json, os, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1])
JWKS = {"keys": [{
    "kty": "RSA", "use": "sig", "kid": os.environ["KID"],
    "n": os.environ["RSA_N"], "e": os.environ["RSA_E"],
    "endorsements": ["msteams"],
}]}
OPENID = {"issuer": os.environ["BF_ISSUER"],
          "jwks_uri": f"http://127.0.0.1:{PORT}/keys"}

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
        if self.path.startswith("/bf/openid"):
            self._send(200, OPENID)
        elif self.path.startswith("/keys"):
            self._send(200, JWKS)
        else:
            self._send(404, {"error": "not found", "path": self.path})

ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
PY
KID="${KID}" RSA_N="${RSA_N}" RSA_E="${RSA_E}" BF_ISSUER="${BF_ISSUER}" \
    python3 "${TEST_DATA_DIR}/mock.py" "${MOCK_PORT}" >"${MOCK_LOG}" 2>&1 &
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

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC} (mock BF authority)..."
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
RUNTARA_ENDPOINT_REF_SECRET="teams-inbound-e2e-secret-$$" \
RUNTARA_TEAMS_OPENID_CONFIG_URL="http://127.0.0.1:${MOCK_PORT}/bf/openid" \
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

# --- Create the teams_bot connection (single-tenant) ----------------------
print_step "Creating teams_bot connection..."
CONN_PAYLOAD=$(python3 - "${APP_ID}" "${AZURE_TENANT}" <<'PY'
import json, sys
print(json.dumps({
  "title": "teams-inbound-e2e",
  "integrationId": "teams_bot",
  "connectionParameters": {
    "app_id": sys.argv[1],
    "app_password": "mock-app-secret",
    "azure_tenant_id": sys.argv[2],
    "app_type": "single_tenant",
  },
}))
PY
)
RESP=$(api_post /connections "${CONN_PAYLOAD}")
CONN_ID=$(echo "${RESP}" | jq -r '.connectionId // .data.id // .id // empty')
[ -z "${CONN_ID}" ] && { print_error "Connection create failed: ${RESP}"; tail -30 "${TEST_LOG}"; exit 1; }
echo "  Connection ${CONN_ID} ✓"

# --- Trivial no-egress workflow (datetime.get-current-date → Finish) -------
print_step "Creating trivial datetime workflow..."
RESP=$(api_post /workflows/create '{"name":"teams-inbound-e2e","description":"inbound trigger target"}')
WF_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -z "${WF_ID}" ] && { print_error "Workflow create failed: ${RESP}"; exit 1; }

GRAPH=$(python3 - <<'PY'
import json
print(json.dumps({
  "name": "teams-inbound-e2e",
  "steps": {
    "now": {
      "stepType": "Agent", "id": "now", "agentId": "datetime",
      "capabilityId": "get-current-date", "inputMapping": {},
    },
    "finish": {"stepType": "Finish", "id": "finish",
      "inputMapping": {"result": {"valueType": "reference", "value": "steps.now.outputs"}}},
  },
  "entryPoint": "now",
  "executionPlan": [{"fromStep": "now", "toStep": "finish"}],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {},
}))
PY
)
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ] && { print_error "Update failed: ${RESP}"; exit 1; }
VERSION=$(curl -sS "${API}/workflows/${WF_ID}/versions" \
    | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(api_post "/workflows/${WF_ID}/versions/${VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ] && { print_error "Compile failed: ${RESP}"; tail -40 "${TEST_LOG}"; exit 1; }
echo "  Workflow ${WF_ID} compiled ✓"

# --- Channel trigger (per_message so each fresh activity = one instance) ---
print_step "Creating Channel trigger..."
TRIG_PAYLOAD=$(python3 - "${WF_ID}" "${CONN_ID}" <<'PY'
import json, sys
print(json.dumps({
  "workflow_id": sys.argv[1],
  "trigger_type": "CHANNEL",
  "active": True,
  "configuration": {"connection_id": sys.argv[2], "session_mode": "per_message"},
}))
PY
)
RESP=$(curl -sS -X POST -H "Content-Type: application/json" -d "${TRIG_PAYLOAD}" "${API}/triggers")
TRIG_ID=$(echo "${RESP}" | jq -r '.data.id // .id // empty')
[ -z "${TRIG_ID}" ] && { print_error "Trigger create failed: ${RESP}"; tail -30 "${TEST_LOG}"; exit 1; }
echo "  Trigger ${TRIG_ID} ✓"

# --- Helpers --------------------------------------------------------------
NOW=$(date +%s)
claims() {  # $1 = serviceurl override (default SERVICE_URL)
    local svc="${1:-${SERVICE_URL}}"
    printf '{"iss":"%s","aud":"%s","serviceurl":"%s","iat":%s,"nbf":%s,"exp":%s}' \
        "${BF_ISSUER}" "${APP_ID}" "${svc}" "$((NOW-60))" "$((NOW-60))" "$((NOW+3600))"
}

activity() {  # $1 = activity id
    python3 - "$1" "${CONV_ID}" "${SERVICE_URL}" "${AZURE_TENANT}" "${APP_ID}" <<'PY'
import json, sys
aid, conv, svc, tenant, app = sys.argv[1:6]
print(json.dumps({
  "type": "message", "id": aid, "text": "hello inbound",
  "serviceUrl": svc,
  "conversation": {"id": conv, "conversationType": "channel", "tenantId": tenant},
  "channelData": {"tenant": {"id": tenant}},
  "from": {"id": "29:user"},
  "recipient": {"id": "28:" + app},
}))
PY
}

post_activity() {  # $1 = bearer (or "none"), $2 = activity id ; echoes HTTP code
    local bearer="$1" aid="$2" body
    body=$(activity "${aid}")
    if [ "${bearer}" = "none" ]; then
        curl -sS -o /dev/null -w "%{http_code}" -X POST \
            -H "Content-Type: application/json" -d "${body}" "${WEBHOOK}/${CONN_ID}"
    else
        curl -sS -o /dev/null -w "%{http_code}" -X POST \
            -H "Content-Type: application/json" \
            -H "Authorization: Bearer ${bearer}" \
            -d "${body}" "${WEBHOOK}/${CONN_ID}"
    fi
}

instance_count() {
    # Paginated: instances live at .data.content; totalElements is the count.
    curl -sS "${API}/workflows/${WF_ID}/instances?size=100" \
        | jq -r '.data.totalElements // (.data.content | length) // 0' 2>/dev/null || echo 0
}

# Wait until instance count reaches $1 (timeout ~20s); echoes final count.
wait_for_count() {
    local want="$1" c
    for _ in {1..40}; do
        c=$(instance_count)
        [ "${c}" -ge "${want}" ] && break
        sleep 0.5
    done
    echo "${c}"
}

VALID_JWT=$(sign_jwt "${TEST_DATA_DIR}/signing_key.pem" "$(claims)")
FORGED_JWT=$(sign_jwt "${TEST_DATA_DIR}/forged_key.pem" "$(claims)")

# --- Case A: valid activity → 200 + exactly one instance ------------------
print_step "Case A: valid signed activity → 200 + one instance..."
CODE=$(post_activity "${VALID_JWT}" "activity-A")
[ "${CODE}" != "200" ] && { print_error "Expected 200, got ${CODE}"; tail -60 "${TEST_LOG}"; exit 1; }
C=$(wait_for_count 1)
[ "${C}" -lt 1 ] && { print_error "No instance started for a valid activity"; tail -60 "${TEST_LOG}"; exit 1; }
echo "  200 + instance started (count=${C}) ✓"

# --- Case B: identical redelivery → 200 + NO second instance --------------
print_step "Case B: redelivery of the SAME activity id → dedup, no new instance..."
CODE=$(post_activity "${VALID_JWT}" "activity-A")
[ "${CODE}" != "200" ] && { print_error "Expected 200 on redelivery, got ${CODE}"; exit 1; }
sleep 3
C=$(instance_count)
[ "${C}" -ne 1 ] && { print_error "Redelivery was NOT deduped (count=${C}, expected 1)"; tail -60 "${TEST_LOG}"; exit 1; }
echo "  Redelivery deduped (count still ${C}) ✓"

# --- Case C: a DIFFERENT activity id → 200 + a second instance ------------
print_step "Case C: a different activity id → a second instance..."
CODE=$(post_activity "${VALID_JWT}" "activity-C")
[ "${CODE}" != "200" ] && { print_error "Expected 200, got ${CODE}"; exit 1; }
C=$(wait_for_count 2)
[ "${C}" -lt 2 ] && { print_error "Second distinct activity did not start a new instance (count=${C})"; tail -60 "${TEST_LOG}"; exit 1; }
echo "  Distinct activity started a new instance (count=${C}) ✓"

# --- Case D: forged JWT → 403 + no new instance ---------------------------
print_step "Case D: forged JWT (wrong signing key) → 403..."
BEFORE=$(instance_count)
CODE=$(post_activity "${FORGED_JWT}" "activity-D")
[ "${CODE}" != "403" ] && { print_error "Expected 403 for forged JWT, got ${CODE}"; tail -60 "${TEST_LOG}"; exit 1; }
sleep 2
AFTER=$(instance_count)
[ "${AFTER}" -ne "${BEFORE}" ] && { print_error "Forged JWT started an instance (${BEFORE}→${AFTER})"; exit 1; }
echo "  Forged JWT rejected 403, no instance (count=${AFTER}) ✓"

# --- Case E: missing Authorization → 403 ----------------------------------
print_step "Case E: missing Authorization header → 403..."
BEFORE=$(instance_count)
CODE=$(post_activity "none" "activity-E")
[ "${CODE}" != "403" ] && { print_error "Expected 403 for missing bearer, got ${CODE}"; tail -60 "${TEST_LOG}"; exit 1; }
sleep 2
AFTER=$(instance_count)
[ "${AFTER}" -ne "${BEFORE}" ] && { print_error "Unauthenticated activity started an instance (${BEFORE}→${AFTER})"; exit 1; }
echo "  Missing bearer rejected 403, no instance (count=${AFTER}) ✓"

print_success "Teams inbound webhook: valid JWT triggers exactly-once; redelivery deduped; forged + unauthenticated rejected 403"
