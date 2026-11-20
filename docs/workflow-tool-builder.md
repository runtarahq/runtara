# Building workflows as chat tools

A chat (a workflow with an AI Agent step, `docs/native-chat.md`) can be given
new tools while it runs: a person asks for one, a builder workflow drafts it,
the person approves it, and the builder publishes it as a workflow-agent and
adds it to the chat's AI step. Until now the builder could only wrap ONE agent
capability. This plan lets it draft a small workflow of several steps
("fetch this URL, then take the page's title"), and use workflows the tenant
already has.

Nothing here depends on the model: the builder's model and its connection are
inputs, and every check that matters is made by the builder, not by the model.

## What a tool is

Any workflow published as an agent (`publish_workflow_agent`, its slug the
agent id) is a tool once an Agent step `agentId: <slug>`, `capabilityId:
"run"` hangs off the AI step by a labelled edge. Its input schema is the
tool's arguments; its Finish output, the tool's result. It must not pause in
the middle of a chat turn: no WaitForSignal, no Delay, no Agent step that can
retry and park (`maxRetries: 0`), as publishing reports (`parks`,
`usableAsAiTool`).

## The builder

```
request ─▶ create an empty draft
        ─▶ draft it (AI Agent with authoring tools)      ── not possible ─▶ reason, done
        ─▶ policy check (deterministic)                  ── refused ─────▶ reason, done
        ─▶ publish it (the engine's proof it does not park)  ── parks ─▶ unpublish, done
        ─▶ test run (only when nothing in it changes data)
        ─▶ proposal + approve code ─▶ person approves     ── rejected ─▶ unpublish, done
        ─▶ add the published version to the chat
```

### Drafting

The builder creates the draft workflow first, then runs an AI Agent step with
two kinds of tools:

- Reading, over MCP with a read-only key: `list_agents`, `get_agent`,
  `get_capability`, `list_step_types`, `get_step_type_schema`,
  `list_connections`.
- Editing the draft: `edit_draft(operations)`, `read_draft()`,
  `test_draft(input)`. Each is a small workflow published as an agent; the
  draft's `workflow_id` is a **fixed argument** the builder maps into the tool
  step (`docs/ai-agent-tool-schemas.md`), hidden from the model. The model can
  edit only its own draft, never the chat, the builder or another workflow.

The step ends with `{possible, reason, tool_name, description,
sample_input}`. "Not possible" ends the run before any approval.

### Policy check

The builder reads the saved draft and refuses it, before anyone is asked,
when a step:

- uses a connection of the `mcp` integration (the builder's own keys: a tool
  holding one could edit workflows), or an `mcp`, `conversation`, `control`
  or model agent;
- embeds another workflow, or is a step type that waits (WaitForSignal,
  Delay) or loops without bound;
- uses a connection that is not one of the tenant's listed connections;

or when the draft does not validate.

Then the builder publishes the draft. Whether a workflow-agent parks is the
engine's own proof, made when it is published (`parks`,
`usableAsAiTool`); one that parks cannot be a chat tool, so it is
unpublished and refused. A published agent nothing uses is not reachable
from any chat: it is added to the chat only after approval.

### Test

When no step's capability has side effects, the builder runs the draft once
with the model's `sample_input` (`execute_workflow_wait` on the draft's
version) and shows the result. When one has, it is not run and the proposal
says so.

### Proposal and approval

The proposal lists each step (agent, capability, connection), the tool's
inputs, whether it changes data, and the test result, with the approve code
the chat's user must type (`approve 1a2b3c`); the chat maps the user's own
message into the answer as a fixed argument, so the model cannot approve.

Approval is of that version: the published agent is what the proposal showed
and tested, and the draft's editing tools are gone once drafting ends. A
rejected proposal is unpublished.

## Platform changes

1. **Whether a workflow can be a tool, before it is published.** Only
   publishing says whether an agent parks, so the builder publishes the draft
   before approval and unpublishes it if it parks or is rejected. Reporting it
   from validation as well would need the validation sandbox's report to carry
   it; not needed for this.
2. **Unpublishing.** A published agent goes away only with its workflow.
   `unpublish_workflow_agent` removes it from the catalog (parents that
   composed it keep their copy, as with a deleted workflow).
3. **Who may use a connection** (recorded, not done): any workflow of the
   tenant may use any of its connections, which is why the policy check must
   refuse `mcp` connections by name. See `docs/workflow-authoring-gaps.md`.

## Order

1. Unpublish.
2. (Parking is read from publishing.)
3. The draft tools, with the fixed `workflow_id`.
4. The builder: draft, check, test, proposal, version pin; model and
   connection as inputs.
5. Gap 3 in `docs/workflow-authoring-gaps.md`.
6. Tested through the chat: a two-step tool, a refused draft, "not possible".
