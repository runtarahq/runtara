#!/bin/bash
# E2E load test: many tenants on one server within its pool limits (M7.5, C11).
#
# One runtara-server serves MANY_TENANTS tenants (default 24) on one shared
# runtime database, which it reaches as a plain role bound by row-level
# security. Its runtime pool is runtara's default (MANY_TENANTS_RUNTIME_POOL, 32)
# and its external object-model pools are capped across all tenants
# (MANY_TENANTS_POOL_CACHE pools of MANY_TENANTS_POOL_SIZE, default 6 of 2),
# while every tenant has its own external database. Every tenant then, all at
# once, runs its workflow and reads its workflows and its database
# MANY_TENANTS_ROUNDS times (default 3).
#
# Passes when every request succeeds, the runtime role never holds more than
# its pool, the external databases never more than the cap allows (the pools
# it keeps, plus any a request still uses as it is evicted), each run is its
# tenant's own, and another tenant's run is not found. Prints latencies.
#
# Prerequisites: as test_served_tenants.sh.

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

TEST_DB_SERVER="many_tenants_e2e_server_$$"
TEST_DB_RUNTIME="many_tenants_e2e_runtime_$$"
RUNTIME_ROLE="${TEST_DB_RUNTIME}_owner"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17880}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18881}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18882}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18883}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18884}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16480}"
TEST_DATA_DIR="$(mktemp -d -t runtara_many_tenants_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANTS_COUNT="${MANY_TENANTS:-24}"
ROUNDS="${MANY_TENANTS_ROUNDS:-3}"
# How long one run may take before it counts as failed.
RUN_TIMEOUT="${MANY_TENANTS_RUN_TIMEOUT:-60}"
# Far fewer than the tenants' demand, so they have to share them.
RUNTIME_MAX="${MANY_TENANTS_RUNTIME_POOL:-32}"
POOL_CACHE_MAX="${MANY_TENANTS_POOL_CACHE:-6}"
POOL_MAX="${MANY_TENANTS_POOL_SIZE:-2}"
EXT_PREFIX="many_tenants_e2e_ext_$$"
TENANTS=()
for i in $(seq 0 $((TENANTS_COUNT - 1))); do TENANTS+=("many_e2e_$(printf %02d "$i")"); done
TENANT="${TENANTS[0]}"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
# stderr so failures inside $(command substitution) helpers stay visible.
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
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
    psql_quiet -d postgres -c "DROP ROLE IF EXISTS ${RUNTIME_ROLE}" >/dev/null 2>&1 || true
    for i in $(seq 0 $((TENANTS_COUNT - 1))); do
        psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${EXT_PREFIX}_${i}" >/dev/null 2>&1 || true
    done
    if [ -n "${MANY_TENANTS_KEEP:-}" ]; then
        echo "kept ${TEST_DATA_DIR}" >&2
    else
        rm -rf "${TEST_DATA_DIR}" 2>/dev/null || true
    fi
}
trap cleanup EXIT
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"
http_status() { curl -sS -o /dev/null -w '%{http_code}' "$@"; }
server_sql() { psql_quiet -d "${TEST_DB_SERVER}" -c "$1"; }
runtime_sql() { psql_quiet -d "${TEST_DB_RUNTIME}" -c "$1"; }

echo "==============================================================="
echo "E2E: ${TENANTS_COUNT} tenants on one server within its pool limits"
echo "==============================================================="
[ -n "${POSTGRES_PASSWORD}" ] \
    || { print_error "set POSTGRES_PASSWORD (or PGPASSWORD) for ${POSTGRES_USER}@${POSTGRES_HOST}"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 \
    || { print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker is required"; exit 1; }
if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi

print_step "Valkey, the databases and the server..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > "/dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}") 2>/dev/null && break; sleep 0.5; done
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
RUNTIME_PASSWORD=$(openssl rand -hex 16)
psql_quiet -d postgres -c "CREATE ROLE ${RUNTIME_ROLE} LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD '${RUNTIME_PASSWORD}'" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME} OWNER ${RUNTIME_ROLE}" >/dev/null
for i in $(seq 0 $((TENANTS_COUNT - 1))); do
    psql_quiet -d postgres -c "CREATE DATABASE ${EXT_PREFIX}_${i}" >/dev/null
done
SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
SERVED=$(IFS=,; echo "${TENANTS[*]:1}")

# No platform object-model database: tenants sharing a process take none
# (M7.16). Their own databases are local here, which the guard would refuse.
RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
RUNTARA_DATABASE_CONNECTION_GUARD=off \
RUNTARA_DATABASE_URL="postgresql://${RUNTIME_ROLE}:${RUNTIME_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
RUNTARA_RUNTIME_MAX_CONNECTIONS="${RUNTIME_MAX}" \
OBJECT_MODEL_POOL_CACHE_MAX="${POOL_CACHE_MAX}" \
OBJECT_MODEL_POOL_MAX_CONNECTIONS="${POOL_MAX}" \
OBJECT_MODEL_POOL_MIN_CONNECTIONS=0 \
TENANT_ID="${TENANT}" \
RUNTARA_SERVED_TENANTS="${SERVED}" \
SERVER_HOST=127.0.0.1 \
SERVER_PORT="${TEST_PORT_PUBLIC}" \
RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
RUNTARA_AGENT_COMPONENTS_DIR="${AGENTS_DIR-${COMPONENTS_DIR}}" \
MAX_CONCURRENT_EXECUTIONS=200 \
DATA_DIR="${TEST_DATA_DIR}" \
RUST_LOG="warn,runtara_server=info" \
AUTH_PROVIDER=local \
VALKEY_HOST=127.0.0.1 \
VALKEY_PORT="${TEST_VALKEY_PORT}" \
OTEL_SDK_DISABLED=true \
RUNTARA_SDK_BACKEND=http \
SQLX_OFFLINE="${SQLX_OFFLINE}" \
"${RUNTARA_SERVER_BIN}" >>"${TEST_LOG}" 2>&1 &
SERVER_PID=$!
UP=""
for _ in {1..180}; do
    curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2" && { UP=1; break; }
    kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "Server exited during boot."; tail -30 "${TEST_LOG}"; exit 1; }
    sleep 1
done
[ -n "${UP}" ] || { print_error "the server did not come up"; tail -30 "${TEST_LOG}"; exit 1; }
echo "  one server for ${TENANTS_COUNT} tenants; runtime pool ${RUNTIME_MAX}, external pools at most ${POOL_CACHE_MAX} of ${POOL_MAX} ✓"

#-------------------------------------------------------------------------
print_step "Each tenant: a key, a compiled workflow and a connection to its own database..."
RESP=$(curl -sS -X POST -H "Content-Type: application/json" -d '{"name": "e2e"}' "${API}/api-keys")
KEY_ID=$(echo "${RESP}" | jq -r '.id // .data.id // empty')
[ -n "${KEY_ID}" ] || { print_error "api key create failed"; exit 1; }
GRAPH='{"name":"Answer","steps":{"finish":{"stepType":"Finish","id":"finish","inputMapping":{"answer":{"valueType":"immediate","value":42}}}},"entryPoint":"finish","executionPlan":[],"variables":{},"inputSchema":{},"outputSchema":{}}'
declare -A KEYS WFS CONNS
for i in $(seq 0 $((TENANTS_COUNT - 1))); do
    tenant="${TENANTS[$i]}"
    token="rt_${tenant}_$(openssl rand -hex 24)"
    hash=$(printf '%s' "${token}" | shasum -a 256 | cut -d' ' -f1)
    server_sql "CREATE TEMP TABLE k AS SELECT * FROM api_keys WHERE id = '${KEY_ID}';
                UPDATE k SET id = gen_random_uuid(), org_id = '${tenant}', key_hash = '${hash}', jti = gen_random_uuid()::text;
                INSERT INTO api_keys SELECT * FROM k;" >/dev/null
    KEYS[$i]="${token}"
    as() { curl -sS --max-time 900 -H "Authorization: Bearer ${KEYS[$i]}" -H "Content-Type: application/json" "$@"; }
    wf=$(as -X POST -d '{"name":"Answer","description":"many tenants e2e"}' "${API}/workflows/create" | jq -r '.data.id // empty')
    [ -n "${wf}" ] || { print_error "${tenant}: workflow create failed"; exit 1; }
    as -X POST -d "{\"executionGraph\": ${GRAPH}}" "${API}/workflows/${wf}/update" | jq -e '.success == true' >/dev/null \
        || { print_error "${tenant}: update failed"; exit 1; }
    version=$(as "${API}/workflows/${wf}/versions" \
        | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(as -X POST -d '{}' "${API}/workflows/${wf}/versions/${version}/compile")
    [ "$(echo "${resp}" | jq -r '.success // false')" = true ] \
        || { print_error "${tenant}: compile failed: $(echo "${resp}" | head -c 300)"; exit 1; }
    WFS[$i]="${wf}"
    ext="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${EXT_PREFIX}_${i}"
    resp=$(as -X POST -d "{\"title\":\"own-${i}\",\"integrationId\":\"postgres\",\"connectionParameters\":{\"database_url\":\"${ext}\"}}" "${API}/connections")
    conn=$(echo "${resp}" | jq -r '.connection_id // .connectionId // empty')
    [ -n "${conn}" ] || { print_error "${tenant}: connection create failed: $(echo "${resp}" | sed -E 's#postgres(ql)?://[^ "]*#<url>#g' | head -c 300)"; exit 1; }
    CONNS[$i]="${conn}"
done
echo "  ${TENANTS_COUNT} tenants ready ✓"

#-------------------------------------------------------------------------
print_step "Every tenant at once: runs, reads and its own database, ${ROUNDS} rounds..."
SAMPLES="${TEST_DATA_DIR}/samples"
: > "${SAMPLES}"
(
    while :; do
        psql_quiet -d postgres -c "SELECT count(*) FILTER (WHERE usename = '${RUNTIME_ROLE}'), count(*) FILTER (WHERE datname LIKE '${EXT_PREFIX}_%') FROM pg_stat_activity" >> "${SAMPLES}" 2>/dev/null
        sleep 0.2
    done
) &
SAMPLER=$!
tenant_load() {
    local i="$1" tenant="${TENANTS[$1]}" key="${KEYS[$1]}" wf="${WFS[$1]}" conn="${CONNS[$1]}"
    local out="${TEST_DATA_DIR}/load_${i}" resp instance status t
    : > "${out}"
    for _ in $(seq 1 "${ROUNDS}"); do
        t=$(curl -sS -o "${out}.body" -w '%{http_code} %{time_total}' -X POST -H "Authorization: Bearer ${key}" \
            -H "Content-Type: application/json" -d '{"inputs":{"data":{}}}' "${API}/workflows/${wf}/execute")
        echo "execute ${t}" >> "${out}"
        instance=$(jq -r '.data.instanceId // empty' "${out}.body")
        run_started=$(date +%s)
        [ -n "${instance}" ] || { echo "FAIL execute ${t}" >> "${out}"; continue; }
        echo "${instance}" >> "${out}.instances"
        status=""
        for _ in $(seq 1 $((RUN_TIMEOUT * 4))); do
            status=$(curl -sS -H "Authorization: Bearer ${key}" "${API}/workflows/instances/${instance}" | jq -r '.data.status // empty')
            case "${status}" in completed|failed|crashed|stopped|cancelled) break ;; esac
            sleep 0.25
        done
        [ "${status}" = completed ] || echo "FAIL run ${instance} ${status}" >> "${out}"
        echo "run ${instance} $(( $(date +%s) - run_started ))" >> "${out}.durations"
        t=$(curl -sS -o /dev/null -w '%{http_code} %{time_total}' -H "Authorization: Bearer ${key}" "${API}/workflows")
        echo "list ${t}" >> "${out}"
        t=$(curl -sS -o /dev/null -w '%{http_code} %{time_total}' -H "Authorization: Bearer ${key}" "${API}/object-model/schemas?connectionId=${conn}")
        echo "database ${t}" >> "${out}"
    done
}
STARTED=$(date +%s)
LOADS=()
for i in $(seq 0 $((TENANTS_COUNT - 1))); do tenant_load "$i" & LOADS+=($!); done
for pid in "${LOADS[@]}"; do wait "${pid}"; done
ELAPSED=$(( $(date +%s) - STARTED ))
kill "${SAMPLER}" 2>/dev/null || true; wait "${SAMPLER}" 2>/dev/null || true

LOADS_OUT=$(for i in $(seq 0 $((TENANTS_COUNT - 1))); do echo "${TEST_DATA_DIR}/load_${i}"; done)
FAILURES=$(cat ${LOADS_OUT} | grep -c '^FAIL' || true)
NON_2XX=$(cat ${LOADS_OUT} | awk '$1 != "FAIL" && $2 !~ /^2/' | wc -l | tr -d ' ')
[ "${FAILURES}" = 0 ] && [ "${NON_2XX}" = 0 ] \
    || { print_error "${FAILURES} failed runs and ${NON_2XX} refused requests:"; cat ${LOADS_OUT} | grep -E '^FAIL|^[a-z]+ [^2]' | head -20; tail -40 "${TEST_LOG}"; exit 1; }
MAX_RUNTIME=$(cut -d'|' -f1 "${SAMPLES}" | sort -n | tail -1)
MAX_EXTERNAL=$(cut -d'|' -f2 "${SAMPLES}" | sort -n | tail -1)
[ "${MAX_RUNTIME}" -le "${RUNTIME_MAX}" ] \
    || { print_error "the runtime role held ${MAX_RUNTIME} connections, over its pool of ${RUNTIME_MAX}"; exit 1; }
# The cap bounds the pools kept; one a request is still using when it is
# evicted closes when that request ends. So at most every tenant's pool may
# be open beyond the cap at once.
EXTERNAL_BOUND=$(((POOL_CACHE_MAX + TENANTS_COUNT) * POOL_MAX))
[ "${MAX_EXTERNAL}" -le "${EXTERNAL_BOUND}" ] \
    || { print_error "the external databases held ${MAX_EXTERNAL} connections, over ${EXTERNAL_BOUND}"; exit 1; }
percentile() { # KIND P
    awk -v k="$1" '$1 == k {print $3}' ${LOADS_OUT} | sort -n \
        | awk -v p="$2" '{v[NR]=$1} END {i=int((NR-1)*p/100)+1; printf "%.0f", v[i]*1000}'
}
REQUESTS=$((TENANTS_COUNT * ROUNDS))
echo "  ${REQUESTS} runs and $((REQUESTS * 3)) requests from ${TENANTS_COUNT} tenants at once in ${ELAPSED} s, every one served ✓"
STEADY_EXTERNAL=$(cut -d'|' -f2 "${SAMPLES}" | sort -n | uniq -c | sort -rn | head -1 | awk '{print $2}')
echo "  at most ${MAX_RUNTIME}/${RUNTIME_MAX} runtime connections; external ones at most ${MAX_EXTERNAL} (mostly ${STEADY_EXTERNAL}) against ${POOL_CACHE_MAX} pools of ${POOL_MAX} kept, ${EXTERNAL_BOUND} with every tenant mid-request ✓"
RUN_SECONDS=$(for f in ${LOADS_OUT}; do cat "${f}.durations" 2>/dev/null; done | awk '{print $3}' | sort -n)
echo "  each run, from start to completed: median $(echo "${RUN_SECONDS}" | awk '{v[NR]=$1} END {print v[int((NR+1)/2)]}') s, longest $(echo "${RUN_SECONDS}" | tail -1) s"
for kind in execute list database; do
    echo "  ${kind}: p50 $(percentile "${kind}" 50) ms, p95 $(percentile "${kind}" 95) ms"
done

#-------------------------------------------------------------------------
print_step "Each run is its own tenant's..."
OWN=$(runtime_sql "SELECT count(*) FROM instances WHERE tenant_id = '${TENANTS[1]}'")
[ "${OWN}" -ge "${ROUNDS}" ] || { print_error "${TENANTS[1]} has ${OWN} runs"; exit 1; }
OTHER=$(head -1 "${TEST_DATA_DIR}/load_0.instances")
[ "$(http_status -H "Authorization: Bearer ${KEYS[1]}" "${API}/workflows/instances/${OTHER}")" = 404 ] \
    || { print_error "${TENANTS[1]} read ${TENANT}'s run"; exit 1; }
SEEN=$(PGPASSWORD="${RUNTIME_PASSWORD}" psql -qtA -U "${RUNTIME_ROLE}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -d "${TEST_DB_RUNTIME}" \
    -c "SELECT set_config('runtara.tenant_id', '${TENANTS[1]}', false); SELECT count(*) FROM instances WHERE tenant_id <> '${TENANTS[1]}'" | tail -1)
[ "${SEEN}" = 0 ] || { print_error "naming ${TENANTS[1]}, the runtime role saw ${SEEN} other runs"; exit 1; }
echo "  ${TENANTS[1]} reads none of ${TENANT}'s runs, and the runtime role naming it sees only its ${OWN} ✓"

print_success "many tenants: ${TENANTS_COUNT} tenants served at once on one server within its runtime pool and the external pool cap, each its own"
