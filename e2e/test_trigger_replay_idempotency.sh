#!/bin/bash
# E2E regression: stale compiled artifact repair + idempotent trigger replay.
#
# Prerequisites:
#   - A local runtara-server, Postgres, and Valkey (normally `./start.sh`).
#   - psql, redis-cli, curl, and jq.
#   - RUNTARA_SERVER_DATABASE_URL, RUNTARA_DATABASE_URL, and TENANT_ID in the
#     environment or the repository .env file.
#
# The test creates an isolated workflow/image, backs up and removes that image's
# binary, then publishes a trigger event directly. The first delivery must stay
# pending while a forced recompilation restores the artifact. Replaying the same
# instance id must be deduplicated and ACKed without a second instance row.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
DOTENV="${PROJECT_ROOT}/.env"

load_dotenv_value() {
    local name="$1"
    if [ -z "${!name:-}" ] && [ -f "${DOTENV}" ]; then
        local value
        value=$(sed -n "s/^${name}=//p" "${DOTENV}" | tail -1)
        if [ -n "${value}" ]; then
            printf -v "${name}" '%s' "${value}"
            export "${name?}"
        fi
    fi
}

for name in RUNTARA_SERVER_DATABASE_URL RUNTARA_DATABASE_URL TENANT_ID SERVER_PORT \
    RUNTARA_ENV_HTTP_PORT VALKEY_HOST VALKEY_PORT VALKEY_TRIGGER_STREAM_PREFIX \
    VALKEY_TRIGGER_CONSUMER_GROUP; do
    load_dotenv_value "${name}"
done

: "${RUNTARA_SERVER_DATABASE_URL:?set RUNTARA_SERVER_DATABASE_URL or add it to .env}"
: "${RUNTARA_DATABASE_URL:?set RUNTARA_DATABASE_URL or add it to .env}"
: "${TENANT_ID:?set TENANT_ID or add it to .env}"

API_BASE="${API_BASE:-http://127.0.0.1:${SERVER_PORT:-7001}}"
API="${API_BASE}/api/runtime"
ENVIRONMENT_API="${RUNTARA_ENVIRONMENT_API:-http://127.0.0.1:${RUNTARA_ENV_HTTP_PORT:-8004}}"
VALKEY_HOST="${VALKEY_HOST:-127.0.0.1}"
VALKEY_PORT="${VALKEY_PORT:-6379}"
STREAM="${VALKEY_TRIGGER_STREAM_PREFIX:-runtara:triggers}:${TENANT_ID}"
GROUP="${VALKEY_TRIGGER_CONSUMER_GROUP:-runtara-trigger-workers}"

WORKFLOW_ID=""
IMAGE_ID=""
INSTANCE_ID=""
FIRST_ENTRY_ID=""
REPLAY_ENTRY_ID=""
ARTIFACT=""
ARTIFACT_BACKUP=""

db_scalar() {
    psql "$1" -v ON_ERROR_STOP=1 -tA -c "$2" | tr -d '[:space:]'
}

redis() {
    redis-cli --raw -h "${VALKEY_HOST}" -p "${VALKEY_PORT}" "$@"
}

group_entries_read() {
    redis-cli --json -h "${VALKEY_HOST}" -p "${VALKEY_PORT}" \
        XINFO GROUPS "${STREAM}" \
        | jq -r --arg group "${GROUP}" '.[] | select(.name == $group) | ."entries-read" // 0'
}

wait_for_group_read() {
    local baseline="$1"
    for _ in {1..60}; do
        local current
        current=$(group_entries_read)
        if [ "${current}" -gt "${baseline}" ]; then
            return 0
        fi
        sleep 1
    done
    return 1
}

cleanup() {
    set +e

    if [ -n "${ARTIFACT_BACKUP}" ] && [ -f "${ARTIFACT_BACKUP}" ] \
        && [ -n "${ARTIFACT}" ] && [ ! -f "${ARTIFACT}" ]; then
        mkdir -p "$(dirname "${ARTIFACT}")"
        cp "${ARTIFACT_BACKUP}" "${ARTIFACT}"
        chmod +x "${ARTIFACT}"
    fi

    if [ -n "${INSTANCE_ID}" ]; then
        psql "${RUNTARA_DATABASE_URL}" -v ON_ERROR_STOP=0 >/dev/null <<SQL
DELETE FROM container_registry WHERE instance_id = '${INSTANCE_ID}';
DELETE FROM container_status WHERE instance_id = '${INSTANCE_ID}';
DELETE FROM container_cancellations WHERE instance_id = '${INSTANCE_ID}';
DELETE FROM container_heartbeats WHERE instance_id = '${INSTANCE_ID}';
DELETE FROM instances WHERE instance_id = '${INSTANCE_ID}';
SQL
    fi

    if [ -n "${IMAGE_ID}" ]; then
        curl -sS -X DELETE \
            "${ENVIRONMENT_API}/api/v1/images/${IMAGE_ID}?tenant_id=${TENANT_ID}" >/dev/null
    fi
    if [ -n "${WORKFLOW_ID}" ]; then
        curl -sS -X POST "${API}/workflows/${WORKFLOW_ID}/delete" \
            -H 'Content-Type: application/json' -d '{}' >/dev/null
    fi
    if [ -n "${FIRST_ENTRY_ID}" ]; then
        redis XDEL "${STREAM}" "${FIRST_ENTRY_ID}" >/dev/null
    fi
    if [ -n "${REPLAY_ENTRY_ID}" ]; then
        redis XDEL "${STREAM}" "${REPLAY_ENTRY_ID}" >/dev/null
    fi
    if [ -n "${ARTIFACT_BACKUP}" ]; then
        rm -f "${ARTIFACT_BACKUP}"
    fi
}
trap cleanup EXIT

for command in curl jq psql redis-cli git uuidgen; do
    command -v "${command}" >/dev/null || { echo "missing required command: ${command}"; exit 1; }
done

echo "1. Verifying local build metadata"
HEALTH=$(curl -fsS "${API_BASE}/health")
EXPECTED_COMMIT=$(git -C "${PROJECT_ROOT}" rev-parse --short=12 HEAD)
ACTUAL_COMMIT=$(jq -r '.commit' <<<"${HEALTH}")
[ "${ACTUAL_COMMIT}" = "${EXPECTED_COMMIT}" ] || {
    echo "health commit ${ACTUAL_COMMIT} does not match HEAD ${EXPECTED_COMMIT}"
    exit 1
}

echo "2. Creating and compiling an isolated passthrough workflow"
CREATE_RESPONSE=$(curl -fsS -X POST "${API}/workflows/create" \
    -H 'Content-Type: application/json' \
    -d '{"name":"trigger-replay-idempotency-e2e","description":"temporary stale-artifact and replay regression"}')
WORKFLOW_ID=$(jq -r '.data.id // empty' <<<"${CREATE_RESPONSE}")
[ -n "${WORKFLOW_ID}" ] || { echo "workflow creation failed: ${CREATE_RESPONSE}"; exit 1; }

UPDATE_BODY=$(jq -n --slurpfile graph "${PROJECT_ROOT}/e2e/workflows/simple_passthrough.json" \
    '{executionGraph: $graph[0]}')
UPDATE_RESPONSE=$(curl -fsS -X POST "${API}/workflows/${WORKFLOW_ID}/update" \
    -H 'Content-Type: application/json' -d "${UPDATE_BODY}")
[ "$(jq -r '.success // false' <<<"${UPDATE_RESPONSE}")" = "true" ] || {
    echo "workflow update failed: ${UPDATE_RESPONSE}"
    exit 1
}

VERSION=$(curl -fsS "${API}/workflows/${WORKFLOW_ID}/versions" \
    | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
COMPILE_RESPONSE=$(curl -fsS --max-time 900 -X POST \
    "${API}/workflows/${WORKFLOW_ID}/versions/${VERSION}/compile" \
    -H 'Content-Type: application/json' -d '{}')
[ "$(jq -r '.success // false' <<<"${COMPILE_RESPONSE}")" = "true" ] || {
    echo "workflow compilation failed: ${COMPILE_RESPONSE}"
    exit 1
}

IMAGE_ID=$(db_scalar "${RUNTARA_SERVER_DATABASE_URL}" \
    "SELECT registered_image_id FROM workflow_compilations WHERE tenant_id = '${TENANT_ID}' AND workflow_id = '${WORKFLOW_ID}' AND version = ${VERSION}")
[ -n "${IMAGE_ID}" ] || { echo "compiled image id was not recorded"; exit 1; }
ARTIFACT=$(db_scalar "${RUNTARA_DATABASE_URL}" \
    "SELECT binary_path FROM images WHERE tenant_id = '${TENANT_ID}' AND image_id = '${IMAGE_ID}'")
[ -f "${ARTIFACT}" ] || { echo "compiled artifact is missing before the test: ${ARTIFACT}"; exit 1; }

ARTIFACT_BACKUP=$(mktemp -t runtara-trigger-replay-artifact.XXXXXX)
cp "${ARTIFACT}" "${ARTIFACT_BACKUP}"
rm "${ARTIFACT}"

echo "3. Publishing one event against the stale image and waiting for forced repair"
INSTANCE_ID=$(uuidgen | tr '[:upper:]' '[:lower:]')
REQUESTED_AT="$(date +%s)000"
EVENT=$(jq -nc \
    --arg instance_id "${INSTANCE_ID}" \
    --arg tenant_id "${TENANT_ID}" \
    --arg workflow_id "${WORKFLOW_ID}" \
    --argjson version "${VERSION}" \
    --argjson requested_at "${REQUESTED_AT}" \
    '{instance_id:$instance_id,tenant_id:$tenant_id,workflow_id:$workflow_id,version:$version,
      inputs:{data:{input:{e2e:true}},variables:{}},
      trigger:{type:"http_api",correlation_id:null},requested_at:$requested_at,
      track_events:false,debug:false}')

READS_BEFORE=$(group_entries_read)
FIRST_ENTRY_ID=$(redis XADD "${STREAM}" '*' \
    event_type trigger trigger_type http_api instance_id "${INSTANCE_ID}" \
    workflow_id "${WORKFLOW_ID}" data "${EVENT}")

for _ in {1..120}; do
    INSTANCE_COUNT=$(db_scalar "${RUNTARA_DATABASE_URL}" \
        "SELECT count(*) FROM instances WHERE instance_id = '${INSTANCE_ID}'")
    if [ "${INSTANCE_COUNT}" = "1" ] && [ -f "${ARTIFACT}" ]; then
        break
    fi
    sleep 1
done
[ "${INSTANCE_COUNT:-0}" = "1" ] || { echo "repaired execution never registered"; exit 1; }
[ -f "${ARTIFACT}" ] || { echo "forced recompilation did not restore ${ARTIFACT}"; exit 1; }
wait_for_group_read "${READS_BEFORE}" || { echo "trigger group did not consume first event"; exit 1; }
[ -z "$(redis XPENDING "${STREAM}" "${GROUP}" "${FIRST_ENTRY_ID}" "${FIRST_ENTRY_ID}" 1)" ] || {
    echo "first event was not acknowledged after repair"
    exit 1
}
[ -n "$(redis XRANGE "${STREAM}" "${FIRST_ENTRY_ID}" "${FIRST_ENTRY_ID}" COUNT 1)" ] || {
    echo "acknowledged stream entry unexpectedly disappeared"
    exit 1
}

echo "4. Verifying Environment and trigger-worker replay deduplication"
REPLAY_BODY=$(jq -nc \
    --arg image_id "${IMAGE_ID}" --arg tenant_id "${TENANT_ID}" --arg instance_id "${INSTANCE_ID}" \
    '{image_id:$image_id,tenant_id:$tenant_id,instance_id:$instance_id,input:{e2e:true},env:{}}')
REPLAY_RESPONSE_FILE=$(mktemp -t runtara-trigger-replay-response.XXXXXX)
REPLAY_STATUS=$(curl -sS -o "${REPLAY_RESPONSE_FILE}" -w '%{http_code}' -X POST \
    "${ENVIRONMENT_API}/api/v1/instances" -H 'Content-Type: application/json' -d "${REPLAY_BODY}")
REPLAY_RESPONSE=$(cat "${REPLAY_RESPONSE_FILE}")
rm -f "${REPLAY_RESPONSE_FILE}"
[ "${REPLAY_STATUS}" = "200" ] && [ "$(jq -r '.deduplicated // false' <<<"${REPLAY_RESPONSE}")" = "true" ] || {
    echo "Environment did not return a deduplicated 200 response: ${REPLAY_STATUS} ${REPLAY_RESPONSE}"
    exit 1
}

READS_BEFORE=$(group_entries_read)
REPLAY_ENTRY_ID=$(redis XADD "${STREAM}" '*' \
    event_type trigger trigger_type http_api instance_id "${INSTANCE_ID}" \
    workflow_id "${WORKFLOW_ID}" data "${EVENT}")
wait_for_group_read "${READS_BEFORE}" || { echo "trigger group did not consume replay"; exit 1; }
[ -z "$(redis XPENDING "${STREAM}" "${GROUP}" "${REPLAY_ENTRY_ID}" "${REPLAY_ENTRY_ID}" 1)" ] || {
    echo "deduplicated replay was not acknowledged"
    exit 1
}

INSTANCE_COUNT=$(db_scalar "${RUNTARA_DATABASE_URL}" \
    "SELECT count(*) FROM instances WHERE instance_id = '${INSTANCE_ID}'")
[ "${INSTANCE_COUNT}" = "1" ] || {
    echo "expected one instance row after replay, found ${INSTANCE_COUNT}"
    exit 1
}

# Let the real process and its monitor finish before the cleanup trap removes
# runtime rows and image files.
for _ in {1..60}; do
    INSTANCE_STATUS=$(db_scalar "${RUNTARA_DATABASE_URL}" \
        "SELECT status::text FROM instances WHERE instance_id = '${INSTANCE_ID}'")
    case "${INSTANCE_STATUS}" in
        completed|failed|cancelled) break ;;
    esac
    sleep 1
done
case "${INSTANCE_STATUS:-}" in
    completed|failed|cancelled) ;;
    *) echo "instance did not reach a terminal state before cleanup"; exit 1 ;;
esac
# The execution-engine outcome watcher polls independently; give it one cycle
# to observe the terminal row before the cleanup trap removes test data.
sleep 3

echo "PASS: health=${ACTUAL_COMMIT}, stale artifact repaired, replay ACKed, instance rows=1"
