#!/bin/bash
# E2E Test: report conditions accept either wire-shape at every location
#
# The reports DSL carries two encodings for "a condition". Source filters are
# flat and positional — {op, arguments} with the field name as the first operand
# — because they push down to SQL. Row-visibility conditions
# (visibleWhen/hiddenWhen/disabledWhen) use the tagged ConditionExpression form
# with typed operands, because they are evaluated in memory against a rendered
# row. Each location now accepts *either* encoding and normalizes to its own
# canonical one, so an author never has to switch shapes by tree position.
#
# Unit tests cover the converter and each validator in isolation. This test
# covers the wiring they can only reach by proxy: the real save path, the stored
# definition, and a render that pushes the normalized condition down to SQL.
#
#   1. Seed an Object Model schema + rows.
#   2. Create a report with each condition written in the OTHER surface's shape.
#   3. Assert the create passes save-time validation, the stored definition comes
#      back normalized per location, and the render actually filters rows.
#   4. Assert a canonically-written definition stores byte-identically.
#
# Prerequisites: Postgres and Valkey (the repo's docker dev stack provides both).
# No workflow components are needed — nothing here executes a workflow.

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
VALKEY_HOST="${VALKEY_HOST:-localhost}"
VALKEY_PORT="${VALKEY_PORT:-6379}"

TEST_DB_SERVER="cond_shape_e2e_server_$$"
TEST_DB_RUNTIME="cond_shape_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17740}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17741}"
TEST_DATA_DIR="$(mktemp -d -t runtara_cond_shape_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

OM="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime/object-model"
REPORTS="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime/reports"

print_step() { echo -e "${GREEN}[STEP]${NC} $1"; }
print_ok()   { echo -e "${GREEN}  ✓${NC} $1"; }

# Prefer psql on PATH; fall back to the dev Postgres container when the host
# has the server but not the client tools.
PG_CONTAINER="${PG_CONTAINER:-runtara-dev-postgres}"
if command -v psql >/dev/null 2>&1; then
    psql_quiet() {
        PGPASSWORD="${POSTGRES_PASSWORD}" psql \
            -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
    }
else
    psql_quiet() {
        docker exec -e PGPASSWORD="${POSTGRES_PASSWORD}" -i "${PG_CONTAINER}" \
            psql -U "${POSTGRES_USER}" -h 127.0.0.1 -p 5432 -tA "$@"
    }
fi

om_post() { curl -sS -X POST -H "Content-Type: application/json" -d "$2" "${OM}$1"; }

fail() {
    echo -e "${RED}[ERROR]${NC} $1"
    echo "--- server log tail ---"
    tail -40 "${TEST_LOG}" 2>/dev/null || true
    exit 1
}

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

echo "==============================================================="
echo "E2E Test: report conditions accept either wire-shape"
echo "==============================================================="

if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi

print_step "Pre-flight: Postgres and Valkey..."
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || fail "Cannot reach Postgres"
(echo > /dev/tcp/${VALKEY_HOST}/${VALKEY_PORT}) 2>/dev/null || fail "Cannot reach Valkey"

print_step "Creating test databases (${TEST_DB_SERVER}, ${TEST_DB_RUNTIME})..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
RUNTARA_SERVER_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}" \
OBJECT_MODEL_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}" \
RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
TENANT_ID=cond_shape_e2e \
SERVER_HOST=127.0.0.1 \
SERVER_PORT="${TEST_PORT_PUBLIC}" \
INTERNAL_PORT="${TEST_PORT_INTERNAL}" \
RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:18740" \
RUNTARA_AGENT_COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}" \
DATA_DIR="${TEST_DATA_DIR}" \
RUST_LOG="warn" \
AUTH_PROVIDER=local \
SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2 \
VALKEY_HOST="${VALKEY_HOST}" \
VALKEY_PORT="${VALKEY_PORT}" \
OTEL_SDK_DISABLED=true \
RUNTARA_SDK_BACKEND=http \
SQLX_OFFLINE="${SQLX_OFFLINE}" \
"${RUNTARA_SERVER_BIN}" >"${TEST_LOG}" 2>&1 &
SERVER_PID=$!

for i in {1..60}; do
    curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2" && break
    sleep 1
    kill -0 "${SERVER_PID}" 2>/dev/null || fail "Server exited during boot."
done
print_ok "server up (PID ${SERVER_PID})"

#-------------------------------------------------------------------------
print_step "Seeding Order schema + rows..."
om_post /schemas '{
  "name": "Order",
  "tableName": "cond_shape_order",
  "columns": [
    {"name": "order_id", "type": "string"},
    {"name": "customer_name", "type": "string"},
    {"name": "status", "type": "string"},
    {"name": "total_amount", "type": "integer"}
  ]
}' | jq -e '.schemaId' >/dev/null || fail "Order schema create failed"

for row in \
  '{"order_id":"O-1","customer_name":"Alpha","status":"pending_review","total_amount":100}' \
  '{"order_id":"O-2","customer_name":"Beta","status":"on_hold","total_amount":200}' \
  '{"order_id":"O-3","customer_name":"Gamma","status":"cancelled","total_amount":300}'; do
    om_post /instances "$(jq -nc --argjson p "$row" '{schemaName:"Order",properties:$p}')" >/dev/null
done
print_ok "3 rows seeded (one cancelled, which the source condition must exclude)"

#-------------------------------------------------------------------------
print_step "Creating report with each condition in the OTHER surface's wire-shape..."
# source.condition uses the tagged form; visibleWhen/hiddenWhen use the flat one.
REPORT_DEF='{
  "name": "Condition shape interop",
  "definition": {
    "definitionVersion": 1,
    "blocks": [
      {
        "id": "orders",
        "type": "table",
        "title": "Orders",
        "source": {
          "schema": "Order",
          "mode": "filter",
          "orderBy": [{"field": "order_id", "direction": "asc"}],
          "condition": {
            "type": "operation",
            "op": "NE",
            "arguments": [
              {"valueType": "reference", "value": "status"},
              {"valueType": "immediate", "value": "cancelled"}
            ]
          }
        },
        "table": {
          "columns": [
            {"field": "order_id", "label": "Order"},
            {"field": "status", "label": "Status"},
            {"field": "total_amount", "label": "Total"},
            {
              "field": "actions", "label": "Actions", "type": "interaction_buttons",
              "interactionButtons": [
                {
                  "id": "hold_note",
                  "label": "Hold note",
                  "visibleWhen": {"op": "EQ", "arguments": ["status", "on_hold"]},
                  "hiddenWhen": {"op": "IS_EMPTY", "arguments": ["customer_name"]},
                  "actions": [{"type": "clear_filters"}]
                }
              ]
            }
          ]
        }
      }
    ]
  }
}'
CREATE_RESP=$(curl -sS -X POST -H "Content-Type: application/json" -d "${REPORT_DEF}" "${REPORTS}")
REPORT_ID=$(echo "${CREATE_RESP}" | jq -r '.report.id // empty')
[ -n "${REPORT_ID}" ] || fail "report create rejected the cross-shape definition: ${CREATE_RESP}"
print_ok "report ${REPORT_ID} saved — save-time validation accepted both cross-shape conditions"

#-------------------------------------------------------------------------
print_step "Reading it back: each location must hold its canonical shape..."
STORED=$(curl -sS "${REPORTS}/${REPORT_ID}" | jq '.report.definition')
SRC=$(echo "${STORED}" | jq -c '.blocks[0].source.condition')
BTN=$(echo "${STORED}" | jq -c '.blocks[0].table.columns[3].interactionButtons[0]')

echo "${SRC}" | jq -e 'has("type") | not' >/dev/null \
    || fail "source.condition kept the tagged shape: ${SRC}"
echo "${SRC}" | jq -e '.op == "NE" and .arguments[0] == "status" and .arguments[1] == "cancelled"' >/dev/null \
    || fail "source.condition did not flatten correctly: ${SRC}"
print_ok "source.condition flattened: ${SRC}"

echo "${BTN}" | jq -e '.visibleWhen.type == "operation"' >/dev/null \
    || fail "visibleWhen not lifted: ${BTN}"
echo "${BTN}" | jq -e '.visibleWhen.arguments[0].valueType == "reference" and .visibleWhen.arguments[0].value == "status"' >/dev/null \
    || fail "visibleWhen field operand lost: ${BTN}"
echo "${BTN}" | jq -e '.visibleWhen.arguments[1].valueType == "immediate" and .visibleWhen.arguments[1].value == "on_hold"' >/dev/null \
    || fail "visibleWhen literal operand lost: ${BTN}"
echo "${BTN}" | jq -e '.hiddenWhen.type == "operation" and .hiddenWhen.op == "IS_EMPTY"' >/dev/null \
    || fail "hiddenWhen not lifted: ${BTN}"
print_ok "visibleWhen lifted: $(echo "${BTN}" | jq -c '.visibleWhen')"
print_ok "hiddenWhen lifted:  $(echo "${BTN}" | jq -c '.hiddenWhen')"

#-------------------------------------------------------------------------
print_step "Rendering: the normalized source condition must actually filter rows..."
RENDER=$(curl -sS -X POST -H "Content-Type: application/json" -d '{"filters":{}}' "${REPORTS}/${REPORT_ID}/render")
echo "${RENDER}" | jq -e '(.errors | length) == 0' >/dev/null \
    || fail "render errors: $(echo "${RENDER}" | jq -c '.errors')"
IDS=$(echo "${RENDER}" | jq -c '[.blocks.orders.data.rows[]?.order_id]')
[ "${IDS}" = '["O-1","O-2"]' ] \
    || fail "expected the cancelled row excluded by the normalized condition, got ${IDS}"
print_ok "rendered rows ${IDS} — O-3 (cancelled) excluded by the flattened source condition"

#-------------------------------------------------------------------------
print_step "The canonical shapes must store identically..."
CANON=$(echo "${REPORT_DEF}" | jq '
  .name = "Canonical shapes" |
  .definition.blocks[0].source.condition = {op:"NE", arguments:["status","cancelled"]} |
  .definition.blocks[0].table.columns[3].interactionButtons[0].visibleWhen = {
    type:"operation", op:"EQ",
    arguments:[{valueType:"reference",value:"status"},{valueType:"immediate",value:"on_hold"}]
  } |
  .definition.blocks[0].table.columns[3].interactionButtons[0].hiddenWhen = {
    type:"operation", op:"IS_EMPTY",
    arguments:[{valueType:"reference",value:"customer_name"}]
  }')
CANON_RESP=$(curl -sS -X POST -H "Content-Type: application/json" -d "${CANON}" "${REPORTS}")
CANON_ID=$(echo "${CANON_RESP}" | jq -r '.report.id // empty')
[ -n "${CANON_ID}" ] || fail "canonical definition rejected: ${CANON_RESP}"
CANON_STORED=$(curl -sS "${REPORTS}/${CANON_ID}" | jq '.report.definition')
diff <(echo "${STORED}" | jq -S '.blocks[0]') <(echo "${CANON_STORED}" | jq -S '.blocks[0]') >/dev/null \
    || fail "cross-shape and canonical definitions did not converge to the same stored block"
print_ok "cross-shape and canonical definitions store byte-identical blocks"

echo ""
echo -e "${GREEN}===============================================================${NC}"
echo -e "${GREEN}PASS — both wire-shapes accepted, normalized, and executed${NC}"
echo -e "${GREEN}===============================================================${NC}"
