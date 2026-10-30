#!/bin/bash
# E2E Test: two servers, each for its own tenant, share one Valkey (M6.1).
#
# Every key a server reads or writes names its tenant, so neither reads the
# other's membership, revocations or counters, nor takes the other's work.
# Both servers sign users in with OIDC (a mock JWKS) and require membership.
#
#   1. MEMBERSHIP — a user is refused until `{tenant}:member:{sub}` names
#      them; a record in the other tenant, or an unprefixed version 1
#      `member:{sub}` (RUNTARA_AUTH_LEGACY_MEMBER_KEYS=false), does not answer.
#   2. REVOCATION — `{tenant}:token:revoked:{jti}` refuses that token in its
#      tenant only.
#   3. WORK — both tenants save, compile and run several workflows at once
#      through the shared Valkey; every compile and run completes.
#   4. KEYS — each tenant's compilation queue and admission counter live
#      under its own prefix, and the unprefixed ones are gone.
#   5. TENANT FROM THE REQUEST (M6.2) — server A also serves A2. A token with
#      no `org_id` acts for the tenant its path names (`/api/runtime/{t}/…`),
#      authorized by membership there; `/api/runtime/me/tenants` lists where
#      the user may act; each tenant shows only its own data; a non-member
#      and a token naming another tenant are refused; a tenant-less path acts
#      for the primary as before.
#
# Prerequisites: Postgres (POSTGRES_PASSWORD or PGPASSWORD in the
# environment), docker (one Valkey), python3, openssl, and the shared workflow
# components in target/wasm32-wasip2/release (scripts/build-agent-components.sh).

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

TENANT_A="shared_valkey_a"
TENANT_B="shared_valkey_b"
TENANT_A2="shared_valkey_a2"
DB_PREFIX="shared_valkey_e2e_$$"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16590}"
MOCK_PORT="${MOCK_PORT:-17990}"
TEST_DATA_DIR="$(mktemp -d -t runtara_shared_valkey_e2e_XXXXXX)"
VALKEY_CONTAINER=""
MOCK_PID=""
PIDS=()
ISSUER="http://127.0.0.1:${MOCK_PORT}/"
KID="shared-valkey-e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql \
        -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" \
        -tA "$@"
}

cleanup() {
    for pid in ${PIDS[@]+"${PIDS[@]}"}; do
        kill "${pid}" 2>/dev/null || true
        wait "${pid}" 2>/dev/null || true
    done
    [ -n "${MOCK_PID}" ] && kill "${MOCK_PID}" 2>/dev/null || true
    [ -n "${VALKEY_CONTAINER}" ] && docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    for db in a_server a_runtime b_server b_runtime; do
        psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${DB_PREFIX}_${db}" >/dev/null 2>&1 || true
    done
    rm -rf "${TEST_DATA_DIR}" 2>/dev/null || true
}
trap cleanup EXIT

valkey() { docker exec "${VALKEY_CONTAINER}" valkey-cli --raw "$@"; }
b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }

# A signed OIDC token for `sub` and `jti` that names no tenant.
tenantless_token() {
    local sub="$1" jti="$2" now header claims signing sig
    now=$(date +%s)
    header='{"alg":"RS256","typ":"JWT","kid":"'"${KID}"'"}'
    claims='{"iss":"'"${ISSUER}"'","sub":"'"${sub}"'","jti":"'"${jti}"'","iat":'"${now}"',"exp":'"$((now + 3600))"'}'
    signing="$(printf '%s' "${header}" | b64url).$(printf '%s' "${claims}" | b64url)"
    sig=$(printf '%s' "${signing}" | openssl dgst -sha256 -sign "${TEST_DATA_DIR}/key.pem" -binary | b64url)
    printf '%s.%s' "${signing}" "${sig}"
}

# A signed OIDC token for `tenant` (org_id), `sub` and `jti`.
token() {
    local tenant="$1" sub="$2" jti="$3" now header claims signing sig
    now=$(date +%s)
    header='{"alg":"RS256","typ":"JWT","kid":"'"${KID}"'"}'
    claims='{"iss":"'"${ISSUER}"'","sub":"'"${sub}"'","org_id":"'"${tenant}"'","jti":"'"${jti}"'","iat":'"${now}"',"exp":'"$((now + 3600))"'}'
    signing="$(printf '%s' "${header}" | b64url).$(printf '%s' "${claims}" | b64url)"
    sig=$(printf '%s' "${signing}" | openssl dgst -sha256 -sign "${TEST_DATA_DIR}/key.pem" -binary | b64url)
    printf '%s.%s' "${signing}" "${sig}"
}

api_of() { [ "$1" = A ] && echo "http://127.0.0.1:17951/api/runtime" || echo "http://127.0.0.1:17952/api/runtime"; }
log_of() { echo "${TEST_DATA_DIR}/server_$1.log"; }
# call SERVER TOKEN METHOD PATH [BODY]
call() {
    curl -sS --max-time 900 -X "$3" -H "Authorization: Bearer $2" -H "Content-Type: application/json" \
        ${5:+-d "$5"} "$(api_of "$1")$4"
}
status_of() {
    curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $2" "$(api_of "$1")/workflows"
}

echo "==============================================================="
echo "E2E Test: two tenants' servers share one Valkey"
echo "==============================================================="

[ -n "${POSTGRES_PASSWORD}" ] \
    || { print_error "set POSTGRES_PASSWORD (or PGPASSWORD) for ${POSTGRES_USER}@${POSTGRES_HOST}"; exit 1; }
if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi
[ -f "${COMPONENTS_DIR}/runtara_workflow_stdlib.wasm" ] \
    || { print_error "Missing ${COMPONENTS_DIR}/runtara_workflow_stdlib.wasm — run scripts/build-agent-components.sh"; exit 1; }

print_step "Starting one Valkey, a mock JWKS, and the databases..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "${TEST_DATA_DIR}/key.pem" 2>/dev/null
RSA_N=$(openssl rsa -in "${TEST_DATA_DIR}/key.pem" -noout -modulus 2>/dev/null | cut -d= -f2 \
    | python3 -c 'import sys,base64; print(base64.urlsafe_b64encode(bytes.fromhex(sys.stdin.read().strip())).decode().rstrip("="))')
cat > "${TEST_DATA_DIR}/jwks.py" <<'PY'
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
KID="${KID}" RSA_N="${RSA_N}" python3 "${TEST_DATA_DIR}/jwks.py" "${MOCK_PORT}" >/dev/null 2>&1 &
MOCK_PID=$!
for db in a_server a_runtime b_server b_runtime; do
    psql_quiet -d postgres -c "CREATE DATABASE ${DB_PREFIX}_${db}" >/dev/null
done

start_server() {
    local name="$1" tenant="$2" port="$3" base="$4" served="${5:-}" db
    db=$(echo "${name}" | tr 'AB' 'ab')
    print_step "Starting server ${name} (tenant ${tenant}) on :${port}..."
    RUNTARA_SERVER_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${DB_PREFIX}_${db}_server" \
    OBJECT_MODEL_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${DB_PREFIX}_${db}_server" \
    RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${DB_PREFIX}_${db}_runtime" \
    TENANT_ID="${tenant}" \
    RUNTARA_SERVED_TENANTS="${served}" \
    SERVER_HOST=127.0.0.1 \
    SERVER_PORT="${port}" \
    RUNTARA_CORE_PORT="$((base + 1))" \
    RUNTARA_ENVIRONMENT_PORT="$((base + 2))" \
    RUNTARA_CORE_HTTP_PORT="$((base + 3))" \
    RUNTARA_ENV_HTTP_PORT="$((base + 4))" \
    RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
    MAX_CONCURRENT_EXECUTIONS=16 \
    DATA_DIR="${TEST_DATA_DIR}/data_${name}" \
    RUST_LOG="warn,runtara_server=info" \
    AUTH_PROVIDER=oidc \
    OAUTH2_ISSUER="${ISSUER}" \
    OAUTH2_JWKS_URI="http://127.0.0.1:${MOCK_PORT}/keys" \
    RUNTARA_AUTH_MEMBERSHIP_POLICY=required \
    RUNTARA_AUTH_LEGACY_MEMBER_KEYS=false \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true \
    RUNTARA_SDK_BACKEND=http \
    SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}" >>"$(log_of "${name}")" 2>&1 &
    PIDS+=($!)
    local pid=$!
    for _ in {1..90}; do
        curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${port}/health" 2>/dev/null | grep -q "^2" && break
        sleep 1
        kill -0 "${pid}" 2>/dev/null || { print_error "server ${name} exited during boot"; tail -30 "$(log_of "${name}")"; exit 1; }
    done
    echo "  server ${name} up (PID ${pid})"
}
start_server A "${TENANT_A}" 17951 18950 "${TENANT_A2}"
start_server B "${TENANT_B}" 17952 18960

#-------------------------------------------------------------------------
print_step "1. Membership is read from the caller's own tenant..."
TOKEN_A=$(token "${TENANT_A}" "user-1" "jti-a-1")
TOKEN_B=$(token "${TENANT_B}" "user-1" "jti-b-1")
[ "$(status_of A "${TOKEN_A}")" = "403" ] || { print_error "a non-member was not refused in A"; exit 1; }
valkey SET "${TENANT_A}:member:user-1" '{"role":"owner","updated_at":"2026-10-27T00:00:00Z"}' >/dev/null
[ "$(status_of A "${TOKEN_A}")" = "200" ] || { print_error "A's member was refused: $(status_of A "${TOKEN_A}")"; exit 1; }
[ "$(status_of B "${TOKEN_B}")" = "403" ] || { print_error "A's member record answered in B"; exit 1; }
valkey SET "member:user-1" '{"role":"owner"}' >/dev/null
[ "$(status_of B "${TOKEN_B}")" = "403" ] || { print_error "an unprefixed member record answered with legacy keys off"; exit 1; }
valkey DEL "member:user-1" >/dev/null
valkey SET "${TENANT_B}:member:user-1" '{"role":"owner","updated_at":"2026-10-27T00:00:00Z"}' >/dev/null
[ "$(status_of B "${TOKEN_B}")" = "200" ] || { print_error "B's member was refused"; exit 1; }
echo "  each tenant reads only its own member record; an unprefixed one does not answer ✓"

#-------------------------------------------------------------------------
print_step "2. A revocation refuses the token in its own tenant only..."
TOKEN_A2=$(token "${TENANT_A}" "user-1" "jti-shared")
TOKEN_B2=$(token "${TENANT_B}" "user-1" "jti-shared")
valkey SET "${TENANT_A}:token:revoked:jti-shared" '{"revoked_at":"2026-10-27T00:00:00Z"}' >/dev/null
[ "$(status_of A "${TOKEN_A2}")" = "401" ] || { print_error "A's revoked token was not refused"; exit 1; }
[ "$(status_of B "${TOKEN_B2}")" = "200" ] || { print_error "A's revocation refused a token in B"; exit 1; }
echo "  revoked in A, still valid in B ✓"

#-------------------------------------------------------------------------
print_step "3. Both tenants compile and run several workflows at once..."
GRAPH='{"name":"Answer","steps":{"finish":{"stepType":"Finish","id":"finish","inputMapping":{"answer":{"valueType":"immediate","value":42}}}},"entryPoint":"finish","executionPlan":[],"variables":{},"inputSchema":{},"outputSchema":{}}'
work() {
    local server="$1" tok="$2" n="$3" resp wf version instance status
    resp=$(call "${server}" "${tok}" POST /workflows/create "{\"name\":\"W${server}${n}\",\"description\":\"shared valkey\"}")
    wf=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf}" ] || { echo "create ${server}${n}: ${resp}"; return 1; }
    resp=$(call "${server}" "${tok}" POST "/workflows/${wf}/update" "{\"executionGraph\": ${GRAPH}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { echo "update ${server}${n}: ${resp}"; return 1; }
    version=$(call "${server}" "${tok}" GET "/workflows/${wf}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(call "${server}" "${tok}" POST "/workflows/${wf}/versions/${version}/compile" '{}')
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { echo "compile ${server}${n}: ${resp}"; return 1; }
    for run in 1 2; do
        resp=$(call "${server}" "${tok}" POST "/workflows/${wf}/execute" '{"inputs": {"data": {}}}')
        instance=$(echo "${resp}" | jq -r '.data.instanceId // empty')
        [ -n "${instance}" ] || { echo "execute ${server}${n}: ${resp}"; return 1; }
        status=""
        for _ in {1..240}; do
            status=$(call "${server}" "${tok}" GET "/workflows/instances/${instance}" | jq -r '.data.status // empty')
            case "${status}" in completed|failed|crashed|stopped|cancelled) break ;; esac
            sleep 0.5
        done
        [ "${status}" = "completed" ] || { echo "run ${server}${n}.${run} ended '${status}'"; return 1; }
    done
}
JOBS=()
for n in 1 2 3 4; do
    for server in A B; do
        tok=$([ "${server}" = A ] && echo "${TOKEN_A}" || echo "${TOKEN_B}")
        work "${server}" "${tok}" "${n}" >"${TEST_DATA_DIR}/work_${server}${n}.out" 2>&1 &
        JOBS+=($!)
    done
done
FAILED=0
for job in "${JOBS[@]}"; do wait "${job}" || FAILED=1; done
if [ "${FAILED}" != 0 ]; then
    cat "${TEST_DATA_DIR}"/work_*.out >&2
    tail -20 "$(log_of A)" "$(log_of B)" >&2
    print_error "a tenant's compile or run did not complete"; exit 1
fi
for server in A B; do
    tok=$([ "${server}" = A ] && echo "${TOKEN_A}" || echo "${TOKEN_B}")
    count=$(call "${server}" "${tok}" GET "/workflows" | jq '[.data.content // .data // [] | .[]?] | length')
    [ "${count}" = "4" ] || { print_error "server ${server} lists ${count} workflows, not its own 4"; exit 1; }
done
echo "  eight workflows compiled and sixteen runs completed across both tenants; each lists only its own ✓"

#-------------------------------------------------------------------------
print_step "4. Each tenant's queue and counter live under its own prefix..."
for tenant in "${TENANT_A}" "${TENANT_B}"; do
    [ "$(valkey EXISTS "${tenant}:admission:reserved")" = "1" ] \
        || { print_error "no admission counter for ${tenant}"; exit 1; }
    [ "$(valkey GET "${tenant}:admission:reserved")" = "0" ] \
        || { print_error "${tenant}'s admission counter did not return to 0"; exit 1; }
    [ "$(valkey SCARD "${tenant}:compilation:pending")" = "0" ] \
        || { print_error "${tenant} has compilations still pending"; exit 1; }
done
LEFT=$(valkey --scan --pattern 'runtara:compilation:*' | grep -v '^runtara:compilation:progress' || true; valkey --scan --pattern 'runtara:admission:*')
[ -z "${LEFT}" ] || { print_error "unprefixed keys remain: ${LEFT}"; exit 1; }
echo "  per-tenant admission counters back at 0, no compilation pending, no unprefixed queue or counter keys ✓"

#-------------------------------------------------------------------------
print_step "5. A token naming no tenant acts for the tenant its path names..."
A="http://127.0.0.1:17951/api/runtime"
get() { curl -sS -H "Authorization: Bearer $1" "$2"; }
code() { curl -sS -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $1" "$2"; }
valkey SET "${TENANT_A2}:member:user-1" '{"role":"admin","updated_at":"2026-10-27T00:00:00Z"}' >/dev/null
valkey SET "${TENANT_A}:member:user-2" '{"role":"viewer","updated_at":"2026-10-27T00:00:00Z"}' >/dev/null
U1=$(tenantless_token "user-1" "jti-u1")
U2=$(tenantless_token "user-2" "jti-u2")

MINE=$(get "${U1}" "${A}/me/tenants" | jq -c '[.tenants[] | [.id, .role]] | sort')
[ "${MINE}" = "[[\"${TENANT_A}\",\"owner\"],[\"${TENANT_A2}\",\"admin\"]]" ] \
    || { print_error "user-1's tenants on A: ${MINE}"; exit 1; }
MINE=$(get "${U2}" "${A}/me/tenants" | jq -c '[.tenants[].id]')
[ "${MINE}" = "[\"${TENANT_A}\"]" ] || { print_error "user-2's tenants on A: ${MINE}"; exit 1; }

count_in() { get "$1" "${A}${2:+/$2}/workflows" | jq '[.data.content // .data // [] | .[]?] | length'; }
[ "$(count_in "${U1}" "${TENANT_A}")" = "4" ] || { print_error "user-1 does not see A's four workflows"; exit 1; }
[ "$(count_in "${U1}" "${TENANT_A2}")" = "0" ] || { print_error "A2 shows another tenant's workflows"; exit 1; }
RESP=$(curl -sS -X POST -H "Authorization: Bearer ${U1}" -H "Content-Type: application/json" \
    -d '{"name":"Only in A2","description":"switch"}' "${A}/${TENANT_A2}/workflows/create")
echo "${RESP}" | jq -e '.data.id' >/dev/null || { print_error "user-1 could not create in A2: ${RESP}"; exit 1; }
[ "$(count_in "${U1}" "${TENANT_A2}")" = "1" ] || { print_error "A2 does not show its own workflow"; exit 1; }
[ "$(count_in "${U1}" "${TENANT_A}")" = "4" ] || { print_error "A shows A2's workflow"; exit 1; }

[ "$(code "${U2}" "${A}/${TENANT_A2}/workflows")" = "403" ] || { print_error "a non-member reached A2"; exit 1; }
[ "$(code "${TOKEN_A}" "${A}/${TENANT_A2}/workflows")" = "403" ] \
    || { print_error "a token naming A acted in A2"; exit 1; }
[ "$(code "${U1}" "${A}/${TENANT_B}/workflows")" = "404" ] \
    || { print_error "a tenant A does not serve was routed"; exit 1; }
[ "$(count_in "${U1}" "")" = "4" ] || { print_error "a tenant-less path did not act for the primary"; exit 1; }
[ "$(code "${TOKEN_A}" "${A}/workflows")" = "200" ] || { print_error "a token naming its tenant no longer works"; exit 1; }
echo "  one user switches between A and A2 by path and sees only each one's data; non-members and other tenants' tokens are refused ✓"

print_success "shared Valkey: two tenants' servers share one Valkey without reading each other's membership, revocations, queues or counters, and a tenant-less token acts where its path and membership say"
