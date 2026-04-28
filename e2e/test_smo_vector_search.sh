#!/bin/bash
# E2E Test: SMO Object Model — Tier 3 pgvector embeddings + Levenshtein
#
# Spins up runtara-server on test ports against fresh databases, exercises
# the Tier 3 surface (vector(N) column type, HNSW + IVFFlat index emission,
# COSINE_DISTANCE / L2_DISTANCE / INNER_PRODUCT / LEVENSHTEIN ExprFns), then
# tears everything down. Self-contained.
#
# Scenarios:
#   1. pgvector + fuzzystrmatch extensions are loaded after server boot.
#   2. Schema-create with type:"vector" + indexMethod:hnsw emits an HNSW
#      index against the column.
#   3. Schema-create with type:"vector" + indexMethod:ivfflat(lists) emits
#      an IVFFlat index.
#   4. INSERT accepts vector values (JSON array of numbers); dimension match
#      enforced.
#   5. extract_column_value surfaces vector columns as JSON arrays of f64.
#   6. COSINE_DISTANCE score_expression + alias orderBy ASC: closest vector
#      first, distances monotonic ascending.
#   7. L2_DISTANCE score_expression: same monotonicity check.
#   8. LEVENSHTEIN("kitten", "sitting") = 3.
#   9. Negative cases:
#        - dimension mismatch on INSERT → 400
#        - vector dimension out of range → 400
#        - IVFFlat without lists > 0 → 400
#        - COSINE_DISTANCE on non-vector column → 400
#        - COSINE_DISTANCE literal length doesn't match column dimension → 400
#        - vector column in CSV import → 400
#
# Prerequisites:
#   - Postgres + Valkey reachable (matches test_smo_trigram_similarity.sh).
#   - Postgres has the `vector` and `fuzzystrmatch` extensions installed.
#     pgvector availability varies by managed Postgres provider; the
#     migration runner soft-fails if they aren't present.

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

TEST_DB_SERVER="tier3_e2e_server_$$"
TEST_DB_RUNTIME="tier3_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17700}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17701}"
TEST_DATA_DIR="$(mktemp -d -t runtara_tier3_e2e_XXXXXX)"
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
echo "============================================================"
echo "E2E Test: SMO Object Model — Tier 3 pgvector + Levenshtein"
echo "============================================================"

if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi

# Pre-flight.
print_step "Pre-flight: Postgres, Valkey, pgvector, fuzzystrmatch..."
if ! psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1; then
    print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"
    exit 1
fi
if ! (echo > /dev/tcp/${VALKEY_HOST}/${VALKEY_PORT}) 2>/dev/null; then
    print_error "Cannot reach Valkey at ${VALKEY_HOST}:${VALKEY_PORT}"
    exit 1
fi
AVAILABLE=$(psql_quiet -d postgres -c "SELECT name FROM pg_available_extensions WHERE name IN ('vector','fuzzystrmatch') ORDER BY name" | tr '\n' ' ')
if [ "${AVAILABLE}" != "fuzzystrmatch vector " ]; then
    print_error "Required extensions not available. Got: '${AVAILABLE}'. Install pgvector and fuzzystrmatch on the dev Postgres before running this test."
    exit 1
fi

print_step "Creating test databases (${TEST_DB_SERVER}, ${TEST_DB_RUNTIME})..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC} (internal :${TEST_PORT_INTERNAL})..."
DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}" \
OBJECT_MODEL_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}" \
RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
TENANT_ID=tier3_e2e \
SERVER_HOST=127.0.0.1 \
SERVER_PORT="${TEST_PORT_PUBLIC}" \
INTERNAL_PORT="${TEST_PORT_INTERNAL}" \
RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:18700" \
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

for i in {1..60}; do
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
    print_error "Server failed to come up on :${TEST_PORT_PUBLIC} within 60s."
    tail -30 "${TEST_LOG}"
    exit 1
fi
echo "  Server up (PID ${SERVER_PID})"

#-------------------------------------------------------------------------
# Scenario 1: Migration loaded both extensions on the server DB.
print_step "Scenario 1: vector + fuzzystrmatch extensions loaded after boot"
LOADED=$(psql_quiet -d "${TEST_DB_SERVER}" -c "SELECT extname FROM pg_extension WHERE extname IN ('vector','fuzzystrmatch') ORDER BY extname" | tr '\n' ' ')
if [ "${LOADED}" != "fuzzystrmatch vector " ]; then
    print_error "Expected both extensions in pg_extension; got '${LOADED}'"
    exit 1
fi
echo "  vector + fuzzystrmatch loaded ✓"

#-------------------------------------------------------------------------
# Scenario 2: HNSW index emission.
print_step "Scenario 2: Schema-create with vector + HNSW emits HNSW index"
RESP=$(api_post /schemas '{
  "name": "Doc",
  "tableName": "tier3_e2e_doc",
  "columns": [
    {"name": "title", "type": "string"},
    {"name": "qty", "type": "integer", "nullable": true},
    {"name": "embedding", "type": "vector", "dimension": 4, "indexMethod": {"type": "hnsw"}, "nullable": true}
  ]
}')
SCHEMA_ID=$(echo "${RESP}" | jq -r '.schemaId // empty')
if [ -z "${SCHEMA_ID}" ]; then
    print_error "HNSW schema-create failed: ${RESP}"
    exit 1
fi
INDEX_AM=$(psql_quiet -d "${TEST_DB_SERVER}" -c "SELECT am.amname FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid JOIN pg_am am ON am.oid=c.relam WHERE c.relname='idx_tier3_e2e_doc_embedding_hnsw'")
if [ "${INDEX_AM}" != "hnsw" ]; then
    print_error "HNSW vector index missing for embedding. Got amname='${INDEX_AM}'"
    exit 1
fi
COL_TYPE=$(psql_quiet -d "${TEST_DB_SERVER}" -c "SELECT format_type(atttypid, atttypmod) FROM pg_attribute WHERE attrelid='tier3_e2e_doc'::regclass AND attname='embedding'")
if [ "${COL_TYPE}" != "vector(4)" ]; then
    print_error "Expected column type 'vector(4)', got '${COL_TYPE}'"
    exit 1
fi
echo "  Schema ${SCHEMA_ID} created with vector(4) + HNSW index ✓"

#-------------------------------------------------------------------------
# Scenario 3: IVFFlat index emission.
print_step "Scenario 3: Schema-create with vector + IVFFlat emits IVFFlat index"
RESP=$(api_post /schemas '{
  "name": "DocIvf",
  "tableName": "tier3_e2e_doc_ivf",
  "columns": [
    {"name": "title", "type": "string"},
    {"name": "embedding", "type": "vector", "dimension": 4, "indexMethod": {"type": "ivfflat", "lists": 10}, "nullable": true}
  ]
}')
SCHEMA_ID_IVF=$(echo "${RESP}" | jq -r '.schemaId // empty')
if [ -z "${SCHEMA_ID_IVF}" ]; then
    print_error "IVFFlat schema-create failed: ${RESP}"
    exit 1
fi
INDEX_AM=$(psql_quiet -d "${TEST_DB_SERVER}" -c "SELECT am.amname FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid JOIN pg_am am ON am.oid=c.relam WHERE c.relname='idx_tier3_e2e_doc_ivf_embedding_ivf'")
if [ "${INDEX_AM}" != "ivfflat" ]; then
    print_error "IVFFlat vector index missing. Got amname='${INDEX_AM}'"
    exit 1
fi
echo "  IVFFlat index emitted ✓"

#-------------------------------------------------------------------------
# Scenario 4: Insert vectors and verify they round-trip.
print_step "Scenario 4: Insert vectors of correct dimension"
declare -a TITLES=("alpha" "beta" "gamma" "delta" "epsilon")
declare -a VECS=(
  "[1.0, 0.0, 0.0, 0.0]"
  "[0.0, 1.0, 0.0, 0.0]"
  "[0.0, 0.0, 1.0, 0.0]"
  "[0.0, 0.0, 0.0, 1.0]"
  "[0.7071, 0.7071, 0.0, 0.0]"
)
# qty values mean = 30, used by the AVG aggregate scenario.
declare -a QTYS=(10 20 30 40 50)
for i in "${!TITLES[@]}"; do
    api_post /instances "{\"schemaName\":\"Doc\",\"properties\":{\"title\":\"${TITLES[$i]}\",\"qty\":${QTYS[$i]},\"embedding\":${VECS[$i]}}}" >/dev/null
done
TOTAL=$(api_get "/instances/schema/name/Doc?limit=100" | jq -r '.totalCount')
if [ "${TOTAL}" != "5" ]; then
    print_error "Expected 5 docs, got ${TOTAL}"
    exit 1
fi
echo "  5 docs inserted ✓"

#-------------------------------------------------------------------------
# Scenario 5: Vector column round-trips as a JSON array of numbers.
print_step "Scenario 5: vector column surfaces as JSON array on read"
RESP=$(api_post /instances/schema/Doc/filter '{"limit":10,"sortBy":["title"],"sortOrder":["asc"]}')
ALPHA_EMBED=$(echo "${RESP}" | jq -c '.instances[] | select(.properties.title=="alpha") | .properties.embedding')
LEN=$(echo "${ALPHA_EMBED}" | jq 'length')
if [ "${LEN}" != "4" ]; then
    print_error "alpha embedding length mismatch: got '${ALPHA_EMBED}'"
    exit 1
fi
FIRST=$(echo "${ALPHA_EMBED}" | jq -r '.[0]')
if ! awk "BEGIN { exit !(${FIRST} > 0.99 && ${FIRST} < 1.01) }"; then
    print_error "alpha embedding[0] expected ~1.0, got '${FIRST}'"
    exit 1
fi
echo "  embedding=${ALPHA_EMBED} ✓"

#-------------------------------------------------------------------------
# Scenario 6: COSINE_DISTANCE alias orderBy ASC → closest first.
print_step "Scenario 6: COSINE_DISTANCE alias orderBy ASC"
QUERY='{
  "limit": 5,
  "scoreExpression": {
    "alias": "distance",
    "expression": {
      "fn": "COSINE_DISTANCE",
      "arguments": [
        {"valueType": "reference", "value": "embedding"},
        {"valueType": "immediate", "value": [1.0, 0.0, 0.0, 0.0]}
      ]
    }
  },
  "orderBy": [{"expression": {"kind": "alias", "name": "distance"}, "direction": "ASC"}]
}'
RESP=$(api_post /instances/schema/Doc/filter "${QUERY}")
TOP_TITLE=$(echo "${RESP}" | jq -r '.instances[0].properties.title')
TOP_DIST=$(echo "${RESP}" | jq -r '.instances[0].computed.distance')
if [ "${TOP_TITLE}" != "alpha" ]; then
    print_error "Expected alpha (identical vector) on top, got '${TOP_TITLE}'"
    echo "${RESP}" | jq .
    exit 1
fi
if ! awk "BEGIN { exit !(${TOP_DIST} < 0.001) }"; then
    print_error "alpha cosine distance to itself should be ~0, got ${TOP_DIST}"
    exit 1
fi
DISTS=$(echo "${RESP}" | jq -r '.instances[].computed.distance')
prev=-0.001
for d in ${DISTS}; do
    if ! awk "BEGIN { exit !(${d} >= ${prev} - 1e-6) }"; then
        print_error "Cosine distances not ASC: ${prev} -> ${d}"
        exit 1
    fi
    prev=${d}
done
echo "  Top match='${TOP_TITLE}' distance=${TOP_DIST}, monotonic ASC ✓"

#-------------------------------------------------------------------------
# Scenario 7: L2_DISTANCE.
print_step "Scenario 7: L2_DISTANCE alias orderBy ASC"
QUERY='{
  "limit": 5,
  "scoreExpression": {
    "alias": "distance",
    "expression": {
      "fn": "L2_DISTANCE",
      "arguments": [
        {"valueType": "reference", "value": "embedding"},
        {"valueType": "immediate", "value": [0.7071, 0.7071, 0.0, 0.0]}
      ]
    }
  },
  "orderBy": [{"expression": {"kind": "alias", "name": "distance"}, "direction": "ASC"}]
}'
RESP=$(api_post /instances/schema/Doc/filter "${QUERY}")
TOP_TITLE=$(echo "${RESP}" | jq -r '.instances[0].properties.title')
if [ "${TOP_TITLE}" != "epsilon" ]; then
    print_error "Expected epsilon (identical vector) on top, got '${TOP_TITLE}'"
    echo "${RESP}" | jq .
    exit 1
fi
echo "  Top L2 match='${TOP_TITLE}' ✓"

#-------------------------------------------------------------------------
# Scenario 8: LEVENSHTEIN("kitten","sitting") = 3.
print_step "Scenario 8: LEVENSHTEIN edit distance"
QUERY='{
  "limit": 1,
  "scoreExpression": {
    "alias": "edits",
    "expression": {
      "fn": "LEVENSHTEIN",
      "arguments": [
        {"valueType": "immediate", "value": "kitten"},
        {"valueType": "immediate", "value": "sitting"}
      ]
    }
  }
}'
RESP=$(api_post /instances/schema/Doc/filter "${QUERY}")
EDITS=$(echo "${RESP}" | jq -r '.instances[0].computed.edits')
# `edits` surfaces as f32 (real cast) — compare numerically, not string-wise.
if ! awk "BEGIN { exit !(${EDITS} == 3) }"; then
    print_error "Expected levenshtein(kitten, sitting)=3, got '${EDITS}'"
    echo "${RESP}" | jq .
    exit 1
fi
echo "  levenshtein('kitten','sitting')=${EDITS} ✓"

#-------------------------------------------------------------------------
# Scenario 9: AVG aggregate over qty (10,20,30,40,50) == 30.
print_step "Scenario 9: AVG aggregate"
RESP=$(api_post /instances/schema/Doc/aggregate '{
  "aggregates": [
    {"alias": "mean_qty", "fn": "AVG", "column": "qty"}
  ]
}')
MEAN=$(echo "${RESP}" | jq -r '.rows[0][0]')
if ! awk "BEGIN { exit !(${MEAN} == 30) }"; then
    print_error "Expected AVG(qty)=30, got '${MEAN}'"
    echo "${RESP}" | jq .
    exit 1
fi
echo "  AVG(qty)=${MEAN} ✓"

#-------------------------------------------------------------------------
# Scenario 10: Negative — AVG on a non-numeric column → 400.
print_step "Scenario 10: AVG on non-numeric column rejected"
assert_http_status "avg-on-string" 400 POST /instances/schema/Doc/aggregate '{
  "aggregates": [
    {"alias": "bad_avg", "fn": "AVG", "column": "title"}
  ]
}'

#-------------------------------------------------------------------------
# Scenario 11: COSINE_DISTANCE_LTE filter — neighbors within threshold.
# alpha = [1,0,0,0] is the query; threshold 0.5 (cosine ∈ [0, 2]).
# Self-distance = 0; orthogonal vectors (beta/gamma/delta) = 1; epsilon =
# [0.7071,0.7071,0,0] ≈ 0.293 (~45° from alpha). So expect alpha + epsilon.
print_step "Scenario 11: COSINE_DISTANCE_LTE returns neighbors within threshold"
RESP=$(api_post /instances/schema/Doc/filter '{
  "limit": 100,
  "condition": {
    "op": "COSINE_DISTANCE_LTE",
    "arguments": ["embedding", [1.0, 0.0, 0.0, 0.0], 0.5]
  },
  "sortBy": ["title"], "sortOrder": ["asc"]
}')
COUNT=$(echo "${RESP}" | jq -r '.instances | length')
TITLES=$(echo "${RESP}" | jq -r '.instances[].properties.title' | tr '\n' ',' | sed 's/,$//')
if [ "${COUNT}" != "2" ] || [ "${TITLES}" != "alpha,epsilon" ]; then
    print_error "Expected 2 neighbors (alpha,epsilon), got count=${COUNT} titles=${TITLES}"
    echo "${RESP}" | jq .
    exit 1
fi
echo "  cosine_distance<=0.5 → ${TITLES} ✓"

#-------------------------------------------------------------------------
# Scenario 12: L2_DISTANCE_LTE filter. Query = epsilon's vector itself;
# expect epsilon (self-distance 0) plus neighbors with L2 distance ≤ 1.0.
# beta = [0,1,0,0] → L2 distance to [0.7071,0.7071,0,0] ≈ 0.7654 → in.
# alpha = [1,0,0,0] → same ≈ 0.7654 → in.
# gamma/delta orthogonal axes → L2 ≈ 1.225 → out.
print_step "Scenario 12: L2_DISTANCE_LTE returns neighbors within threshold"
RESP=$(api_post /instances/schema/Doc/filter '{
  "limit": 100,
  "condition": {
    "op": "L2_DISTANCE_LTE",
    "arguments": ["embedding", [0.7071, 0.7071, 0.0, 0.0], 1.0]
  },
  "sortBy": ["title"], "sortOrder": ["asc"]
}')
COUNT=$(echo "${RESP}" | jq -r '.instances | length')
TITLES=$(echo "${RESP}" | jq -r '.instances[].properties.title' | tr '\n' ',' | sed 's/,$//')
if [ "${COUNT}" != "3" ] || [ "${TITLES}" != "alpha,beta,epsilon" ]; then
    print_error "Expected 3 neighbors (alpha,beta,epsilon), got count=${COUNT} titles=${TITLES}"
    echo "${RESP}" | jq .
    exit 1
fi
echo "  l2_distance<=1.0 → ${TITLES} ✓"

#-------------------------------------------------------------------------
# Scenario 13: Negative — INSERT with wrong dimension → 400.
print_step "Scenario 13: dimension mismatch on INSERT rejected"
assert_http_status "vector-dim-mismatch" 400 POST /instances '{
  "schemaName": "Doc",
  "properties": {"title": "bad", "embedding": [1.0, 0.0, 0.0]}
}'

#-------------------------------------------------------------------------
# Scenario 14: Negative — vector dimension out of range (16001) → 400.
print_step "Scenario 14: vector dimension > 16000 rejected"
assert_http_status "vector-dim-range" 400 POST /schemas '{
  "name": "BadDim",
  "tableName": "tier3_e2e_bad_dim",
  "columns": [
    {"name": "v", "type": "vector", "dimension": 16001}
  ]
}'

#-------------------------------------------------------------------------
# Scenario 15: Negative — IVFFlat with lists=0 → 400.
print_step "Scenario 15: IVFFlat with lists=0 rejected"
assert_http_status "vector-ivf-lists" 400 POST /schemas '{
  "name": "BadIvf",
  "tableName": "tier3_e2e_bad_ivf",
  "columns": [
    {"name": "v", "type": "vector", "dimension": 4, "indexMethod": {"type": "ivfflat", "lists": 0}}
  ]
}'

#-------------------------------------------------------------------------
# Scenario 16: Negative — COSINE_DISTANCE on non-vector column → 400.
print_step "Scenario 16: COSINE_DISTANCE on non-vector column rejected"
assert_http_status "cosine-on-string" 400 POST /instances/schema/Doc/filter '{
  "limit": 1,
  "scoreExpression": {
    "alias": "distance",
    "expression": {"fn": "COSINE_DISTANCE", "arguments": [
      {"valueType": "reference", "value": "title"},
      {"valueType": "immediate", "value": [1.0, 0.0, 0.0, 0.0]}
    ]}
  }
}'

#-------------------------------------------------------------------------
# Scenario 17: Negative — distance literal length doesn't match column dim.
print_step "Scenario 17: distance literal length mismatch rejected"
assert_http_status "cosine-literal-dim" 400 POST /instances/schema/Doc/filter '{
  "limit": 1,
  "scoreExpression": {
    "alias": "distance",
    "expression": {"fn": "COSINE_DISTANCE", "arguments": [
      {"valueType": "reference", "value": "embedding"},
      {"valueType": "immediate", "value": [1.0, 0.0]}
    ]}
  }
}'

#-------------------------------------------------------------------------
# Scenario 18: Negative — COSINE_DISTANCE_LTE on non-vector column → 400.
print_step "Scenario 18: COSINE_DISTANCE_LTE on non-vector column rejected"
assert_http_status "cosine-lte-on-string" 400 POST /instances/schema/Doc/filter '{
  "limit": 1,
  "condition": {
    "op": "COSINE_DISTANCE_LTE",
    "arguments": ["title", [1.0, 0.0, 0.0, 0.0], 0.5]
  }
}'

#-------------------------------------------------------------------------
# Scenario 19: Negative — distance threshold literal dim mismatch → 400.
print_step "Scenario 19: COSINE_DISTANCE_LTE literal dim mismatch rejected"
assert_http_status "cosine-lte-dim" 400 POST /instances/schema/Doc/filter '{
  "limit": 1,
  "condition": {
    "op": "COSINE_DISTANCE_LTE",
    "arguments": ["embedding", [1.0, 0.0], 0.5]
  }
}'

#-------------------------------------------------------------------------
echo ""
print_success "All Tier 3 scenarios passed ✓"
echo ""
echo "Summary:"
echo "  - vector(N) + HNSW + IVFFlat indexes emitted at schema-create"
echo "  - Vector inserts validated against declared dimension"
echo "  - Vector column round-trips as JSON array of numbers"
echo "  - COSINE_DISTANCE / L2_DISTANCE: monotonic ASC ordering"
echo "  - LEVENSHTEIN returns expected edit distance"
echo "  - AVG aggregate returns arithmetic mean"
echo "  - COSINE_DISTANCE_LTE / L2_DISTANCE_LTE filter neighbors within"
echo "    a threshold (no Top-K limit)"
echo "  - Seven negative cases (INSERT dim mismatch, dim out of range,"
echo "    IVFFlat lists=0, AVG on non-numeric, COSINE_DISTANCE on"
echo "    non-vector, literal dim mismatch, COSINE_DISTANCE_LTE on"
echo "    non-vector, COSINE_DISTANCE_LTE literal dim) → 400"
