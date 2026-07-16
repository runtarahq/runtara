# Checkpoint namespacing for composed workflow-agent children — plan

_Verified against `feature/workflow-agent-unification` (2026-07-16).
Status: IMPLEMENTED — N1 (stdlib whitelist + shared `child_cache_prefix` +
key-builder audit), N2 (`is_workflow_agent` manifest flag + `agent-scope-input`
envelope wrap), N3 (`checkpoint-scope:1` capability-tag marker + stale-artifact
compose gate + live e2e in `e2e/test_workflow_agent_parity.sh` steps 5–6).
§4 signal-id scoping is ALSO IMPLEMENTED (N4): both signal-id builders fold
`_cache_key_prefix` into the step segment (`{instance}/{workflow}/{prefix}::
{step}{indices}`), covering embeds and composed children uniformly; the wait
timeout deadline (checkpointed under the signal id) scopes with it. Top-level
ids stay byte-identical. Note: the artifact marker landed as a capability TAG
(`checkpoint-scope:1`) rather than a top-level `checkpointScope` meta field —
same semantics, zero `AgentInfo` schema change._

## 1. The problem, precisely

A durable workflow-agent child composed into a parent shares the **parent
instance's** runtime (`report_terminal_status` suppression already keeps it
from completing the parent; this plan is about the *state* it writes). Every
id-carrying runtime call — `checkpoint`, `get-checkpoint`,
`durable-sleep-checkpoint`, `record-retry-attempt`, `poll-custom-signal` —
lands in the parent instance's checkpoint store under ids the CHILD baked at
its own compile time, derived from its own step ids.

The collision cases, worst first:

1. **Same child invoked twice in one parent** (two Agent steps targeting the
   same slug, or a Split/While iterating one) — the child's internal keys are
   IDENTICAL across invocations. Invocation 2's `get-checkpoint` HITS
   invocation 1's state: skipped sleeps, wrong replay state. This is a
   guaranteed collision, not a naming coincidence, and it breaks the flagship
   "Split over a durable child" use case.
2. **Parent step id == child step id** (`call`, `delay`, `finish` are common)
   — parent and child replay-corrupt each other.
3. **Two different children sharing step ids** — same as (2) between children.

**Why "prefix with the parent workflow id" is not the fix:** the checkpoint
store is already scoped per *instance*, and one instance runs exactly one
parent workflow — the workflow id is a constant within the colliding scope and
adds zero discrimination. The colliding dimension is the **invocation site
within the instance**: which step, which loop iteration, at which nesting
depth. (The chosen scheme below does embed the workflow id at the root of the
prefix — the instinct is right, it's just not sufficient alone.)

## 2. The mechanism already exists — EmbedWorkflow solved this

Inlined (`EmbedWorkflow`) children have the exact same problem and already
solve it in the stdlib (`direct_json.rs`), with tests:

- Key builders honor two reserved **variables**:
  - `_cache_key_prefix` — prepended to the key
    (`agent_cache_key` ~:3357, `split_cache_key` ~:3388,
    `embed_workflow_cache_key` ~:4911);
  - `_loop_indices` — appended as `::[i,j]` for per-iteration uniqueness.
- The prefix **composes recursively**: `embed_workflow_iteration_variables`
  (~:4973–4988) computes the child's prefix as
  `{parent_prefix}__{step_id}{loop_indices}` — falling back to
  `{workflow_id}::{step_id}{loop_indices}` at the root (where `_workflow_id`
  is already `"{workflow_id}::{instance_id}"`) — and injects it into the
  child's variables. Nested embeds chain automatically.

So the plan is NOT a new mechanism: it is **extending `_cache_key_prefix`
across the composition boundary**, so a composed workflow-agent child is
indistinguishable from an embedded child in key-space.

## 3. Design: prefix rides the child's input envelope

### Parent side (emitter)

At the workflow-agent invoke boundary — the same pre-invoke stdlib rewrite
slot `agent-connection-input` occupies — wrap the child's mapped input in the
canonical envelope carrying the site scope:

```json
{ "data": { ...mapped input... },
  "variables": { "_cache_key_prefix": "<site scope>" } }
```

- `<site scope>` = the SAME formula the embed path uses:
  `{parent_prefix}__{step_id}{loop_indices}` /
  `{workflow_id}::{step_id}{loop_indices}` — factor the existing
  `embed_workflow_iteration_variables` prefix computation into a shared
  `child_cache_prefix(step_id, source)` stdlib fn and call it from a new
  `agent-scope-input(agent_id, input, source)` (or fold into the existing
  connection-input rewrite as one `agent-child-input` pass).
- **Replay-stable by construction**: step id is compile-time; `_loop_indices`
  are the deterministic iteration counters replay re-derives; the inherited
  `parent_prefix` recurses. (Host-side occurrence counters were considered and
  REJECTED: checkpoint-hit skipping on resume desynchronizes any counter that
  only increments on live invocations.)
- **Gated to workflow-agent targets only**: native agents must not receive an
  envelope-shaped input. Thread `is_workflow_agent: bool` into
  `DirectAgentManifest` (from the catalog overlay's `workflow-agent` tag, the
  same way `connection_ref` rides the manifest). No catalog / no tag → no
  wrapping → today's behavior.
- Ordering: compute the parent's own agent-step cache key from the UNWRAPPED
  mapped input (as today), wrap afterwards — the parent's dedup semantics are
  untouched.

### Child side (stdlib, one whitelist line + audit)

`build_source` already unwraps the `{data, variables}` envelope and merges
runtime variables over declared defaults — but deliberately **filters
`_`-prefixed keys** (anti-spoofing for `_workflow_id`/`_tenant_id`/
`_instance_id`). Change: whitelist exactly `_cache_key_prefix` (and only it)
through the filter. The identity variables stay non-overridable — the prefix
is a namespace hint, not identity, and the worst a malicious caller could do
by setting it is namespace their own child's state, which is precisely the
feature.

With that one line, every key builder that already honors the prefix works
for composed children — and **nesting is free**: a child composing a
grandchild runs the same emitter code, so it wraps the grandchild's input
with `{child's own inherited prefix}__{its step id}[indices]`, chaining
exactly like nested embeds. Mixed embed-inside-composed and
composed-inside-embed chain through the same variable.

### Audit: key builders that must honor the prefix

| Builder | Honors `_cache_key_prefix` today? | Action |
|---|---|---|
| `agent_cache_key` (+ `::attempt::N` retry keys derived from it) | YES | — |
| `split_cache_key` | YES | — |
| `embed_workflow_cache_key` | YES | — |
| `delay-sleep-key` (Delay durable sleep) | **verify** | extend if not |
| `DURABLE_SLEEP_CHECKPOINT_ID` alias (plain durable-sleep) | NO (host-side constant) | extend: prefix in the stdlib caller, or accept (single well-known key — colliding sleeps of the same duration are benign? NO: deadlines differ — extend) |
| Wait-for-signal deadline checkpoint | **verify** | extend if not |
| `ai-turn-cache-key` / memory keys | **verify** | extend if not |
| `wait_signal_id` (custom signal ids) | resolved: scoped (option (a), N4) | done |

Any builder found NOT honoring the prefix is a latent bug for **EmbedWorkflow
children too** — fixing it serves both paths.

## 4. Decision point: custom signal ids

Signal ids are *external addressing* (a caller posts a signal to
`instance + signal_id`), unlike checkpoint ids (internal state). Two children
waiting on `"approval"` inside one parent are ambiguous today — for embeds as
well. Options:

- **(a) Scope them too** (uniform): senders must target the scoped id;
  `list_pending_signals` already surfaces pending ids, so the scoped name is
  discoverable. Collision-safe, changes signaling UX into children.
- **(b) Leave unscoped**: both waiters wake on one signal (the
  non-destructive-read semantics make this survivable but ambiguous).

Recommendation: (a), in a separate PR from the checkpoint work, applied to
embeds and composed children together — it is a behavior change for signal
senders and deserves its own review. The checkpoint fix must not wait for it.

## 5. Artifact compatibility — the one real wrinkle

Already-published children were compiled with the `_`-filter and **silently
drop** the injected prefix — the collision would return invisibly. Guard:

- Stamp the synthesized meta (and/or the `DIRECT_WORKFLOW_ABI` custom
  section) with `"checkpointScope": 1` at publish time.
- At parent compile, when a composed workflow-agent dependency's sidecar
  lacks the marker AND the child needs the runtime (durable): **fail the
  compile** with a clear "republish `<slug>` (stale artifact predates
  checkpoint namespacing)" error. Pure children (no runtime import) have no
  checkpoints to protect — compose freely, no marker needed.
- Republish is one `POST /workflows/{id}/publish-agent` call.

## 6. Non-goals / accepted semantics

- **Identical input at the same site** (duplicate Split items) shares the
  scope — intentionally: it matches the parent-level agent cache-key dedup
  semantics (identical input ⇒ same computation ⇒ shared durable state).
- No cross-instance concerns: the store is instance-scoped.
- Prefix visibility in checkpoint tables/diagnostics is fine (internal ids);
  step summaries are unaffected (children publish with track_events off).

## 7. Test plan

Unit (stdlib):
- `build_source` whitelists `_cache_key_prefix` and ONLY it (identity vars
  still filtered — explicit spoof-attempt test).
- Shared `child_cache_prefix` formula: root, nested, with loop indices —
  byte-equal to the embed path's existing output.
- Audit-driven: each key builder in §3's table prefixed under a set prefix.

Emitter unit:
- Workflow-agent invoke wraps input in the envelope (manifest test mirroring
  the `connection_ref` ones); native-agent invoke does NOT.

Gated in-process e2e (extend the durable-child test family):
- Parent step `call` + durable child ALSO containing step `call` — recording
  host asserts disjoint checkpoint-id sets.
- Split over a durable-Delay child (3 items) — three distinct scoped key
  families; then the drain/resume variant: persistent-checkpoint host, kill
  after item 1, resume, assert item 2's child state isolated from item 1's.
- Nested: composed child composing a grandchild — three-level chained prefix.
- Stale-artifact gate: strip the marker from a staged sidecar → parent
  compile fails with the republish error.

Live-server: extend `e2e/test_workflow_agent_parity.sh` step 4 with a
same-step-id child and a Split-loop parent; assert completion + (via the
checkpoints listing endpoint) scoped ids.

## 8. Rejected alternatives

- **Host-side scope stack with `push/pop-checkpoint-scope` runtime calls**
  (prefix in the linker glue over `WorkflowState`): covers old child
  artifacts without republish, but adds new runtime WIT + a second namespace
  mechanism that diverges from the embed precedent, needs the legacy
  Composed-binding guest runtime to grow the same stack, and still has the
  same compat hole one level down (an old child composing a grandchild never
  pushes). One mechanism (`_cache_key_prefix`) everywhere wins.
- **Bake slug@version prefix at publish** (static): fixes cases 2–3 but not
  the guaranteed case 1 (same child twice / Split loops); subsumed by the
  dynamic site scope.
- **Host occurrence counters**: not replay-stable under checkpoint-hit
  skipping (resume replays skip completed invokes, desynchronizing counts).
- **Validation-only** (reject shared step ids between parent and children):
  cannot address case 1 at all, couples parent validation to child internals.

## 9. Sequencing (each shippable)

1. stdlib: `_cache_key_prefix` whitelist in `build_source` + shared
   `child_cache_prefix` + audit/extend the §3 table builders (also hardens
   EmbedWorkflow). Unit tests.
2. emitter: `is_workflow_agent` on the manifest + envelope wrap at the
   invoke boundary. Manifest tests + gated e2e (same-step-id, Split-loop,
   nested).
3. publish/compose: `checkpointScope` marker + stale-artifact compile gate.
   Gate test + live-server sweep extension.
4. (separate) signal-id scoping decision of §4, embeds + composed together.
