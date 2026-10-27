#!/bin/bash
# E2E Test: every artifact lives in an S3-compatible bucket (RustFS).
#
#   1. PUBLISH — a server with RUNTARA_ARTIFACT_STORE_URL pointing at the
#      bucket publishes its components as a release and compiles and runs a
#      workflow: the release, its records and the workflow image are objects
#      in the bucket, and nothing is stored under the data dir.
#   2. NOTHING LOCAL — a second server with an empty data dir and no
#      components directory loads its agents from the bucket, runs the first
#      workflow from its image in the bucket, and compiles and runs a new one.
#
# Prerequisites: Postgres (POSTGRES_PASSWORD or PGPASSWORD in the
# environment) + docker (isolated Valkey, and RustFS from the pinned
# rustfs/rustfs image) and the shared workflow components
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

TEST_DB_SERVER="object_store_e2e_server_$$"
TEST_DB_RUNTIME="object_store_e2e_runtime_$$"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17840}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18841}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18842}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18843}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18844}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16440}"
TEST_DATA_DIR="$(mktemp -d -t runtara_object_store_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
VALKEY_CONTAINER=""
S3_CONTAINER=""
S3_IMAGE="rustfs/rustfs:1.0.4"
# Test-only credentials of the throwaway S3 container.
S3_USER="runtara-test"
S3_SECRET="runtara-test-secret"
BUCKET="runtara-artifacts"
TENANT="object_store_e2e"

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
    if [ -n "${S3_CONTAINER}" ]; then
        docker rm -f "${S3_CONTAINER}" >/dev/null 2>&1 || true
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
    resp=$(api_post /workflows/create "{\"name\":\"${name}\",\"description\":\"object store e2e\"}")
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
echo "E2E Test: every artifact lives in an S3-compatible bucket"
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

print_step "Starting RustFS with bucket ${BUCKET}..."
S3_CONTAINER=$(docker run -d --rm -p 127.0.0.1::9000 \
    -e "RUSTFS_ACCESS_KEY=${S3_USER}" -e "RUSTFS_SECRET_KEY=${S3_SECRET}" \
    "${S3_IMAGE}")
S3_ENDPOINT="http://$(docker port "${S3_CONTAINER}" 9000/tcp | head -1)"
# A SigV4-signed request from the container's own curl.
s3_request() {
    docker exec "${S3_CONTAINER}" curl -sf --aws-sigv4 "aws:amz:us-east-1:s3" \
        --user "${S3_USER}:${S3_SECRET}" "$@"
}
for _ in {1..60}; do
    s3_request -o /dev/null -X PUT "http://127.0.0.1:9000/${BUCKET}" 2>/dev/null && break
    sleep 0.5
done
s3_request -o /dev/null "http://127.0.0.1:9000/${BUCKET}?list-type=2" \
    || { print_error "RustFS did not come up"; exit 1; }

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
    RUNTARA_ARTIFACT_STORE_URL="s3://${BUCKET}/runtara" \
    AWS_ENDPOINT="${S3_ENDPOINT}" \
    AWS_ALLOW_HTTP=true \
    AWS_REGION=us-east-1 \
    AWS_ACCESS_KEY_ID="${S3_USER}" \
    AWS_SECRET_ACCESS_KEY="${S3_SECRET}" \
    MAX_CONCURRENT_EXECUTIONS=40 \
    DATA_DIR="${SERVER_DATA_DIR:-${TEST_DATA_DIR}/a}" \
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


runtime_sql() { psql_quiet -d "${TEST_DB_RUNTIME}" -c "$1"; }
# The keys under runtara/<prefix>, relative to it.
bucket_objects() {
    local prefix="runtara/$1"
    s3_request "http://127.0.0.1:9000/${BUCKET}?list-type=2&max-keys=1000&prefix=${prefix//\//%2F}" \
        | grep -o '<Key>[^<]*</Key>' | sed -e 's#^<Key>##' -e 's#</Key>$##' -e "s#^${prefix}##"
}

HASH_OF_HELLO="2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
run_workflow() {
    local resp instance status
    resp=$(api_post "/workflows/$1/execute" '{"inputs": {"data": {}}}')
    instance=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    [ -n "${instance}" ] || { print_error "execute failed: ${resp}"; exit 1; }
    status=""
    for _ in {1..120}; do
        status=$(curl -sS "${API}/workflows/instances/${instance}" | jq -r '.data.status // empty')
        case "${status}" in completed|failed|crashed|stopped|cancelled) break ;; esac
        sleep 0.5
    done
    resp=$(curl -sS "${API}/workflows/instances/${instance}")
    [ "${status}" = "completed" ] && echo "${resp}" | jq -e --arg h "${HASH_OF_HELLO}" '.data.outputs.hash == $h' >/dev/null \
        || { print_error "the run ended '${status}': $(echo "${resp}" | jq -c '.data.error // .data.outputs')"; tail -40 "${TEST_LOG}"; exit 1; }
}

GRAPH='{
  "name": "Hash",
  "steps": {
    "hash": {
      "stepType": "Agent", "id": "hash", "agentId": "crypto", "capabilityId": "hash",
      "inputMapping": { "data": { "valueType": "immediate", "value": "hello" } }
    },
    "finish": {
      "stepType": "Finish", "id": "finish",
      "inputMapping": { "hash": { "valueType": "reference", "value": "steps.hash.outputs.hash" } }
    }
  },
  "entryPoint": "hash",
  "executionPlan": [{ "fromStep": "hash", "toStep": "finish" }],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}'

#-------------------------------------------------------------------------
print_step "1. Publish, compile and run with every artifact in the bucket..."
WF_ID=$(create_and_compile "Hash" "${GRAPH}")
run_workflow "${WF_ID}"
DIGEST=$(runtime_sql "SELECT binary_digest FROM images WHERE tenant_id = '${TENANT}'")
[[ "${DIGEST}" =~ ^[0-9a-f]{64}$ ]] || { print_error "the image names no digest: ${DIGEST}"; exit 1; }
bucket_objects "blobs/sha256/" | grep -qx "${DIGEST}" \
    || { print_error "the workflow image ${DIGEST} is not in the bucket"; exit 1; }
RELEASE=$(bucket_objects "manifests/agents/index/" | head -1 | sed "s/\.json$//")
[ -n "${RELEASE}" ] || { print_error "no release index in the bucket"; exit 1; }
bucket_objects "manifests/agents/crypto/" | grep -q '\.json$' \
    || { print_error "no crypto record in the bucket"; exit 1; }
[ ! -e "${TEST_DATA_DIR}/a/artifacts" ] || { print_error "artifacts were stored under the data dir"; exit 1; }
echo "  release ${RELEASE}, agent records and the workflow image are objects in the bucket; none under the data dir ✓"

#-------------------------------------------------------------------------
print_step "2. A second server: empty data dir, no components directory..."
kill "${SERVER_PID}"; wait "${SERVER_PID}" 2>/dev/null || true
SERVER_PID=""
SERVER_DATA_DIR="${TEST_DATA_DIR}/b" AGENTS_DIR="" start_server
grep -q "loaded [0-9]* agent(s) from release ${RELEASE}" "${TEST_LOG}" \
    || { print_error "the second server did not load release ${RELEASE} from the bucket"; tail -20 "${TEST_LOG}"; exit 1; }
run_workflow "${WF_ID}"
[ -f "${TEST_DATA_DIR}/b/artifact-cache/blobs/sha256/${DIGEST}" ] \
    || { print_error "the image was not read from the bucket into the cache"; exit 1; }
AGAIN=$(create_and_compile "Hash Again" "${GRAPH/\"Hash\"/\"Hash Again\"}")
run_workflow "${AGAIN}"
[ "$(runtime_sql "SELECT count(*) FROM images WHERE tenant_id = '${TENANT}'")" = "2" ] \
    || { print_error "expected the second workflow's image too"; exit 1; }
for digest in $(runtime_sql "SELECT binary_digest FROM images WHERE tenant_id = '${TENANT}'"); do
    bucket_objects "blobs/sha256/" | grep -qx "${digest}" || { print_error "image ${digest} is not in the bucket"; exit 1; }
done
[ ! -e "${TEST_DATA_DIR}/b/artifacts" ] || { print_error "artifacts were stored under the data dir"; exit 1; }
echo "  agents from the bucket's release; the first image ran from the bucket; a new workflow compiled into it and ran ✓"

print_success "object store: releases, agent records and workflow images live in an S3-compatible bucket; a server with nothing local boots, compiles and runs from it"
