---
name: iterate-capability
description: Use for the tight inner loop when developing or debugging a single capability — rebuild only the changed crate, recompile a small test workflow that exercises it, register, run, dump output. ~10s feedback vs full e2e-verify. Use e2e-verify for the final pre-commit check; use this while iterating.
---

# Iterate on a capability quickly

`e2e-verify` is the safety net — slow but thorough. This skill is the inner loop: small, fast, focused on one capability, run repeatedly while you tweak.

Assumes the server is already running (from `e2e-verify` step 4) and the WASM stdlib is built (step 1). If either isn't true, run `e2e-verify` first or `reset-local-env`.

## The loop

### 1. Edit the capability

Make your change in `crates/runtara-agents/src/agents/<agent>.rs`.

### 2. Rebuild only what changed

```bash
# rebuild runtara-agents and the compile binary (which links agents)
cargo build -p runtara-agents -p runtara-workflows --bin runtara-compile
```

If the change was to the **DSL** (steps, schemas), also include `runtara-dsl`:

```bash
cargo build -p runtara-dsl -p runtara-agents -p runtara-workflows --bin runtara-compile
```

If the change was to the **server** API surface, you'll need to restart the server (and after that, `regen-frontend-api`). For pure capability changes, the server doesn't need to restart — the capability lives in the compiled workflow binary.

### 3. Use a tiny dedicated test workflow

Don't recompile your full app workflow. Keep a minimal JSON workflow next to your dev notes that calls the capability with known inputs:

```bash
mkdir -p /tmp/runtara-iter
cat > /tmp/runtara-iter/probe.json <<'EOF'
{
  "name": "probe",
  "version": 1,
  "input_schema": { "type": "object" },
  "output_schema": { "type": "object" },
  "steps": [
    {
      "id": "do_it",
      "type": "Agent",
      "config": {
        "agent": "<your_agent>",
        "capability": "<your_capability>",
        "input": { "field": "from data" }
      },
      "next": "finish"
    },
    { "id": "finish", "type": "Finish", "config": { "output": { "result": "from do_it" } } }
  ]
}
EOF
```

(Adjust the input/output shape to your capability's actual `CapabilityInput`/`CapabilityOutput`. See [e2e/workflows/](../../../e2e/workflows/) for real examples.)

### 4. Compile + register + run + dump

One-shot script:

```bash
RUNTARA_LTO=off target/debug/runtara-compile \
  --workflow /tmp/runtara-iter/probe.json \
  --tenant iter --workflow probe \
  --output /tmp/runtara-iter/probe.bin

IMAGE_ID=$(curl -s -X POST "http://127.0.0.1:8004/api/v1/images/upload" \
  -F "binary=@/tmp/runtara-iter/probe.bin" \
  -F "tenant_id=iter" -F "name=probe" -F "description=iter" -F "runner_type=wasm" \
  | python3 -c "import sys,json; print(json.load(sys.stdin)['image_id'])")

INSTANCE_ID=$(RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:8004" RUNTARA_SKIP_CERT_VERIFICATION=true \
  target/debug/runtara-ctl start \
    --image "$IMAGE_ID" --tenant iter \
    --input '{"data":{"input":{"field":"hello"}}}')

RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:8004" RUNTARA_SKIP_CERT_VERIFICATION=true \
  target/debug/runtara-ctl wait "$INSTANCE_ID" --poll 200

# Get the output via runtime API (need workflow_id — pull from the start response or list endpoint)
WORKFLOW_ID=$(curl -s "http://127.0.0.1:7001/api/runtime/workflows?search=probe" | jq -r '.items[0].id')
curl -s "http://127.0.0.1:7001/api/runtime/workflows/$WORKFLOW_ID/instances/$INSTANCE_ID" | jq '{status, output, error}'
```

Save this as a shell function or `~/bin/iter` so you don't retype it.

### 5. If it fails, drill in

Use `trace-instance` with the `INSTANCE_ID` and `WORKFLOW_ID` to see per-step inputs/outputs. The capability's input arrives as the step's `input` field — compare against what your `CapabilityInput` struct expects.

For runtime-side issues (the WASM didn't compile, the agent didn't register), use `tail-logs` with the `runtara_environment=debug,runtara_agents=debug` recipe.

## When to escalate to `e2e-verify`

When this loop is green for your change. Before commit:

1. Run the tiny workflow once more from scratch
2. Run `e2e-verify` end-to-end with a workflow you didn't write specifically for this capability — catches "I made the test workflow match the bug"

## Why not just `cargo test`?

The `#[capability]` macro registers via `inventory` at link time. Unit tests don't exercise the WASM target, the registration path, the compile pipeline, or the runtime — all of which can break independently. Per the `always-e2e-verify` rule, prove it ran.
