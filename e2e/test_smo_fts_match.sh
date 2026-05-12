#!/bin/bash
# E2E Test: SMO Object Model — Tier 2 full-text search
#
# Spins up runtara-server on test ports against fresh databases, exercises
# the Tier 2 surface (tsvector generated columns, MATCH operator, TS_RANK
# scoring), then tears everything down. Self-contained.
#
# Scenarios:
#   1. Schema-create with `type: "tsvector"` column emits a GIN index
#      and the source column survives validation.
#   2. INSERT does not need a value for the tsvector column; it's populated
#      automatically via the `GENERATED ALWAYS AS (to_tsvector(...)) STORED`
#      clause.
#   3. `extract_column_value` skips the tsvector — the tsvector column does
#      not appear in instance.properties on read.
#   4. MATCH operator filters rows and TS_RANK scores them; orderBy on the
#      ts_rank alias produces a sensible ranking for a multi-word query.
#   5. Negative cases: MATCH on a non-tsvector column → 400; TS_RANK on a
#      non-tsvector column → 400; tsvector source pointing at a non-string
#      column → 400; tsvector source pointing at a missing column → 400;
#      attempting to set a value on the tsvector column → 400.
#
# Prerequisites match test_smo_trigram_similarity.sh.

set -euo pipefail

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"
VALKEY_HOST="${VALKEY_HOST:-localhost}"
VALKEY_PORT="${VALKEY_PORT:-6379}"

TEST_DB_SERVER="tier2_e2e_server_$$"
TEST_DB_RUNTIME="tier2_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17600}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17601}"
TEST_DATA_DIR="$(mktemp -d -t runtara_tier2_e2e_XXXXXX)"
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

api_post() { curl -sS -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"; }
api_get()  { curl -sS "${API}$1"; }

assert_http_status() {
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
echo "E2E Test: SMO Object Model — Tier 2 full-text search"
echo "========================================================"

if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi

# Pre-flight.
print_step "Pre-flight: Postgres and Valkey..."
if ! psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1; then
    print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"
    exit 1
fi
if ! (echo > /dev/tcp/${VALKEY_HOST}/${VALKEY_PORT}) 2>/dev/null; then
    print_error "Cannot reach Valkey at ${VALKEY_HOST}:${VALKEY_PORT}"
    exit 1
fi

print_step "Creating test databases (${TEST_DB_SERVER}, ${TEST_DB_RUNTIME})..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC} (internal :${TEST_PORT_INTERNAL})..."
DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}" \
OBJECT_MODEL_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}" \
RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
TENANT_ID=tier2_e2e \
SERVER_HOST=127.0.0.1 \
SERVER_PORT="${TEST_PORT_PUBLIC}" \
INTERNAL_PORT="${TEST_PORT_INTERNAL}" \
RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:18600" \
DATA_DIR="${TEST_DATA_DIR}" \
RUST_LOG="warn,runtara_server=warn,runtara_object_store=warn" \
AUTH_PROVIDER=local \
SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2 \
VALKEY_HOST="${VALKEY_HOST}" \
VALKEY_PORT="${VALKEY_PORT}" \
OTEL_SDK_DISABLED=true \
RUNTARA_SDK_BACKEND=http \
RUNTARA_COMPILE_TARGET=wasm32-wasip2 \
ENABLE_OPERATOR_TESTING=false \
SQLX_OFFLINE="${SQLX_OFFLINE}" \
"${RUNTARA_SERVER_BIN}" >"${TEST_LOG}" 2>&1 &
SERVER_PID=$!

# Agent testing is disabled above, so boot skips the first-run dispatcher
# rustc compile; the ceiling stays generous for cold migrations / debug builds.
for i in {1..120}; do
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
if ! curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
    print_error "Server failed to come up on :${TEST_PORT_PUBLIC} within 120s."
    tail -30 "${TEST_LOG}"
    exit 1
fi
echo "  Server up (PID ${SERVER_PID})"

#-------------------------------------------------------------------------
# Scenario 1: Schema-create with tsvector → GIN index.
print_step "Scenario 1: Schema-create with tsvector emits GIN index"
RESP=$(api_post /schemas '{
  "name": "Article",
  "tableName": "tier2_e2e_article",
  "columns": [
    {"name": "title", "type": "string"},
    {"name": "body", "type": "string"},
    {"name": "title_tsv", "type": "tsvector", "sourceColumn": "title", "language": "english", "nullable": false},
    {"name": "body_tsv",  "type": "tsvector", "sourceColumn": "body",  "language": "english", "nullable": false}
  ]
}')
SCHEMA_ID=$(echo "${RESP}" | jq -r '.schemaId // empty')
if [ -z "${SCHEMA_ID}" ]; then
    print_error "Schema-create failed: ${RESP}"
    exit 1
fi
INDEX_AM=$(psql_quiet -d "${TEST_DB_SERVER}" -c "SELECT am.amname FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid JOIN pg_am am ON am.oid=c.relam WHERE c.relname='idx_tier2_e2e_article_title_tsv_fts'")
if [ "${INDEX_AM}" != "gin" ]; then
    print_error "GIN tsvector index missing for title_tsv. Got amname='${INDEX_AM}'"
    exit 1
fi
echo "  Schema ${SCHEMA_ID} created with GIN index on title_tsv ✓"

#-------------------------------------------------------------------------
# Scenario 2: tsvector source on non-string column → 400.
print_step "Scenario 2: tsvector source on non-string column rejected"
assert_http_status "tsvector-on-int-source" 400 POST /schemas '{
  "name": "BadSrc",
  "tableName": "tier2_e2e_bad_src",
  "columns": [
    {"name": "qty", "type": "integer"},
    {"name": "qty_tsv", "type": "tsvector", "sourceColumn": "qty"}
  ]
}'

#-------------------------------------------------------------------------
# Scenario 3: tsvector source pointing at missing column → 400.
print_step "Scenario 3: tsvector source missing column rejected"
assert_http_status "tsvector-missing-source" 400 POST /schemas '{
  "name": "MissingSrc",
  "tableName": "tier2_e2e_missing_src",
  "columns": [
    {"name": "title", "type": "string"},
    {"name": "title_tsv", "type": "tsvector", "sourceColumn": "nonexistent"}
  ]
}'

#-------------------------------------------------------------------------
# Scenario 4: Insert ~12 articles. tsvector columns must NOT be set.
print_step "Scenario 4: Insert articles (no tsvector value)"
TITLES=(
  "Postgres full text search guide"
  "Trigram similarity in postgres"
  "Indexing strategies for big tables"
  "Why json columns can be slow"
  "Building a search engine with sql"
  "Migrating from elasticsearch to postgres"
  "GIN vs GiST indexes"
  "Lexemes and stemming explained"
  "Managing tsvector generated columns"
  "Performance tips for full text queries"
  "Trigram and tsvector compared"
  "Stop words and dictionary configuration"
)
for t in "${TITLES[@]}"; do
    api_post /instances "{\"schemaName\":\"Article\",\"properties\":{\"title\":\"$t\",\"body\":\"$t — sample body content\"}}" >/dev/null
done
TOTAL=$(api_get "/instances/schema/name/Article?limit=100" | jq -r '.totalCount')
if [ "${TOTAL}" != "12" ]; then
    print_error "Expected 12 articles, got ${TOTAL}"
    exit 1
fi
echo "  12 articles inserted ✓"

#-------------------------------------------------------------------------
# Scenario 5: extract_column_value skips tsvector columns.
print_step "Scenario 5: tsvector column not surfaced in instance.properties"
RESP=$(api_post /instances/schema/Article/filter '{"limit":1}')
HAS_TSV=$(echo "${RESP}" | jq -r '.instances[0].properties | has("title_tsv")')
if [ "${HAS_TSV}" != "false" ]; then
    print_error "tsvector column leaked into properties: $(echo "${RESP}" | jq -r '.instances[0].properties.title_tsv')"
    exit 1
fi
echo "  tsvector column omitted from properties ✓"

#-------------------------------------------------------------------------
# Scenario 6: Trying to set a value for tsvector → 400.
print_step "Scenario 6: setting a tsvector value rejected"
assert_http_status "tsvector-write-rejected" 400 POST /instances '{
  "schemaName": "Article",
  "properties": {"title": "x", "body": "y", "title_tsv": "naive"}
}'

#-------------------------------------------------------------------------
# Scenario 7: MATCH filter + TS_RANK score + alias orderBy.
print_step "Scenario 7: MATCH filter + TS_RANK score + alias orderBy"
QUERY='{
  "limit": 5,
  "condition": {
    "op": "MATCH",
    "arguments": ["title_tsv", "trigram tsvector"]
  },
  "scoreExpression": {
    "alias": "rank",
    "expression": {
      "fn": "TS_RANK",
      "arguments": [
        {"valueType": "reference", "value": "title_tsv"},
        {"valueType": "immediate", "value": "trigram tsvector"}
      ]
    }
  },
  "orderBy": [{"expression": {"kind": "alias", "name": "rank"}, "direction": "DESC"}]
}'
RESP=$(api_post /instances/schema/Article/filter "${QUERY}")
COUNT=$(echo "${RESP}" | jq -r '.instances | length')
TOP=$(echo "${RESP}" | jq -r '.instances[0].properties.title')
if [ "${COUNT}" -lt "1" ]; then
    print_error "MATCH returned 0 instances; expected at least 1"
    echo "${RESP}" | jq .
    exit 1
fi
# The article literally titled "Trigram and tsvector compared" should rank
# highest for the query "trigram tsvector".
if [ "${TOP}" != "Trigram and tsvector compared" ]; then
    print_error "Top match wrong. Expected 'Trigram and tsvector compared', got '${TOP}'"
    echo "${RESP}" | jq .
    exit 1
fi
SCORES=$(echo "${RESP}" | jq -r '.instances[].computed.rank')
prev=2.0
for s in ${SCORES}; do
    if ! awk "BEGIN { exit !(${s} <= ${prev} + 1e-9) }"; then
        print_error "Ranks not in DESC order: ${prev} -> ${s}"
        exit 1
    fi
    prev=${s}
done
echo "  Top match='${TOP}' (returned ${COUNT}, all rank-DESC) ✓"

#-------------------------------------------------------------------------
# Scenario 8: MATCH on a non-tsvector column → 400.
print_step "Scenario 8: MATCH on non-tsvector column rejected"
assert_http_status "match-non-tsvector" 400 POST /instances/schema/Article/filter '{
  "limit": 1,
  "condition": {"op": "MATCH", "arguments": ["title", "anything"]}
}'

#-------------------------------------------------------------------------
# Scenario 9: TS_RANK on a non-tsvector column → 400.
print_step "Scenario 9: TS_RANK on non-tsvector column rejected"
assert_http_status "ts_rank-non-tsvector" 400 POST /instances/schema/Article/filter '{
  "limit": 1,
  "scoreExpression": {
    "alias": "rank",
    "expression": {"fn": "TS_RANK", "arguments": [
      {"valueType": "reference", "value": "title"},
      {"valueType": "immediate", "value": "x"}
    ]}
  }
}'

#-------------------------------------------------------------------------
# Scenario 10: MATCH bad arity → 400.
print_step "Scenario 10: MATCH bad arity rejected"
assert_http_status "match-bad-arity" 400 POST /instances/schema/Article/filter '{
  "limit": 1,
  "condition": {"op": "MATCH", "arguments": ["title_tsv"]}
}'

#-------------------------------------------------------------------------
echo ""
print_success "All Tier 2 scenarios passed ✓"
echo ""
echo "Summary:"
echo "  - Tsvector columns auto-generated via GENERATED ALWAYS AS STORED"
echo "  - GIN index emitted automatically per tsvector column"
echo "  - Tsvector column omitted from row properties on read"
echo "  - MATCH filter + TS_RANK score + alias orderBy: ranked Top-K"
echo "  - All five negative cases (non-string source, missing source,"
echo "    write-attempt, MATCH non-tsvector, TS_RANK non-tsvector,"
echo "    MATCH bad arity) → 400"
