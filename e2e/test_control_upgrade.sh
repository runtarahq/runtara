#!/bin/bash
# E2E Test: parked control:wait parents survive a binary and bundle upgrade.
#
# Release N (server binary + component bundle) parks two parents on their
# Finance and Legal approvals; one approval of the second parent is answered
# before the upgrade. The server is stopped and release N+1 is started on the
# same databases with a control agent whose digest differs (asserted). Both
# parked parents keep their compiled packages and bound images, the old
# control pin stays approved beside the new one, and both resume to their
# results once their approvals are answered. A parent started on N+1 runs too.
#
# N defaults to the current build and N+1 to the same binary with a forced
# version bump of the control agent in a scratch copy of the bundle (the crate
# version is embedded in neither the component nor its sidecar, so a rebuild
# of the same source would keep the digest). Point RUNTARA_SERVER_BIN_NEXT and
# NEXT_COMPONENTS_DIR at a real N+1 release to test that instead.
#
# Not part of run_all.sh.
#
# Usage:  ./e2e/test_control_upgrade.sh
#
# Prereqs: Postgres + docker (isolated Valkey), a built runtara-server, and
# prebuilt components (scripts/build-agent-components.sh).

set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; NC='\033[0m'
print_step()    { echo -e "${GREEN}[STEP]${NC} $1"; }
print_error()   { echo -e "${RED}[ERROR]${NC} $1" >&2; }
print_success() { echo -e "${GREEN}[SUCCESS]${NC} $1"; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-smo_worker}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD-GueUkDKea0CjKP4Rn5Bk0FDV}"

TEST_DB_SERVER="${TEST_DB_SERVER:-control_upgrade_server_$$}"
TEST_DB_RUNTIME="${TEST_DB_RUNTIME:-control_upgrade_runtime_$$}"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17760}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18761}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18762}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18763}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18764}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16396}"
TEST_DATA_DIR="$(mktemp -d -t runtara_control_upgrade_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
BUNDLE_DIR="${TEST_DATA_DIR}/components"
SERVER_PID=""
VALKEY_CONTAINER=""
TENANT="control_upgrade_$$"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
RUNTARA_SERVER_BIN_NEXT="${RUNTARA_SERVER_BIN_NEXT:-${RUNTARA_SERVER_BIN}}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"
NEXT_COMPONENTS_DIR="${NEXT_COMPONENTS_DIR:-}"
SQLX_OFFLINE="${SQLX_OFFLINE:-true}"

if [ -n "${POSTGRES_PASSWORD}" ]; then
    CREDENTIALS="${POSTGRES_USER}:${POSTGRES_PASSWORD}"
else
    CREDENTIALS="${POSTGRES_USER}"
fi
SERVER_DB_URL="postgresql://${CREDENTIALS}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_SERVER}"
RUNTIME_DB_URL="postgresql://${CREDENTIALS}@${POSTGRES_HOST}:${POSTGRES_PORT}/${TEST_DB_RUNTIME}"
API="http://127.0.0.1:${TEST_PORT_PUBLIC}/api/runtime"

psql_quiet() {
    PGPASSWORD="${POSTGRES_PASSWORD}" psql -X -U "${POSTGRES_USER}" -h "${POSTGRES_HOST}" -p "${POSTGRES_PORT}" -tA "$@"
}
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

# start_server <binary>
start_server() {
    local bin="$1"
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
        RUST_LOG="${RUST_LOG_OVERRIDE:-warn,runtara_server=info,runtara_environment=info,runtara_component_host=info}" \
        AUTH_PROVIDER=local \
        VALKEY_HOST=127.0.0.1 \
        VALKEY_PORT="${TEST_VALKEY_PORT}" \
        OTEL_SDK_DISABLED=true \
        SQLX_OFFLINE="${SQLX_OFFLINE}" \
        exec "${bin}"
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

instance_status() { curl -sS "${API}/workflows/instances/$1" | jq -r '.data.status // .status // empty'; }
instance_row() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT COALESCE(status::text,''), COALESCE(convert_from(output, 'UTF8'),''), COALESCE(error,'') FROM instances WHERE instance_id = '$1'"
}
instance_output() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT COALESCE(convert_from(output, 'UTF8'),'null') FROM instances WHERE instance_id = '$1'"
}
bound_image() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT image_id FROM instance_images WHERE instance_id = '$1'"
}
wait_status() {
    local id="$1" want="$2" limit="$3" st deadline
    deadline=$(( $(date +%s) + limit ))
    while [ "$(date +%s)" -lt "${deadline}" ]; do
        st=$(instance_status "${id}")
        case "${st}" in "${want}") echo "${st}"; return 0 ;; completed|failed|cancelled) echo "${st}"; return 0 ;; esac
        sleep 1
    done
    echo "timeout"
}

# Create a workflow, save `graph`, compile it, and echo "id version".
make_workflow() {
    local name="$1" graph="$2" resp wf_id version
    resp=$(api_post /workflows/create "{\"name\": \"${name}\", \"description\": \"control upgrade e2e\"}")
    wf_id=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf_id}" ] || { print_error "Workflow create failed: ${resp}"; exit 1; }
    resp=$(api_post "/workflows/${wf_id}/update" "{\"executionGraph\": ${graph}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Update failed: ${resp}"; exit 1; }
    version=$(curl -sS "${API}/workflows/${wf_id}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf_id}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Compile of ${name} failed: ${resp}"; exit 1; }
    echo "${wf_id} ${version}"
}

# Launch with `data`, retrying while the version is (re)compiling.
launch() {
    local wf_id="$1" data="${2:-}" resp id
    [ -n "${data}" ] || data='{}'
    for _ in {1..90}; do
        resp=$(api_post "/workflows/${wf_id}/execute" "{\"inputs\": {\"data\": ${data}, \"variables\": {}}}")
        id=$(echo "${resp}" | jq -r '.data.instanceId // empty')
        if [ -n "${id}" ]; then echo "${id}"; return 0; fi
        sleep 2
    done
    print_error "Execute never launched: ${resp}"; exit 1
}

labelled() {
    psql_quiet -d "${TEST_DB_SERVER}" -c \
        "SELECT instance_id FROM execution_requests WHERE tenant_id = '${TENANT}' AND parent_instance_id = '$1' AND run_label = '$2'"
}
open_request() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT request_id FROM instance_input_requests WHERE instance_id = '$1' AND state = 'open'"
}
parked_reason() {
    psql_quiet -d "${TEST_DB_RUNTIME}" -c \
        "SELECT COALESCE(termination_reason::text, '') FROM instances WHERE instance_id = '$1'"
}
# Answer a child's open WaitForSignal request through the public API.
answer() {
    local child="$1" approved="$2" request resp
    request=""
    for _ in {1..60}; do
        request=$(open_request "${child}")
        [ -n "${request}" ] && break
        sleep 1
    done
    [ -n "${request}" ] || { print_error "Child ${child} has no open request: $(instance_row "${child}")"; exit 1; }
    resp=$(api_post "/signals/${child}" "$(jq -nc --arg r "${request}" --arg o "upgrade-answer-${child}" --argjson a "${approved}" \
        '{requestId: $r, operationId: $o, payload: {approved: $a}}')")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Answering ${child} failed: ${resp}"; exit 1; }
}
approvals_step() {
    jq -n --arg id "$1" '{
        id: $id, stepType: "Agent", agentId: "control", capabilityId: "start",
        maxRetries: 0, durable: true, inputMapping: {
            workflowId: { valueType: "reference", value: "data.approver" },
            runLabel: { valueType: "immediate", value: $id },
            parentClosePolicy: { valueType: "immediate", value: "cancel" },
            inputs: { valueType: "immediate", value: { data: {}, variables: {} } } } }'
}
# Launch a parent; wait until it parks on both approvals; echo "parent finance legal".
parked_parent() {
    local wf="$1" data="$2" parent finance legal
    parent=$(launch "${wf}" "${data}")
    for _ in {1..60}; do [ -n "$(labelled "${parent}" legal)" ] && break; sleep 0.5; done
    finance=$(labelled "${parent}" finance); legal=$(labelled "${parent}" legal)
    [ -n "${finance}" ] && [ -n "${legal}" ] || { print_error "Parent ${parent} did not start both approvals"; exit 1; }
    for child in "${finance}" "${legal}"; do
        [ "$(wait_status "${child}" suspended 90)" = "suspended" ] || { print_error "Approval ${child} did not park: $(instance_row "${child}")"; exit 1; }
    done
    [ "$(wait_status "${parent}" suspended 90)" = "suspended" ] || { print_error "Parent ${parent} did not park: $(instance_row "${parent}")"; exit 1; }
    for _ in {1..30}; do [ "$(parked_reason "${parent}")" = "waiting_instances" ] && break; sleep 0.5; done
    [ "$(parked_reason "${parent}")" = "waiting_instances" ] || { print_error "Parent ${parent} is not waiting on its children"; exit 1; }
    echo "${parent} ${finance} ${legal}"
}
parked_or_exit() { [ -n "${3:-}" ] || { print_error "No parked parent"; exit 1; }; }
# Wait until the parent finishes and check it saw both approvals.
expect_resumed() {
    local parent="$1" out
    [ "$(wait_status "${parent}" completed 180)" = "completed" ] || { print_error "Parent ${parent} did not resume: $(instance_row "${parent}")"; exit 1; }
    out=$(instance_output "${parent}")
    [ "$(echo "${out}" | jq -r '.result.resolution + "|" + (.result.finished | length | tostring)')" = "satisfied|2" ] \
        || { print_error "Unexpected result for ${parent}: ${out}"; exit 1; }
}

control_pin_of() {
    echo "runtara:builtin-artifacts/control-h$(shasum -a 256 "$1/runtara_agent_control.wasm" | awk '{print $1}')-h$(shasum -a 256 "$1/runtara_agent_control.meta.json" | awk '{print $1}')@0.1.0"
}
# The same control agent with a forced version bump: a `version` custom
# section on the component and a `version` field in its sidecar.
bump_control_bundle() {
    python3 - "$1" "$2" <<'PYEOF'
import json, sys
bundle, version = sys.argv[1], sys.argv[2]
def leb(n):
    out = bytearray()
    while True:
        b = n & 0x7f; n >>= 7
        if n: out.append(b | 0x80)
        else: out.append(b); return bytes(out)
path = f"{bundle}/runtara_agent_control.wasm"
name, payload = b"runtara-version", version.encode()
body = leb(len(name)) + name + payload
with open(path, "ab") as f:
    f.write(b"\x00" + leb(len(body)) + body)
path = f"{bundle}/runtara_agent_control.meta.json"
meta = json.load(open(path))
meta["version"] = version
json.dump(meta, open(path, "w"), indent=2)
PYEOF
}

echo "==============================================================="
echo "E2E: control upgrade (parked parents across binary + bundle N -> N+1)"
echo "==============================================================="

for bin in "${RUNTARA_SERVER_BIN}" "${RUNTARA_SERVER_BIN_NEXT}"; do
    [ -x "${bin}" ] || { print_error "Missing server bin ${bin} (cargo build -p runtara-server --bin runtara-server)"; exit 1; }
done
[ -f "${COMPONENTS_DIR}/runtara_agent_control.wasm" ] || { print_error "Missing control component — run scripts/build-agent-components.sh"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }

print_step "Staging bundles N and N+1..."
mkdir -p "${BUNDLE_DIR}"
cp "${COMPONENTS_DIR}"/*.wasm "${COMPONENTS_DIR}"/*.meta.json "${BUNDLE_DIR}/"
NEXT_BUNDLE="${TEST_DATA_DIR}/components-next"
mkdir -p "${NEXT_BUNDLE}"
if [ -n "${NEXT_COMPONENTS_DIR}" ]; then
    cp "${NEXT_COMPONENTS_DIR}"/*.wasm "${NEXT_COMPONENTS_DIR}"/*.meta.json "${NEXT_BUNDLE}/"
else
    cp "${BUNDLE_DIR}"/*.wasm "${BUNDLE_DIR}"/*.meta.json "${NEXT_BUNDLE}/"
    bump_control_bundle "${NEXT_BUNDLE}" "999.0.0-upgrade"
fi
OLD_PIN=$(control_pin_of "${BUNDLE_DIR}")
NEW_PIN=$(control_pin_of "${NEXT_BUNDLE}")
[ "${OLD_PIN}" != "${NEW_PIN}" ] || { print_error "N and N+1 share the control digest ${OLD_PIN}; force a version bump"; exit 1; }
echo "  N:   ${OLD_PIN}"
echo "  N+1: ${NEW_PIN}"

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting release N on :${TEST_PORT_PUBLIC}..."
start_server "${RUNTARA_SERVER_BIN}"

APPROVER_GRAPH=$(jq -n '{
    name: "upgrade-approver", durable: true, entryPoint: "approve",
    steps: { approve: { id: "approve", stepType: "WaitForSignal", name: "Approve",
                        pollIntervalMs: 500,
                        responseSchema: { approved: { type: "boolean", required: true } } },
             finish: { id: "finish", stepType: "Finish", inputMapping: {
                 decision: { valueType: "reference", value: "steps.approve.outputs" } } } },
    executionPlan: [ { fromStep: "approve", toStep: "finish" } ],
    variables: {}, outputSchema: {}, inputSchema: {}
}')
PARENT_GRAPH=$(jq -n --argjson finance "$(approvals_step finance)" --argjson legal "$(approvals_step legal)" '{
    name: "upgrade-approvals", durable: true, entryPoint: "finance",
    steps: { finance: $finance, legal: $legal,
             wait: { id: "wait", stepType: "Agent", agentId: "control", capabilityId: "wait",
                     maxRetries: 0, durable: true, timeout: 600000, inputMapping: {
                         instanceIds: { valueType: "composite", value: [
                             { valueType: "reference", value: "steps.finance.outputs.instanceId" },
                             { valueType: "reference", value: "steps.legal.outputs.instanceId" } ] },
                         mode: { valueType: "immediate", value: "all" } } },
             finish: { id: "finish", stepType: "Finish", inputMapping: {
                 result: { valueType: "reference", value: "steps.wait.outputs" } } } },
    executionPlan: [ { fromStep: "finance", toStep: "legal" }, { fromStep: "legal", toStep: "wait" },
                     { fromStep: "wait", toStep: "finish" } ],
    variables: {}, outputSchema: {}, inputSchema: { approver: { type: "string", required: true } }
}')
read -r APPROVER_WF _ <<< "$(make_workflow upgrade-approver "${APPROVER_GRAPH}")"
read -r PARENT_WF _ <<< "$(make_workflow upgrade-approvals "${PARENT_GRAPH}")"
DATA=$(jq -nc --arg a "${APPROVER_WF}" '{approver: $a}')

print_step "Release N: parking two parents on their approvals..."
read -r P1 P1_FIN P1_LEG <<< "$(parked_parent "${PARENT_WF}" "${DATA}")"
parked_or_exit "${P1:-}" "${P1_FIN:-}" "${P1_LEG:-}"
read -r P2 P2_FIN P2_LEG <<< "$(parked_parent "${PARENT_WF}" "${DATA}")"
parked_or_exit "${P2:-}" "${P2_FIN:-}" "${P2_LEG:-}"
# Partial progress before the upgrade: one of P2's approvals is answered.
answer "${P2_FIN}" true
[ "$(wait_status "${P2_FIN}" completed 90)" = "completed" ] || { print_error "P2's finance approval did not finish: $(instance_row "${P2_FIN}")"; exit 1; }
sleep 2
[ "$(instance_status "${P2}")" = "suspended" ] || { print_error "One answer woke an \`all\` wait: $(instance_row "${P2}")"; exit 1; }
P1_IMAGE=$(bound_image "${P1}"); P2_IMAGE=$(bound_image "${P2}")
[ -n "${P1_IMAGE}" ] && [ -n "${P2_IMAGE}" ] || { print_error "Parked parents have no bound image"; exit 1; }
[ "$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT count(*) FROM approved_builtin_artifacts WHERE pin = '${OLD_PIN}' AND revoked_at IS NULL")" = "1" ] \
    || { print_error "Release N did not approve its control bytes"; exit 1; }

print_step "Upgrading: stopping N, starting N+1 on the same databases..."
stop_server
rm -rf "${BUNDLE_DIR}" && mv "${NEXT_BUNDLE}" "${BUNDLE_DIR}"
start_server "${RUNTARA_SERVER_BIN_NEXT}"
[ "$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT count(*) FROM approved_builtin_artifacts
      WHERE pin IN ('${OLD_PIN}', '${NEW_PIN}') AND revoked_at IS NULL")" = "2" ] \
    || { print_error "N+1 should approve its control bytes and keep N's in the history"; exit 1; }
for parent in "${P1}" "${P2}"; do
    [ "$(instance_status "${parent}")" = "suspended" ] || { print_error "Parent ${parent} did not stay parked: $(instance_row "${parent}")"; exit 1; }
done

print_step "Release N+1: answering the remaining approvals..."
answer "${P1_FIN}" true
answer "${P1_LEG}" false
answer "${P2_LEG}" true
expect_resumed "${P1}"
expect_resumed "${P2}"
[ "$(bound_image "${P1}")" = "${P1_IMAGE}" ] && [ "$(bound_image "${P2}")" = "${P2_IMAGE}" ] \
    || { print_error "A parked parent was rebound to another image"; exit 1; }
[ "$(psql_quiet -d "${TEST_DB_RUNTIME}" -c "SELECT count(*) FROM images WHERE image_id IN ('${P1_IMAGE}', '${P2_IMAGE}')")" -ge 1 ] \
    || { print_error "The parked parents' images are gone"; exit 1; }
print_success "Both parents parked on N resumed on N+1 to both results ✓"

print_step "Release N+1: a new parent runs end to end..."
read -r P3 P3_FIN P3_LEG <<< "$(parked_parent "${PARENT_WF}" "${DATA}")"
parked_or_exit "${P3:-}" "${P3_FIN:-}" "${P3_LEG:-}"
answer "${P3_LEG}" true
answer "${P3_FIN}" true
expect_resumed "${P3}"
print_success "A parent started on N+1 completes ✓"

echo
print_success "Control upgrade: parked parents survive the binary and bundle upgrade."
