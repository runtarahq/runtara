#!/bin/bash
# E2E Test: descriptor-declared NAMED ENDPOINTS on a connection, end to end
# through the real credential proxy — using QuickBooks Online as the case.
#
# The problem this closes: Intuit splits one OAuth credential across two hosts.
# The Accounting API v3 lives at `quickbooks.api.intuit.com/v3/company/{realmId}`
# (the connection's base URL), but Intuit Enterprise Suite DIMENSION and custom
# field definitions — the only way to turn the opaque ids stamped on invoice
# lines into labels — live behind the app-foundations GraphQL API on a different
# host. Two things blocked reaching it:
#   1. the `http` agent would not accept a quickbooks_online connection at all
#      (its integration list IS the HttpConnectionExtractor registry), and
#   2. even attached, `pin_url_to_base` re-roots every connection-scoped request
#      onto the connection's base URL and rejects anything outside its base path.
#
# The fix keeps (2) fully intact and adds an opt-in selector: a connection type
# DECLARES the extra host in `named_endpoints`, and a request names it. The
# caller supplies a NAME, never a URL, so egress can only ever widen to a host a
# descriptor author vetted.
#
# Assertions (all offline — nothing egresses to Intuit at any point):
#   1. The `http` agent now lists `quickbooks_online`, and a workflow whose http
#      step binds a QBO connection saves, validates and compiles. This is the
#      reported bug: previously not bindable.
#   2. WITHOUT a selector, a request to the GraphQL host fail-closes on the pin,
#      and the error names the ACCOUNTING base path — the exact reported blocker.
#   3. WITH the `graphql` selector, the same fail-closed pin now reports the
#      GRAPHQL base path. Reading the base path back out of the proxy's own error
#      is what proves the swap actually happened, with zero network egress: a
#      selector that was ignored would still report the accounting base path.
#   4. An UNDECLARED selector is refused with NAMED_ENDPOINT_REJECTED, and does
#      NOT fall through to the connection's default base URL. That fall-through
#      is the dangerous failure mode — a typo'd selector would quietly send the
#      Intuit token to the wrong API and read as a provider-side 404.
#
# The positive path (a 2xx from the GraphQL endpoint) needs live Intuit
# credentials and is therefore NOT covered here; it is covered offline by
# `internal_proxy::tests::named_endpoint_round_trips_through_the_pin`, which
# runs the same decision function against the same real descriptor.
#
# Prerequisites: Postgres + docker (for an isolated Valkey) and the agent /
# shared workflow components in target/wasm32-wasip2/release (see
# scripts/build-agent-components.sh).

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

TEST_DB_SERVER="named_ep_e2e_server_$$"
TEST_DB_RUNTIME="named_ep_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17730}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17731}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18731}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18732}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18733}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18734}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16393}"
TEST_DATA_DIR="$(mktemp -d -t runtara_named_ep_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="named_ep_e2e"

# Stands in for a realm in request paths. The connection has no real realm_id
# (no OAuth is completed here), so this only ever appears in the URL the step
# asks for — never in a resolved base URL.
REALM="9130000000000001"

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
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" \
        -d "$2" "${API}$1"
}

# Execute the workflow with a URL + endpoint selector and return the terminal
# status and error text in globals. The proxy's rejection surfaces in the
# instance error, which is what every assertion below reads.
run_case() {
    local url="$1" endpoint="$2"
    local inputs resp instance
    inputs=$(python3 - "$url" "$endpoint" <<'PY'
import json, sys
print(json.dumps({"inputs": {"data": {"url": sys.argv[1], "endpoint": sys.argv[2]}}}))
PY
)
    resp=$(api_post "/workflows/${WF_ID}/execute" "${inputs}")
    instance=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    if [ -z "${instance}" ]; then
        print_error "Execute failed: ${resp}"
        exit 1
    fi
    RUN_STATUS=""; RUN_ERR=""
    for _ in {1..90}; do
        resp=$(curl -sS "${API}/workflows/instances/${instance}")
        RUN_STATUS=$(echo "${resp}" | jq -r '.data.status // .status // empty')
        case "${RUN_STATUS}" in completed|failed|crashed|stopped) break ;; esac
        sleep 2
    done
    RUN_ERR=$(echo "${resp}" | jq -r '.data.error // .error // empty')
}

# Fail unless the run's error contains `needle`. `label` describes the property
# under test so a failure reads as a claim about behaviour, not a diff.
assert_err_contains() {
    local needle="$1" label="$2"
    case "${RUN_ERR}" in
        *"${needle}"*) echo "  ${label} ✓" ;;
        *)
            print_error "${label} — expected the error to contain: ${needle}"
            print_error "  status : ${RUN_STATUS}"
            print_error "  error  : ${RUN_ERR:-<none>}"
            tail -40 "${TEST_LOG}"
            exit 1
            ;;
    esac
}

assert_err_lacks() {
    local needle="$1" label="$2"
    case "${RUN_ERR}" in
        *"${needle}"*)
            print_error "${label} — the error must NOT contain: ${needle}"
            print_error "  error  : ${RUN_ERR}"
            exit 1
            ;;
        *) echo "  ${label} ✓" ;;
    esac
}

#-------------------------------------------------------------------------
echo "==============================================================="
echo "E2E Test: connection named endpoints (QuickBooks Online)"
echo "==============================================================="

if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi
for f in runtara_agent_http.wasm runtara_workflow_stdlib.wasm runtara_workflow_runtime.wasm; do
    if [ ! -f "${COMPONENTS_DIR}/${f}" ]; then
        print_error "Missing component ${COMPONENTS_DIR}/${f} — run scripts/build-agent-components.sh"
        exit 1
    fi
done

print_step "Pre-flight: Postgres, docker, python3, jq..."
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }
command -v python3 >/dev/null 2>&1 || { print_error "python3 required"; exit 1; }
command -v jq >/dev/null 2>&1 || { print_error "jq required"; exit 1; }

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do
    if (echo > "/dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}") 2>/dev/null; then break; fi
    sleep 0.5
done

print_step "Creating test databases (${TEST_DB_SERVER}, ${TEST_DB_RUNTIME})..."
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
RUNTARA_CONNECTION_SERVICE_URL="http://127.0.0.1:${TEST_PORT_INTERNAL}/api/connections" \
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
        print_error "Server exited during boot."
        tail -30 "${TEST_LOG}"
        exit 1
    fi
done
echo "  Server up (PID ${SERVER_PID})"

#-------------------------------------------------------------------------
# Assertion 1a — the http agent accepts quickbooks_online at all.
#-------------------------------------------------------------------------
print_step "Checking the http agent's integration ids include quickbooks_online..."
# Found recursively so the assertion is robust to the response envelope shape.
HTTP_INTEGRATIONS=$(curl -sS "${API}/agents" \
    | jq -r '[.. | objects | select((.id? // "") | ascii_downcase == "http")]
             | first | (.integrationIds // .integration_ids // []) | join(",")')
case ",${HTTP_INTEGRATIONS}," in
    *,quickbooks_online,*) echo "  http agent offers quickbooks_online ✓" ;;
    *)
        print_error "http agent does NOT offer quickbooks_online — the connection"
        print_error "cannot be bound to an HTTP step. Got: ${HTTP_INTEGRATIONS:-<none>}"
        exit 1
        ;;
esac

#-------------------------------------------------------------------------
print_step "Creating a quickbooks_online connection..."
# No OAuth is completed, so the connection carries no realm_id or access_token.
# That is deliberate and sufficient: every case below fail-closes at the pin or
# the selector, before any credential is needed and before anything egresses.
# It also pins a property worth keeping — reaching the declared GraphQL endpoint
# must not depend on a realm the descriptor has no confirmed use for.
CONN_PAYLOAD=$(python3 - <<'PY'
import json
print(json.dumps({
  "title": "qbo-named-endpoint-e2e",
  "integrationId": "quickbooks_online",
  "connectionParameters": {
    "client_id": "mock-intuit-client-id",
    "client_secret": "mock-intuit-client-secret",
    "environment": "production",
    "scopes": "com.intuit.quickbooks.accounting",
  },
}))
PY
)
RESP=$(api_post /connections "${CONN_PAYLOAD}")
CONN_ID=$(echo "${RESP}" | jq -r '.connectionId // .data.id // .id // empty')
if [ -z "${CONN_ID}" ]; then
    print_error "Connection create failed: ${RESP}"
    tail -30 "${TEST_LOG}"
    exit 1
fi
echo "  Connection ${CONN_ID} ✓"

#-------------------------------------------------------------------------
# Assertion 1b — a workflow binding that connection to an http step compiles.
#-------------------------------------------------------------------------
print_step "Creating workflow: http step bound to the QBO connection..."
RESP=$(api_post /workflows/create '{"name":"qbo-named-endpoint-e2e","description":"named endpoint selection"}')
WF_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -z "${WF_ID}" ] && { print_error "Workflow create failed: ${RESP}"; exit 1; }

GRAPH=$(python3 - "${CONN_ID}" <<'PY'
import json, sys
conn = sys.argv[1]
print(json.dumps({
  "name": "qbo-named-endpoint-e2e",
  "steps": {
    "call": {
      "stepType": "Agent", "id": "call", "agentId": "http", "capabilityId": "http-request",
      "connectionId": conn,
      "inputMapping": {
        "url": {"valueType": "reference", "value": "data.url"},
        "method": {"valueType": "immediate", "value": "GET"},
        "connection_endpoint": {"valueType": "reference", "value": "data.endpoint"},
      },
    },
    "finish": {"stepType": "Finish", "id": "finish",
      "inputMapping": {"result": {"valueType": "reference", "value": "steps.call.outputs"}}},
  },
  "entryPoint": "call",
  "executionPlan": [{"fromStep": "call", "toStep": "finish"}],
  "variables": {},
  "inputSchema": {
    "url": {"type": "string", "required": True},
    "endpoint": {"type": "string", "required": False},
  },
  "outputSchema": {},
}))
PY
)
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${GRAPH}}")
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "Update/validate failed — an http step must accept a quickbooks_online connection: ${RESP}"
    exit 1
fi
VERSION=$(curl -sS "${API}/workflows/${WF_ID}/versions" \
    | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(api_post "/workflows/${WF_ID}/versions/${VERSION}/compile" '{}' 900)
if [ "$(echo "${RESP}" | jq -r '.success // false')" != "true" ]; then
    print_error "Compile failed: ${RESP}"
    tail -40 "${TEST_LOG}"
    exit 1
fi
echo "  Saved, validated and compiled ✓"

#-------------------------------------------------------------------------
# Assertion 2 — the reported blocker, reproduced: no selector → pinned to the
# ACCOUNTING base, so the GraphQL host is unreachable.
#-------------------------------------------------------------------------
print_step "Case 1: no selector → GraphQL host blocked by the accounting-base pin..."
run_case "https://qb.api.intuit.com/graphql" ""
assert_err_contains "/v3/company" \
    "pinned to the accounting base path (the reported blocker)"

#-------------------------------------------------------------------------
# Assertion 3 — with the selector, the pin base IS the GraphQL endpoint. Proven
# by a request that escapes THAT base: the reported base path flips.
#-------------------------------------------------------------------------
print_step "Case 2: 'graphql' selector → base URL swaps to the GraphQL endpoint..."
run_case "https://qb.api.intuit.com/v3/company/${REALM}/invoice" "graphql"
assert_err_contains "base path '/graphql'" \
    "base URL swapped to the declared GraphQL endpoint"
# If the selector had been ignored, this URL would sit happily under the
# accounting base and would not have been rejected at all.
assert_err_lacks "base path '/v3/company" \
    "the accounting base is no longer the pin base"

#-------------------------------------------------------------------------
# Assertion 4 — an undeclared selector is refused outright, and must NOT quietly
# fall back to the connection's default base URL.
#-------------------------------------------------------------------------
print_step "Case 3: undeclared selector → refused, with no fall-through..."
run_case "https://qb.api.intuit.com/graphql" "graphqll"
if [ "${RUN_STATUS}" = "completed" ]; then
    print_error "An undeclared endpoint name must not produce a successful run"
    exit 1
fi
assert_err_contains "NAMED_ENDPOINT_REJECTED" "undeclared selector refused"
# The message names what IS declared, so a typo is self-diagnosing.
assert_err_contains "graphql" "rejection names the declared endpoints"

print_success "Named endpoints: http steps can bind a QuickBooks Online connection; the \
'graphql' selector swaps the pin base to the declared Intuit host; an undeclared name \
fail-closes without falling back to the default base"
