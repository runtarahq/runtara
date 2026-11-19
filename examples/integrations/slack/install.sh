#!/usr/bin/env bash
# Installs the Slack adapter into a running Runtara: the two workflows in this
# directory and the synchronous trigger Slack's Events API calls.
#
#   RUNTARA_API          the API, e.g. https://runtara.example.com/api/runtime
#   RUNTARA_API_KEY      optional: an API key, sent as `X-API-Key`
#   SLACK_CONNECTION_ID  a Slack Bot connection holding the bot token and the
#                        signing secret
#   TARGET_WORKFLOW_ID   the workflow that answers the conversation
#
# Prints the trigger's URL: give it to Slack as the Request URL of Event
# Subscriptions, and subscribe the app to the message events it should see.
set -euo pipefail

: "${RUNTARA_API:?the API, e.g. https://runtara.example.com/api/runtime}"
: "${SLACK_CONNECTION_ID:?a Slack Bot connection id}"
: "${TARGET_WORKFLOW_ID:?the workflow that answers the conversation}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

call() { # method path [json]
    local auth=()
    [ -n "${RUNTARA_API_KEY:-}" ] && auth=(-H "X-API-Key: ${RUNTARA_API_KEY}")
    curl -sS --max-time 900 -X "$1" -H 'Content-Type: application/json' ${auth[@]+"${auth[@]}"} \
        ${3:+-d "$3"} "${RUNTARA_API}$2"
}

# Creates, stores and compiles a workflow from a graph; prints its id.
install_workflow() {
    local graph="$1" name id version resp
    name=$(echo "${graph}" | jq -r '.name')
    resp=$(call POST /workflows/create "$(echo "${graph}" | jq -c '{name, description}')")
    id=$(echo "${resp}" | jq -r '.data.id // empty' 2>/dev/null)
    [ -n "${id}" ] || { echo "could not create ${name}: ${resp}" >&2; exit 1; }
    resp=$(call POST "/workflows/${id}/update" "$(jq -nc --argjson g "${graph}" '{executionGraph: $g}')")
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { echo "could not store ${name}: ${resp}" >&2; exit 1; }
    version=$(call GET "/workflows/${id}/versions" | jq -r '[.data[]?.version // .data[]?.versionNumber // empty] | max // 1')
    resp=$(call POST "/workflows/${id}/versions/${version}/compile" '{}')
    [ "$(echo "${resp}" | jq -r '.success // false')" = "true" ] || { echo "could not compile ${name}: ${resp}" >&2; exit 1; }
    echo "${id}"
}

OUTBOUND=$(install_workflow "$(jq -c --arg c "${SLACK_CONNECTION_ID}" \
    '(.steps.post.connectionId) = $c' "${HERE}/outbound.json")")
INBOUND=$(install_workflow "$(jq -c --arg t "${TARGET_WORKFLOW_ID}" --arg o "${OUTBOUND}" \
    '(.steps.send.inputMapping.workflowId.value) = $t | (.steps.send.inputMapping.replyWorkflowId.value) = $o' \
    "${HERE}/inbound.json")")
resp=$(call POST /triggers "$(jq -nc --arg w "${INBOUND}" --arg c "${SLACK_CONNECTION_ID}" \
    '{workflow_id: $w, trigger_type: "HTTP", active: true, configuration: {connection_id: $c, synchronous: true}}')")
TRIGGER=$(echo "${resp}" | jq -r '.data.id // .id // empty' 2>/dev/null)
[ -n "${TRIGGER}" ] || { echo "could not create the trigger: ${resp}" >&2; exit 1; }

jq -n --arg inbound "${INBOUND}" --arg outbound "${OUTBOUND}" --arg trigger "${TRIGGER}" \
    --arg url "${RUNTARA_API}/events/http/${TRIGGER}/slack" \
    '{inboundWorkflowId: $inbound, outboundWorkflowId: $outbound, triggerId: $trigger, requestUrl: $url}'
