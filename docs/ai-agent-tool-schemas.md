# AI Agent tools: tell the model each tool's arguments

Status: implemented (DSL 3.8.0). Two fixes: tool schemas (compiler only)
and fixed arguments (compiler, a stdlib import, validation). No runtime or
console change.

## The problem

An AI Agent step's tools are its labelled edges. Each is offered to the model
with an empty parameter schema:

```json
{"name": "<edge label>", "description": "<target step's name>",
 "parameters": {"type": "object", "properties": {}, "additionalProperties": {}}}
```

The model is told a tool's name and nothing of its arguments, so it guesses
them from the step's name. A tool with no arguments works (`random-double`,
`get-current-iso-datetime`); a tool that needs one (`format-date-from-iso`,
`calculate`, a published workflow that takes `city`) is called with whatever
the model makes up.

## Where it is lost

Only in the compiler. Everything after it already carries a schema:

- **Runtime.** `ai-tools` reads each tool as a `ToolDefinition {name,
  description, parameters}` (`crates/runtara-ai/src/types.rs`); the OpenAI,
  Gemini and Bedrock providers send `parameters` to the model as JSON Schema
  (`crates/runtara-ai/src/providers/{openai,gemini,bedrock}.rs`).
- **Arguments.** What the model sends is passed through, at run time:
  - an Agent tool's arguments are the capability's input
    (`direct_wasm/compile/ai_agent_loop.rs`, `ai-turn-tool-args` →
    `emit_agent_invoke`); the tool step's own `inputMapping` is not used;
  - an EmbedWorkflow tool's arguments are the child workflow's input data
    (`direct_wasm/plan.rs`, `DirectAiToolPlan::Embed`);
  - a WaitForSignal tool ignores them (`emit_ai_wait_tool_arm`).
- **Compiler.** `direct_wasm/manifest.rs`, in the `Step::AiAgent` arm, builds
  the tool list from `ai_agent_tool_edges` and writes the empty schema above
  for every tool.

The compiler has what it needs at that point:

- the agent catalog (`Option<&AgentCatalog>`, passed by the server on every
  real compile, `direct_wasm/compile.rs`), whose `CapabilityInfo.inputs` are
  the capability's fields (`name`, `type`, `description`, `required`, `enum`,
  `format`, `default`, `items`, `schema`). The manifest already reads it for
  `required_agent_inputs`;
- each embedded child's graph (`DirectManifestChildWorkflowInput`), whose
  `input_schema` converts with `runtara_dsl::schema_convert::
  dsl_schema_to_json_schema`, as the AI Agent's `outputSchema` already does.

So the DSL is not the limit. It has no place to *override* a tool's
description or schema (an edge is `fromStep`, `toStep`, `label`), but the
default can be derived without one.

## The change

In `manifest.rs`, give each tool the schema of what its arguments become.

| Tool target | `parameters` | `description` |
|---|---|---|
| Agent step | the capability's inputs, from the catalog | step name, else capability description, else display name, else label |
| Agent step of a workflow-agent | the same (its catalog inputs are the workflow's inputs) | as above |
| EmbedWorkflow step | the child workflow's `inputSchema` | step name, else child workflow name / description, else label |
| WaitForSignal step | unchanged (empty): the arguments are not used | unchanged |
| `mcp.<toolset>` | unchanged (the synthetic `_search` / `_invoke`) | unchanged |

### Capability inputs to JSON Schema

A new `capability_inputs_json_schema(&CapabilityInfo, hidden: &[&str])` next
to `required_agent_inputs`:

- `{"type":"object","properties":{…},"required":[…],"additionalProperties":{}}`.
  `additionalProperties` stays `{}`, not `true` (strict validators reject a
  bare boolean: `ai_agent_tool_defs_emit_no_bare_boolean_subschema`) and not
  `false` (an extra argument is passed through today; refusing it is a
  separate decision).
- Per field: `type` from `type` (`any` → no `type`), `description` (else
  `displayName`), `enum`, `format`, `default`, `items` (from `FieldTypeInfo`,
  recursively). Where a field has a detailed `schema`, use it as is.
- Hidden, never shown to the model:
  - fields whose name starts with `_` (`_connection`, injected by the host);
  - `timeout_ms` when the tool step has a `timeout` (the host injects it,
    `ai_tool_args_with_timeout`). Four capabilities declare it.
- `required` lists the shown required fields only.

### Embedded workflows

`graph_manifest` builds the root graph before the child list, and does not see
the children. Put a map of embedded step id → child input schema (JSON Schema)
into `DirectManifestBuildState` before the root graph is built, keyed as the
child list already is (by the EmbedWorkflow step id), and read it in the
`AiAgent` arm.

### When the schema is not known

No catalog (some test builds), an unknown capability, or a child with no input
schema: keep today's empty schema. No warning was added for it: validation
already refuses an unknown agent or capability, and a real compile always has
the catalog, so the case is left to test builds.

### What stays the same

- Tool order, names and the `tool_index` mapping (the loop dispatches by
  position).
- The runtime, the stdlib and the `ai-tools` component: none change.
- Compiled versions: unchanged until recompiled. A recompile changes only the
  tool definitions in the manifest (canonicalized with `canonical_json`, so
  deterministic).

## Second fix: a tool step's `inputMapping` is ignored

### What happens today

A tool's arguments are the model's and nothing else:

- Agent tool: `ai-turn-tool-args` → `emit_agent_invoke`
  (`direct_wasm/compile/ai_agent_loop.rs`). The step's mapping is compiled
  into the manifest like any Agent step's, and never applied.
- EmbedWorkflow tool: "The LLM tool arguments ARE the child workflow's input
  data (no input mapping …)" (`compile/embed_workflow.rs`,
  `emit_embed_workflow_tool_arm`). Its `input_mapping_id` is used only as a
  deadline owner id.

Validation pushes the other way: `E022 MissingRequiredInput`
(`validation.rs`, the required-input check of Agent steps) and `E055
MissingChildRequiredInputs` do not exempt tool targets. An author who wants
the model to supply `date` must map `date` anyway to save the workflow, and
the value is then dropped. So mappings on tool steps exist today mostly as
placeholders written to pass E022, and an author who means one as a real,
fixed value (a table name, a base URL, a connection) gets nothing, silently.
`tests/fixtures/ai_agent_multi_tool.json` maps `roll`'s `value` to
`"rolled"`, which never reaches the capability.

### The fix: a mapped input is a fixed argument

- **At run time.** When a tool step has a mapping, the tool arm evaluates it
  with `stdlib.apply-mapping(mapping_id, source)`, as an ordinary Agent step
  does, against the run's source at the time of the call (`data`,
  `variables`, `steps.*` so far), and merges it over the model's arguments.
  The mapping wins: the model cannot override a fixed argument.
  - Merge: objects are merged deeply; at a leaf the fixed value wins. A
    mapping key with a path (`data.table`) fixes that leaf only.
  - Order: fixed arguments, then the workflow-agent namespace envelope (an
    Agent tool targeting a workflow-agent), then `timeout_ms` (the step's
    timeout), as today.
  - A mapping that fails (a reference to a step that has not run) fails the
    run, as a step's failing input mapping does. (Feeding it back to the model
    as a tool error would need an error envelope of its own; not done.)
  - The tool call's debug start event shows the model's arguments, as before.
- **In the schema.** A root input mapped as a whole is removed from the
  `properties` and `required` the model sees. One mapped only by path
  (`data.table`) stays, so the model can fill the rest of it.
- **In validation.**
  - E022 and E055: for a tool target, a required input is satisfied by the
    mapping *or* by the model, so it is no longer an error. It is an error
    only when the input could be neither: hidden from the model (`_`-prefixed,
    a workflow-agent's `connection` input) and not mapped.
  - A new warning, `W094 AiAgentToolFixedArguments`, on every tool target
    with a mapping: "`date`, `format` are fixed; the model is not asked for
    them". Authors see at save what the model will not be asked for.
- **Embedded workflows.** The same, with the child's input data in place of
  the capability's input.
- **Wait tools.** Unchanged: their arguments are not used.

What this enables: exposing `http-request` as "GET this API" with only
`query` left to the model; a workflow-agent with a required `connection`
input as a tool (the mapping supplies the connection id); and smaller schemas
for large capabilities.

### Compatibility

A mapping on a tool step that was a placeholder becomes a fixed argument the
model can no longer set. Nothing changes for a compiled version; it changes
when a version is saved and compiled after this ships, and the new warning
names every fixed argument at that save. Release note: "A tool step's mapped
inputs are now fixed arguments; remove a mapping written only to pass
validation (E022 no longer asks for one)."

Decided (2026-11-20): the mapping *is* the fixed argument, with the warning
and the release note above. Validation no longer forces placeholders, so the
ambiguity ends with this change. Not taken: an explicit marker for fixed
arguments (`fixed: true` on a mapping, or a `toolFixedInputs` list), which
would change nothing for existing graphs but adds a DSL field and leaves the
placeholders in place.

### Where

- `crates/runtara-wit/wit/workflow-stdlib/runtara-workflow-stdlib.wit`: a new
  `ai-tool-args-with-fixed: func(ctx: u64, args: list<u8>, fixed: list<u8>)
  -> result<list<u8>, string>` next to `ai-tool-args-with-timeout`.
- `crates/runtara-workflow-stdlib/src/{lib.rs,direct_json.rs}`: the deep
  merge, with unit tests.
- `crates/runtara-workflows/src/direct_wasm/plan.rs`: `DirectAiToolPlan::Agent`
  gains `fixed_mapping_id: Option<u32>` (the step's mapping id, when it has a
  non-empty mapping); `Embed` the same.
- `crates/runtara-workflows/src/direct_wasm/compile/ai_agent_loop.rs` and
  `compile/embed_workflow.rs`: when set, `apply-mapping` then
  `ai-tool-args-with-fixed` before the existing envelope and timeout steps.
  `compile/core_imports.rs`: the new import.
- `crates/runtara-workflows/src/direct_wasm/manifest.rs`: drop mapped root
  inputs from the schema.
- `crates/runtara-workflows/src/validation.rs`: E022 / E055 for tool targets,
  and the new warning.
- `crates/runtara-workflows/tests/fixtures/ai_agent_multi_tool.json` and the
  tests that use it: `roll`'s `value` is now fixed.

## Later: overrides in the DSL

Optional, after the above: `toolDescription` and `toolParameters` on the tool
target (or the edge), for a description better than the step name or a
narrower schema. It touches the DSL schema, the console and the DSL
changelog, so it is its own change.

## Tests

- `manifest.rs` unit tests, with an `AgentCatalog::from_agents` catalog:
  - an Agent tool gets `properties`, `required`, `enum`, `description` from
    its capability; `_connection` is hidden; `timeout_ms` is hidden only when
    the step has a `timeout`; `any` has no `type`;
  - an unknown capability keeps the empty schema;
  - an EmbedWorkflow tool gets its child's input schema;
  - a WaitForSignal tool and the MCP meta-tools are unchanged;
  - extend `ai_agent_tool_defs_emit_no_bare_boolean_subschema` to build with
    a catalog, so the new schemas are held to it too.
- `validation.rs`: the new warnings; E022 / E055 no longer fire for a tool
  target's required input the model can supply, and still fire for one it
  cannot (hidden and unmapped).
- Fixed arguments:
  - stdlib: the deep merge (fixed wins at a leaf; paths; non-object
    arguments);
  - compile/execute tests (`tests/direct_wasm_execute.rs`, which drive the AI
    loop with a scripted model): an Agent tool with a mapped input receives
    the mapped value even when the model sends another; one with a
    reference mapping (`steps.x.outputs.y`) receives the value at call time;
    an EmbedWorkflow tool's child receives the merged data; a failing mapping
    reaches the model as a tool error and the run goes on;
  - manifest: a mapped root input is missing from the schema, a path-mapped
    one is not.
- E2E: `e2e/stand_in_model_provider.py` logs each request's tool names and
  `parameters`; `test_ai_streaming.sh` (or a small new script) gives its AI
  step a tool with an argument and checks the stand-in received its schema.
- By hand, on the local server: `llama-chat-turn` with a tool that needs an
  argument (`utils/format-date-from-iso` or `utils/calculate`); the local
  model passes it.

## Files

- `crates/runtara-workflows/src/direct_wasm/manifest.rs`: the schemas, the
  child input map, tests.
- `crates/runtara-workflows/src/validation.rs`: two warnings.
- `crates/runtara-dsl/src/schema_types.rs`: the `AiAgentStep` doc comment
  says what a tool is shown.
- `e2e/stand_in_model_provider.py`, `e2e/test_ai_streaming.sh`.
- The authoring text served to MCP clients
  (`get_workflow_authoring_schema`, `crates/runtara-server/src/mcp/tools/workflows.rs`)
  where it describes tools.

The fixed-argument files are listed in its own section above.

Size: schemas about a day (most of it the field conversion and its tests);
fixed arguments about two more (a stdlib import, two tool arms, validation,
execute tests). One PR, schemas first and fixed arguments after, in separate
commits: schemas alone are safe to ship, and fixed arguments change what an
existing mapping does.

## To check first

- **Workflow-agent inputs in the catalog: checked, they are there, with three
  gaps.** Publishing (`POST /workflows/{id}/publish-agent`) synthesizes the
  catalog entry with `runtara_dsl::agent_meta::workflow_agent_info`: one
  capability, `run`, whose `inputs` are the workflow's `inputSchema` fields
  (sorted by name; `required`, `description`, `enum`, `default`, `format`,
  `items` kept). Every compile merges the tenant's published agents into the
  catalog (`workflow_agents::catalog_with_workflow_agents`, called from
  `api/services/compilation.rs`), so the compiler sees them. The gaps, for
  the schema shown to a model:
  - **`connection` inputs.** A workflow-agent's connections are inputs of
    type `connection` (a connection id). A model cannot know one; shown, it
    is a required argument it must invent. Hide them from the schema; the
    tool step supplies them as fixed arguments (second fix below). Without
    a mapping for one, validation refuses the tool (E022 for a hidden,
    unmapped required input).
  - **`file` inputs.** Not a JSON Schema type: map to the file object
    (`{content, filename, mimeType}`, content base64), or hide.
  - **Nested objects.** `capability_field_from_schema` keeps an `object`
    field's type but not its `properties` (`schema: None`), so the model sees
    `{"type":"object"}` only. Either fill `schema` at publish with
    `dsl_schema_to_json_schema` of the properties (a change to
    `workflow_agent_info`, so to every published agent's `meta.json` on its
    next publish), or accept it for now. An EmbedWorkflow tool does not have
    this gap: it reads the child's `inputSchema` directly.
  - The entry is the published version's: changing the workflow's inputs
    without publishing again leaves the old ones, which matches the artifact
    a caller runs. A parent compiled before a publish needs a recompile to
    see it.
- **Schema size.** Shown in full, `http-request`'s ten inputs are about 2 KB
  per call. Fine for a few tools; fixed arguments (above) are the remedy for
  many.
- **Provider quirks.** Gemini's `parametersJsonSchema` and Bedrock accept a
  subset of JSON Schema; a capability's detailed `schema` may use more. Test a
  capability with a `schema` field against each provider's validator, or drop
  keywords they refuse.
