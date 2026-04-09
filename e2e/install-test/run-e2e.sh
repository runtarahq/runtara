#!/usr/bin/env bash
# End-to-end install test
#
# Spins up postgres + valkey + runtara-server (from the latest release bundle),
# creates a one-step "random-double" scenario, executes it, and verifies the
# result is a valid number.
#
# Usage:
#   cd e2e/install-test && ./run-e2e.sh
#
# Prerequisites: docker compose

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; BOLD='\033[1m'; NC='\033[0m'
info()  { printf "${GREEN}[+]${NC} %s\n" "$*"; }
warn()  { printf "${YELLOW}[!]${NC} %s\n" "$*"; }
fail()  { printf "${RED}[FAIL]${NC} %s\n" "$*"; docker compose logs runtara 2>/dev/null | tail -20; cleanup; exit 1; }
pass()  { printf "${GREEN}${BOLD}[PASS]${NC} %s\n" "$*"; }

API="http://localhost:7001"
TENANT="test-tenant"
API_KEY="rt_e2e-test-key-12345"
API_KEY_HASH=""

cleanup() {
    info "Tearing down..."
    docker compose down -v --remove-orphans 2>/dev/null || true
}
trap cleanup EXIT

# ─── Start stack ─────────────────────────────────────────────────────────────

printf '\n%s  Runtara E2E Install Test%s\n\n' "${BOLD}" "$NC"

info "Building and starting stack..."
docker compose up -d --build 2>&1 | tail -5

# ─── Wait for healthy ────────────────────────────────────────────────────────

info "Waiting for runtara-server to be healthy..."
for i in $(seq 1 30); do
    if curl -sf "$API/health" > /dev/null 2>&1; then
        break
    fi
    if [ "$i" -eq 30 ]; then
        fail "Server did not become healthy within 30s"
    fi
    sleep 1
done
info "Server is healthy"

# ─── Insert API key ─────────────────────────────────────────────────────────

info "Creating API key in database..."
API_KEY_HASH=$(echo -n "$API_KEY" | sha256sum | cut -d' ' -f1)

docker compose exec -T postgres psql -U runtara -d runtara -c "
INSERT INTO public.api_keys (id, org_id, name, key_prefix, key_hash, created_by, created_at, is_revoked)
VALUES (
    gen_random_uuid(),
    '${TENANT}',
    'e2e-test-key',
    'rt_e2e-',
    '${API_KEY_HASH}',
    'e2e-test',
    NOW(),
    false
) ON CONFLICT DO NOTHING;
" > /dev/null 2>&1 || fail "Failed to insert API key"

info "API key created"

# ─── Create scenario ────────────────────────────────────────────────────────

info "Creating scenario..."
CREATE_RESP=$(curl -sf "$API/api/runtime/scenarios/create" \
    -H "Authorization: Bearer $API_KEY" \
    -H "Content-Type: application/json" \
    -d '{
        "name": "E2E Random Double Test",
        "description": "One-step scenario that generates a random double",
        "trackEvents": true
    }') || fail "Failed to create scenario: $(echo "$CREATE_RESP" 2>/dev/null)"

SCENARIO_ID=$(echo "$CREATE_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['id'])" 2>/dev/null) \
    || fail "Failed to parse scenario ID from: $CREATE_RESP"

info "Scenario created: $SCENARIO_ID"

# ─── Update with execution graph ────────────────────────────────────────────

info "Updating scenario with random-double execution graph..."
UPDATE_RESP=$(curl -sf "$API/api/runtime/scenarios/${SCENARIO_ID}/update" \
    -H "Authorization: Bearer $API_KEY" \
    -H "Content-Type: application/json" \
    -d '{
        "executionGraph": {
            "name": "E2E Random Double Test",
            "description": "Generates a random double",
            "steps": {
                "randomStep": {
                    "stepType": "Agent",
                    "id": "randomStep",
                    "agentId": "utils",
                    "capabilityId": "random-double",
                    "inputMapping": {}
                },
                "finish": {
                    "stepType": "Finish",
                    "id": "finish",
                    "inputMapping": {
                        "result": {
                            "valueType": "reference",
                            "value": "randomStep.result"
                        }
                    }
                }
            },
            "entryPoint": "randomStep",
            "executionPlan": [
                {
                    "fromStep": "randomStep",
                    "toStep": "finish"
                }
            ],
            "variables": {},
            "inputSchema": {},
            "outputSchema": {}
        },
        "trackEvents": true
    }') || fail "Failed to update scenario: $(echo "$UPDATE_RESP" 2>/dev/null)"

info "Scenario updated"

# ─── Execute scenario ───────────────────────────────────────────────────────

info "Executing scenario..."
EXEC_RESP=$(curl -sf "$API/api/runtime/scenarios/${SCENARIO_ID}/execute" \
    -H "Authorization: Bearer $API_KEY" \
    -H "Content-Type: application/json" \
    -d '{
        "inputs": { "data": {} },
        "debug": false
    }') || fail "Failed to execute scenario: $(echo "$EXEC_RESP" 2>/dev/null)"

INSTANCE_ID=$(echo "$EXEC_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('data',{}).get('instanceId', d.get('data',{}).get('instance_id','')))" 2>/dev/null) \
    || fail "Failed to parse instance ID from: $EXEC_RESP"

if [ -z "$INSTANCE_ID" ]; then
    fail "No instance ID in execution response: $EXEC_RESP"
fi

info "Execution started: $INSTANCE_ID"

# ─── Poll for result ────────────────────────────────────────────────────────

info "Waiting for execution to complete..."
RESULT=""
for i in $(seq 1 30); do
    STATUS_RESP=$(curl -sf "$API/api/runtime/scenarios/${SCENARIO_ID}/executions/${INSTANCE_ID}" \
        -H "Authorization: Bearer $API_KEY" 2>/dev/null) || true

    if [ -n "$STATUS_RESP" ]; then
        STATUS=$(echo "$STATUS_RESP" | python3 -c "import sys,json; print(json.load(sys.stdin).get('data',{}).get('status',''))" 2>/dev/null) || true

        if [ "$STATUS" = "completed" ] || [ "$STATUS" = "Completed" ]; then
            RESULT="$STATUS_RESP"
            break
        elif [ "$STATUS" = "failed" ] || [ "$STATUS" = "Failed" ]; then
            fail "Scenario execution failed: $STATUS_RESP"
        fi
    fi

    if [ "$i" -eq 30 ]; then
        fail "Execution did not complete within 30s. Last status: $STATUS_RESP"
    fi
    sleep 1
done

info "Execution completed"

# ─── Verify result ──────────────────────────────────────────────────────────

info "Verifying result..."
RANDOM_VALUE=$(echo "$RESULT" | python3 -c "
import sys, json
data = json.load(sys.stdin).get('data', {})
output = data.get('output', data.get('outputs', {}))
# Try various paths the result might be at
if isinstance(output, dict):
    val = output.get('result', output.get('data', {}).get('result', None))
else:
    val = output
print(val if val is not None else '')
" 2>/dev/null) || true

if [ -z "$RANDOM_VALUE" ]; then
    warn "Could not extract result value. Full response:"
    echo "$RESULT" | python3 -m json.tool 2>/dev/null || echo "$RESULT"
    fail "No result value found in execution output"
fi

# Verify it's a valid number
IS_NUMBER=$(python3 -c "
try:
    v = float('$RANDOM_VALUE')
    print('yes' if 0.0 <= v <= 1.0 else 'range')
except:
    print('no')
" 2>/dev/null)

if [ "$IS_NUMBER" = "yes" ]; then
    pass "Random double returned: $RANDOM_VALUE (valid number in [0, 1])"
elif [ "$IS_NUMBER" = "range" ]; then
    warn "Random double returned: $RANDOM_VALUE (valid number but outside [0, 1])"
    pass "Scenario compiled and executed successfully"
else
    fail "Result is not a valid number: $RANDOM_VALUE"
fi

# ─── Summary ─────────────────────────────────────────────────────────────────

echo ""
pass "E2E install test passed!"
echo "  - Installed runtara-server from GitHub release bundle"
echo "  - Started with PostgreSQL 16 + Valkey 7.2"
echo "  - Created and compiled a one-step scenario"
echo "  - Executed scenario and got result: $RANDOM_VALUE"
echo ""
