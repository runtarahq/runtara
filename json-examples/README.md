# Runtara fan-out bug — repro JSONs

Three workflow JSONs accompanying the bug report on the runtime fan-out regression where a parent step's two `next`-labelled outgoing edges only fire one of them. Drop all three into a fresh Runtara tenant; one should fail and two should pass.

## Files

| File | Size | Steps | Purpose |
|---|---|---|---|
| `failing_case.json` | ~175 KB | 113 in the row-level subgraph + 8 at root | **Reproduces the bug.** Full production CategorizeViaUnspsc graph with every Agent's real `inputMapping` populated. After cache lookup returns empty, the runtime takes the cache-miss branch but only schedules one of `miss_gate`'s two fan-out targets. |
| `topology_control.json` | ~33 KB | 113 | **Passes.** Identical graph shape to the row-level subgraph in `failing_case.json` — same step ids, same edges, same Conditional/Switch placement, same fan-out — but every Agent step replaced with `utils:get-current-iso-datetime` and `inputMapping: {}`. All 45 unique step types emit `step_debug_end` events, including `embed_query` and `pick_first_emb`. Proves the scheduler handles this shape correctly; the bug is in inputMapping content, not graph shape. |
| `minimal_control.json` | ~2 KB | 6 | **Passes.** The smallest standalone repro of the exact fan-out shape from the failing case (`Conditional → Agent → [text-completion, embed-text]`). Both branches fire; returns a `gpt-4o-mini` text completion and a 1536-dim embedding. Proves the small-scale scheduler is fine. |

## Placeholders to replace

Both `failing_case.json` and `minimal_control.json` reference connections. Before deploying, replace:

- `<postgres-connection-id>` — any `postgres` Runtara connection on your tenant (`object-model` agent uses it). `failing_case.json` queries the object-model schemas `CategoryTreeNode`, `TppUnspscMapping2`, `UnspscNode`, `CustomerSkuMapping`, `CustomerVendorAlias`, `Vendor` and writes `UnspscCategorizationResult`. Create these as empty schemas with arbitrary columns; the bug fires before any field-level read matters.
- `<openai-connection-id>` — any OpenAI Runtara connection. `failing_case.json` uses `text-embedding-3-small` and `gpt-4o-mini`; `minimal_control.json` uses the same. Any model id works.

`topology_control.json` has no connection references — it uses only `utils:get-current-iso-datetime`.

## Reproduction

```
# Deploy all three
deploy_workflow(workflow_id=<a>, execution_graph=<contents of failing_case.json>)
deploy_workflow(workflow_id=<b>, execution_graph=<contents of topology_control.json>)
deploy_workflow(workflow_id=<c>, execution_graph=<contents of minimal_control.json>)

# Run them.

# failing_case.json takes the usual CategorizeViaUnspsc inputs.
# A 2-row CSV is enough to repro; the cache-miss path triggers on each row.
execute_workflow_wait(
  workflow_id=<a>,
  inputs={
    "data": {
      "customer_id":         "test-customer",
      "source_file":         "repro.csv",
      "sku_column":          "SKU",
      "description_column":  "Description",
      "quantity_column":     "Qty",
      "price_column":        "Price",
      "input_csv": {
        "filename":   "repro.csv",
        "mime_type":  "text/csv",
        "content":    "<base64 of: SKU,Description,Qty,Price\\nS-001,Box of nails 100ct,10,5.50\\nS-002,Bleach 1 gallon,5,4.25\\n>"
      }
    }
  },
  timeout_seconds=120
)

# topology_control.json takes no inputs.
execute_workflow_wait(workflow_id=<b>, inputs={"data": {}}, timeout_seconds=60)

# minimal_control.json takes no inputs.
execute_workflow_wait(workflow_id=<c>, inputs={"data": {}}, timeout_seconds=30)

# Capture step events from each run.
get_step_events(workflow_id=<a>, instance_id=<a-instance>, subtype="step_debug_end", limit=200)
get_step_events(workflow_id=<b>, instance_id=<b-instance>, subtype="step_debug_end", limit=200)
get_step_events(workflow_id=<c>, instance_id=<c-instance>, subtype="step_debug_end", limit=200)
```

## What you should see

For `failing_case.json`:

- Workflow `status: completed`. No result rows in `UnspscCategorizationResult` for non-error CSV rows. Per-iteration step trace ends with `parse_vec_score` failing with `TRANSFORM_JSON_PARSE_ERROR: expected ident at line 1 column 151`.
- **Critically:** zero `step_debug_*` events with `step_id: "embed_query"` or `step_id: "pick_first_emb"`. They are never scheduled.
- `miss_gate.outputs.outputs = "miss"`, but only `build_combined_prompt` (one of `miss_gate`'s two outgoing `next` edges) emits any events. The `embed_query` edge is silently dropped.
- `inspect_step(step_id="embed_query")` returns `"Step 'embed_query' not found in execution …"`.
- `list_edges(from_step="miss_gate", path=["split_rows"])` confirms two outgoing edges are present in the graph: `miss_gate -> build_combined_prompt` and `miss_gate -> embed_query`.

For `topology_control.json`:

- Workflow `status: completed`, output `{"x": "ok"}`.
- Step events include all 45 unique step ids in the subgraph. In particular: `embed_query`, `pick_first_emb`, `judge`, `final_pick`, `persist`, `switch_pick`, `row_finish` — i.e. both fan-out branches fire and the full miss-path runs through `switch_pick` to `row_finish`.

For `minimal_control.json`:

- Workflow `status: completed`. Output `{"left_output": {"text": "OK", …}, "right_output": {"dimension": 1536, "embeddings": [[…]]}}`. Both branches fire.

## Suggested diff

Comparing `failing_case.json` and `topology_control.json` shows what changes between failing and passing: only the per-step `agentId`, `capabilityId`, `connectionId`, and `inputMapping` content differ. The `entryPoint`, `executionPlan`, and the set of `id` + `stepType` fields per step are identical. That's the bug surface.

`jq` for the diff:

```sh
jq -S '.steps | to_entries | map({k:.key, v:{stepType:.value.stepType, agentId:.value.agentId, capabilityId:.value.capabilityId}})' \
   docs/json-examples/failing_case.json    > /tmp/failing_steps.json
jq -S '.steps | to_entries | map({k:.key, v:{stepType:.value.stepType, agentId:.value.agentId, capabilityId:.value.capabilityId}})' \
   docs/json-examples/topology_control.json > /tmp/topology_steps.json
diff /tmp/failing_steps.json /tmp/topology_steps.json
```

Everything that diffs is a candidate for what makes the scheduler drop `embed_query`. In practice, the single most suspicious diff is that the failing case's `embed_query` has a composite-array `texts` input mapping referencing `steps.extract_desc.outputs` plus an `onError` outgoing edge to a shared error handler (`capture_row_err`); the topology version has none of that. But I could not reduce further than this without losing the bug, so the diff is wider than necessary — the full graph is needed to reproduce.
