#!/bin/bash
# A local server for experimenting with the System analytics pipeline view.
#
# Brings up an isolated Postgres pair, a Valkey, and a runtara-server serving
# the built UI, then seeds workflows shaped to drive the view into each of the
# states it exists to tell apart. Everything is namespaced `pipelinelab`, on
# ports well away from the usual dev ones, so it cannot disturb a running :7001.
#
# Usage:
#   scripts/pipeline-playground.sh up          # start, seed, print the URL
#   scripts/pipeline-playground.sh load [N]    # offer N executions (default 300)
#   scripts/pipeline-playground.sh saturate    # hold every run permit
#   scripts/pipeline-playground.sh park [N]    # add N parked instances
#   scripts/pipeline-playground.sh squeeze     # restart with a tiny cap
#   scripts/pipeline-playground.sh snapshot    # print the raw JSON
#   scripts/pipeline-playground.sh watch       # follow the SSE stream
#   scripts/pipeline-playground.sh logs
#   scripts/pipeline-playground.sh down        # stop and remove everything

set -uo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BLUE='\033[0;34m'; NC='\033[0m'
say()  { echo -e "${GREEN}▸${NC} $1"; }
warn() { echo -e "${YELLOW}!${NC} $1"; }
die()  { echo -e "${RED}✗${NC} $1"; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

LAB_DIR="${RUNTARA_LAB_DIR:-${TMPDIR:-/tmp}/runtara-pipeline-lab}"
PORT="${LAB_PORT:-17800}"
INTERNAL_PORT=$((PORT + 1))
CORE_PORT=$((PORT + 100))
ENV_PORT=$((PORT + 101))
CORE_HTTP_PORT=$((PORT + 102))
ENV_HTTP_PORT=$((PORT + 103))
VALKEY_PORT="${LAB_VALKEY_PORT:-16410}"
VALKEY_NAME=pipelinelab-valkey
TENANT=pipelinelab
DB_SERVER=pipelinelab_server
DB_RUNTIME=pipelinelab_runtime

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"
PG_CONTAINER="${PG_CONTAINER:-runtara-dev-postgres}"

BIN="${RUNTARA_SERVER_BIN:-${ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${ROOT}/target/wasm32-wasip2/release}"
DIST_DIR="${ROOT}/crates/runtara-server/frontend/dist"

API="http://127.0.0.1:${PORT}/api/runtime"
UI="http://127.0.0.1:${PORT}/ui/analytics/system"
PIDFILE="${LAB_DIR}/server.pid"
LOG="${LAB_DIR}/server.log"
WF_FILE="${LAB_DIR}/workflows.env"

if command -v psql >/dev/null 2>&1 && PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA -d postgres -c "SELECT 1" >/dev/null 2>&1; then
    psql_lab() { PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"; }
elif docker exec "${PG_CONTAINER}" true >/dev/null 2>&1; then
    psql_lab() { docker exec -e PGPASSWORD="${POSTGRES_PASSWORD}" "${PG_CONTAINER}" psql -U "${POSTGRES_USER}" -tA "$@"; }
else
    die "no Postgres reachable (host psql, or the '${PG_CONTAINER}' container)"
fi

api_post() {
    curl -sS --max-time "${3:-120}" -X POST "${API}$1" \
        -H 'Content-Type: application/json' -d "$2"
}

# Create + compile a workflow, echo its id.
#
# `versionNumber` is the field the API returns; `.version` is always null, and
# a list of nulls yields a null max that silently falls back to 1 — which
# compiles an empty version 1 of a workflow whose steps are in version 2.
make_workflow() {
    local name="$1" graph="$2" resp wf_id version
    resp=$(api_post /workflows/create "{\"name\":\"${name}\",\"description\":\"pipeline playground\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || die "create failed: ${resp}"

    resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${graph}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || die "update failed: ${resp}"

    version=$(curl -sS "${API}/workflows/${wf_id}/versions" | jq -r '[.data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf_id}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || die "compile failed: ${resp}"
    echo "${wf_id}"
}

server_running() {
    [ -f "${PIDFILE}" ] && kill -0 "$(cat "${PIDFILE}")" 2>/dev/null
}

require_up() {
    server_running || die "not running — start it with: scripts/pipeline-playground.sh up"
}

start_server() {
    # Guarded expansion: macOS ships bash 3.2, where "${arr[@]}" on an empty
    # array is an unbound-variable error under `set -u` — and it surfaces as
    # "server exited during boot", nowhere near the actual mistake.
    mkdir -p "${LAB_DIR}"
    (
        if [ "$#" -gt 0 ]; then
            for kv in "$@"; do export "${kv?}"; done
        fi
        export RUNTARA_SERVER_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${DB_SERVER}"
        export OBJECT_MODEL_DATABASE_URL="${RUNTARA_SERVER_DATABASE_URL}"
        export RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${DB_RUNTIME}"
        export TENANT_ID="${TENANT}"
        export SERVER_HOST=127.0.0.1 SERVER_PORT="${PORT}" INTERNAL_PORT="${INTERNAL_PORT}"
        export RUNTARA_CORE_PORT="${CORE_PORT}" RUNTARA_ENVIRONMENT_PORT="${ENV_PORT}"
        export RUNTARA_CORE_HTTP_PORT="${CORE_HTTP_PORT}" RUNTARA_ENV_HTTP_PORT="${ENV_HTTP_PORT}"
        export DATA_DIR="${LAB_DIR}/data"
        export RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}"
        # Serves the built dist from disk, so a frontend rebuild needs no relink.
        export RUNTARA_UI_DIST_DIR="${DIST_DIR}"
        # The origin only: the client appends /api/runtime itself, and including
        # it here produces /api/runtime/api/runtime/... and a page of 404s.
        export RUNTARA_UI_API_BASE_URL="http://127.0.0.1:${PORT}"
        export RUNTARA_DEV_MODE=false
        export RUST_LOG="${RUST_LOG:-warn,runtara_server=info}"
        export AUTH_PROVIDER=local
        export SESSION_TOKEN_SECRET=8efacf953eb244e07346edb64d1a8adca5bdf92049611737ce09e2c6388cb5f2
        export VALKEY_HOST=127.0.0.1 VALKEY_PORT="${VALKEY_PORT}"
        export OTEL_SDK_DISABLED=true RUNTARA_SDK_BACKEND=http SQLX_OFFLINE=true
        exec "${BIN}" >>"${LOG}" 2>&1
    ) &
    echo $! > "${PIDFILE}"

    for i in $(seq 1 90); do
        if curl -sS -o /dev/null -w '%{http_code}' "http://127.0.0.1:${PORT}/health" 2>/dev/null | grep -q '^2'; then
            return 0
        fi
        sleep 1
        kill -0 "$(cat "${PIDFILE}")" 2>/dev/null || { tail -30 "${LOG}"; die "server exited during boot"; }
    done
    tail -30 "${LOG}"; die "server never became healthy"
}

stop_server() {
    if server_running; then
        kill "$(cat "${PIDFILE}")" 2>/dev/null || true
        sleep 2
        kill -9 "$(cat "${PIDFILE}")" 2>/dev/null || true
    fi
    rm -f "${PIDFILE}"
}

FINISH_GRAPH='{
  "name": "instant", "durable": false, "entryPoint": "finish",
  "steps": { "finish": { "stepType": "Finish", "id": "finish",
             "inputMapping": { "ok": { "valueType": "immediate", "value": true } } } },
  "executionPlan": [], "variables": {}, "inputSchema": {}, "outputSchema": {}
}'

slow_graph() {
    jq -n --argjson ms "$1" '{
      name: "slow", durable: false, entryPoint: "wait",
      steps: {
        wait:   { stepType: "Delay", id: "wait", name: "Hold a run slot",
                  durationMs: { valueType: "immediate", value: $ms } },
        finish: { stepType: "Finish", id: "finish",
                  inputMapping: { ok: { valueType: "immediate", value: true } } }
      },
      executionPlan: [ { fromStep: "wait", toStep: "finish" } ],
      variables: {}, inputSchema: {}, outputSchema: {}
    }'
}

cmd_up() {
    [ -x "${BIN}" ] || die "missing ${BIN} — cargo build -p runtara-server --bin runtara-server"
    [ -d "${COMPONENTS_DIR}" ] || die "missing ${COMPONENTS_DIR} — scripts/build-agent-components.sh"
    [ -d "${DIST_DIR}" ] || die "missing ${DIST_DIR} — (cd crates/runtara-server/frontend && npm run build)"
    command -v jq >/dev/null || die "jq required"
    docker info >/dev/null 2>&1 || die "docker required (isolated Valkey)"

    server_running && { warn "already running"; cmd_where; return 0; }

    mkdir -p "${LAB_DIR}/data"; : > "${LOG}"

    say "Valkey on :${VALKEY_PORT}"
    docker rm -f "${VALKEY_NAME}" >/dev/null 2>&1 || true
    docker run -d --rm --name "${VALKEY_NAME}" -p "${VALKEY_PORT}:6379" valkey/valkey:8-alpine >/dev/null \
        || die "could not start Valkey (is :${VALKEY_PORT} taken? set LAB_VALKEY_PORT)"
    for _ in $(seq 1 20); do (echo > "/dev/tcp/127.0.0.1/${VALKEY_PORT}") 2>/dev/null && break; sleep 0.5; done

    say "databases"
    for db in "${DB_SERVER}" "${DB_RUNTIME}"; do
        psql_lab -d postgres -c "DROP DATABASE IF EXISTS ${db}" >/dev/null 2>&1
        psql_lab -d postgres -c "CREATE DATABASE ${db}" >/dev/null 2>&1
    done

    say "server on :${PORT}"
    start_server

    say "seeding workflows (compiling, ~30s)"
    {
        echo "WF_INSTANT=$(make_workflow "lab-instant" "${FINISH_GRAPH}")"
        echo "WF_SLOW=$(make_workflow "lab-slow" "$(slow_graph 20000)")"
        echo "WF_PARK=$(make_workflow "lab-park" "$(slow_graph 86400000)")"
    } > "${WF_FILE}"
    # shellcheck disable=SC1090
    source "${WF_FILE}"
    echo "    lab-instant  ${WF_INSTANT}"
    echo "    lab-slow     ${WF_SLOW}   (20s per run — fills run slots)"
    echo "    lab-park     ${WF_PARK}   (24h delay — parks immediately)"

    cmd_where
}

cmd_where() {
    echo
    echo -e "  ${BLUE}Open${NC}      ${UI}"
    echo -e "  ${BLUE}Snapshot${NC}  ${API}/analytics/pipeline"
    echo -e "  ${BLUE}Stream${NC}    ${API}/analytics/pipeline/stream"
    echo
    echo "  Drive it:"
    echo "    scripts/pipeline-playground.sh load 300     # steady throughput"
    echo "    scripts/pipeline-playground.sh saturate     # every run slot held"
    echo "    scripts/pipeline-playground.sh park 5000    # a big parked population"
    echo "    scripts/pipeline-playground.sh squeeze      # restart with a tiny cap"
    echo
}

cmd_load() {
    require_up
    # shellcheck disable=SC1090
    source "${WF_FILE}"
    local n="${1:-300}"
    say "offering ${n} instant executions — watch Offered/Accepted climb"
    for _ in $(seq 1 "${n}"); do
        curl -sS -o /dev/null --max-time 5 -X POST "${API}/workflows/${WF_INSTANT}/execute" \
            -H 'Content-Type: application/json' -d '{"inputs":{"data":{}}}' 2>/dev/null || true
    done
    say "done"
    cmd_snapshot
}

cmd_saturate() {
    require_up
    # shellcheck disable=SC1090
    source "${WF_FILE}"
    local limit
    limit=$(curl -sS "${API}/analytics/pipeline" | jq -r '.data.stages[] | select(.key=="runPermits") | .limit // 16')
    local n=$(( limit * 2 ))
    say "launching ${n} runs of 20s each against a bound of ${limit}"
    say "expect: Concurrent runs pins at ${limit}/${limit} and is named the chokepoint"
    for _ in $(seq 1 "${n}"); do
        curl -sS -o /dev/null --max-time 3 -X POST "${API}/workflows/${WF_SLOW}/execute" \
            -H 'Content-Type: application/json' -d '{"inputs":{"data":{}}}' 2>/dev/null || true &
    done
    wait 2>/dev/null || true
    sleep 3
    cmd_snapshot
}

cmd_park() {
    require_up
    local n="${1:-5000}"
    # Inserted directly rather than executed: the point is a large parked
    # population to look at, and running 5000 workflows to get one is slow.
    # The count is read from this table either way.
    say "adding ${n} parked instances"
    psql_lab -d "${DB_RUNTIME}" -c "
        INSERT INTO instances (instance_id, tenant_id, status, created_at)
        SELECT 'lab-park-'||md5(random()::text||g), '${TENANT}', 'suspended'::instance_status, NOW()
        FROM generate_series(1, ${n}) g;" >/dev/null
    say "done — the Parked row updates on the 30s slow tick"
}

cmd_squeeze() {
    require_up
    say "restarting with MAX_CONCURRENT_EXECUTIONS=4 and RUNTARA_MAX_CONCURRENT_RUNS=2"
    say "expect: Admission fills fast and starts refusing (Denied 403 climbs)"
    stop_server
    start_server MAX_CONCURRENT_EXECUTIONS=4 RUNTARA_MAX_CONCURRENT_RUNS=2
    cmd_where
}

cmd_snapshot() {
    require_up
    curl -sS "${API}/analytics/pipeline" | jq '{
        rates: .data.rates,
        stages: [.data.stages[] | {stage: .label, knob, used, limit, oldestAgeMs}]
    }'
}

cmd_watch() {
    require_up
    say "following the stream (ctrl-c to stop)"
    curl -sSN -H 'Accept: text/event-stream' "${API}/analytics/pipeline/stream" \
    | while IFS= read -r line; do
        case "${line}" in
          data:*) echo "${line#data: }" | jq -c '{
              t: (.capturedAt | split("T")[1][0:8]),
              offered: (.rates.offered // null), accepted: (.rates.accepted // null),
              denied: (.rates.denied // null),
              runs: [(.stages[] | select(.key=="runPermits") | .used, .limit)],
              parked: (.stages[] | select(.key=="parked") | .used)
          }' 2>/dev/null ;;
        esac
      done
}

cmd_logs() { tail -f "${LOG}"; }

cmd_down() {
    say "stopping"
    stop_server
    docker rm -f "${VALKEY_NAME}" >/dev/null 2>&1 || true
    if [ "${KEEP_DB:-0}" != "1" ]; then
        for db in "${DB_SERVER}" "${DB_RUNTIME}"; do
            psql_lab -d postgres -c "DROP DATABASE IF EXISTS ${db}" >/dev/null 2>&1
        done
        rm -rf "${LAB_DIR}"
    else
        say "KEEP_DB=1 — databases and ${LAB_DIR} left in place"
    fi
    say "down"
}

case "${1:-up}" in
    up)       cmd_up ;;
    load)     cmd_load "${2:-300}" ;;
    saturate) cmd_saturate ;;
    park)     cmd_park "${2:-5000}" ;;
    squeeze)  cmd_squeeze ;;
    snapshot) cmd_snapshot ;;
    watch)    cmd_watch ;;
    logs)     cmd_logs ;;
    where)    cmd_where ;;
    down)     cmd_down ;;
    *)        sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' ; exit 1 ;;
esac
