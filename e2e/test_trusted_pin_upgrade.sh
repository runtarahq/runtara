#!/bin/bash
# E2E Test: a trusted built-in upgrade recompiles new launches, and a run
# parked on the old version resumes and presigns (trusted pins, option B).
#
# A workflow that presigns with `s3-storage` pins that built-in's exact
# version (component and metadata hashes). After an operator installs a
# different version:
#
#   READINESS  the recorded artifact no longer counts as compiled; the next
#              launch recompiles once, pins the installed version, and runs.
#   HISTORY    boot records every installed trusted pin in
#              `approved_builtin_artifacts`, so after the upgrade the history
#              holds the old and the new s3-storage pins.
#   OLD RUN    an instance parked on the old artifact still loads when it
#              wakes, and its trusted call runs: the old pin is approved and
#              not revoked, and the launch is a wake, so the host runs the
#              installed s3-storage bytes and presigns. A start under the old
#              pin is never admitted (READINESS recompiles it, PUBLISHED fails
#              it); revoked or never-approved pins keep failing with
#              TRUSTED_VERSION_REQUIRED (unit-tested in runtara-component-host).
#   PUBLISHED  a parent reaching the built-in through a published
#              workflow-agent cannot shed the old pin by recompiling (nothing
#              restages the child), so its launch fails terminally, naming the
#              workflow-agent to republish, instead of recompiling forever.
#              Republishing the workflow-agent releases that failure: the
#              parent's next launch recompiles and runs, with no forced
#              recompile.
#   DRIFT      a bundle replaced on disk without a restart compiles to pins the
#              server did not install; that failure is terminal and records
#              its pins, and retries once a restart installs them.
#
# Before the upgrade it also checks that an unchanged bundle keeps the artifact
# ready, so a second launch reuses the image without recompiling.
#
# The "upgrade" is the same component with one extra byte in its metadata
# sidecar: any change to either hash is a different approved version. The S3
# connection holds synthetic credentials; presigning never reaches the network.
#
# Usage:  ./e2e/test_trusted_pin_upgrade.sh
#
# Prereqs: Postgres + docker (isolated Valkey), a built runtara-server, and
# prebuilt components in target/wasm32-wasip2/release
# (scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
# Stderr, so a failure inside $(...) is visible rather than captured.
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Long enough for the parked instance to outlive a server restart.
PARK_DELAY_MS="${PARK_DELAY_MS:-45000}"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD:-GueUkDKea0CjKP4Rn5Bk0FDV}"

TEST_DB_SERVER="${TEST_DB_SERVER:-trusted_pin_e2e_server_$$}"
TEST_DB_RUNTIME="${TEST_DB_RUNTIME:-trusted_pin_e2e_runtime_$$}"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17730}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18731}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18732}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18733}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18734}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16393}"
TEST_DATA_DIR="$(mktemp -d -t runtara_trusted_pin_e2e_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
BUNDLE_DIR="${TEST_DATA_DIR}/components"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="trusted_pin_e2e_$$"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

SERVER_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${POSTGRES_USER}:${POSTGRES_PASSWORD}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

PG_CONTAINER="${PG_CONTAINER:-runtara-dev-postgres}"
if command -v psql >/dev/null 2>&1; then
    psql_quiet() {
        PGPASSWORD="${POSTGRES_PASSWORD}" psql -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
    }
else
    psql_quiet() {
        docker exec -e PGPASSWORD="${POSTGRES_PASSWORD}" -i "${PG_CONTAINER}" \
            psql -U "${POSTGRES_USER}" -tA "$@"
    }
fi
api_post() {
    curl -sS --max-time "${3:-60}" -X POST -H "Content-Type: application/json" -d "$2" "${API}$1"
}

stop_server() {
    if [ -n "${SERVER_PID}" ]; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
        SERVER_PID=""
    fi
}

cleanup() {
    local code=$?
    stop_server
    [ -n "${VALKEY_CONTAINER}" ] && docker rm -f "${VALKEY_CONTAINER}" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_SERVER} WITH (FORCE)" >/dev/null 2>&1 || true
    psql_quiet -d postgres -c "DROP DATABASE IF EXISTS ${TEST_DB_RUNTIME} WITH (FORCE)" >/dev/null 2>&1 || true
    [ ${code} -ne 0 ] && [ -f "${TEST_LOG}" ] && { echo "--- server log tail ---"; tail -60 "${TEST_LOG}"; }
    rm -rf "${TEST_DATA_DIR}"
    exit ${code}
}
trap cleanup EXIT

start_server() {
    (
        # dotenvy walks parent directories; start outside the repository.
        cd "${TEST_DATA_DIR}"
        RUNTARA_SERVER_DATABASE_URL="${SERVER_DB_URL}" \
        OBJECT_MODEL_DATABASE_URL="${SERVER_DB_URL}" \
        RUNTARA_DATABASE_URL="${RUNTIME_DB_URL}" \
        TENANT_ID="${TENANT}" \
        SERVER_HOST=127.0.0.1 \
        SERVER_PORT="${TEST_PORT_PUBLIC}" \
        RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
        RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
        RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
        RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
        RUNTARA_AGENT_COMPONENTS_DIR="${BUNDLE_DIR}" \
        DATA_DIR="${TEST_DATA_DIR}" \
        RUNTARA_DEV_MODE=false \
        RUST_LOG="${RUST_LOG_OVERRIDE:-warn,runtara_server=info,runtara_environment=info}" \
        AUTH_PROVIDER=local \
        VALKEY_HOST=127.0.0.1 \
        VALKEY_PORT="${TEST_VALKEY_PORT}" \
        OTEL_SDK_DISABLED=true \
        SQLX_OFFLINE="${SQLX_OFFLINE}" \
        exec "${RUNTARA_SERVER_BIN}"
    ) >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!

    for _ in {1..90}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        sleep 1
        if ! kill -0 "${SERVER_PID}" 2>/dev/null; then
            print_error "Server exited during boot."; exit 1
        fi
    done
    print_error "Server did not become healthy."; exit 1
}

instance_json() { curl -sS "${API}/workflows/instances/$1"; }
instance_status() { instance_json "$1" | jq -r '.data.status // .status // empty'; }
instance_row() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT COALESCE(status::text,''), COALESCE(convert_from(output, 'UTF8'),''), COALESCE(error,'') FROM instances WHERE instance_id = '$1'"
}
# Server-side compilation record: status | image | pins | compiled_at.
compilation_row() {
    psql_quiet -d "${TEST_DB_SERVER}" -c \
        "SELECT compilation_status, COALESCE(registered_image_id,''), COALESCE(array_to_string(trusted_pins, ','),'<null>'), compiled_at
         FROM workflow_compilations WHERE tenant_id = '${TENANT}' AND workflow_id = '$1' AND version = $2"
}
compile_version() {
    api_post "/workflows/$1/versions/$2/compile${3:-}" '{}' 900
}
version_compiled() {
    curl -sS "${API}/workflows/$1/versions" | jq -r --argjson v "$2" \
        '[.data[]? | select((.version // .versionNumber) == $v) | .compiled] | first // false'
}

# Poll one instance to a terminal status and echo it.
wait_terminal() {
    local id="$1" limit="$2" st deadline
    deadline=$(( $(date +%s) + limit ))
    while [ "$(date +%s)" -lt "${deadline}" ]; do
        st=$(instance_status "${id}")
        case "${st}" in completed|failed|cancelled) echo "${st}"; return 0 ;; esac
        sleep 1
    done
    echo "timeout"
}

# Create a workflow, save `graph`, compile it, and echo "id version".
make_workflow() {
    local name="$1" graph="$2" resp wf_id version
    resp=$(api_post /workflows/create "{\"name\": \"${name}\", \"description\": \"trusted pin upgrade\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || { print_error "Workflow create failed: ${resp}"; exit 1; }
    resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${graph}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Update failed: ${resp}"; exit 1; }
    version=$(curl -sS "${API}/workflows/${wf_id}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf_id}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Compile failed: ${resp}"; exit 1; }
    echo "${wf_id} ${version}"
}

# Launch, retrying while the version is (re)compiling; echo the instance id.
launch() {
    local wf_id="$1" resp id
    for _ in {1..180}; do
        resp=$(api_post "/workflows/${wf_id}/execute" '{"inputs": {"data": {}, "variables": {}}}')
        id=$(echo "${resp}" | jq -r '.data.instanceId // empty')
        if [ -n "${id}" ]; then echo "${id}"; return 0; fi
        sleep 2
    done
    print_error "Execute never launched: ${resp}"; exit 1
}

sign_step() {
    jq -n --arg conn "$1" '{
        id: "sign", stepType: "Agent", agentId: "s3-storage",
        capabilityId: "storage-generate-presigned-url", maxRetries: 0,
        connectionId: $conn,
        inputMapping: {
            bucket: { valueType: "immediate", value: "uploads" },
            key: { valueType: "immediate", value: "report.csv" },
            operation: { valueType: "immediate", value: "download" }
        }
    }'
}

echo "==============================================================="
echo "E2E: trusted built-in upgrade (readiness recompiles, parked run resumes under its approved pin)"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN} (cargo build -p runtara-server --bin runtara-server)"; exit 1; }
[ -f "${COMPONENTS_DIR}/runtara_agent_s3_storage.wasm" ] || { print_error "Missing s3-storage component — run scripts/build-agent-components.sh"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres (psql on PATH, or container '${PG_CONTAINER}')"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }

print_step "Staging a private component bundle..."
mkdir -p "${BUNDLE_DIR}"
cp "${COMPONENTS_DIR}"/*.wasm "${COMPONENTS_DIR}"/*.meta.json "${BUNDLE_DIR}/"

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC}..."
start_server

print_step "Creating a synthetic S3 connection..."
RESP=$(api_post /connections '{"title":"trusted-pin-e2e","integrationId":"s3_compatible","connectionParameters":{"endpoint":"https://storage.example.test","access_key_id":"e2e-access","secret_access_key":"e2e-synthetic-secret","region":"us-east-1"}}')
CONN=$(echo "${RESP}" | jq -r '.data.id // .id // .connectionId // empty')
[ -n "${CONN}" ] || { print_error "Connection create failed: ${RESP}"; exit 1; }

SIGN_GRAPH=$(jq -n --argjson sign "$(sign_step "${CONN}")" '{
    name: "trusted-pin-sign", durable: true, entryPoint: "sign",
    steps: { sign: $sign,
             finish: { id: "finish", stepType: "Finish",
                       inputMapping: { url: { valueType: "reference", value: "steps.sign.outputs.url" } } } },
    executionPlan: [ { fromStep: "sign", toStep: "finish" } ],
    variables: {}, inputSchema: {}, outputSchema: {}
}')
PARK_GRAPH=$(jq -n --argjson sign "$(sign_step "${CONN}")" --argjson ms "${PARK_DELAY_MS}" '{
    name: "trusted-pin-park", durable: true, entryPoint: "delay",
    steps: { delay: { id: "delay", stepType: "Delay", name: "Wait", durationMs: { valueType: "immediate", value: $ms } },
             sign: $sign,
             finish: { id: "finish", stepType: "Finish",
                       inputMapping: { url: { valueType: "reference", value: "steps.sign.outputs.url" } } } },
    executionPlan: [ { fromStep: "delay", toStep: "sign" }, { fromStep: "sign", toStep: "finish" } ],
    variables: {}, inputSchema: {}, outputSchema: {}
}')

# ---------------------------------------------------------------------------
# Unchanged bundle: the pinned artifact is ready and reused.
# ---------------------------------------------------------------------------
print_step "Compiling and running the presign workflow..."
SIGN_WORKFLOW=$(make_workflow trusted-pin-sign "${SIGN_GRAPH}")
read -r SIGN_WF SIGN_V <<< "${SIGN_WORKFLOW}"
BEFORE=$(compilation_row "${SIGN_WF}" "${SIGN_V}")
IFS='|' read -r B_STATUS B_IMAGE B_PINS B_AT <<< "${BEFORE}"
echo "  compilation: status=${B_STATUS} image=${B_IMAGE} pins=${B_PINS}"
[ "${B_STATUS}" = "success" ] && [ -n "${B_IMAGE}" ] || { print_error "No registered success: ${BEFORE}"; exit 1; }
case "${B_PINS}" in runtara:trusted-artifacts/s3-storage-h*) ;; *) print_error "Recorded pins are not the s3-storage pin: ${B_PINS}"; exit 1 ;; esac
[ "$(version_compiled "${SIGN_WF}" "${SIGN_V}")" = "true" ] || { print_error "Version list does not report the pinned artifact compiled"; exit 1; }

for run in 1 2; do
    INST=$(launch "${SIGN_WF}")
    [ "$(wait_terminal "${INST}" 120)" = "completed" ] || { print_error "Run ${run} did not complete: $(instance_row "${INST}")"; exit 1; }
    instance_row "${INST}" | grep -q "X-Amz-Signature=" || { print_error "Run ${run} has no signed URL: $(instance_row "${INST}")"; exit 1; }
done
[ "$(compilation_row "${SIGN_WF}" "${SIGN_V}")" = "${BEFORE}" ] || { print_error "An unchanged bundle recompiled: $(compilation_row "${SIGN_WF}" "${SIGN_V}")"; exit 1; }
print_success "Unchanged bundle: pinned artifact stays ready, two runs reuse image ${B_IMAGE} ✓"

print_step "Publishing a presigning workflow-agent and a parent that calls it..."
RESP=$(api_post /workflows/create '{"name":"Trusted Pin Signer","description":"trusted pin upgrade","slug":"trusted-pin-signer"}')
SIGNER_WF=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${SIGNER_WF}" ] || { print_error "Signer create failed: ${RESP}"; exit 1; }
SIGNER_GRAPH=$(echo "${SIGN_GRAPH}" | jq '.name = "trusted-pin-signer" | .durable = false')
RESP=$(api_post "/workflows/${SIGNER_WF}/update" "{\"executionGraph\": ${SIGNER_GRAPH}}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Signer update failed: ${RESP}"; exit 1; }
publish_signer() {
    local resp
    resp=$(api_post "/workflows/${SIGNER_WF}/publish-agent" "" 900)
    [ "$(echo "${resp}" | jq -r '.data.agentId // empty')" = "trusted-pin-signer" ] \
        || { print_error "publish-agent failed: ${resp}"; exit 1; }
}
publish_signer
PARENT_GRAPH=$(jq -n '{
    name: "trusted-pin-parent", durable: true, entryPoint: "call",
    steps: { call: { id: "call", stepType: "Agent", agentId: "trusted-pin-signer", capabilityId: "run", maxRetries: 0 },
             finish: { id: "finish", stepType: "Finish",
                       inputMapping: { url: { valueType: "reference", value: "steps.call.outputs.url" } } } },
    executionPlan: [ { fromStep: "call", toStep: "finish" } ],
    variables: {}, inputSchema: {}, outputSchema: {}
}')
PARENT_WORKFLOW=$(make_workflow trusted-pin-parent "${PARENT_GRAPH}")
read -r PARENT_WF PARENT_V <<< "${PARENT_WORKFLOW}"
[ "$(compilation_row "${PARENT_WF}" "${PARENT_V}" | cut -d'|' -f3)" = "${B_PINS}" ] \
    || { print_error "Parent does not record the child's pin: $(compilation_row "${PARENT_WF}" "${PARENT_V}")"; exit 1; }
INST=$(launch "${PARENT_WF}")
[ "$(wait_terminal "${INST}" 120)" = "completed" ] || { print_error "Parent run did not complete: $(instance_row "${INST}")"; exit 1; }
instance_row "${INST}" | grep -q "X-Amz-Signature=" || { print_error "Parent run has no signed URL: $(instance_row "${INST}")"; exit 1; }
print_success "Parent presigns through the published workflow-agent and records its pin ✓"

print_step "Parking an instance on the current artifact (${PARK_DELAY_MS}ms Delay before the trusted step)..."
PARK_WORKFLOW=$(make_workflow trusted-pin-park "${PARK_GRAPH}")
read -r PARK_WF PARK_V <<< "${PARK_WORKFLOW}"
PARKED=$(launch "${PARK_WF}")
echo "  parked workflow ${PARK_WF} v${PARK_V}"
for _ in {1..30}; do
    [ "$(instance_row "${PARKED}" | cut -d'|' -f1)" = "suspended" ] && break
    sleep 1
done
[ "$(instance_row "${PARKED}" | cut -d'|' -f1)" = "suspended" ] || { print_error "Instance never parked: $(instance_row "${PARKED}")"; exit 1; }
echo "  instance ${PARKED} parked"

# ---------------------------------------------------------------------------
# Upgrade: a different approved version of s3-storage.
# ---------------------------------------------------------------------------
print_step "Restarting with an upgraded s3-storage bundle..."
stop_server
echo "" >> "${BUNDLE_DIR}/runtara_agent_s3_storage.meta.json"
start_server

[ "$(version_compiled "${SIGN_WF}" "${SIGN_V}")" = "false" ] || { print_error "The version list still reports the stale artifact compiled"; exit 1; }
print_success "Stale artifact no longer reported compiled ✓"

HISTORY=$(psql_quiet -d "${TEST_DB_RUNTIME}" -c \
    "SELECT count(*) FILTER (WHERE revoked_at IS NULL) || '|' || count(*) || '|' || count(*) FILTER (WHERE pin = '${B_PINS}')
     FROM approved_builtin_artifacts WHERE pin LIKE 'runtara:trusted-artifacts/s3-storage-h%'")
echo "  s3-storage history (approved|total|old pin): ${HISTORY}"
[ "${HISTORY}" = "2|2|1" ] || { print_error "Expected the old and new s3-storage pins approved: ${HISTORY}"; exit 1; }
print_success "Boot recorded both s3-storage versions in the approved history ✓"

print_step "Next launch recompiles once against the installed version..."
INST=$(launch "${SIGN_WF}")
[ "$(wait_terminal "${INST}" 300)" = "completed" ] || { print_error "Post-upgrade run did not complete: $(instance_row "${INST}")"; exit 1; }
instance_row "${INST}" | grep -q "X-Amz-Signature=" || { print_error "Post-upgrade run has no signed URL: $(instance_row "${INST}")"; exit 1; }
AFTER=$(compilation_row "${SIGN_WF}" "${SIGN_V}")
IFS='|' read -r A_STATUS A_IMAGE A_PINS A_AT <<< "${AFTER}"
echo "  compilation: status=${A_STATUS} image=${A_IMAGE} pins=${A_PINS}"
[ "${A_STATUS}" = "success" ] && [ "${A_IMAGE}" != "${B_IMAGE}" ] && [ "${A_PINS}" != "${B_PINS}" ] \
    || { print_error "Expected a new image and pin after the upgrade: before=${BEFORE} after=${AFTER}"; exit 1; }
[ "$(version_compiled "${SIGN_WF}" "${SIGN_V}")" = "true" ] || { print_error "Rebuilt artifact not reported compiled"; exit 1; }
INST=$(launch "${SIGN_WF}")
[ "$(wait_terminal "${INST}" 120)" = "completed" ] || { print_error "Second post-upgrade run failed: $(instance_row "${INST}")"; exit 1; }
[ "$(compilation_row "${SIGN_WF}" "${SIGN_V}")" = "${AFTER}" ] || { print_error "The rebuilt artifact recompiled again"; exit 1; }
print_success "Recompiled once to image ${A_IMAGE}, then reused ✓"

# ---------------------------------------------------------------------------
# A parent composed from a workflow-agent published before the upgrade.
# ---------------------------------------------------------------------------
print_step "Launching the parent whose published workflow-agent predates the upgrade..."
[ "$(version_compiled "${PARENT_WF}" "${PARENT_V}")" = "false" ] || { print_error "The version list still reports the stale parent compiled"; exit 1; }
# Launches are accepted and queued; readiness is decided when the request is
# launched. The first one recompiles, the compiler refuses the stale child, and
# the request is terminalized on that recorded failure instead of being
# retried while the workflow recompiles.
request_state() {
    psql_quiet -d "${TEST_DB_SERVER}" -c \
        "SELECT state || '|' || COALESCE(terminal_reason, '') FROM execution_requests WHERE tenant_id = '${TENANT}' AND instance_id = '$1'"
}
stale_parent_run() {
    local resp inst st=""
    resp=$(api_post "/workflows/${PARENT_WF}/execute" '{"inputs": {"data": {}, "variables": {}}}')
    inst=$(echo "${resp}" | jq -r '.data.instanceId // empty')
    [ -n "${inst}" ] || { print_error "Parent launch was not accepted: ${resp}"; exit 1; }
    for _ in {1..150}; do
        st=$(request_state "${inst}")
        case "${st}" in terminal*|accepted*) break ;; esac
        sleep 2
    done
    [ "${st%%|*}" = "terminal" ] || { print_error "Parent request on a stale workflow-agent should be terminal, got '${st}'"; exit 1; }
    [ -z "$(instance_row "${inst}")" ] || { print_error "Parent instance started on a stale workflow-agent: $(instance_row "${inst}")"; exit 1; }
}
stale_parent_run
FAILED=$(compilation_row "${PARENT_WF}" "${PARENT_V}")
echo "  compilation: $(echo "${FAILED}" | cut -d'|' -f1,3)"
[ "$(echo "${FAILED}" | cut -d'|' -f1)" = "failed" ] || { print_error "No terminal failure recorded: ${FAILED}"; exit 1; }
[ "$(echo "${FAILED}" | cut -d'|' -f3)" = "${B_PINS}" ] \
    || { print_error "The failure does not record the workflow-agent's stale pin: ${FAILED}"; exit 1; }
psql_quiet -d "${TEST_DB_SERVER}" -c \
    "SELECT error_message FROM workflow_compilations WHERE tenant_id = '${TENANT}' AND workflow_id = '${PARENT_WF}' AND version = ${PARENT_V}" \
    | grep -q "workflow-agent \`trusted-pin-signer\`" \
    || { print_error "Recorded failure does not name the workflow-agent"; exit 1; }
stale_parent_run
[ "$(compilation_row "${PARENT_WF}" "${PARENT_V}")" = "${FAILED}" ] \
    || { print_error "The terminal failure was recompiled: before=${FAILED} after=$(compilation_row "${PARENT_WF}" "${PARENT_V}")"; exit 1; }
print_success "Stale workflow-agent: parent fails terminally with a republish diagnostic, no recompile loop ✓"

print_step "Republishing the workflow-agent; the parent's next launch recompiles by itself..."
publish_signer
INST=$(launch "${PARENT_WF}")
[ "$(wait_terminal "${INST}" 300)" = "completed" ] || { print_error "Parent did not complete after republish: $(instance_row "${INST}")"; exit 1; }
[ "$(compilation_row "${PARENT_WF}" "${PARENT_V}" | cut -d'|' -f1,3)" = "success|${A_PINS}" ] \
    || { print_error "Parent does not pin the installed version: $(compilation_row "${PARENT_WF}" "${PARENT_V}")"; exit 1; }
instance_row "${INST}" | grep -q "X-Amz-Signature=" || { print_error "Republished parent run has no signed URL: $(instance_row "${INST}")"; exit 1; }
print_success "Republished workflow-agent: parent pins the installed version and runs ✓"

print_step "Waiting for the parked instance to wake on its old artifact..."
ST=$(wait_terminal "${PARKED}" $(( PARK_DELAY_MS / 1000 + 120 )))
ROW=$(instance_row "${PARKED}")
echo "  status=${ST} row=$(echo "${ROW}" | head -c 400)"
[ "${ST}" = "completed" ] || { print_error "Old instance should wake and presign under its approved pin, got ${ST}: ${ROW}"; exit 1; }
echo "${ROW}" | grep -q "X-Amz-Signature=" \
    || { print_error "Old instance has no signed URL: ${ROW}"; exit 1; }
if echo "${ROW} $(instance_json "${PARKED}")" | grep -q "TRUSTED_VERSION_REQUIRED"; then
    print_error "Old instance was refused at its trusted call: ${ROW}"; exit 1
fi
if echo "${ROW} $(instance_json "${PARKED}")" | grep -q "e2e-synthetic-secret"; then
    print_error "Credential material leaked into the instance record"; exit 1
fi
print_success "Old instance woke on its old artifact and presigned on the installed bytes ✓"

# ---------------------------------------------------------------------------
# A bundle replaced on disk without a restart: the compiler reads a version
# the running server did not install.
# ---------------------------------------------------------------------------
print_step "Replacing s3-storage on disk without restarting..."
echo "" >> "${BUNDLE_DIR}/runtara_agent_s3_storage.meta.json"
RESP=$(api_post /workflows/create '{"name": "trusted-pin-drift", "description": "trusted pin upgrade"}')
DRIFT_WF=$(echo "${RESP}" | jq -r '.data.id // empty')
[ -n "${DRIFT_WF}" ] || { print_error "Workflow create failed: ${RESP}"; exit 1; }
RESP=$(api_post "/workflows/${DRIFT_WF}/update" "{\"executionGraph\": $(echo "${SIGN_GRAPH}" | jq '.name = "trusted-pin-drift"')}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "true" ] || { print_error "Update failed: ${RESP}"; exit 1; }
DRIFT_V=$(curl -sS "${API}/workflows/${DRIFT_WF}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
RESP=$(compile_version "${DRIFT_WF}" "${DRIFT_V}")
[ "$(echo "${RESP}" | jq -r '.success // false')" = "false" ] && echo "${RESP}" | grep -q "does not run" \
    || { print_error "Compiling against a drifted bundle should fail naming the version: ${RESP}"; exit 1; }
DRIFTED=$(compilation_row "${DRIFT_WF}" "${DRIFT_V}")
echo "  compilation: $(echo "${DRIFTED}" | cut -d'|' -f1,3)"
[ "$(echo "${DRIFTED}" | cut -d'|' -f1)" = "failed" ] && [ "$(echo "${DRIFTED}" | cut -d'|' -f3)" != "<null>" ] \
    || { print_error "Expected a failure carrying the compiled pins: ${DRIFTED}"; exit 1; }
RESP=$(api_post "/workflows/${DRIFT_WF}/execute" '{"inputs": {"data": {}, "variables": {}}}')
INST=$(echo "${RESP}" | jq -r '.data.instanceId // empty')
[ -n "${INST}" ] || { print_error "Launch was not accepted: ${RESP}"; exit 1; }
ST=""
for _ in {1..60}; do
    ST=$(request_state "${INST}")
    case "${ST}" in terminal*|accepted*) break ;; esac
    sleep 2
done
[ "${ST%%|*}" = "terminal" ] || { print_error "Launch on a drifted bundle should be terminal, got '${ST}'"; exit 1; }
[ "$(compilation_row "${DRIFT_WF}" "${DRIFT_V}")" = "${DRIFTED}" ] || { print_error "The drift failure was recompiled before a restart"; exit 1; }
print_success "Drifted bundle: compile fails terminally with its pins recorded, no recompile loop ✓"

print_step "Restarting onto the drifted bundle heals the failure..."
stop_server
start_server
INST=$(launch "${DRIFT_WF}")
[ "$(wait_terminal "${INST}" 300)" = "completed" ] || { print_error "Run after the restart did not complete: $(instance_row "${INST}")"; exit 1; }
[ "$(compilation_row "${DRIFT_WF}" "${DRIFT_V}" | cut -d'|' -f1)" = "success" ] || { print_error "No rebuilt success: $(compilation_row "${DRIFT_WF}" "${DRIFT_V}")"; exit 1; }
print_success "After the restart the failure retried once and the workflow runs ✓"

echo
print_success "Trusted built-in upgrades recompile new launches and parked runs resume under approved pins."
