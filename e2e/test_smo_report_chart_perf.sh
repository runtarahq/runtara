#!/bin/bash
# E2E Test: Report rendering performance fixes (correctness end-to-end)
#
# Exercises the three round-trip-reduction changes against a real server +
# Postgres object model, asserting the rendered output is still correct:
#
#   1. Parallel block rendering (render_blocks bounded buffer_unordered):
#      a two-block report renders both blocks and returns correct data.
#   2. Batched chart-column hydration (render_chart_column_cells): a table with
#      a per-row "chart" column issues ONE grouped aggregate for the whole page
#      instead of one-per-row, and the per-store series come back correct —
#      including a parent row with no matching children (empty series).
#   3. Schema metadata cache (ObjectStore get_schema): exercised implicitly by
#      every render; correctness here means the cache returns the same schema
#      the DB would.
#
# Self-contained: spins up runtara-server on test ports against fresh DBs,
# seeds data over the HTTP API, renders, asserts, tears down.

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"
VALKEY_HOST="${VALKEY_HOST:-localhost}"
VALKEY_PORT="${VALKEY_PORT:-6379}"

TEST_DB_SERVER="report_perf_e2e_server_$$"
TEST_DB_RUNTIME="report_perf_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17700}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17701}"
TEST_DATA_DIR="$(mktemp -d -t runtara_report_perf_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime/object-model"
REPORTS="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime/reports"

print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
}
api_post() { curl -sS -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"; }
om_instance() { api_post /instances "$1" >/dev/null; }

cleanup() {
    if [ -n "${SERVER_PID}" ] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME}" >/dev/null 2>&1 || true
    rm -rf "${TEST_DATA_DIR}" 2>/dev/null || true
}
trap cleanup EXIT

fail() { print_error "$1"; echo "--- server log tail ---"; tail -40 "${TEST_LOG}" 2>/dev/null || true; exit 1; }

echo "========================================================"
echo "E2E Test: Report rendering performance fixes (correctness)"
echo "========================================================"

if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi

print_step "Pre-flight: Postgres and Valkey..."
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || fail "Cannot reach Postgres"
(echo > /dev/tcp/${VALKEY_HOST}/${VALKEY_PORT}) 2>/dev/null || fail "Cannot reach Valkey"

print_step "Creating test databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
RUNTARA_SERVER_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}" \
OBJECT_MODEL_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}" \
RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
TENANT_ID=report_perf_e2e \
SERVER_HOST=127.0.0.1 \
SERVER_PORT="${TEST_PORT_PUBLIC}" \
INTERNAL_PORT="${TEST_PORT_INTERNAL}" \
RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:18700" \
RUNTARA_AGENT_COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}" \
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
    curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2" && break
    sleep 1
    kill -0 "${SERVER_PID}" 2>/dev/null || fail "Server exited during boot."
done
curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2" || fail "Server failed to come up."
echo "  Server up (PID ${SERVER_PID})"

#-------------------------------------------------------------------------
print_step "Seeding schemas (Store parent, Sale child)..."
# Store carries a large base64 'file upload' column the report never displays —
# the column projection must keep it out of the fetched rows / JSON payload.
api_post /schemas '{
  "name": "Store",
  "tableName": "report_perf_store",
  "columns": [
    {"name": "code", "type": "string"},
    {"name": "name", "type": "string"},
    {"name": "logo_base64", "type": "string"}
  ]
}' | jq -e '.schemaId' >/dev/null || fail "Store schema create failed"

api_post /schemas '{
  "name": "Sale",
  "tableName": "report_perf_sale",
  "columns": [
    {"name": "store_code", "type": "string"},
    {"name": "month", "type": "string"},
    {"name": "amount", "type": "integer"}
  ]
}' | jq -e '.schemaId' >/dev/null || fail "Sale schema create failed"

print_step "Seeding instances..."
# ~32 kB base64 blob per store (stand-in for a file upload), never displayed.
BLOB=$(head -c 24000 /dev/zero | base64 | tr -d '\n')
om_instance "$(jq -nc --arg b "$BLOB" '{schemaName:"Store",properties:{code:"ST1",name:"Alpha",logo_base64:$b}}')"
om_instance "$(jq -nc --arg b "$BLOB" '{schemaName:"Store",properties:{code:"ST2",name:"Beta",logo_base64:$b}}')"
om_instance "$(jq -nc --arg b "$BLOB" '{schemaName:"Store",properties:{code:"ST3",name:"Gamma",logo_base64:$b}}')"
# ST1: Jan 100, Feb 200 ; ST2: Jan 50 ; ST3: (no sales)
om_instance '{"schemaName":"Sale","properties":{"store_code":"ST1","month":"2026-01","amount":100}}'
om_instance '{"schemaName":"Sale","properties":{"store_code":"ST1","month":"2026-02","amount":200}}'
om_instance '{"schemaName":"Sale","properties":{"store_code":"ST2","month":"2026-01","amount":50}}'

#-------------------------------------------------------------------------
print_step "Creating two-block report with a chart column..."
REPORT_DEF='{
  "name": "Store Trends",
  "definition": {
    "definitionVersion": 1,
    "filters": [
      { "id": "store_filter", "label": "Store", "type": "text", "appliesTo": [{ "field": "code", "op": "eq" }] }
    ],
    "blocks": [
      {
        "id": "stores",
        "type": "table",
        "title": "Stores",
        "source": { "schema": "Store", "mode": "filter", "orderBy": [{"field":"code","direction":"asc"}] },
        "table": {
          "columns": [
            {"field": "code", "label": "Code"},
            {"field": "name", "label": "Name"},
            {
              "field": "trend", "label": "Trend", "type": "chart",
              "chart": {"kind":"line","series":[{"field":"total","label":"Total"}],"x":"month"},
              "source": {
                "schema": "Sale", "mode": "aggregate",
                "join": [{"field":"store_code","parentField":"code"}],
                "groupBy": ["month"],
                "aggregates": [{"alias":"total","field":"amount","op":"sum"}],
                "orderBy": [{"field":"month","direction":"asc"}]
              }
            }
          ]
        }
      },
      {
        "id": "stores2",
        "type": "table",
        "title": "Stores Copy",
        "source": { "schema": "Store", "mode": "filter", "orderBy": [{"field":"code","direction":"asc"}] },
        "table": { "columns": [ {"field":"code","label":"Code"}, {"field":"name","label":"Name"} ] }
      },
      {
        "id": "stores_sel",
        "type": "table",
        "title": "Selectable (must NOT project)",
        "source": { "schema": "Store", "mode": "filter", "orderBy": [{"field":"code","direction":"asc"}] },
        "table": { "selectable": true, "columns": [ {"field":"code","label":"Code"}, {"field":"name","label":"Name"} ] }
      },
      {
        "id": "stores_interactive",
        "type": "table",
        "title": "Row-click drilldown (interaction must NOT disable projection)",
        "source": { "schema": "Store", "mode": "filter", "orderBy": [{"field":"code","direction":"asc"}] },
        "table": { "columns": [ {"field":"code","label":"Code"}, {"field":"name","label":"Name"} ] },
        "interactions": [
          { "id": "open_store", "trigger": {"event":"row_click"},
            "actions": [{"type":"set_filter","filterId":"store_filter","valueFrom":"datum.code"}] }
        ]
      },
      {
        "id": "stores_joined",
        "type": "table",
        "title": "Joined-filter table (blob elided by catch-all)",
        "source": {
          "schema": "Store", "mode": "filter",
          "join": [{ "alias": "sale", "schema": "Sale", "parentField": "code", "field": "store_code", "kind": "left" }]
        },
        "table": { "columns": [ {"field":"code","label":"Code"}, {"field":"name","label":"Name"}, {"field":"sale.month","label":"Month"} ] }
      }
    ],
    "layout": {
      "id": "root", "columns": 1,
      "items": [
        {"id":"r0","child":{"id":"n0","type":"block","blockId":"stores"}},
        {"id":"r1","child":{"id":"n1","type":"block","blockId":"stores2"}},
        {"id":"r2","child":{"id":"n2","type":"block","blockId":"stores_sel"}},
        {"id":"r3","child":{"id":"n3","type":"block","blockId":"stores_interactive"}},
        {"id":"r4","child":{"id":"n4","type":"block","blockId":"stores_joined"}}
      ]
    }
  }
}'
CREATE_RESP=$(curl -sS -X POST -H "Content-Type: application/json" -d "${REPORT_DEF}" "${REPORTS}")
REPORT_ID=$(echo "${CREATE_RESP}" | jq -r '.id // .report.id // empty')
[ -n "${REPORT_ID}" ] || fail "Report create failed: ${CREATE_RESP}"
echo "  Report ${REPORT_ID} created"

#-------------------------------------------------------------------------
print_step "Rendering report..."
RENDER=$(curl -sS -X POST -H "Content-Type: application/json" -d '{"filters":{}}' "${REPORTS}/${REPORT_ID}/render")
echo "${RENDER}" > "${TEST_DATA_DIR}/render.json"

jq -e '.success == true' <<<"${RENDER}" >/dev/null || fail "render success != true: ${RENDER}"
jq -e '(.errors // []) | length == 0' <<<"${RENDER}" >/dev/null || fail "render had errors: $(jq -c .errors <<<"${RENDER}")"

assert() { # desc, jq-filter
    if jq -e "$2" <<<"${RENDER}" >/dev/null; then
        echo "  ✓ $1"
    else
        fail "assertion failed: $1"
    fi
}

# --- Block presence & parallel rendering (both blocks rendered) ---
assert "block 'stores' rendered with 3 rows" '.blocks.stores.data.rows | length == 3'
assert "block 'stores2' rendered with 3 rows (parallel block)" '.blocks.stores2.data.rows | length == 3'

# --- Column projection: the undisplayed base64 blob must NOT be fetched/returned ---
assert "stores rows still carry displayed columns code+name" \
  '[.blocks.stores.data.rows[] | (has("code") and has("name"))] | all'
assert "blob column 'logo_base64' projected OUT of stores rows" \
  '[.blocks.stores.data.rows[] | has("logo_base64")] | any | not'
assert "blob column 'logo_base64' projected OUT of stores2 rows" \
  '[.blocks.stores2.data.rows[] | has("logo_base64")] | any | not'
# The projected blocks (stores, stores2) carry NO blob bytes; the only blob in
# the payload comes from the selectable fallback block asserted below.
PROJECTED_BLOB=$(jq -r '[.blocks.stores, .blocks.stores2] | tostring | contains("AAAAAAAA")' <<<"${RENDER}")
[ "${PROJECTED_BLOB}" = "false" ] || fail "base64 blob leaked into a projected block's payload"
echo "  ✓ projected blocks carry no base64 blob bytes"
# Fallback safety: a selectable block ships whole rows to actions, so projection
# MUST fall back to all columns — the blob is present there (correctness > savings).
assert "selectable block falls back: 3 rows" '.blocks.stores_sel.data.rows | length == 3'
assert "selectable block keeps ALL columns incl. blob (no projection)" \
  '[.blocks.stores_sel.data.rows[] | has("logo_base64")] | all'

# A row-click interaction must NOT disable projection (the reported bug): its
# valueFrom field is collected, so the blob is still projected OUT.
assert "interaction table rendered 3 rows" \
  '.blocks.stores_interactive.data.rows | length == 3'
assert "interaction table keeps displayed code+name" \
  '[.blocks.stores_interactive.data.rows[] | (has("code") and has("name"))] | all'
assert "row-click interaction does NOT disable projection (blob excluded)" \
  '[.blocks.stores_interactive.data.rows[] | has("logo_base64")] | any | not'

# Report-agnostic catch-all: a joined-filter table is NOT SQL-projected (it
# fetches all columns), but the undisplayed blob is ELIDED in the response —
# present as a stub, never the full base64 — while displayed fields are kept.
assert "joined table rendered rows" '.blocks.stores_joined.data.rows | length >= 1'
assert "joined table keeps displayed code+name" \
  '[.blocks.stores_joined.data.rows[] | (has("code") and has("name"))] | all'
assert "joined table blob ELIDED by catch-all (stub, not full value)" \
  '[.blocks.stores_joined.data.rows[] | .logo_base64._elided == true] | all'
# The full ~32 kB-per-row blob must be gone; only the <=256-char preview stub
# remains. Size-bound the whole block: un-elided it would be 3 x ~32 kB.
JOINED_LEN=$(jq -r '.blocks.stores_joined | tostring | length' <<<"${RENDER}")
[ "${JOINED_LEN}" -lt 8000 ] || fail "joined-table block is ${JOINED_LEN} bytes — full blob not elided"
echo "  ✓ joined-table block compact (${JOINED_LEN} bytes — blob reduced to a preview stub)"

# --- Chart-column batching correctness (per-store series) ---
# ST1: Jan=100, Feb=200 (ordered by month asc)
assert "ST1 trend months" \
  '(.blocks.stores.data.rows[] | select(.code=="ST1") | [.trend.rows[][0]]) == ["2026-01","2026-02"]'
assert "ST1 trend totals" \
  '(.blocks.stores.data.rows[] | select(.code=="ST1") | [.trend.rows[][1] | tonumber]) == [100,200]'
# ST2: Jan=50
assert "ST2 trend single point" \
  '(.blocks.stores.data.rows[] | select(.code=="ST2") | [[.trend.rows[][0]],[.trend.rows[][1]|tonumber]]) == [["2026-01"],[50]]'
# ST3: no sales -> empty series, but cell columns still present
assert "ST3 trend empty series" \
  '(.blocks.stores.data.rows[] | select(.code=="ST3") | .trend.rows) == []'
assert "ST3 trend cell keeps columns [month,total]" \
  '(.blocks.stores.data.rows[] | select(.code=="ST3") | .trend.columns) == ["month","total"]'

#-------------------------------------------------------------------------
# Cross-check the batched chart total against a direct aggregate of the child
# schema, to prove the partitioned series equals the source-of-truth sum.
print_step "Cross-checking batched series vs direct DB aggregate..."
ST1_DB_SUM=$(psql_quiet -d "${TEST_DB_SERVER}" -c \
  "SELECT COALESCE(SUM(amount),0) FROM report_perf_sale WHERE store_code='ST1' AND deleted=FALSE")
ST1_REPORT_SUM=$(jq -r '[.blocks.stores.data.rows[] | select(.code=="ST1") | .trend.rows[][1] | tonumber] | add' <<<"${RENDER}")
[ "${ST1_DB_SUM}" = "${ST1_REPORT_SUM}" ] || fail "ST1 sum mismatch: DB=${ST1_DB_SUM} report=${ST1_REPORT_SUM}"
echo "  ✓ ST1 batched series sum (${ST1_REPORT_SUM}) matches DB (${ST1_DB_SUM})"

print_success "All report-perf correctness assertions passed."
