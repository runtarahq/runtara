#!/bin/bash
# E2E Test: SMO Object Model — Tier 1 trigram similarity + scored Top-K
#
# Spins up runtara-server on test ports against fresh databases, exercises
# the full HTTP API surface introduced in Tier 1, then tears everything
# down. Self-contained: does not assume an already-running server.
#
# Scenarios covered:
#   1. pg_trgm extension is installed by the migration runner
#   2. Schema-create with `textIndex: trigram` produces a partial GIN index
#   3. Schema-create with `textIndex: trigram` on a non-string column → 400
#   4. Insert ~20 product rows
#   5. SIMILARITY_GTE filter + structured scoreExpression + alias-based
#      orderBy returns ranked Top-K with `instance.computed.score`
#   6. SIMILARITY_GTE on a non-string column → 400
#   7. SIMILARITY_GTE with threshold > 1 → 400
#   8. SIMILARITY_GTE with bad arity → 400
#   9. Invalid scoreExpression alias → 400
#   10. Legacy `sortBy` / `sortOrder` still works when `orderBy` is absent
#   11. Query without scoreExpression returns no `computed` field
#
# Prerequisites:
#   - Postgres reachable on $POSTGRES_HOST:$POSTGRES_PORT with creds in
#     $POSTGRES_USER / $POSTGRES_PASSWORD (defaults below match the dev
#     docker-compose setup).
#   - Valkey/Redis reachable on $VALKEY_HOST:$VALKEY_PORT (server requires it).
#   - target/debug/runtara-server (or pass RUNTARA_SERVER_BIN) built; the
#     script will `cargo build` if it is missing.

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Postgres / Valkey defaults (override via env when running outside the dev compose).
POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"
VALKEY_HOST="${VALKEY_HOST:-localhost}"
VALKEY_PORT="${VALKEY_PORT:-6379}"

# Test-only databases and ports (won't collide with the dev environment).
TEST_DB_SERVER="tier1_e2e_server_$$"
TEST_DB_RUNTIME="tier1_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17500}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17501}"
TEST_DATA_DIR="$(mktemp -d -t runtara_tier1_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql \
        -U "${POSTGRES_USER}" \
        -h "${POSTGRES_HOST}" \
        -p "${POSTGRES_PORT}" \
        -tA "$@"
}

cleanup() {
    if [ -n "${SERVER_PID}" ] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME}" >/dev/null 2>&1 || true
    rm -rf "${TEST_DATA_DIR}"
}
trap cleanup EXIT

API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime/object-model"

api_post() {
    # $1 = path, $2 = body. Echos the response body and exits non-zero on
    # transport errors. Caller is responsible for asserting on body content.
    curl -sS -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}

api_get() {
    curl -sS "${API}$1"
}

assert_http_status() {
    # Args: <description> <expected_code> <method> <path> [<body>]
    local desc="$1" expected="$2" method="$3" path="$4" body="${5:-}"
    local got
    if [ "${method}" = "GET" ]; then
        got=$(curl -sS -o /dev/null -w "%{http_code}" "${API}${path}")
    else
        got=$(curl -sS -o /dev/null -w "%{http_code}" -X "${method}" \
                -H "Content-Type: application/json" -d "${body}" "${API}${path}")
    fi
    if [ "${got}" != "${expected}" ]; then
        print_error "${desc}: expected HTTP ${expected}, got ${got}"
        exit 1
    fi
    echo "  ${desc}: HTTP ${got} ✓"
}

#-------------------------------------------------------------------------
echo "========================================================"
echo "E2E Test: SMO Object Model — Tier 1 trigram similarity"
echo "========================================================"

# Build the server if the binary is missing.
if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi

# Pre-flight: Postgres + Valkey reachable.
print_step "Pre-flight: Postgres and Valkey..."
if ! psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1; then
    print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"
    exit 1
fi
if ! (echo > /dev/tcp/${VALKEY_HOST}/${VALKEY_PORT}) 2>/dev/null; then
    print_error "Cannot reach Valkey at ${VALKEY_HOST}:${VALKEY_PORT}"
    exit 1
fi

# Provision test databases.
print_step "Creating test databases (${TEST_DB_SERVER}, ${TEST_DB_RUNTIME})..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

# Boot server.
print_step "Starting runtara-server on :${TEST_PORT_PUBLIC} (internal :${TEST_PORT_INTERNAL})..."
DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}" \
OBJECT_MODEL_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}" \
RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
TENANT_ID=tier1_e2e \
SERVER_HOST=127.0.0.1 \
SERVER_PORT="${TEST_PORT_PUBLIC}" \
INTERNAL_PORT="${TEST_PORT_INTERNAL}" \
RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:18500" \
DATA_DIR="${TEST_DATA_DIR}" \
RUST_LOG="warn,runtara_server=warn,runtara_object_store=warn" \
AUTH_PROVIDER=local \
SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2 \
VALKEY_HOST="${VALKEY_HOST}" \
VALKEY_PORT="${VALKEY_PORT}" \
OTEL_SDK_DISABLED=true \
RUNTARA_SDK_BACKEND=http \
RUNTARA_COMPILE_TARGET=wasm32-wasip2 \
SQLX_OFFLINE="${SQLX_OFFLINE}" \
"${RUNTARA_SERVER_BIN}" >"${TEST_LOG}" 2>&1 &
SERVER_PID=$!

# Wait for the public port to listen (up to 60s — first boot compiles the dispatcher).
for i in {1..60}; do
    if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
        break
    fi
    sleep 1
    if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
        print_error "Server exited during boot. Last 30 log lines:"
        tail -30 "${TEST_LOG}"
        exit 1
    fi
done
if ! curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
    print_error "Server failed to come up on :${TEST_PORT_PUBLIC} within 60s. Last 30 log lines:"
    tail -30 "${TEST_LOG}"
    exit 1
fi
echo "  Server up (PID ${SERVER_PID})"

#-------------------------------------------------------------------------
# Scenario 1: pg_trgm extension installed by the migration.
print_step "Scenario 1: pg_trgm extension is installed"
EXT=$(psql_quiet -d "${TEST_DB_SERVER}" -c "SELECT extname FROM pg_extension WHERE extname='pg_trgm'")
if [ "${EXT}" != "pg_trgm" ]; then
    print_error "pg_trgm not installed. Got: '${EXT}'"
    exit 1
fi
SIM_OK=$(psql_quiet -d "${TEST_DB_SERVER}" -c "SELECT 'foo' % 'fooo'")
if [ "${SIM_OK}" != "t" ]; then
    print_error "% operator not working. Got: '${SIM_OK}'"
    exit 1
fi
echo "  pg_trgm present and % operator works ✓"

#-------------------------------------------------------------------------
# Scenario 2: Schema-create with trigram column → GIN index.
print_step "Scenario 2: Schema-create with textIndex=trigram emits GIN index"
RESP=$(api_post /schemas '{
  "name": "Product",
  "tableName": "tier1_e2e_product",
  "columns": [
    {"name": "name", "type": "string"},
    {"name": "keywords", "type": "string", "textIndex": "trigram"},
    {"name": "status", "type": "string"}
  ]
}')
SCHEMA_ID=$(echo "${RESP}" | jq -r '.schemaId // empty')
if [ -z "${SCHEMA_ID}" ]; then
    print_error "Schema-create failed: ${RESP}"
    exit 1
fi
INDEX_AM=$(psql_quiet -d "${TEST_DB_SERVER}" -c "SELECT am.amname FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid JOIN pg_am am ON am.oid=c.relam WHERE c.relname='idx_tier1_e2e_product_keywords_trgm'")
if [ "${INDEX_AM}" != "gin" ]; then
    print_error "GIN trigram index missing. Got amname='${INDEX_AM}'"
    exit 1
fi
echo "  Schema ${SCHEMA_ID} created with GIN trigram index ✓"

#-------------------------------------------------------------------------
# Scenario 3: textIndex=trigram on Integer column → 400.
print_step "Scenario 3: textIndex=trigram on Integer column rejected"
assert_http_status "trigram-on-integer" 400 POST /schemas '{
  "name": "BadTrigramSchema",
  "tableName": "tier1_e2e_bad",
  "columns": [{"name": "qty", "type": "integer", "textIndex": "trigram"}]
}'

#-------------------------------------------------------------------------
# Scenario 4: Insert ~20 product rows.
print_step "Scenario 4: Insert 20 rows"
KEYWORDS=(
  "leather wallet brown small"
  "brown leather wallet medium"
  "leather backpack large brown"
  "blue denim jacket"
  "red cotton tshirt"
  "leather belt black"
  "brown leather messenger bag"
  "black leather wallet bifold"
  "white sneakers running"
  "navy blue jacket wool"
  "leather card holder slim"
  "cotton hoodie grey"
  "denim jeans skinny blue"
  "leather laptop bag 15 inch"
  "brown messenger leather satchel"
  "silk scarf floral"
  "leather phone case brown"
  "wool sweater navy crew"
  "leather wallet trifold black"
  "cotton socks white pair"
)
for kw in "${KEYWORDS[@]}"; do
    api_post /instances "{\"schemaName\":\"Product\",\"properties\":{\"name\":\"item\",\"keywords\":\"${kw}\",\"status\":\"active\"}}" >/dev/null
done
TOTAL=$(api_get "/instances/schema/name/Product?limit=100" | jq -r '.totalCount')
if [ "${TOTAL}" != "20" ]; then
    print_error "Expected 20 rows, got ${TOTAL}"
    exit 1
fi
echo "  20 rows inserted ✓"

#-------------------------------------------------------------------------
# Scenario 5: SIMILARITY_GTE + scoreExpression + alias orderBy → ranked Top-K.
print_step "Scenario 5: similarity ranking with score column"
QUERY='{
  "limit": 3,
  "condition": {
    "op": "AND",
    "arguments": [
      {"op": "EQ", "arguments": ["status", "active"]},
      {"op": "SIMILARITY_GTE", "arguments": ["keywords", "leather wallet brown", 0.3]}
    ]
  },
  "scoreExpression": {
    "alias": "score",
    "expression": {
      "fn": "SIMILARITY",
      "arguments": [
        {"valueType": "reference", "value": "keywords"},
        {"valueType": "immediate", "value": "leather wallet brown"}
      ]
    }
  },
  "orderBy": [
    {"expression": {"kind": "alias", "name": "score"}, "direction": "DESC"}
  ]
}'
RESP=$(api_post /instances/schema/Product/filter "${QUERY}")
TOP_KEYWORDS=$(echo "${RESP}" | jq -r '.instances[0].properties.keywords')
TOP_SCORE=$(echo "${RESP}" | jq -r '.instances[0].computed.score')
COUNT=$(echo "${RESP}" | jq -r '.instances | length')

if [ "${TOP_KEYWORDS}" != "leather wallet brown small" ]; then
    print_error "Top-1 ranking wrong. Expected 'leather wallet brown small', got '${TOP_KEYWORDS}'"
    echo "${RESP}" | jq .
    exit 1
fi
# Score should be > 0.3 (passed the filter) and a reasonable trigram value (< 1.0).
if ! awk "BEGIN { exit !(${TOP_SCORE} > 0.3 && ${TOP_SCORE} < 1.0) }"; then
    print_error "Top-1 score out of expected range: ${TOP_SCORE}"
    exit 1
fi
# Subsequent scores must be non-increasing.
SCORES=$(echo "${RESP}" | jq -r '.instances[].computed.score')
prev=2.0
for s in ${SCORES}; do
    if ! awk "BEGIN { exit !(${s} <= ${prev} + 1e-9) }"; then
        print_error "Scores not in DESC order: ${prev} -> ${s}"
        exit 1
    fi
    prev=${s}
done
echo "  Top match='${TOP_KEYWORDS}' score=${TOP_SCORE} (returned ${COUNT} rows, all DESC) ✓"

#-------------------------------------------------------------------------
# Scenario 6: SIMILARITY_GTE on non-string column → 400.
print_step "Scenario 6: SIMILARITY_GTE on non-string column rejected"
assert_http_status "similarity-non-string" 400 POST /instances/schema/Product/filter '{
  "limit": 1,
  "condition": {"op": "SIMILARITY_GTE", "arguments": ["createdAt", "x", 0.3]}
}'

#-------------------------------------------------------------------------
# Scenario 7: threshold > 1 → 400.
print_step "Scenario 7: SIMILARITY_GTE threshold out of range rejected"
assert_http_status "threshold-out-of-range" 400 POST /instances/schema/Product/filter '{
  "limit": 1,
  "condition": {"op": "SIMILARITY_GTE", "arguments": ["keywords", "x", 1.5]}
}'

#-------------------------------------------------------------------------
# Scenario 8: bad arity → 400.
print_step "Scenario 8: SIMILARITY_GTE bad arity rejected"
assert_http_status "similarity-bad-arity" 400 POST /instances/schema/Product/filter '{
  "limit": 1,
  "condition": {"op": "SIMILARITY_GTE", "arguments": ["keywords", "x"]}
}'

#-------------------------------------------------------------------------
# Scenario 9: invalid scoreExpression alias → 400.
print_step "Scenario 9: invalid scoreExpression alias rejected"
assert_http_status "bad-alias" 400 POST /instances/schema/Product/filter '{
  "limit": 1,
  "scoreExpression": {
    "alias": "1bad-alias",
    "expression": {"fn": "SIMILARITY", "arguments": [
      {"valueType": "reference", "value": "keywords"},
      {"valueType": "immediate", "value": "x"}
    ]}
  }
}'

#-------------------------------------------------------------------------
# Scenario 10: legacy sort_by still works without orderBy.
print_step "Scenario 10: legacy sortBy/sortOrder still works"
RESP=$(api_post /instances/schema/Product/filter '{
  "limit": 5,
  "sortBy": ["createdAt"],
  "sortOrder": ["desc"]
}')
COUNT=$(echo "${RESP}" | jq -r '.instances | length')
if [ "${COUNT}" != "5" ]; then
    print_error "legacy sortBy returned ${COUNT} rows, expected 5"
    exit 1
fi
echo "  legacy sortBy/sortOrder returned ${COUNT} rows ✓"

#-------------------------------------------------------------------------
# Scenario 11: query without scoreExpression → no `computed` key.
print_step "Scenario 11: response without scoreExpression has no computed field"
RESP=$(api_post /instances/schema/Product/filter '{"limit": 1}')
HAS_COMPUTED=$(echo "${RESP}" | jq -r '.instances[0] | has("computed")')
if [ "${HAS_COMPUTED}" != "false" ]; then
    print_error "Expected no 'computed' key when scoreExpression absent. Got: $(echo "${RESP}" | jq -r '.instances[0].computed')"
    exit 1
fi
echo "  no computed key when scoreExpression absent ✓"

#-------------------------------------------------------------------------
# Scenario 12: orderBy column target also works.
print_step "Scenario 12: orderBy column target"
RESP=$(api_post /instances/schema/Product/filter '{
  "limit": 3,
  "orderBy": [
    {"expression": {"kind": "column", "name": "createdAt"}, "direction": "ASC"}
  ]
}')
COUNT=$(echo "${RESP}" | jq -r '.instances | length')
if [ "${COUNT}" != "3" ]; then
    print_error "orderBy column returned ${COUNT} rows, expected 3"
    exit 1
fi
echo "  orderBy column target returned ${COUNT} rows ✓"

#-------------------------------------------------------------------------
echo ""
print_success "All Tier 1 scenarios passed ✓"
echo ""
echo "Summary:"
echo "  - pg_trgm extension installed by migration runner"
echo "  - GIN trigram index emitted on schema-create"
echo "  - SIMILARITY_GTE filter + structured scoreExpression + alias orderBy: ranked Top-K"
echo "  - Negative cases (non-string col, threshold range, arity, alias) → 400"
echo "  - Legacy sortBy/sortOrder still works"
echo "  - orderBy column target works"
