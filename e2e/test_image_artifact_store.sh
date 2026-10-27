#!/bin/bash
# E2E Test: workflow images and platform components live in the artifact
# store, by digest.
#
#   1. RUN — a compiled workflow runs; its image row names its artifact by
#      digest (no path), and the bytes are at artifacts/blobs/sha256/<digest>.
#   2. LEGACY — the server stops and the image is put back the way images were
#      stored before the artifact store: bytes at images/<id>/binary, the row
#      pointing at that path.
#   3. IMPORT — on boot the server moves the bytes into the store, clears the
#      path and removes the old directory; the workflow runs again.
#   4. RELEASE — the components directory was published as the server's
#      release: one record per agent and an index. A server booted with no
#      components directory loads its agents from that release, serves them,
#      runs one, and runs a workflow that calls one.
#
# Prerequisites: Postgres (POSTGRES_PASSWORD or PGPASSWORD in the
# environment) + docker (isolated Valkey) and the shared workflow components
# in target/wasm32-wasip2/release (scripts/build-agent-components.sh).

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

TEST_DB_SERVER="artifact_store_e2e_server_$$"
TEST_DB_RUNTIME="artifact_store_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17820}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18821}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18822}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18823}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18824}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16420}"
TEST_DATA_DIR="$(mktemp -d -t runtara_artifact_store_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="artifact_store_e2e"

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
    rm -rf "${TEST_DATA_DIR}" 2>/dev/null || true
}
trap cleanup EXIT

API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

api_post() {
    curl -sS --max-time "${3:-120}" -X POST -H "Content-Type: application/json" \
        ${2:+-d "$2"} "${API}$1"
}

# Create a workflow, save its graph, compile it; echoes the workflow id.
create_and_compile() {
    local name="$1" graph="$2"
    local resp wf_id version
    resp=$(api_post /workflows/create "{\"name\":\"${name}\",\"description\":\"artifact store e2e\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || { print_error "create failed: ${resp}"; exit 1; }
    resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${graph}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] \
        || { print_error "update/validate failed for ${name}: ${resp}"; exit 1; }
    version=$(curl -sS "${API}/workflows/${wf_id}/versions" \
        | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf_id}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] \
        || { print_error "compile failed for ${name}: ${resp}"; tail -40 "${TEST_LOG}" >&2; exit 1; }
    echo "${wf_id}"
}

#-------------------------------------------------------------------------
echo "==============================================================="
echo "E2E Test: workflow images live in the artifact store"
echo "==============================================================="

[ -n "${POSTGRES_PASSWORD}" ] \
    || { print_error "set POSTGRES_PASSWORD (or PGPASSWORD) for ${POSTGRES_USER}@${POSTGRES_HOST}"; exit 1; }
if [ ! -x "${RUNTARA_SERVER_BIN}" ]; then
    print_step "Building runtara-server (debug)..."
    SQLX_OFFLINE="${SQLX_OFFLINE}" cargo build -p runtara-server --bin runtara-server >&2
fi
for f in runtara_workflow_stdlib.wasm; do
    if [ ! -f "${COMPONENTS_DIR}/${f}" ]; then
        print_error "Missing component ${COMPONENTS_DIR}/${f} — run scripts/build-agent-components.sh"
        exit 1
    fi
done

print_step "Pre-flight: Postgres and docker..."
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 \
    || { print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker is required"; exit 1; }

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do
    if (echo > "/dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}") 2>/dev/null; then break; fi
    sleep 0.5
done

print_step "Creating test databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null
SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"

start_server() {
    print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
    RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
    OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
    RUNTARA_DATABASE_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}" \
    TENANT_ID="${TENANT}" \
    SERVER_HOST=127.0.0.1 \
    SERVER_PORT="${TEST_PORT_PUBLIC}" \
    RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
    RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
    RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
    RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
    RUNTARA_AGENT_COMPONENTS_DIR="${AGENTS_DIR-${COMPONENTS_DIR}}" \
    RUNTARA_DIRECT_WASM_COMPONENTS_DIR="${COMPONENTS_DIR}" \
    MAX_CONCURRENT_EXECUTIONS=40 \
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

    for _ in {1..60}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            break
        fi
        sleep 1
        kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "Server exited during boot."; tail -30 "${TEST_LOG}"; exit 1; }
    done
    echo "  Server up (PID ${SERVER_PID})"
}

start_server


runtime_sql() { psql_quiet -d "${TEST_DB_RUNTIME}" -c "$1"; }

# Run the workflow; fail unless it completes with its output.
run_echo() {
    local resp instance status
    resp=$(api_post "/workflows/${WF_ID}/execute" '{"inputs": {"data": {}}}')
    instance=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    [ -n "${instance}" ] || { print_error "execute failed: ${resp}"; exit 1; }
    status=""
    for _ in {1..120}; do
        status=$(curl -sS "${API}/workflows/instances/${instance}" | jq -r '.data.status // empty')
        case "${status}" in completed|failed|crashed|stopped|cancelled) break ;; esac
        sleep 0.5
    done
    resp=$(curl -sS "${API}/workflows/instances/${instance}")
    [ "${status}" = "completed" ] && echo "${resp}" \
        | jq -e '.data.outputs.answer == 42 and .data.outputs.hash == "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"' >/dev/null \
        || { print_error "the run ended '${status}': $(echo "${resp}" | jq -c '.data.error // .data.outputs')"; tail -40 "${TEST_LOG}"; exit 1; }
}

#-------------------------------------------------------------------------
print_step "1. A compiled workflow runs from the artifact store..."
GRAPH='{
  "name": "Answer",
  "steps": {
    "hash": {
      "stepType": "Agent", "id": "hash", "agentId": "crypto", "capabilityId": "hash",
      "inputMapping": { "data": { "valueType": "immediate", "value": "hello" } }
    },
    "finish": {
      "stepType": "Finish", "id": "finish",
      "inputMapping": {
        "answer": { "valueType": "immediate", "value": 42 },
        "hash": { "valueType": "reference", "value": "steps.hash.outputs.hash" }
      }
    }
  },
  "entryPoint": "hash",
  "executionPlan": [{ "fromStep": "hash", "toStep": "finish" }],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}'
WF_ID=$(create_and_compile "Answer" "${GRAPH}")
run_echo
ROW=$(runtime_sql "SELECT image_id || '|' || coalesce(binary_digest, '') || '|' || coalesce(binary_path, '') FROM images WHERE tenant_id = '${TENANT}'")
[ "$(echo "${ROW}" | wc -l | tr -d ' ')" = "1" ] || { print_error "expected one image, got: ${ROW}"; exit 1; }
IMAGE_ID="${ROW%%|*}"; REST="${ROW#*|}"; DIGEST="${REST%%|*}"; PATH_COL="${REST#*|}"
[[ "${DIGEST}" =~ ^[0-9a-f]{64}$ ]] || { print_error "the image names no digest: ${ROW}"; exit 1; }
[ -z "${PATH_COL}" ] || { print_error "the image still records a path: ${PATH_COL}"; exit 1; }
BLOB="${TEST_DATA_DIR}/artifacts/blobs/sha256/${DIGEST}"
[ -f "${BLOB}" ] || { print_error "no artifact at ${BLOB}"; exit 1; }
[ "$(shasum -a 256 "${BLOB}" | cut -d' ' -f1)" = "${DIGEST}" ] \
    || { print_error "the stored bytes do not match their digest"; exit 1; }
[ ! -e "${TEST_DATA_DIR}/images" ] || { print_error "an images/ directory was written"; exit 1; }
echo "  the image names its artifact by digest, and the store holds those bytes ✓"

#-------------------------------------------------------------------------
print_step "2. Stop the server and store the image the old way..."
kill "${SERVER_PID}"; wait "${SERVER_PID}" 2>/dev/null || true
SERVER_PID=""
LEGACY="${TEST_DATA_DIR}/images/${IMAGE_ID}/binary"
mkdir -p "$(dirname "${LEGACY}")"
mv "${BLOB}" "${LEGACY}"
runtime_sql "UPDATE images SET binary_path = '${LEGACY}' WHERE image_id = '${IMAGE_ID}'" >/dev/null
echo "  bytes at images/${IMAGE_ID}/binary, the row pointing there"

#-------------------------------------------------------------------------
print_step "3. Restart: the boot import moves the bytes into the store..."
start_server
[ -f "${BLOB}" ] || { print_error "the artifact was not imported: $(grep -i artifact "${TEST_LOG}" | tail -5)"; exit 1; }
[ ! -e "${TEST_DATA_DIR}/images/${IMAGE_ID}" ] || { print_error "the legacy directory was left behind"; exit 1; }
PATH_COL=$(runtime_sql "SELECT coalesce(binary_path, '') FROM images WHERE image_id = '${IMAGE_ID}'")
[ -z "${PATH_COL}" ] || { print_error "the row still records ${PATH_COL}"; exit 1; }
run_echo
echo "  imported, the path cleared, the old directory removed, and the workflow runs ✓"

#-------------------------------------------------------------------------
print_step "4. The components were published as a release; boot without them..."
INDEX=$(find "${TEST_DATA_DIR}/artifacts/manifests/agents/index" -name '*.json')
[ "$(echo "${INDEX}" | wc -l | tr -d ' ')" = "1" ] || { print_error "expected one release index, got: ${INDEX}"; exit 1; }
RELEASE=$(basename "${INDEX}" .json)
CRYPTO=$(find "${TEST_DATA_DIR}/artifacts/manifests/agents/crypto" -name '*.json' | head -1)
[ -n "${CRYPTO}" ] || { print_error "no record for the crypto agent"; exit 1; }
jq -e '.metadata.id == "crypto" and (.content | test("^[0-9a-f]{64}$"))' "${CRYPTO}" >/dev/null \
    || { print_error "the crypto record is not its metadata and content digest: $(head -c 200 "${CRYPTO}")"; exit 1; }
[ -f "${TEST_DATA_DIR}/artifacts/blobs/sha256/$(jq -r .content "${CRYPTO}")" ] \
    || { print_error "the crypto record names content that is not stored"; exit 1; }
echo "  release ${RELEASE}: one record per agent (metadata + content digest), and an index ✓"

kill "${SERVER_PID}"; wait "${SERVER_PID}" 2>/dev/null || true
SERVER_PID=""
AGENTS_DIR="" start_server
grep -q "loaded [0-9]* agent(s) from release ${RELEASE}" "${TEST_LOG}" \
    || { print_error "the server did not load its agents from release ${RELEASE}"; tail -20 "${TEST_LOG}"; exit 1; }
curl -sS "${API}/agents" | jq -e '[.. | objects | select(.id? == "crypto")] | length > 0' >/dev/null \
    || { print_error "the agents API does not list crypto: $(curl -sS "${API}/agents" | head -c 300)"; exit 1; }
RESP=$(api_post "/agents/crypto/capabilities/hash/test" '{"input": {"data": "hello"}}')
echo "${RESP}" | jq -e '.. | objects | select(has("hash")) | .hash == "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"' >/dev/null \
    || { print_error "testing crypto from the release failed: ${RESP}"; exit 1; }
run_echo
echo "  with no components directory: agents loaded from the release, served, tested, and the workflow runs ✓"

print_success "artifact store: images name their workflow by digest, legacy images are imported at boot, and agents load from a release in the store"
