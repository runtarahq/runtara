#!/bin/bash
# E2E Test: object-model raw SQL capabilities (query-sql / execute-sql)
#
# Exercises the full feature from docs/object-model-raw-sql-plan-2026-07-03.md
# against a self-contained server stack:
#
#   Leg 1  Main graph, all six motivating SQL features:
#          create-instance x3 -> execute-sql DDL (CREATE TABLE IF NOT EXISTS)
#          -> execute-sql computed UPDATE with a $1 reference param
#          -> execute-sql advisory-locked CTE full-replace with a FILTER
#             aggregate (INSERT...SELECT...GROUP BY write-back)
#          -> query-sql with CROSS JOIN (VALUES ...), array_agg(... ORDER BY)
#             subscript, and a typed result_schema
#          -> Finish
#   Leg 2  Run the same compiled binary AGAIN: the full-replace must not
#          duplicate derived rows (idempotent rebuild).
#   Leg 3  Internal-API contract, direct curl against :INTERNAL_PORT:
#          multi-statement string -> HTTP 400 and nothing executed;
#          write-spelled-as-query -> HTTP 400 (READ ONLY transaction).
#   Leg 4  SIGTERM exactly-once: counter-bump execute-sql -> delay; drain
#          mid-delay, restart, instance resumes; checkpoint-cache hit means
#          the bump applied exactly once.
#   Leg 5  CRON trigger fires the rebuild workflow through the Valkey trigger
#          stream (the path direct execute never touches).
#
# Prerequisites: Postgres + docker (isolated Valkey) and the agent / shared
# workflow components in target/wasm32-wasip2/release
# (see scripts/build-agent-components.sh).

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

TEST_DB_SERVER="obm_sql_e2e_server_$$"
TEST_DB_RUNTIME="obm_sql_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17720}"
TEST_PORT_INTERNAL="${TEST_PORT_INTERNAL:-17721}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18711}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18712}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18713}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18714}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16391}"
TEST_DATA_DIR="$(mktemp -d -t runtara_obm_sql_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="obm_sql_e2e"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1"; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql \
        -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" \
        -tA "$@"
}

psql_server() { psql_quiet -d "${TEST_DB_SERVER}" "$@"; }

cleanup() {
    if [ -n "${SERVER_PID}" ] && kill -0 "${SERVER_PID}" 2>/dev/null; then
        kill -9 "${SERVER_PID}" 2>/dev/null || true
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
INTERNAL="http://127.0.0.1:${TEST_PORT_INTERNAL}/api/internal"

api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" \
        -d "$2" "${API}$1"
}

# Start (or restart, for the SIGTERM leg) runtara-server with identical env so
# the wake scheduler and orphan recovery see the same DBs / data dir / Valkey.
# RUNTARA_DEV_MODE=false keeps the graceful drain (debug builds skip it).
start_server() {
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
    RUST_LOG="warn,runtara_server=info,runtara_object_store=warn" \
    AUTH_PROVIDER=local \
    SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2 \
    VALKEY_HOST=127.0.0.1 \
    VALKEY_PORT="${TEST_VALKEY_PORT}" \
    OTEL_SDK_DISABLED=true \
    RUNTARA_SDK_BACKEND=http \
    RUNTARA_DEV_MODE=false \
    SQLX_OFFLINE="${SQLX_OFFLINE}" \
    "${RUNTARA_SERVER_BIN}" >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!

    for i in {1..60}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        sleep 1
        if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
            print_error "Server exited during boot."
            tail -30 "${TEST_LOG}"
            exit 1
        fi
    done
    print_error "Server did not become healthy."
    tail -30 "${TEST_LOG}"
    exit 1
}

instance_status() {
    curl -sS "${API}/workflows/instances/$1" | jq -r '.data.status // .status // empty'
}

wait_completed() {
    local id="$1" status=""
    for i in {1..90}; do
        status=$(instance_status "${id}")
        case "${status}" in completed|failed|crashed|stopped) break ;; esac
        sleep 2
    done
    if [ "${status}" != "completed" ]; then
        print_error "Instance ${id} did not complete (status='${status}')"
        curl -sS "${API}/workflows/instances/${id}" | jq . || true
        tail -40 "${TEST_LOG}"
        exit 1
    fi
}

#-------------------------------------------------------------------------
echo "==============================================================="
echo "E2E Test: object-model raw SQL (query-sql / execute-sql)"
echo "==============================================================="

if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi
for f in runtara_agent_object_model.wasm runtara_agent_utils.wasm runtara_workflow_stdlib.wasm runtara_workflow_runtime.wasm; do
    if [ ! -f "${COMPONENTS_DIR}/${f}" ]; then
        print_error "Missing component ${COMPONENTS_DIR}/${f} — run scripts/build-agent-components.sh"
        exit 1
    fi
done

print_step "Pre-flight: Postgres and docker..."
if ! psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1; then
    print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"
    exit 1
fi
if ! docker info >/dev/null 2>&1; then
    print_error "docker is required (isolated Valkey for the trigger stream)"
    exit 1
fi

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for i in {1..20}; do
    if (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null; then break; fi
    sleep 0.5
done

print_step "Creating test databases (${TEST_DB_SERVER}, ${TEST_DB_RUNTIME})..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null
SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
start_server
echo "  Server up (PID ${SERVER_PID})"

#-------------------------------------------------------------------------
print_step "Creating object schema StockSnapshot..."
RESP=$(api_post /object-model/schemas '{
  "name": "StockSnapshot",
  "tableName": "obm_sql_e2e_stock",
  "columns": [
    {"name": "sku", "type": "string"},
    {"name": "qty", "type": "integer"},
    {"name": "age_days", "type": "integer"}
  ]
}')
SCHEMA_ID=$(echo "${RESP}" | jq -r '.schemaId // empty')
if [ -z "${SCHEMA_ID}" ]; then
    print_error "Schema create failed: ${RESP}"
    exit 1
fi
echo "  Schema ${SCHEMA_ID} ✓"

print_step "Creating postgres connection..."
RESP=$(api_post /connections "{
  \"title\": \"obm-sql-e2e store\",
  \"integrationId\": \"postgres\",
  \"connectionParameters\": {\"database_url\": \"${SERVER_DB_URL}\"}
}")
CONN_ID=$(echo "${RESP}" | jq -r '.connection_id // .connectionId // empty')
if [ -z "${CONN_ID}" ]; then
    print_error "Connection create failed: ${RESP}"
    exit 1
fi
echo "  Connection ${CONN_ID} ✓"

#-------------------------------------------------------------------------
print_step "Creating rebuild workflow..."
RESP=$(api_post /workflows/create '{"name": "sql-rebuild", "description": "raw SQL derived-table rebuild (DDL, computed UPDATE, CTE full-replace, windowed read)"}')
WF_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${WF_ID}" ] || { print_error "Workflow create failed: ${RESP}"; exit 1; }
echo "  Workflow ${WF_ID} ✓"

print_step "Pushing rebuild workflow definition..."
DEFINITION=$(jq -n --arg conn "${CONN_ID}" '{
  name: "sql-rebuild",
  steps: {
    c1: {
      stepType: "Agent", id: "c1",
      agentId: "object_model", capabilityId: "create-instance",
      connectionId: $conn,
      inputMapping: {
        schema_name: {valueType: "immediate", value: "StockSnapshot"},
        data: {valueType: "immediate", value: {sku: "A", qty: 5}}
      }
    },
    c2: {
      stepType: "Agent", id: "c2",
      agentId: "object_model", capabilityId: "create-instance",
      connectionId: $conn,
      inputMapping: {
        schema_name: {valueType: "immediate", value: "StockSnapshot"},
        data: {valueType: "immediate", value: {sku: "A", qty: 0}}
      }
    },
    c3: {
      stepType: "Agent", id: "c3",
      agentId: "object_model", capabilityId: "create-instance",
      connectionId: $conn,
      inputMapping: {
        schema_name: {valueType: "immediate", value: "StockSnapshot"},
        data: {valueType: "immediate", value: {sku: "B", qty: 7}}
      }
    },
    ddl: {
      stepType: "Agent", id: "ddl",
      agentId: "object_model", capabilityId: "execute-sql",
      connectionId: $conn,
      inputMapping: {
        sql: {valueType: "immediate", value: "CREATE TABLE IF NOT EXISTS sku_velocity (sku TEXT PRIMARY KEY, in_stock BIGINT)"}
      }
    },
    age: {
      stepType: "Agent", id: "age",
      agentId: "object_model", capabilityId: "execute-sql",
      connectionId: $conn,
      inputMapping: {
        sql: {valueType: "immediate", value: "UPDATE obm_sql_e2e_stock SET age_days = (CURRENT_DATE - created_at::date) WHERE id::text = $1"},
        params: {valueType: "composite", value: [
          {valueType: "composite", value: {
            type: {valueType: "immediate", value: "string"},
            value: {valueType: "reference", value: "steps.c1.outputs.instance_id"}
          }}
        ]}
      }
    },
    rebuild: {
      stepType: "Agent", id: "rebuild",
      agentId: "object_model", capabilityId: "execute-sql",
      connectionId: $conn,
      inputMapping: {
        sql: {valueType: "immediate", value: "WITH lock AS (SELECT pg_advisory_xact_lock(hashtext($tag$rebuild:sku_velocity$tag$))), del AS (DELETE FROM sku_velocity WHERE (SELECT true FROM lock) RETURNING 1) INSERT INTO sku_velocity (sku, in_stock) SELECT sku, COALESCE(SUM(qty) FILTER (WHERE qty > 0), 0) FROM obm_sql_e2e_stock WHERE (SELECT count(*) FROM del) >= 0 GROUP BY sku"}
      }
    },
    report: {
      stepType: "Agent", id: "report",
      agentId: "object_model", capabilityId: "query-sql",
      connectionId: $conn,
      inputMapping: {
        sql: {valueType: "immediate", value: "SELECT w.win AS win, (array_agg(s.sku ORDER BY s.in_stock DESC))[1] AS top_sku, COUNT(*) AS sku_count FROM sku_velocity s CROSS JOIN (VALUES ($tag$all$tag$)) AS w(win) GROUP BY w.win"},
        result_schema: {valueType: "immediate", value: [
          {name: "win", type: "string"},
          {name: "top_sku", type: "string"},
          {name: "sku_count", type: "integer"}
        ]}
      }
    },
    finish: {
      stepType: "Finish", id: "finish",
      inputMapping: {
        aged: {valueType: "reference", value: "steps.age.outputs.rows_affected"},
        rebuilt: {valueType: "reference", value: "steps.rebuild.outputs.rows_affected"},
        report_rows: {valueType: "reference", value: "steps.report.outputs.rows"},
        report_count: {valueType: "reference", value: "steps.report.outputs.row_count"}
      }
    }
  },
  entryPoint: "c1",
  executionPlan: [
    {fromStep: "c1", toStep: "c2"},
    {fromStep: "c2", toStep: "c3"},
    {fromStep: "c3", toStep: "ddl"},
    {fromStep: "ddl", toStep: "age"},
    {fromStep: "age", toStep: "rebuild"},
    {fromStep: "rebuild", toStep: "report"},
    {fromStep: "report", toStep: "finish"}
  ],
  variables: {},
  inputSchema: {},
  outputSchema: {}
}')
RESP=$(api_post "/workflows/${WF_ID}/update" "{\"executionGraph\": ${DEFINITION}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Workflow update failed: ${RESP}"; exit 1; }
VERSION=$(curl -sS "${API}/workflows/${WF_ID}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
echo "  Definition pushed (version ${VERSION}) ✓"

print_step "Compiling version ${VERSION} (direct-wasm, in-process)..."
RESP=$(api_post "/workflows/${WF_ID}/versions/${VERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Compile failed: ${RESP}"; tail -40 "${TEST_LOG}"; exit 1; }
echo "  Compiled ✓"

#-------------------------------------------------------------------------
run_rebuild_and_assert() {
    local run_label="$1" expect_top="$2"
    print_step "Executing rebuild workflow (${run_label})..."
    local resp instance_id output
    resp=$(api_post "/workflows/${WF_ID}/execute" '{"inputs": {"data": {}}}')
    instance_id=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    [ -n "${instance_id}" ] || { print_error "Execute failed: ${resp}"; exit 1; }
    wait_completed "${instance_id}"

    output=$(curl -sS "${API}/workflows/instances/${instance_id}" | jq '.data.outputs // .outputs')
    echo "  Output: ${output}"

    [ "$(echo "${output}" | jq -r '.aged')" = "1" ] \
        || { print_error "computed UPDATE: expected rows_affected 1, got $(echo "${output}" | jq -r '.aged')"; exit 1; }
    [ "$(echo "${output}" | jq -r '.rebuilt')" = "2" ] \
        || { print_error "CTE full-replace: expected rows_affected 2, got $(echo "${output}" | jq -r '.rebuilt')"; exit 1; }
    [ "$(echo "${output}" | jq -r '.report_count')" = "1" ] \
        || { print_error "windowed read: expected 1 row, got $(echo "${output}" | jq -r '.report_count')"; exit 1; }
    [ "$(echo "${output}" | jq -r '.report_rows[0].top_sku')" = "${expect_top}" ] \
        || { print_error "windowed read: expected top_sku ${expect_top}, got $(echo "${output}" | jq -r '.report_rows[0].top_sku')"; exit 1; }
    [ "$(echo "${output}" | jq -r '.report_rows[0].sku_count')" = "2" ] \
        || { print_error "windowed read: expected sku_count 2, got $(echo "${output}" | jq -r '.report_rows[0].sku_count')"; exit 1; }
    echo "  ${run_label}: aged=1 rebuilt=2 top_sku=${expect_top} ✓"
}

# Leg 1: first run. B (7) tops A (5).
run_rebuild_and_assert "run 1" "B"

# Leg 2: second run of the SAME binary. The creates run again (6 stock rows),
# the full-replace must still leave exactly 2 derived rows — no duplicates.
run_rebuild_and_assert "run 2 (idempotent full-replace)" "B"

DERIVED_ROWS=$(psql_server -c "SELECT COUNT(*) FROM sku_velocity")
STOCK_ROWS=$(psql_server -c "SELECT COUNT(*) FROM obm_sql_e2e_stock")
[ "${DERIVED_ROWS}" = "2" ] || { print_error "derived table has ${DERIVED_ROWS} rows after 2 runs, expected 2 (full-replace duplicated rows)"; exit 1; }
[ "${STOCK_ROWS}" = "6" ] || { print_error "stock table has ${STOCK_ROWS} rows after 2 runs, expected 6"; exit 1; }
print_success "Leg 1+2: rebuild pipeline correct and idempotent (derived=2, stock=6)"

#-------------------------------------------------------------------------
print_step "Leg 3: internal-API contract (status-coded failures)..."

# Multi-statement string must fail at the prepared-statement protocol with a
# status-coded 400 — and nothing may have executed.
HTTP_CODE=$(curl -sS -o "${TEST_DATA_DIR}/multi.json" -w "%{http_code}" --max-time 30 \
    -X POST -H "Content-Type: application/json" -H "X-Org-Id: ${TENANT}" \
    -d "{\"sql\": \"DELETE FROM sku_velocity; DROP TABLE sku_velocity\", \"connectionId\": \"${CONN_ID}\"}" \
    "${INTERNAL}/object-model/sql/execute")
[ "${HTTP_CODE}" = "400" ] || { print_error "multi-statement: expected HTTP 400, got ${HTTP_CODE}: $(cat "${TEST_DATA_DIR}/multi.json")"; exit 1; }
DERIVED_ROWS=$(psql_server -c "SELECT COUNT(*) FROM sku_velocity")
[ "${DERIVED_ROWS}" = "2" ] || { print_error "multi-statement executed something (derived=${DERIVED_ROWS})"; exit 1; }
echo "  multi-statement → 400, nothing executed ✓"

# A write spelled as a query must be rejected by the READ ONLY transaction.
HTTP_CODE=$(curl -sS -o "${TEST_DATA_DIR}/ro.json" -w "%{http_code}" --max-time 30 \
    -X POST -H "Content-Type: application/json" -H "X-Org-Id: ${TENANT}" \
    -d "{\"sql\": \"UPDATE sku_velocity SET in_stock = 0 RETURNING sku\", \"connectionId\": \"${CONN_ID}\"}" \
    "${INTERNAL}/object-model/sql/query")
[ "${HTTP_CODE}" = "400" ] || { print_error "write-as-query: expected HTTP 400, got ${HTTP_CODE}: $(cat "${TEST_DATA_DIR}/ro.json")"; exit 1; }
grep -q "read-only" "${TEST_DATA_DIR}/ro.json" || { print_error "write-as-query error does not mention read-only: $(cat "${TEST_DATA_DIR}/ro.json")"; exit 1; }
B_STOCK=$(psql_server -c "SELECT in_stock FROM sku_velocity WHERE sku = 'B'")
[ "${B_STOCK}" = "14" ] || { print_error "write-as-query mutated data (B in_stock=${B_STOCK}, expected 14)"; exit 1; }
print_success "Leg 3: internal SQL routes are status-coded and fail closed"

#-------------------------------------------------------------------------
print_step "Leg 4: SIGTERM exactly-once (drain mid-run, restart, resume)..."

psql_server -c "CREATE TABLE sql_counter (id INT PRIMARY KEY, n INT NOT NULL)" >/dev/null

RESP=$(api_post /workflows/create '{"name": "sql-counter", "description": "counter bump + delay; SIGTERM exactly-once harness"}')
WF_COUNTER_ID=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${WF_COUNTER_ID}" ] || { print_error "Workflow create failed: ${RESP}"; exit 1; }

DEFINITION=$(jq -n --arg conn "${CONN_ID}" '{
  name: "sql-counter",
  steps: {
    bump: {
      stepType: "Agent", id: "bump",
      agentId: "object_model", capabilityId: "execute-sql",
      connectionId: $conn,
      inputMapping: {
        sql: {valueType: "immediate", value: "INSERT INTO sql_counter (id, n) VALUES (1, 1) ON CONFLICT (id) DO UPDATE SET n = sql_counter.n + 1"}
      }
    },
    pace: {
      stepType: "Agent", id: "pace",
      agentId: "utils", capabilityId: "delay-in-ms",
      inputMapping: { delay_value: {valueType: "immediate", value: 20000} }
    },
    finish: {
      stepType: "Finish", id: "finish",
      inputMapping: { bumped: {valueType: "reference", value: "steps.bump.outputs.rows_affected"} }
    }
  },
  entryPoint: "bump",
  executionPlan: [
    {fromStep: "bump", toStep: "pace"},
    {fromStep: "pace", toStep: "finish"}
  ],
  variables: {},
  inputSchema: {},
  outputSchema: {}
}')
RESP=$(api_post "/workflows/${WF_COUNTER_ID}/update" "{\"executionGraph\": ${DEFINITION}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Workflow update failed: ${RESP}"; exit 1; }
CVERSION=$(curl -sS "${API}/workflows/${WF_COUNTER_ID}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(api_post "/workflows/${WF_COUNTER_ID}/versions/${CVERSION}/compile" '{}' 900)
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Compile failed: ${RESP}"; tail -40 "${TEST_LOG}"; exit 1; }

RESP=$(api_post "/workflows/${WF_COUNTER_ID}/execute" '{"inputs": {"data": {}}}')
COUNTER_INSTANCE=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${COUNTER_INSTANCE}" ] || { print_error "Execute failed: ${RESP}"; exit 1; }
echo "  Instance ${COUNTER_INSTANCE}"

# Wait until the bump committed (instance now sits in the 20s delay window).
COUNTER=""
for i in {1..60}; do
    COUNTER=$(psql_server -c "SELECT n FROM sql_counter WHERE id = 1" 2>/dev/null || true)
    [ "${COUNTER}" = "1" ] && break
    sleep 0.5
done
[ "${COUNTER}" = "1" ] || { print_error "bump never committed (counter='${COUNTER}')"; exit 1; }
echo "  Bump committed (n=1), instance mid-delay — SIGTERM server..."

kill -TERM "${SERVER_PID}" 2>/dev/null || true
for i in {1..60}; do
    kill -0 "${SERVER_PID}" 2>/dev/null || break
    sleep 1
done
kill -0 "${SERVER_PID}" 2>/dev/null && { print_error "server did not drain within 60s"; exit 1; }
echo "  Server drained. Restarting..."

start_server
echo "  Server back up (PID ${SERVER_PID}); waiting for instance to resume and complete..."
wait_completed "${COUNTER_INSTANCE}"

COUNTER=$(psql_server -c "SELECT n FROM sql_counter WHERE id = 1")
[ "${COUNTER}" = "1" ] || { print_error "execute-sql re-ran on replay: counter=${COUNTER}, expected 1 (checkpoint-cache miss)"; exit 1; }
print_success "Leg 4: drain + restart + resume applied the write exactly once (n=1)"

#-------------------------------------------------------------------------
print_step "Leg 5: CRON trigger fires the rebuild workflow..."

RESP=$(api_post /triggers "{
  \"workflow_id\": \"${WF_ID}\",
  \"trigger_type\": \"CRON\",
  \"active\": true,
  \"configuration\": {\"expression\": \"* * * * *\", \"timezone\": \"UTC\"}
}")
TRIGGER_ID=$(echo "${RESP}" | jq -r '.data.id // .id // empty')
[ -n "${TRIGGER_ID}" ] || { print_error "Trigger create failed: ${RESP}"; exit 1; }
echo "  Trigger ${TRIGGER_ID} (every minute) ✓"

# The cron run re-executes the full pipeline: stock grows to 9 rows and the
# rebuild leaves exactly 2 derived rows with B's in_stock recomputed to 21.
CRON_OK=""
for i in {1..50}; do
    STOCK_ROWS=$(psql_server -c "SELECT COUNT(*) FROM obm_sql_e2e_stock")
    B_STOCK=$(psql_server -c "SELECT in_stock FROM sku_velocity WHERE sku = 'B'" 2>/dev/null || true)
    if [ "${STOCK_ROWS}" -ge "9" ] && [ "${B_STOCK}" = "21" ]; then CRON_OK=1; break; fi
    sleep 3
done
curl -sS -X DELETE "${API}/triggers/${TRIGGER_ID}" >/dev/null 2>&1 || true
[ -n "${CRON_OK}" ] || { print_error "CRON run did not complete the pipeline within 150s (stock=${STOCK_ROWS}, B=${B_STOCK})"; tail -40 "${TEST_LOG}"; exit 1; }
DERIVED_ROWS=$(psql_server -c "SELECT COUNT(*) FROM sku_velocity")
[ "${DERIVED_ROWS}" = "2" ] || { print_error "derived table has ${DERIVED_ROWS} rows after cron run, expected 2"; exit 1; }
print_success "Leg 5: CRON trigger ran the rebuild end-to-end (stock=${STOCK_ROWS}, B in_stock=21, derived=2)"

echo ""
print_success "All raw-SQL e2e legs passed."
