#!/bin/bash
# E2E Test: the Slack adapter (docs/native-chat-phase-5.md,
# examples/integrations/slack).
#
# The adapter is installed with its own install script into a running server,
# against a stand-in Slack on this machine (RUNTARA_STAND_IN_API_BASES), with
# a Slack Bot connection whose bot token and signing secret are sealed. The
# test signs Events API calls as Slack does, and checks:
#
#   1. Slack's URL handshake is answered with its challenge, by the workflow,
#      through a synchronous trigger.
#   2. A call that is unsigned, signed with another secret, signed over
#      another body, or signed too long ago is refused (401) and starts
#      nothing.
#   3. A person's message reaches the conversation of its channel once, even
#      when Slack delivers it again; its reply is posted to that channel once,
#      with the bot token seal attached, and to the thread it came from.
#   4. The bot's own messages are not sent back in.
#
# Usage:  POSTGRES_PORT=55432 POSTGRES_USER=postgres ./e2e/test_slack_adapter.sh
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
# runtara-server needs a credential encryption key; a throwaway one per run.
# shellcheck source=../scripts/credential-key.sh
source "${PROJECT_ROOT}/scripts/credential-key.sh"
ensure_credential_key

POSTGRES_HOST="${POSTGRES_HOST:-localhost}"
POSTGRES_PORT="${POSTGRES_PORT:-5432}"
POSTGRES_USER="${POSTGRES_USER:-postgres}"
POSTGRES_PASSWORD="${POSTGRES_PASSWORD-}"

TEST_DB_SERVER="${TEST_DB_SERVER:-slack_adapter_server_$$}"
TEST_DB_RUNTIME="${TEST_DB_RUNTIME:-slack_adapter_runtime_$$}"
TEST_PORT_PUBLIC="${TEST_PORT_PUBLIC:-17960}"
TEST_CORE_PORT="${TEST_CORE_PORT:-18961}"
TEST_ENV_PORT="${TEST_ENV_PORT:-18962}"
TEST_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT:-18963}"
TEST_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT:-18964}"
TEST_VALKEY_PORT="${TEST_VALKEY_PORT:-16401}"
TEST_DATA_DIR="$(mktemp -d -t runtara_slack_adapter_XXXXXX)"
TEST_LOG="${TEST_DATA_DIR}/server.log"
SERVER_PID=""
SLACK_PID=""
SLACK_PORT="${SLACK_PORT:-17969}"
SLACK_LOG="${TEST_DATA_DIR}/slack.jsonl"
VALKEY_CONTAINER=""
TENANT="${TENANT_ID_OVERRIDE:-slack_adapter_$$}"
# A throwaway signing secret for the stand-in workspace.
SIGNING_SECRET="stand-in-signing-secret-$$"

RUNTARA_SERVER_BIN="${RUNTARA_SERVER_BIN:-${PROJECT_ROOT}/target/debug/runtara-server}"
COMPONENTS_DIR="${RUNTARA_AGENT_COMPONENTS_DIR:-${PROJECT_ROOT}/target/wasm32-wasip2/release}"

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

# A connection is saved with its secrets sealed: the server takes none
# readable. $1 is the create request, written as it was before that.
connection_post() {
    python3 "${SCRIPT_DIR}/sealed_connection.py" --api "${API}" "$1"
}
# Send a request and echo "<http status> <body>".
api_status() {
    local method="$1" path="$2" data="${3-}" body status
    if [ -n "${data}" ]; then
        body=$(curl -sS --max-time 60 -w '\n%{http_code}' -X "${method}" -H "Content-Type: application/json" -d "${data}" "${API}${path}")
    else
        body=$(curl -sS --max-time 60 -w '\n%{http_code}' -X "${method}" "${API}${path}")
    fi
    status="${body##*$'\n'}"
    echo "${status} ${body%$'\n'*}"
}

cleanup() {
    local code=$?
    [ -n "${SLACK_PID}" ] && kill "${SLACK_PID}" 2>/dev/null || true
    if [ -n "${SERVER_PID}" ]; then
        kill "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
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
        RUNTARA_DATABASE_URL="${RUNTIME_DB_URL}" \
        TENANT_ID="${TENANT}" \
        SERVER_HOST=127.0.0.1 \
        SERVER_PORT="${TEST_PORT_PUBLIC}" \
        RUNTARA_CORE_PORT="${TEST_CORE_PORT}" \
        RUNTARA_ENVIRONMENT_PORT="${TEST_ENV_PORT}" \
        RUNTARA_CORE_HTTP_PORT="${TEST_CORE_HTTP_PORT}" \
        RUNTARA_ENV_HTTP_PORT="${TEST_ENV_HTTP_PORT}" \
        RUNTARA_AGENT_COMPONENTS_DIR="${COMPONENTS_DIR}" \
        RUNTARA_PROXY_ALLOWED_HOSTS="127.0.0.1:${SLACK_PORT}" \
        RUNTARA_STAND_IN_API_BASES="slack_bot=http://127.0.0.1:${SLACK_PORT}" \
        RUNTARA_PROXY_ALLOW_HTTP_HOSTS=127.0.0.1 \
        RUNTARA_SEAL_PRIVATE_CIDRS="127.0.0.0/8" \
        RUNTARA_MCP_SESSION_STORE=local \
        DATA_DIR="${TEST_DATA_DIR}" \
        RUNTARA_DEV_MODE=false \
        RUST_LOG="warn,runtara_server=info" \
        AUTH_PROVIDER=local \
        VALKEY_HOST=127.0.0.1 \
        VALKEY_PORT="${TEST_VALKEY_PORT}" \
        OTEL_SDK_DISABLED=true \
        SQLX_OFFLINE=true \
        exec "${RUNTARA_SERVER_BIN}"
    ) >>"${TEST_LOG}" 2>&1 &
    SERVER_PID=$!
    for _ in {1..90}; do
        if curl -sS -o /dev/null -w "%{http_code}" "http://127.0.0.1:${TEST_PORT_PUBLIC}/health" 2>/dev/null | grep -q "^2"; then
            return 0
        fi
        sleep 1
        kill -0 "${SERVER_PID}" 2>/dev/null || { print_error "Server exited during boot."; exit 1; }
    done
    print_error "Server did not become healthy."; exit 1
}

# Create, save and compile a workflow from a graph; echo its id.
workflow() {
    local name="$1" graph="$2" resp wf version
    resp=$(api_post /workflows/create "{\"name\": \"${name}\", \"description\": \"slack adapter e2e\"}")
    wf=$(echo "${resp}" | jq -r '.data.id // empty')
    [ -n "${wf}" ] || { print_error "Workflow create failed: ${resp}"; exit 1; }
    resp=$(api_post "/workflows/${wf}/update" "{\"executionGraph\": ${graph}}")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Update of ${name} failed: ${resp}"; exit 1; }
    version=$(curl -sS "${API}/workflows/${wf}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(api_post "/workflows/${wf}/versions/${version}/compile" '{}' 900)
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { print_error "Compile of ${name} failed: ${resp}"; exit 1; }
    echo "${wf}"
}

# The conversation $2 of workflow $1.
conversation() { echo "/workflows/$1/conversations/$2"; }
events() { curl -sS --max-time 30 "${API}$(conversation "$1" "$2")/events?limit=500" | jq -c '.data // []'; }

# Calls the adapter's Request URL as Slack does: body $1, signed at time $2
# (now if empty) with secret $3 (the workspace's if empty). Echoes
# "<status> <body>".
slack_call() {
    local body="$1" at="${2:-$(date +%s)}" secret="${3:-${SIGNING_SECRET}}" signature out
    signature="v0=$(printf 'v0:%s:%s' "${at}" "${body}" | openssl dgst -sha256 -hmac "${secret}" -hex | sed 's/^.* //')"
    out=$(curl -sS --max-time 60 -w '\n%{http_code}' -X POST -H 'Content-Type: application/json' \
        -H "X-Slack-Request-Timestamp: ${at}" -H "X-Slack-Signature: ${signature}" \
        -d "${body}" "${REQUEST_URL}")
    echo "${out##*$'\n'} ${out%$'\n'*}"
}
unsigned_call() {
    local out
    out=$(curl -sS --max-time 60 -w '\n%{http_code}' -X POST -H 'Content-Type: application/json' -d "$1" "${REQUEST_URL}")
    echo "${out##*$'\n'} ${out%$'\n'*}"
}
# A message event from Slack: event id, channel, text, and extra event fields.
message_event() {
    local extra="${4:-}"
    [ -n "${extra}" ] || extra='{}'
    jq -nc --arg id "$1" --arg channel "$2" --arg text "$3" --argjson extra "${extra}" '{
        token: "unused", team_id: "T1", api_app_id: "A1", type: "event_callback",
        event_id: $id, event_time: 1700000000,
        event: ({type: "message", channel: $channel, user: "U1", text: $text, ts: "1700000000.000100"} + $extra) }'
}
posted() { [ -f "${SLACK_LOG}" ] && jq -sc '.' "${SLACK_LOG}" || echo '[]'; }
wait_posted() { # count
    for _ in {1..120}; do
        [ "$(posted | jq 'length')" -ge "$1" ] && return 0
        sleep 1
    done
    print_error "Slack was not posted to $1 times: $(posted)"; exit 1
}

echo "==============================================================="
echo "E2E: the Slack adapter, against a stand-in Slack"
echo "==============================================================="

[ -x "${RUNTARA_SERVER_BIN}" ] || { print_error "Missing server bin ${RUNTARA_SERVER_BIN}"; exit 1; }
[ -f "${COMPONENTS_DIR}/runtara_agent_slack.wasm" ] || { print_error "Missing slack component: run scripts/build-agent-components.sh"; exit 1; }
psql_quiet -d postgres -c "SELECT 1" >/dev/null 2>&1 || { print_error "Cannot reach Postgres at ${POSTGRES_HOST}:${POSTGRES_PORT}"; exit 1; }
docker info >/dev/null 2>&1 || { print_error "docker required (isolated Valkey)"; exit 1; }

print_step "Starting the stand-in Slack on :${SLACK_PORT}..."
python3 "${SCRIPT_DIR}/stand_in_slack.py" "${SLACK_PORT}" "${SLACK_LOG}" &
SLACK_PID=$!
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${SLACK_PORT}) 2>/dev/null && break; sleep 0.25; done

print_step "Starting isolated Valkey on :${TEST_VALKEY_PORT}..."
VALKEY_CONTAINER=$(docker run -d --rm -p "127.0.0.1:${TEST_VALKEY_PORT}:6379" valkey/valkey:8-alpine)
for _ in {1..20}; do (echo > /dev/tcp/127.0.0.1/${TEST_VALKEY_PORT}) 2>/dev/null && break; sleep 0.5; done

print_step "Creating databases ${TEST_DB_SERVER}, ${TEST_DB_RUNTIME}..."
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_SERVER}" >/dev/null
psql_quiet -d postgres -c "CREATE DATABASE ${TEST_DB_RUNTIME}" >/dev/null

print_step "Starting runtara-server on :${TEST_PORT_PUBLIC} (tenant ${TENANT})..."
start_server

print_step "Creating the Slack Bot connection..."
RESP=$(connection_post "$(jq -nc --arg secret "${SIGNING_SECRET}" '{
    title: "stand-in workspace", integrationId: "slack_bot",
    connectionParameters: { bot_token: "xoxb-stand-in", signing_secret: $secret } }')")
CONN=$(echo "${RESP}" | jq -r '.connectionId // .data.id // .id // empty')
[ -n "${CONN}" ] || { print_error "Connection create failed: ${RESP}"; exit 1; }

print_step "Creating the workflow that answers, and installing the adapter..."
TARGET=$(workflow slack-target "$(jq -n '{
    name: "slack-target", entryPoint: "finish",
    steps: { finish: { id: "finish", stepType: "Finish", inputMapping: {
        response: { valueType: "template", value: "heard: {{ data.message }}" } } } },
    executionPlan: [], variables: {}, inputSchema: { message: { type: "string" } }, outputSchema: {} }')")
INSTALLED=$(RUNTARA_API="${API}" SLACK_CONNECTION_ID="${CONN}" TARGET_WORKFLOW_ID="${TARGET}" \
    "${PROJECT_ROOT}/examples/integrations/slack/install.sh")
REQUEST_URL=$(echo "${INSTALLED}" | jq -r '.requestUrl')
INBOUND=$(echo "${INSTALLED}" | jq -r '.inboundWorkflowId')
OUTBOUND=$(echo "${INSTALLED}" | jq -r '.outboundWorkflowId')
[ -n "${REQUEST_URL}" ] && [ "${REQUEST_URL}" != "null" ] || { print_error "The adapter was not installed: ${INSTALLED}"; exit 1; }
print_success "Installed: ${REQUEST_URL}"

# ---------------------------------------------------------------------------
print_step "1. Slack's URL handshake..."
GOT=$(slack_call '{"token":"unused","challenge":"challenge-1","type":"url_verification"}')
[ "${GOT}" = '200 {"challenge":"challenge-1"}' ] || { print_error "The handshake was not answered with its challenge: ${GOT}"; exit 1; }
print_success "The workflow answered Slack's handshake itself, through a synchronous trigger ✓"

# ---------------------------------------------------------------------------
print_step "2. Calls that are not Slack's..."
BODY=$(message_event Ev-forged C1 "forged")
for got in \
    "$(unsigned_call "${BODY}")" \
    "$(slack_call "${BODY}" "" "another-secret")" \
    "$(slack_call "${BODY}" "$(( $(date +%s) - 3600 ))")"; do
    [ "${got%% *}" = "401" ] || { print_error "A call that is not Slack's was not refused: ${got}"; exit 1; }
done
# Signed over another body.
AT=$(date +%s)
SIG="v0=$(printf 'v0:%s:%s' "${AT}" '{"other":true}' | openssl dgst -sha256 -hmac "${SIGNING_SECRET}" -hex | sed 's/^.* //')"
STATUS=$(curl -sS -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' \
    -H "X-Slack-Request-Timestamp: ${AT}" -H "X-Slack-Signature: ${SIG}" -d "${BODY}" "${REQUEST_URL}")
[ "${STATUS}" = "401" ] || { print_error "A call signed over another body was not refused: ${STATUS}"; exit 1; }
print_success "Unsigned, wrongly signed, re-bodied and stale calls are refused (401) ✓"

# ---------------------------------------------------------------------------
print_step "3. A person's message, delivered twice, and one in a thread..."
GOT=$(slack_call "$(message_event Ev1 C1 "hello")")
[ "${GOT%% *}" = "200" ] || { print_error "A signed message was not accepted: ${GOT}"; exit 1; }
# Slack delivers again when it is not sure it was received.
GOT=$(slack_call "$(message_event Ev1 C1 "hello")")
[ "${GOT%% *}" = "200" ] || { print_error "A repeated message was not accepted: ${GOT}"; exit 1; }
wait_posted 1
GOT=$(slack_call "$(message_event Ev2 C1 "in a thread" '{"thread_ts": "1700000000.000001"}')")
[ "${GOT%% *}" = "200" ] || { print_error "A threaded message was not accepted: ${GOT}"; exit 1; }
wait_posted 2
sleep 3
POSTED=$(posted)
[ "$(echo "${POSTED}" | jq -c '[.[] | [.body.channel, .body.text, .body.thread_ts, .bearer]]')" \
    = '[["C1","heard: hello",null,true],["C1","heard: in a thread","1700000000.000001",true]]' ] \
    || { print_error "The replies were not each posted once, to the channel and thread, with the bot token: ${POSTED}"; exit 1; }
MESSAGES=$(events "${TARGET}" "slack:T1:C1" | jq -c '[.[] | select(.kind == "message") | [.body.id, .body.text]]')
[ "${MESSAGES}" = '[["Ev1","hello"],["Ev2","in a thread"]]' ] \
    || { print_error "The conversation does not hold each message once: ${MESSAGES}"; exit 1; }
print_success "Each message reached the channel's conversation once; each reply was posted once, to its thread, by seal ✓"

# ---------------------------------------------------------------------------
print_step "4. The bot's own message..."
GOT=$(slack_call "$(message_event Ev3 C1 "heard: hello" '{"bot_id": "B1"}')")
[ "${GOT%% *}" = "200" ] || { print_error "The bot's own message was not answered: ${GOT}"; exit 1; }
sleep 3
COUNT=$(events "${TARGET}" "slack:T1:C1" | jq '[.[] | select(.kind == "message")] | length')
[ "${COUNT}" = "2" ] || { print_error "The bot's own message was sent into the conversation"; exit 1; }
[ "$(posted | jq 'length')" = "2" ] || { print_error "The bot answered itself: $(posted)"; exit 1; }
print_success "The bot's own messages are not sent back in ✓"

echo ""
echo "==============================================================="
echo -e "${GREEN}The Slack adapter: all checks passed${NC} (inbound ${INBOUND}, outbound ${OUTBOUND})"
echo "==============================================================="
