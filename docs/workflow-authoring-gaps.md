# Workflows that author workflows: what to fix

Status: findings and proposed fixes; nothing here is implemented unless it
says so.

## Where these come from

A workflow, `tool-builder`, was built on a local 11.0.0 server to give an AI
chat new tools:

1. It reads the connection-free agents' capabilities over the server's own MCP
   endpoint (a `read_only` key).
2. A model chooses one capability and writes a tool spec.
3. A person approves it (or rejects it, or corrects its fixed inputs).
4. With a key scoped to `workflow:read/create/update`, it creates a workflow
   running that capability, publishes it as an agent, adds it as a tool of the
   chat's AI Agent step, and deploys the chat.

It works end to end: the chat's next message used the new tool, with the
arguments its schema named. Getting there hit the problems below. Each one
affects any workflow, MCP client or person that authors workflows this way,
not only this one.

## Already fixed

Not merged yet:

- **The conversation event stream lost its tenant's scope** after its first
  read, and **an object-model connection idle past seal's one-minute limit**
  failed its next query. Both in PR #445.
- On branch `feature/ai-agent-tool-schemas`
  ([ai-agent-tool-schemas.md](ai-agent-tool-schemas.md)):
  - an AI Agent's tools are shown their arguments' schema;
  - a tool step's mapping is a fixed argument (DSL 3.8.0);
  - a scoped key may set a workflow's slug and publish it as an agent
    (`permission_for` had no entry for either route, so a restricted key was
    refused them whatever it was granted);
  - the step an `mcp.<toolset>` edge leads to no longer has to map the inputs
    the model's search and invoke calls supply.

## To fix

In the order proposed: 1 to 3 can leave a workflow broken, or report a failure
as success; 4 and 5 block workflows that build tools beyond one capability; 6
to 11 are smaller.

### 1. A saved version is current before it compiles

A workflow's current version is `COALESCE(current_version, latest_version)`
(`api/repositories/workflows.rs`). Until a version is set current explicitly,
`current_version` is NULL, so every version saved is current at once, whether
it compiles or not. Once one is set explicitly, the workflow stops following
its latest version.

Seen: two graph edits through MCP saved versions 5 and 6 of a chat; both
failed to compile, and 6 was current, so the chat had no working version
until version 4 was set current by hand. A later save (version 7) then did
not become current, because 4 had been set explicitly. Neither behavior is
visible to whoever saves.

Fixed: when no version was chosen, the current version is the latest that is
not known to fail to compile: one that compiled, or one not compiled yet
(compiled on demand, so save-then-run still runs what was saved), but never
one whose compilation failed; the latest only if every version failed.
`current_version_sql` in `api/repositories/workflows.rs` is the rule for what
the API shows, and `get_latest_runnable_version` what a run that names no
version runs when the engine has no choice of its own
(`version_when_none_was_chosen`). A version chosen explicitly still stays
current. Tests: `tests/current_version_rule.rs`.

### 2. Graph edits build on a failed draft

When the latest version is ahead of the current one, MCP graph edits
(`fetch_latest_graph` in `mcp/tools/graph_mutations.rs`) start from the
latest version: a draft, kept on purpose so edits stack. A failed draft is
kept the same way, and there is no way to discard one.

Seen: after the chat was put back on version 4, the next edit started from
failed version 6 and inherited its step, which named a workflow-agent since
deleted ("Agent '…' is not enabled for this tenant").

Fixed: a `restore_version {workflow_id, version}` MCP tool saves a version's
graph (with its memory and event tracking) as a new version, so the latest is
that graph again and edits start from it. Versions are never renumbered or
removed: the failed draft stays in the history. (Lowering the latest version
instead would reuse version numbers.) With 1, the restored version is current
as soon as it is not known to fail.

### 3. A failed MCP tool call looks like success

`deploy_workflow` answered with `"success": false` and validation errors in
its text, and the `mcp` agent's `mcp-tool-invoke` step completed: the run went
on to publish a workflow that had not deployed.

Fixed, both ends:

- The server's workflow and graph tools answer a body that says
  `"success": false` as an error result (`isError`), with the same text
  (`answer_result` in `mcp/tools/internal_api.rs`). A non-2xx answer was an
  error already.
- `mcp-tool-invoke` takes `fail_on_tool_error` (default `false`): set, a
  result marked as an error fails the step (`MCP_TOOL_ERROR`) instead of
  returning it with `is_error`. A workflow's fixed MCP steps set it; an AI
  Agent's tool loop leaves it off, so the model reads the error and can try
  again.

### 4. A workflow-agent that may retry cannot be an AI tool

A workflow published as an agent is judged to "park" if any Agent step in it
may retry with backoff (`parks_under_capability_abi` in
`direct_wasm/support.rs`: `("Agent", "retry-or-rate-limit-backoff")`, unless
`maxRetries` is 0). A parking workflow-agent runs as a child run, and its
capability cannot be an AI Agent tool or memory provider: the caller fails to
compile ("calls a workflow-agent that parks, so it runs as a child run …").
Agent steps retry by default, so almost every published workflow is unusable
as a tool. `tool-builder` sets `maxRetries: 0` on its tools' one step to get
round it.

Partly fixed: it is said where it can be acted on.

- Publishing answers whether the agent parks, whether it can be an AI tool
  (`usableAsAiTool`), and where it parks (`parkingSites`: step, type and
  reason), so the publisher knows before a caller fails to compile.
- The caller's compile error names that list and the two ways out:
  `maxRetries: 0` on the Agent steps that retry, or a workflow that is not
  durable and needs no runtime services (`needs_agent_runtime`), whose
  retries are local waits that do not park.

Still to do, a design of its own: let an AI Agent call a child-run
workflow-agent as a tool, the tool loop parking on the child as the
WaitForSignal tool parks on its signal. Until then a published workflow meant
as a tool keeps to the two ways out above.

### 5. An Agent step's input resolves mapping objects inside data

`resolve_nested_references` (`runtara-workflow-stdlib/src/direct_json.rs`)
walks an Agent step's whole evaluated input and replaces every
`{"valueType": "reference", "value": "<path>"}` object in it with the value at
that path in the current run. It exists for condition arguments (an
object-model filter's operands). It also rewrites a workflow graph passed as
data.

Seen: a graph sent to `deploy_workflow` through `mcp-tool-invoke` arrived with
`{"valueType":"reference","value":"data.country_name"}` replaced by `null`
(the building run has no `data.country_name`), and failed validation (E023).
`tool-builder` builds its graphs with flat edits (`apply_graph_mutations`,
`set_mapping`'s `from_input`) instead; a `template` value passes through,
being no reference.

Fixed, by what the author wrote rather than by capability metadata: the walk
follows the step's authored mapping (`resolve_authored_references` in
`direct_json.rs`). It resolves references inside an `immediate` value (where
a condition's arguments are written), through a `composite`'s fields and items
and under dotted paths, and leaves alone any value that came through a
`reference` or a `template`: that is data. The one-wrapper unwrap of a
top-level field likewise applies only to fields authored as an immediate.

A workflow that built a condition at run time and passed it through a
reference, relying on references inside it being resolved against the run,
now passes it as it is; write such a condition as an immediate instead.

### 6. Single graph edits cannot build a graph step by step

`save_graph` says graph edits run no full validation ("PUT rejects only
structural corruption … and reports everything else as warnings"), but:

- `add_agent_step` refuses a step whose required inputs are not mapped yet
  (E022), before `set_mapping` can map them;
- `set_input_schema` refuses a workflow with no steps;
- `set_mapping` refuses an input not yet in `inputSchema`.

Only `apply_graph_mutations`, which saves once, builds a new workflow, and
only with its operations in an order that satisfies each of these.

Fixed for what made a new workflow unbuildable: an edit of a version that
cannot pass validation itself (no steps yet, or a step whose inputs are not
mapped yet) was sent whole through `update`'s full validation, with no
fallback, so `add_agent_step` failed on E022 and `set_input_schema` on "no
steps". Such a version cannot compile or run; the edit now patches it in place
with the structural checks only, as every later edit does (`save_graph`). Found
half done on the local server: the first edit of a new workflow worked, the
second did not. `set_mapping` still refuses an input that is not in
`inputSchema` yet: that checks the edit's own argument, and catches a typo,
so set the input schema first.

### 7. A published agent's schema loses detail

`workflow_agent_info` (`runtara-dsl/src/agent_meta.rs`) turns a workflow's
`inputSchema` into its capability's inputs, but an `object` field loses its
`properties` (`schema: None`), so a model calling it as a tool is shown
`{"type": "object"}` only. `connection` and `file` inputs need handling in the
tool schema; see
[ai-agent-tool-schemas.md](ai-agent-tool-schemas.md#to-check-first).

Fixed: a field with structure below it (an object's properties, an array's
item properties) carries its whole JSON Schema in `schema`, from the same
converter an AI Agent's `outputSchema` uses, and a model calling the agent as a
tool is shown it. Published agents get it when they are next published.

### 8. A deleted workflow keeps its slug

Creating a workflow with the slug of a deleted one fails with 409 ("A workflow
with slug … already exists"), and nothing frees it. `tool-builder` suffixes
its slugs with the run id.

Kept on purpose: a deleted workflow reserves its slug so a published
capability id is never reused by another workflow (docs/workflow-slug-plan.md,
decision 4). Fixed is the error: it names the deleted workflow that keeps the
slug and says why, instead of that a workflow with it exists, which nothing
listed shows. A builder that may ask for the same tool twice suffixes its
slugs.

### 9. `set_mapping` cannot map a step's whole output

`set_mapping` requires `from_output` with `from_step`, building
`steps.<step>.outputs.<field>`; a capability whose output is a value, not an
object, has no field. `tool-builder` maps its Finish with a template
(`{{ steps.run.outputs | tojson }}`), so its tools return their result as a
JSON string.

Fixed: `from_output` may be omitted (in `set_mapping` and in
`apply_graph_mutations`), for the step's whole output.

### 10. Smaller inconsistencies

- An AI Agent's `mcp.<toolset>` search finds the server's MCP tools
  (`list_agents`, `get_capability`), not agent capabilities: a model asked to
  find a capability searched for it there and found nothing. Fixed: the
  toolset's guidance to the model says the search matches tools by their names
  and descriptions, not the data they return, and to search for the tool that
  lists or reads the data.
- `POST /api-keys` answers with the key at the top level; other routes wrap
  their answer in `data`. Left: the console's generated client reads it as it
  is, so changing it is a change to the console too.
- Templates have no `startswith`; slicing (`name[:1] != "_"`) does the same.
  Left: adding Python-style string methods would add a dependency
  (`minijinja-contrib`) for little.

### 11. Not tested

An AI Agent step with both `outputSchema` and tools (a `chat-turn` with
structured output). Tested since: it works, the model calling its tools and then
answering to the schema, and the step's response is the parsed object
(`direct_wasm_execute_ai_agent_tool_loop_answers_to_its_output_schema`).
`tool-builder` could use one step instead of two.

### 12. An upgrade leaves every published agent stale (found while testing)

A workflow-agent is linked against the workflow stdlib it was published
with. A server whose stdlib differs (any release that changes it; here, the
fixes to 3 and 5) refuses to compile a caller of an agent published before:
"workflow-agent `runtara:agent-…` was linked against workflow stdlib …, not …;
republish it". A chat with such a tool could not take a new tool until the old
one was republished by hand; its current version kept running (1).

To fix: republish a tenant's workflow-agents when the server's stdlib
changes (at boot, or on first use), from the version each was published from,
or link a caller against an agent's stdlib without refusing it.

### 13. "Optional connection" is one agent's name in validation

An agent that takes connections must be given one on every step, except
`http`, whose connection is optional (public requests need none): validation
says so by name (`validation.rs`, `agent.supports_connections &&
!agent_id.eq_ignore_ascii_case("http")`). Another agent with an optional
connection cannot say so.

To fix: a `connectionOptional` flag on the agent's catalog entry (from its
module attributes, as `supportsConnections` is), read by validation and by
tooling instead of the name. Left for now: `AgentInfo` is built literally in
about a hundred places, and `http` is still the only such agent.

## What `tool-builder` cannot do yet

Not platform gaps, but what the version described here is limited to:

- A tool is one capability of a connection-free agent; a tool that is a
  workflow of several steps needs the builder to compose graphs (5 and 6).
- The chat cannot ask for a tool itself: that needs a tool on the chat that
  starts `tool-builder` with the chat's id. The `control` agent's `start`
  cannot be an AI tool (E132), so it would be a small workflow-agent calling
  `execute_workflow` over MCP with a key scoped to `workflow:execute`.
- A small local model gets fixed values wrong (an enum value it was not
  shown, a code type spelled its own way); the approval step, which can
  correct them, is what catches it.
