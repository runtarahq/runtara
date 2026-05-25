#!/bin/bash
# E2E Test: MCP agent registration + AI Agent integration
#
# Verifies the MCP agent (runtara-agent-mcp) is correctly registered with the
# runtime catalog, that the McpConnection type is discoverable, and that
# workflows with `mcp.<toolset>` edges validate per the Phase-3 rules.
#
# This is an API-level smoke test — it does NOT execute a workflow end-to-end
# against a real MCP server (that requires a stub server + OpenAI connection).
# What it covers:
#   1. /api/runtime/agents/mcp returns the agent with two capabilities.
#   2. /api/runtime/connections/types/mcp returns the new connection type.
#   3. Workflow graphs with a valid mcp.<toolset> edge validate (no E12x errors).
#   4. mcp.<toolset> pointing to a non-mcp Agent step fires E121.
#   5. Two mcp.<same-suffix> edges on one AI Agent fire E123.
#
# Prerequisites:
#   - runtara-server running on :7001 with the WASM agents directory configured
#     (RUNTARA_AGENT_COMPONENTS_DIR pointing at the wasm32-wasip1/release dir,
#     built via scripts/build-agent-components.sh).
#   - TENANT_ID env var set (typically org_p0IkAFnrVqVOvQw9 in local dev).

set -e

TENANT_ID="${TENANT_ID:-org_p0IkAFnrVqVOvQw9}"
API_BASE="${API_BASE:-http://127.0.0.1:7001}"

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

ok() { echo -e "${GREEN}[OK]${NC} $1"; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; exit 1; }

# ── 0. Server reachable ───────────────────────────────────────────────────────
echo "0. GET ${API_BASE}/health"
curl -fsS "${API_BASE}/health" > /dev/null || fail "server not reachable on ${API_BASE}"
ok "server reachable"

# ── 1. Agent registration ─────────────────────────────────────────────────────
echo "1. GET /api/runtime/agents/mcp"
AGENT_JSON=$(curl -fsS -H "X-Org-Id: ${TENANT_ID}" "${API_BASE}/api/runtime/agents/mcp")
echo "${AGENT_JSON}" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert d['id'] == 'mcp', f\"id mismatch: {d['id']}\"
cap_ids = sorted(c['id'] for c in d['capabilities'])
assert cap_ids == ['mcp-tool-invoke', 'mcp-tool-search'], f\"caps mismatch: {cap_ids}\"
"
ok "mcp agent registered with mcp-tool-search + mcp-tool-invoke"

# ── 2. Connection type ────────────────────────────────────────────────────────
echo "2. GET /api/runtime/connections/types/mcp"
CONN_JSON=$(curl -fsS -H "X-Org-Id: ${TENANT_ID}" "${API_BASE}/api/runtime/connections/types/mcp")
echo "${CONN_JSON}" | python3 -c "
import sys, json
d = json.load(sys.stdin)
ct = d['connectionType']
assert ct['integrationId'] == 'mcp'
field_names = {f['name'] for f in ct['fields']}
expected = {'url', 'auth_mode', 'bearer_token', 'api_key_header', 'api_key_value',
            'extra_headers', 'tool_hints', 'tool_scope'}
missing = expected - field_names
assert not missing, f\"missing fields: {missing}\"
"
ok "McpConnection type registered with url + auth_mode + ..."

# ── 3. Valid mcp.<toolset> edge ───────────────────────────────────────────────
echo "3. validate-graph: valid mcp.linear edge"
RESP=$(curl -fsS -X POST -H "Content-Type: application/json" -H "X-Org-Id: ${TENANT_ID}" \
  -d '{"name":"Valid MCP","steps":{"ai_agent":{"stepType":"AiAgent","id":"ai_agent","connectionId":"conn-openai","config":{"systemPrompt":{"valueType":"immediate","value":"x"},"userPrompt":{"valueType":"immediate","value":"y"},"provider":"openai","model":"gpt-4o-mini","maxIterations":3,"temperature":0.7}},"mcp_linear":{"stepType":"Agent","id":"mcp_linear","agentId":"mcp","capabilityId":"mcp-tool-search","connectionId":"conn-mcp","inputMapping":{"query":{"valueType":"immediate","value":""}}},"finish":{"stepType":"Finish","id":"finish","inputMapping":{"result":{"valueType":"reference","value":"steps.ai_agent.outputs"}}}},"entryPoint":"ai_agent","executionPlan":[{"fromStep":"ai_agent","toStep":"mcp_linear","label":"mcp.linear"},{"fromStep":"ai_agent","toStep":"finish","label":"next"}],"variables":{},"inputSchema":{},"outputSchema":{}}' \
  "${API_BASE}/api/runtime/workflows/graph/validate")
echo "${RESP}" | python3 -c "
import sys, json
d = json.load(sys.stdin)
mcp_errors = [e for e in d['errors'] if 'E12' in e]
assert not mcp_errors, f\"unexpected MCP errors: {mcp_errors}\"
"
ok "Valid mcp.linear edge passes validation"

# ── 4. Wrong agent_id (E121) ──────────────────────────────────────────────────
echo "4. validate-graph: mcp.linear pointing to transform Agent (E121)"
RESP=$(curl -fsS -X POST -H "Content-Type: application/json" -H "X-Org-Id: ${TENANT_ID}" \
  -d '{"name":"Bad target","steps":{"ai_agent":{"stepType":"AiAgent","id":"ai_agent","connectionId":"conn-openai","config":{"systemPrompt":{"valueType":"immediate","value":"x"},"userPrompt":{"valueType":"immediate","value":"y"},"provider":"openai","model":"gpt-4o-mini","maxIterations":3,"temperature":0.7}},"wrong":{"stepType":"Agent","id":"wrong","agentId":"transform","capabilityId":"extract","inputMapping":{"value":{"valueType":"immediate","value":""},"property_path":{"valueType":"immediate","value":""}}},"finish":{"stepType":"Finish","id":"finish","inputMapping":{"result":{"valueType":"reference","value":"steps.ai_agent.outputs"}}}},"entryPoint":"ai_agent","executionPlan":[{"fromStep":"ai_agent","toStep":"wrong","label":"mcp.linear"},{"fromStep":"ai_agent","toStep":"finish","label":"next"}],"variables":{},"inputSchema":{},"outputSchema":{}}' \
  "${API_BASE}/api/runtime/workflows/graph/validate")
echo "${RESP}" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert any('E121' in e for e in d['errors']), f\"E121 not in {d['errors']}\"
"
ok "Wrong agent_id fires E121"

# ── 5. Duplicate suffix (E123) ────────────────────────────────────────────────
echo "5. validate-graph: two mcp.linear edges (E123)"
RESP=$(curl -fsS -X POST -H "Content-Type: application/json" -H "X-Org-Id: ${TENANT_ID}" \
  -d '{"name":"Dup","steps":{"ai_agent":{"stepType":"AiAgent","id":"ai_agent","connectionId":"conn-openai","config":{"systemPrompt":{"valueType":"immediate","value":"x"},"userPrompt":{"valueType":"immediate","value":"y"},"provider":"openai","model":"gpt-4o-mini","maxIterations":3,"temperature":0.7}},"a":{"stepType":"Agent","id":"a","agentId":"mcp","capabilityId":"mcp-tool-search","connectionId":"c1","inputMapping":{"query":{"valueType":"immediate","value":""}}},"b":{"stepType":"Agent","id":"b","agentId":"mcp","capabilityId":"mcp-tool-search","connectionId":"c2","inputMapping":{"query":{"valueType":"immediate","value":""}}},"finish":{"stepType":"Finish","id":"finish","inputMapping":{"result":{"valueType":"reference","value":"steps.ai_agent.outputs"}}}},"entryPoint":"ai_agent","executionPlan":[{"fromStep":"ai_agent","toStep":"a","label":"mcp.linear"},{"fromStep":"ai_agent","toStep":"b","label":"mcp.linear"},{"fromStep":"ai_agent","toStep":"finish","label":"next"}],"variables":{},"inputSchema":{},"outputSchema":{}}' \
  "${API_BASE}/api/runtime/workflows/graph/validate")
echo "${RESP}" | python3 -c "
import sys, json
d = json.load(sys.stdin)
assert any('E123' in e for e in d['errors']), f\"E123 not in {d['errors']}\"
"
ok "Duplicate suffix fires E123"

echo
echo "All MCP agent e2e checks passed."
