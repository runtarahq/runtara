# Parallel graph branches (heterogeneous fan-out) — detailed plan

Status: **4a + 4b + 4c LANDED.** Every DAG parallel-branch shape now runs concurrently:
4a.1/4a.2 (single-Agent, durable + non-durable), 4b (linear Agent-chain wavefront), 4c.1
(sync non-Agent chain steps), 4c.3 in-branch Conditional / Switch / Edge / While / Split /
Embed / AiAgent (blocking composite nodes), and in-branch Wait / durable-Delay (deferred-
suspend, §4.0.2). The only transitionally-linearised remnants are an AiAgentLoop whose tool
set includes a Wait, and a composite that itself nests a suspension. **Finding (§4.1):
neither needs the CPS segment scheduler for coverage** — a small "suspending composites"
extension (Tier 1, §4.2) removes both via the landed hard-return-inline + replay mechanism;
the general scheduler (Tier 2, §4.3) is a concurrency optimisation, gated behind profiling.
Sibling to `docs/wasip3-parallelism.md`, which
delivered in-guest parallelism for the **Split** step (homogeneous data-parallelism,
Phases 0–3 + concurrent backoff). This document plans **heterogeneous graph
branches** — a fan-out `A → {B, C, …} → M` where the branches are *different*
subgraphs — run concurrently instead of linearised.

**Landed (worktree-wasip3-parallelism):** `DirectRunPlan::ParallelBranches`;
`compile/branch_parallel.rs` launch→drain→assemble window with memoized invoke +
per-branch instance pooling; durable launch checkpoint-gate (replay-safe — the
durable key ignores `source.steps`). Verified: emitter unit tests, e2e battery
(overlap + durable-resume-no-double-fire), and an isolated live server (merge
`{"b":"B","c":"C"}`). See [[project_parallel_branches]] / [[reference_isolated_live_server]].

The constraint from `docs/wasip3-parallelism.md` still binds: **no host-orchestrated
fan-out.** All concurrency lives inside the emitted `workflow.wasm`; the host only
services async imports (agent invoke, host-io HTTP, timers) already added for Split.

---

## 1. Today's behaviour: fan-out is linearised, not branched

Unconditional fan-out is **not represented as branches at all**. Two paths in the
planner diverge here:

- **Conditional / routing Switch** (`plan.rs:510`, `plan.rs` `Conditional` arm):
  these *are* modelled as branches. The planner collects `branch_starts`, calls
  `direct_find_merge_point(graph, &branch_starts)` to find the diamond re-convergence,
  plans each branch with `stop_at = merge`, and emits the merge continuation **once**
  as a shared `merge_plan` (`SwitchRoute`/`EdgeRoute`/`Conditional` variants in
  `DirectRunPlan`). Only **one** branch runs at runtime — the taken route — so there
  is nothing to parallelise.

- **Unconditional fan-out** (a step with ≥2 unconditional normal-flow successors,
  e.g. `A→B`, `A→C` both unlabelled/unconditioned): handled by
  `direct_execution_order` (`plan.rs:1217`), a Kahn topological sort of the whole
  unconditional region. `B` and `C` are simply two nodes in one flat order; the
  planner walks them one at a time via `topo_successor` (`plan.rs:1344`). The
  merge `M` (indegree 2) is emitted once, after both, because Kahn only releases it
  when both predecessors are consumed. There is **no** `ParallelBranches` node, no
  per-branch subplan, no isolated per-branch state — it's a single linear
  instruction stream sharing the one `run`/`invoke` body, its retptr scratch, and
  the ~30 `DIRECT_*` locals.

- **E073** (`ParallelFanoutNoMerge`, guard in `normal_flow_plan` ~`plan.rs:1388`): an
  unconditional fan-out whose branches don't re-converge is rejected. **This stays** — we
  keep the validated reconvergence guarantee and build parallelism on it (§8): every
  parallel fan-out has a single merge point `M`, which the window drains into.

So "add branch parallelism" means: **introduce a real branch/merge structure for
unconditional fan-out** (which doesn't exist yet) and drive the branches through the
same launch → drain → assemble window the Split step already uses.

### Why branches are harder than Split

Split items are **homogeneous**: N items through one single-Agent body. That let V1
(a) memoise a single async-lowered `invoke` and fan it across per-item slots, and
(b) assemble every item uniformly into `data.*` buckets. See
`split_parallel.rs::emit_parallel_split_items` / `parallel_agent_body`.

Graph branches are **heterogeneous**: each branch is a *different* subgraph shape —
different agent(s), different mapping, possibly multiple steps, nested control flow,
its own onError. There is no single memoised invoke to fan out, and no uniform
assemble. Each branch needs its **own** isolated register/scratch state and its own
mini-scheduler, versus the one shared linear body today. That difference is what
splits the work into the phases below.

---

## 2. Design overview

Add one plan node and reuse the Split window's runtime machinery.

### 2.1 New plan node

```rust
// direct_wasm/plan.rs — DirectRunPlan
ParallelBranches {
    origin_step_id: String,        // the fan-out step (for debug scope / provenance)
    branches: Vec<DirectRunPlan>,  // each rooted at a successor, stop_at = merge → ends in Join
    merge_plan: Box<DirectRunPlan>,// shared continuation from the validated merge point M
    durable: bool,                 // inherited from graph.durable
}
```

Structurally this mirrors `SwitchRoute { branches, default_plan, merge_plan }` — the
merge is emitted once as a shared continuation and each branch ends in `Join` at the
merge (`plan.rs:469`, `stop_at == step_id → DirectRunPlan::Join`). The difference from
Switch is runtime: **all** branches run and their results **all** feed the merge,
rather than exactly one route running. Reconvergence is a validated invariant (§8), so a
single merge `M` always exists.

### 2.2 Where the planner builds it (`plan.rs`)

`direct_execution_order` currently absorbs fan-out silently. Intercept **before**
linearisation: when the region walk reaches a step `A` that
1. has ≥2 unconditional normal-flow successors (a fan-out), and
2. every branch is within the currently-landed phase's shape support (§7),

then treat `A` like a branching step: stop the region order at `A` (add it to the set
that `direct_is_branching_step`/`direct_has_conditioned_normal_flow_edges` already use
to halt Kahn traversal, `plan.rs:1236`/`1276`), and emit `A`'s plan as
`ParallelBranches`. The shared continuation is `M = direct_find_merge_point(...)`, which
validation guarantees exists. Each branch is planned with `step_run_plan_inner(successor,
stop_at = M, region_root = successor)` — identical to how Switch plans its routes
(`plan.rs:539`) — so a branch that itself contains nested control flow, Split, or
onError is planned recursively and correctly, ending in `Join` at `M`. The merge
continuation is `step_run_plan_inner(M, stop_at = outer_stop, region_root = M)`.

If any eligibility check fails, **do not** create the node — fall through to today's
linearised `direct_execution_order`. Existing graphs are byte-for-byte unchanged
unless opted in and eligible (§6, §10).

### 2.3 Runtime: reuse the Split window

The emitter for `ParallelBranches` reuses everything Phases 0–3 added, all already
present and load-bearing for Split:

- async-lowered `invoke` imports per agent (`core_module.rs`, per-pool
  `[async-lower]invoke`),
- `$root` waitable-set builtins (`ws.new`, `waitable.join`, `ws.wait`, `subtask.drop`),
- `runtara:host-io/timers.sleep` timer subtasks (for branch `Delay` and retry backoff),
- `func_wrap_concurrent` host-io HTTP (`host_io.rs`) so branch agents' HTTP hops overlap,
- per-item slot layout + drain loop + round scheduler (`split_parallel.rs`),
- the `parallel_enabled` gate (async-typed ABI v2 only; sync ABI falls back to
  sequential).

New machinery is the **branch scheduler** and **heterogeneous slots** (each slot
targets a *different* agent / holds a *different* branch's live state), detailed per
phase below.

---

## 3. Phase 4a — single-Agent branches (tractable, high value)

**Scope:** every branch between the fan-out and the merge is exactly **one Agent step**
(then `Join`). I.e. `A → {AgentB, AgentC, AgentD} → M`. This is the common
"fire N independent API/agent calls, then join" pattern and is a near-direct
generalisation of the Split window.

This is the recommended first (and possibly only necessary) cut: it captures the
majority of real "parallel branches" use, and — like Split V1 requiring a single-Agent
body — it sidesteps the resumable-coroutine problem entirely.

### 3.1 Mechanics

Generalise `emit_parallel_split_items` from *N homogeneous items* to *K heterogeneous
single-agent branches*:

1. **Launch** (in branch declaration/topological order, deterministic): for each
   branch `b`:
   - Build `b`'s source from the fan-out point's context (the same `steps`/`data`/
     `variables` snapshot every branch sees), apply **branch `b`'s own** input mapping,
     validate, inject its connection.
   - Async-lower **branch `b`'s** agent `invoke` into slot `b`'s result buffer; join
     the subtask into the shared waitable-set.
   - Unlike Split (one memoised invoke fanned across items), each slot records a
     **branch/agent selector** — which async-lower import to call — because branches
     use different agents. Store it in the slot (a new `AGENT_SEL_OFFSET` byte/word in
     the 160-byte slot stride, `compile.rs` `DIRECT_PSPLIT_SLOT_STRIDE`). The re-invoke
     path (retry rounds) dispatches on it via a `br_table` over the pooled imports.
2. **Drain:** `emit_drain_pending` unchanged — `ws.wait` until all K subtasks settle.
   Concurrent HTTP overlaps via `func_wrap_concurrent`; two branches hitting the same
   host still overlap at the wasm level (host serialises only what the connection's
   rate limiter forces — same as Split).
3. **Assemble** (fixed order): for each branch `b`:
   - Classify result (error → branch `b`'s onError plan / fail; success → shape output
     via its `outputShape`).
   - Write `b`'s output into the `steps.<branch_step_id>.outputs` context (via the
     stdlib agent-output + build-source append the sequential Agent path uses), so the
     merge can reference `steps.B.outputs.*`, `steps.C.outputs.*` exactly as if run
     sequentially.
   - Emit `b`'s step-debug events under `b`'s scope id.
4. **Merge:** emit `merge_plan` — reads each branch's step context and continues.

### 3.2 Retry / backoff per branch

Each branch's agent can have retries. Reuse the round scheduler in
`emit_parallel_split_items` verbatim, with the per-slot agent selector so a retry
re-invokes the **correct** agent. Concurrent backoff (both durable and non-durable,
§3.4 of the sibling doc) applies unchanged: timer subtasks overlap, per-attempt
`::attempt::N` durable checkpoints, gate on `!HIT` so guest sleep re-sleeps on replay.
`concurrent_backoff` eligibility (`parallel_agent_body`: `agent_retries > 0`, no
breakpoint, not a workflow-agent child) is evaluated **per branch**.

### 3.3 Durability

Each branch is a durable Agent with its own cache key (branch step id + its source).
Per-step and per-attempt checkpoints are **key-addressed**, so concurrent completion
in any order is already safe (proven for Split:
`direct_wasm_execute_durable_backoff_replay_no_double_fire`). On resume/replay-from-
start, each branch's step checkpoint HITs and the branch fast-forwards; an in-flight
branch re-launches. No new durability primitive needed.

### 3.4 Pause / cancel / debug

- **Pause/cancel:** the cooperative drain checks the pause/cancel flag between
  `ws.wait` returns (same as Split, `emit_drain_pending`). A pause mid-window quiesces
  the in-flight branches to a safe point, then suspends — reuse
  `direct_wasm_execute_pause_mid_window_resumes` semantics.
- **Debug events:** per-branch scope id = branch step id; per-attempt timestamps come
  along with the existing step-debug events. Ordering in the summary is deterministic
  because assembly is fixed-order.

### 3.5 Effort

Small–medium. Mostly a generalisation of existing code:
- `plan.rs`: detect eligible single-Agent fan-out, build `ParallelBranches`
  (~mirrors the Switch arm).
- `split_parallel.rs`: parameterise the slot's agent selector; branch-labelled
  assemble into named step contexts instead of `data.*` buckets.
- `core_module.rs`: ensure every branch agent gets an async-lower import + pool entry
  (extend the pool-collection pass to walk `ParallelBranches`).
- `compile.rs`: one new slot field (`AGENT_SEL_OFFSET`).

---

## 4. Phase 4b — linear-chain branches (multi-step, no nested loops)

**Scope:** each branch is a **linear chain** of steps (Agents, Delays, Logs,
Conditionals that immediately re-join within the branch) with **no nested While/Split
inside the branch**. E.g. `A → {B1→B2→B3, C1→C2} → M`.

### 4.0 The DEPTH-WAVEFRONT (implemented first — reuses the 4a window)

Rather than a general coroutine scheduler, the first slice runs branches in a
**wavefront by depth**, which reuses the 4a launch → drain → assemble window almost
verbatim. First slice restricts a branch to a **linear chain of Agent steps**
(`Agent → Agent → … → Join`; intermediate steps carry no `onError`, the terminal step
may — so 4a's single-Agent case is the length-1 special case).

Walk each branch into its agent chain `[s_i0, s_i1, …]`. Then loop `d = 0 .. maxlen`:

```
for d in 0..max_branch_len:
  LAUNCH:   for each branch i with d < len_i: build source (shared context),
            apply s_id's mapping, durable-gate, async-invoke -> slot[i]
  DRAIN:    ws.wait until all launched depth-d subtasks return
  ASSEMBLE: for each branch i with d < len_i (in order): emit_agent_plan(s_id,
            next = Join, memo = slot[i]) — updates the SHARED steps context
emit merge_plan
```

Correctness rests on the same invariant 4a uses: independent DAG branches never
reference a sibling, so a **single shared `steps` context** is sufficient — `s_id`'s
mapping references only `s_i(d-1)` (assembled in round `d-1`), never another branch's
step, so the extra sibling entries in the shared context are inert. Slots are reused
per depth (round `d` fully drains before `d+1`). Pooling is **per depth**: only
same-component steps invoked in the *same* round contend on an instance lock, so a
component's pool = max over depths of its per-depth branch count (clamped). Durable and
retries carry over from 4a unchanged (per-step launch gate; retries in assemble). A
branch that is length 1 emits exactly the 4a window.

**4c.1 (landed):** the chain may also contain SYNC non-Agent steps (Log, Filter,
SwitchValue, GroupBy). Those depths run assemble-only (no launch) via the standard
dispatcher on a clone of the node whose `next_plan` is replaced by `Join` (so exactly
one step emits; its successor runs at the next depth). Pooling counts Agent nodes only.

### 4.0.1 COMPOSITE nodes — arbitrary in-branch control flow WITHOUT hand-CPS (4c.3)

Non-linear in-branch steps (Conditional, Switch, While, Split, EmbedWorkflow, AiAgent)
do **not** need a hand-emitted coroutine scheduler. They ride the SAME wavefront as a
**composite node**: a chain node whose "next" is its continuation (`merge_plan` for
Conditional/Switch, `next_plan` for the rest). At its depth it runs **assemble-only,
blocking**, via `emit_run_plan_mapping` on a clone with that continuation replaced by
`Join` — so exactly the composite runs (its successor is the next depth). Its taken
arm / loop body executes with the ordinary sequential lowering (its internal agents use
the sync invoke — they don't overlap siblings, but the branch's TOP-LEVEL linear steps
still do). This keeps the "guest single-threaded, host I/O overlaps" model and gives
**complete DAG coverage** with far less risk than a CPS state machine.

Eligibility guard: a composite may run blocking only if it contains **no suspension
point** (`WaitForSignal`, durable `Delay`) — otherwise `emit_run_plan_mapping` would
suspend the instance mid-assemble, before sibling branches at that depth checkpoint,
risking a replay double-fire. `plan_contains_suspension` walks the composite; a branch
with a suspending composite linearises transitionally until the quiesce slice lands.

### 4.0.2 In-branch Wait / durable Delay — the deferred-suspend quiesce (4c.3)

A top-level `WaitForSignal` / durable `Delay` node in a chain runs at its depth but is a
suspension point. Handle it with the §4.4 quiesce: the node registers its wake and sets a
**suspend-pending** flag rather than suspending immediately; after ALL branches at that
depth assemble (so their durable results are checkpointed), if any suspend is pending the
window exits `suspended(list<wake>)` covering every pending wait/timer. On resume,
replay-from-start re-drives the wavefront; satisfied waits proceed, completed durable
steps HIT their checkpoints. Multiple same-depth waits → a multi-wake set — reuses the
existing suspend/wake machinery, no per-branch resume state.

The remaining hard combination — a composite that ITSELF contains a suspension point
(a Conditional arm with a Wait, a nested loop with a Delay) — is the only case that would
need the general segment scheduler below; it is deferred (those branches linearise
transitionally).

### 4.1+ Eliminating the last remnants — TWO tiers, and the finding that reshapes them

Two shapes still linearise transitionally after the landed work:

- **R1 — an AiAgentLoop whose tool set includes a Wait** (durable human-in-the-loop). The
  loop suspends *inside* the tool-dispatch loop. `is_linear_chain_branch` has **no
  `AiAgentLoop` arm** (falls to `_ => false`), so the whole branch linearises today — safe,
  not miscompiled. `plan_contains_suspension`'s `AiAgentLoop` arm (`plan.rs:1962`) walks
  only `next_plan`/`error_plan`, **not `tools`** — a latent gap that must be closed before
  AiAgentLoop is ever run as a composite.
- **R2 — a composite that itself nests a suspension** (a Conditional/Switch/Edge arm
  containing a Wait/Delay; a While/Split body or an Embed child that suspends). Rejected
  today by the `plan_contains_suspension` guards in the composite arms of
  `is_linear_chain_branch` — safe, linearises.

**KEY FINDING (grounds the whole plan): neither remnant needs the general CPS segment
scheduler for COVERAGE.** The wavefront's real suspend mechanism is NOT the "deferred
wake-set" §4.0.2 aspires to. Reading `emit_concurrent_branches` (`branch_parallel.rs`
749–821): a suspending node is simply **assembled LAST at its depth (pass 2) and
hard-returns the instance inline** — the Wait/Delay lowering calls
`emit_entry_suspend_return`, which returns from `run`; there is no flag-and-aggregate.
Replay-from-start re-drives the wavefront; durable per-step checkpoints (skip-launch gate
+ durable-block HIT, `4a.2`) prevent double-fire. A composite that nests a suspension
resumes correctly under the **exact same** mechanism, because its blocking emission (via
`emit_run_plan_mapping` on `with_next_join(node)`) **is** the ordinary sequential lowering
— and a durable While-with-in-body-Delay (or AiAgentLoop-with-Wait-tool) *already* resumes
correctly sequentially. The only new requirement is: **assemble the suspending composite
after every non-suspending sibling at its depth** (so they checkpoint first) — which is
precisely what marking it a "suspending node" (pass 2) does.

Consequence: the linearisation remnants are removed by a **small extension** (Tier 1), and
the CPS segment scheduler (Tier 2) is a pure **concurrency optimisation**, not a coverage
requirement. Both remnants become fully parallel (their agent/HTTP work overlaps) under
Tier 1; Tier 2 only additionally overlaps a branch's *post-suspend-depth* work with a
*sibling's* suspension.

---

### 4.2 TIER 1 — "suspending composites": remove every linearisation remnant (small, low-risk)

Extend the existing deferred-suspend wavefront so a composite that nests a suspension — and
an AiAgentLoop — ride it as a **suspending node** (pass 2, hard-return inline, durable-
gated). No new runtime machinery; reuses the landed window verbatim. Three commits.

**T1a — suspension-free AiAgentLoop as a blocking composite** (cheap win, no suspend path):
- `plan.rs plan_contains_suspension` **[required correctness fix]**: the `AiAgentLoop` arm
  must also walk `tools` — `DirectAiToolPlan::Wait ⇒ true`, `Embed { child_plan } ⇒
  recurse`, `Agent ⇒ false`; `memory` never suspends. Without this an AiAgentLoop-with-
  Wait-tool would be misclassified suspension-free and run blocking (mid-loop suspend →
  replay double-fire risk).
- `plan.rs is_linear_chain_branch`: add an `AiAgentLoop { breakpoint: false, next_plan,
  error_plan, tools, .. }` arm mirroring the `AiAgent` arm — accept when the loop body is
  suspension-free (no Wait tool; Embed-tool children suspension-free) and
  `!error_route_suspends(error_plan)`; continuation = `next_plan`.
- `branch_parallel.rs chain_next`: add `AiAgentLoop { next_plan, .. } ⇒ Some(next_plan)`
  (else `branch_chain` truncates at the loop).
- `branch_parallel.rs with_next_join`: add an `AiAgentLoop` arm cloning with
  `next_plan = Join` (else it falls to `other => other.clone()` and emits the whole
  continuation, duplicating the merge).
- `plan.rs chain_step_ids`: add `AiAgentLoop` (collect `step_id`; recurse Embed-tool child
  step ids for `branches_independent`).
- Tests: e2e AiAgentLoop-in-branch (tools, no wait) arrival-overlap; `--lib` battery; live.

**T1b — suspending composites** (Conditional/Switch/Edge arm, While/Split body, Embed child
that nests a Wait/Delay):
- `branch_parallel.rs is_suspending_node`: return `true` when the composite's **body**
  suspends. New helper `composite_body_suspends(node)` = `plan_contains_suspension` applied
  to the arms/nested/child/tools but **NOT** the continuation (`next_plan`/`merge_plan`) —
  the continuation is the next depth, classified on its own. (For top-level Wait/Delay this
  stays `true` as before.)
- `plan.rs is_linear_chain_branch`: **relax** the composite arms — drop the
  `plan_contains_suspension(arm) ⇒ return false` guards for Conditional/SwitchRoute/
  EdgeRoute/While/Split/EmbedWorkflow (keep `breakpoint: false`). The branch-level durable
  gate at `plan_branch_diamond:1737` (`plan_contains_suspension(&plan) && !graph.durable ⇒
  decline`) already enforces replay-safety, so accepted suspending composites are always in
  a durable graph.
- `with_next_join` already reconstructs all six composites → no change.
- Tests: e2e durable Conditional-arm-with-Wait and While-with-in-body-Delay in a branch;
  resume-no-double-fire (drain mid-composite-suspend, resume reproduces, zero re-fires,
  merge reads all branches); `--lib` battery; live isolated server.

**T1c — AiAgentLoop WITH a Wait tool** (composes T1a plumbing + T1b classification):
- `composite_body_suspends(AiAgentLoop)` walks `tools` → a Wait tool ⇒ `true` ⇒ pass 2.
- `is_linear_chain_branch` accepts it (durable-gated); `with_next_join`'s `AiAgentLoop` arm
  (T1a) reconstructs it with `next_plan = Join`. Per-turn `{step}.turn.{n}` checkpoints +
  per-call signal id make the mid-loop suspend replay-safe (same as the sequential path).
- Tests: e2e durable AiAgentLoop with a human-in-the-loop Wait tool in a branch; resume;
  battery.

**After Tier 1: every DURABLE DAG fan-out compiles to `ParallelBranches`.** The one shape
still linearised is **non-durable + in-branch suspension** — declined at
`plan_branch_diamond:1737` for the identical reason non-durable *top-level* waits are (a
non-durable workflow cannot replay-safely resume: no checkpoints to HIT, so replay-from-
start re-fires). That is a durability-semantics constraint orthogonal to parallelism — the
CPS scheduler would not fix it either (it also relies on durable checkpoints for the
pre-suspend work) — not a DAG-shape gap. Note it; do not treat it as a fallback to remove.

**Residual (what Tier 1 does NOT give):** an in-branch suspension hard-returns the *whole*
instance, so a sibling branch's work at depths **after** the suspending depth cannot
overlap the suspension (it resumes only when the wait wakes). All agent/HTTP work up to and
at the suspending depth already overlapped. This residual is **identical to the landed
top-level-Wait handling** (which also hard-returns and was accepted as "4c complete"), so
Tier 1 is exactly as "complete" as the shipped state — just widened to composites.

---

### 4.3 TIER 2 — general per-branch segment scheduler (optional concurrency optimisation)

The fully general design — a **resumable coroutine per branch** (`FuturesUnordered`,
hand-emitted) — removes the Tier-1 residual: while branch X is suspended on its wait, sibling
Y keeps running to *its* next yield instead of parking. It is **not** required for coverage
(Tier 1 already parallelises every durable shape); it is justified only when profiling shows
real workloads with **long in-branch waits** (human-in-the-loop minutes–days) AND
**substantial independent sibling work queued after the wait's depth**. Cost is large and
the highest-risk area in the whole effort (durable replay across nested-loop resumption
interleaved with siblings), so it is gated behind demonstrated need.

The rest of §4.1–4.5 below specifies this scheduler (segmentation, per-branch state spill,
`br_table` resume dispatch, multi-wait wake-set). Two clarifications vs. the sketch:
1. It **replaces** the hard-return-inline suspend with a driver loop that yields control
   back to the scheduler, so it must re-emit each composite type in **resumable** form
   (spill live locals across the yield, `br_table` over resumption points incl. loop
   back-edges) rather than reusing `emit_run_plan_mapping`'s blocking lowering — this is the
   bulk of the cost and why Tier 1 (which *reuses* that lowering) is so much cheaper.
2. §4.4's "wake-set quiesce" is the design target; today's landed mechanism is the simpler
   hard-return-inline (§4.2). The scheduler must build the multi-wake set the sequential
   suspend path does not yet expose to more than one pending wait.

#### 4.3.1 Grounded build plan (the runtime substrate confirms it)

Two runtime facts (verified 2026-07-17) fix the design:
- **`emit_entry_suspend_return` (abi.rs:304) hard-`Return`s from `run`.** No stack survives a
  suspend; resume is **replay-from-start** + durable-checkpoint HIT. So the "coroutine
  scheduler" is NOT stackful — "resumable across a suspend" means replay re-drives the
  scheduler and completed steps fast-forward via their checkpoints.
- **The Split window is already a hand-emitted slot state-machine** (`split_parallel.rs`):
  per-item slots with state codes (`SLOT_EMPTY/AGENT_READY/TIMER_PENDING/SETTLED/
  REINVOKE_NOW`), driver loops (`$launch/$drain/$classify/$reinvoke/$rounds`), one shared
  waitable-set, a `pending` counter. **This is the template** for a per-branch scheduler.

Under replay-from-start the scheduler decomposes into three separable problems of
increasing cost/risk — build in this order, each its own commit (unit + e2e + live):

- **T2.0 — planner acceptance + AiAgentLoop plumbing (shared substrate, low risk).** Make
  the two remnant shapes produce `ParallelBranches` instead of linearising: extend
  `plan_contains_suspension`'s `AiAgentLoop` arm to walk `tools` (the required correctness
  fix); add `AiAgentLoop` arms to `is_linear_chain_branch` / `chain_next` / `with_next_join`
  / `chain_step_ids`; relax the composite-arm suspension guards in `is_linear_chain_branch`.
  Wire initially through the EXISTING depth-wavefront (hard-return-inline) so it is testable
  immediately — a transient intermediate on this dev branch, replaced by T2.1's scheduler
  before completion. This is the substrate every later tier needs.
- **T2.1 — intra-invocation SEGMENT SCHEDULER (case A; the FuturesUnordered core). LANDED
  (T2.1a `f1b5ba88`, T2.1b `4e2ab69e`).** `emit_branch_scheduler` replaces the depth-wavefront
  for branches that are chains of async Agents (T2.1a) and/or sync steps (T2.1b). Each branch
  carries CURSOR(slot+40)/SCHED(slot+44)/SUBTASK(slot+20) and drives independently
  (NEEDS_LAUNCH→PENDING→NEEDS_ASSEMBLE→DONE); the driver `ws.wait`s for ANY settle and advances
  only that branch. Per-branch cursor dispatch = if-chains on cursor (not br_table — chains are
  short). `emit_branch_launch` got `sched_pending_flag: Option<u32>`. Composites still use the
  wavefront (nested-window would clobber the scheduler's live SLOTS/PENDING/WS). Verified:
  battery 109/109, live server (unbalanced a=3/b=1 chains → `a=A3,b=B1`). No durability change.
- **T2.2 — cross-suspension progress (case B; the actual Tier-2 value). IN PROGRESS.** A branch
  reaching a TOP-LEVEL Wait/Delay marks BLOCKED (a new drive state) instead of hard-returning;
  the scheduler keeps driving other branches to completion/their-own-block; only when no branch
  is RUNNABLE or PENDING-on-subtask does it hard-return suspended. Replay re-drives; completed
  branches HIT; blocked branches re-check their waits.
  - **FINDING (from wait.rs/abi):** the suspend outcome is
    `suspended(on-signal{signal-id, deadline})` — **singular**. A true *aggregated* multi-wake
    set (wake on ANY of N blocked signals in one suspend) needs an ABI/host extension (multi-
    signal wake list + host waker fan-in). Split into:
    - **T2.2a (no ABI change): siblings-complete-before-suspend.** The scheduler drives every
      non-blocked branch to DONE, then hard-returns suspended on the FIRST blocked branch's
      wake (reusing `emit_entry_suspend_on_signal` / the delay deadline). Multiple blocked
      branches serialize their suspend/resume cycles — but every non-blocked sibling is already
      DONE and checkpointed before the first suspend, which is the whole payoff over T2.0
      (where a suspend at depth d parks siblings' depth>d work). Requires: a "check-satisfied,
      else register-wake + mark BLOCKED, do-NOT-suspend" variant of the wait/delay lowering,
      threaded into the drive loop; `schedulable_branches` accepts top-level Wait/Delay chain
      nodes (durable-gated); the scheduler's terminal suspend picks the first BLOCKED branch.
    - **T2.2b (ABI multi-wake, optional): true aggregation.** Extend the suspend outcome to a
      wake *list* so one suspend covers all BLOCKED branches; host waker relaunches on any.
      Deferred behind T2.2a proving the shape.
  - Durability risk lives here — extend the Split double-fire adversarial battery to
    interleaved multi-branch suspend/resume (drain mid-schedule with 2+ branches blocked;
    resume reproduces; zero re-fires; merge reads all).
- **T2.3 (C) — resumable composites (deferred, exotic, largest).** Only this re-emits
  While/Split/Embed/AiAgentLoop bodies as segmented resumable state machines so composite
  INTERNAL async interleaves with siblings AND a composite-NESTED suspension participates in
  the scheduler (instead of hard-returning inline as in T2.0–T2.2). Full CPS transform: spill
  every live local across a yield, `br_table` over resumption points incl. loop back-edges,
  nested-parallel-window budget review (`PARALLEL_POOL_MAX`). Gate behind a demonstrated
  workload T2.0–T2.2 don't cover; composite-nested suspension stays hard-return-inline
  (Tier-1-equivalent, correct) until then.

Honest coverage line: **T2.0 alone** already removes every linearisation remnant for durable
DAGs (via hard-return-inline, = Tier 1). **T2.1+T2.2** add yield-granular interleaving and
post-suspend sibling overlap for top-level waits — the practical, buildable Tier 2. **T2.3**
is the only piece that touches composite internals and is deferred behind proven need.

### 4.1 Segmenting a branch

Split each branch's linear plan into **segments** at async boundaries. A segment is a
straight-line run from one yield point to the next. For a linear branch this is a
**fixed list** of segments known at compile time (no re-entrant loop → no dynamic
resumption count), which is exactly why 4b restricts to linear chains.

Each branch gets:
- a **segment pointer** (which segment to run next),
- a **per-branch state block** in linear memory holding its live values across yields:
  its source cursor, its step-context accumulator position, and any locals live across
  the yield. Because the branch is linear, the live set is bounded and static — a
  fixed-size state block per branch (sized at compile time), not a general spill of all
  ~30 `DIRECT_*` locals.

### 4.2 The scheduler loop

Replace the single linear body with a driver:

```
init: every branch → segment 0, state block zeroed
loop:
  for each runnable branch b:
    run b's current segment until its next yield:
      - agent invoke  → async-lower into slot b, join subtask, mark b SUSPENDED
      - delay         → timer subtask into slot b, join, mark b SUSPENDED
      - wait-signal   → register wake, mark b WAITING (see §4.4)
    if segment was terminal (reached Join) → mark b DONE
  if no runnable branch and any SUSPENDED: ws.wait; on completion,
     find the branch owning the settled waitable, store result into its state,
     advance its segment pointer, mark it runnable
  until all branches DONE
emit merge_plan
```

This is `FuturesUnordered`, hand-emitted: the waitable-set already gives "wait for any";
the branch↔waitable mapping is a small table indexed by slot.

### 4.3 Durability & the wake-set

- **Per-step checkpoints** inside a branch work unchanged (key-addressed by step id).
- **Scheduler state** (which branch at which segment) need **not** be persisted: on
  replay-from-start the scheduler re-runs, each completed step HITs its checkpoint, and
  every branch fast-forwards to its first uncompleted step. The segment structure is
  deterministic (same graph), so replay reconstructs the exact in-flight frontier. This
  is the same replay contract the sequential path relies on — no new persisted state.

### 4.4 Waits inside a parallel branch

A branch `WaitForSignal`/durable Wait is a suspension point that, sequentially, exits
the whole instance. In a parallel region with siblings in flight, one branch waiting
must **not** tear down the instance under the others. Use the **wake-set** contract the
ABI already exposes (`suspended(list<wake>)`): when the scheduler has no runnable branch
and one-or-more branches are WAITING (and none SUSPENDED on a resolvable subtask), it
**quiesces** — every in-flight subtask having settled — and suspends the instance with a
wake-set covering *all* pending branch waits (signal ids + any timer deadlines). On
wake, replay-from-start re-drives the scheduler; satisfied waits read their persisted
signal (non-destructive custom-signal read, already landed —
`project_wait_resume_durability_fixed`) and their branches proceed. This reuses the
§4.4 quiesce policy from the sibling doc rather than inventing a new mechanism.

**Ordering subtlety:** a branch that would block on a wait should be driven **last** in
a scheduler pass, so siblings that can make progress do so before the region quiesces.
Otherwise a wait early in the branch order needlessly stalls a ready sibling until the
next wake. This is a scheduler heuristic, not a correctness requirement.

### 4.5 Effort

Medium–large. The segment scheduler + per-branch state blocks + branch↔waitable table
are new. The wake-set quiesce reuses existing suspend/resume plumbing but must be wired
to *multiple* pending waits at once (today a sequential wait is singular).

---

## 5. Phase 4c — LANDED (nested loops / splits / embeds run as blocking composites)

**Superseded outcome.** 4c did **not** need the CPS transform this section originally
proposed. A branch containing a nested While/Split/EmbedWorkflow/Conditional/Switch/AiAgent
runs as a **blocking composite node** in the wavefront (§4.0.1): at its depth it emits via
`emit_run_plan_mapping(with_next_join(node))` — the ordinary sequential lowering, its
internal agents on the sync invoke — so exactly the composite runs and the branch's
top-level linear steps still overlap. In-branch Wait/durable-Delay ride the deferred-suspend
two-pass (§4.0.2/§4.2). Nested Split-in-branch reuses the Split window's own pooling; it is
not a *nested* parallel window (the composite's Split runs blocking, its items pooled
internally), so the `PARALLEL_POOL_MAX` budget is not multiplied.

The only shapes that would need the full CPS transform (a *dynamic* resumption count from a
yield *inside* a loop) are the two remnants in §4.1 — and even those are covered for
correctness by Tier 1 (§4.2) via hard-return-inline + replay; the CPS transform (§4.3 /
Tier 2) is a concurrency optimisation over them, not a coverage requirement.

### 5.1 Effort — actual

Tier 1 (§4.2): small, three commits, reuses the landed window. Tier 2 (§4.3): large,
gated behind profiling need; the adversarial replay/double-fire battery (per Split backoff)
must be extended to nested-loop resumption interleaved with siblings before shipping it.

---

## 6. No opt-in, no permanent fallback — parallel by default

**Decision (supersedes an earlier opt-in proposal):** branch parallelism is **on by
default** for every re-converging/fan-out shape the compiled phase supports. There is
**no** `parallelBranches` flag and **no** permanent sequential-fallback opt-out. Our
workflows are DAGs; a DAG's parallel schedule is unambiguous (§8), so there is nothing
for an author to opt into.

The earlier "side-effect ordering changes" objection does **not** apply: independent
branches of a DAG are, by definition, not ordered with respect to each other. Any two
steps a workflow genuinely needs ordered are connected by an edge and therefore are
**not** independent branches — they never fan out in parallel in the first place. Making
independent branches concurrent cannot change any ordering the graph actually specifies.

The only "fallback" is **transitional**, not an opt-out: until a phase lands, a shape that
phase does not yet emit (e.g. a multi-step branch before 4b) still lowers through today's
`direct_execution_order` linearisation so the workflow keeps compiling and running. Each
phase widens the parallelised set; **by 4c every DAG fan-out runs in parallel** and the
linearisation path is dead code for fan-out.

---

## 7. Phase gating (transitional only)

Build `ParallelBranches` whenever:

1. `parallel_enabled` — async-typed ABI v2 (the sync ABI cannot block in `ws.wait`; it is
   the only real hard gate, and every current build path is v2).
2. A step has ≥2 unconditional normal-flow successors (a fan-out).
3. Every branch is within the **currently-landed phase's** shape support:
   - 4a: each branch is exactly one Agent → sink.
   - 4b: each branch is a linear chain (no nested While/Split).
   - 4c: any subgraph.

A fan-out whose branches exceed the landed phase's support **temporarily** lowers through
`direct_execution_order` linearisation (it still runs, just sequentially) until the phase
that supports it lands. This is a build-order artifact, not a user-facing opt-out: once 4c
lands, gate (3) is always satisfied and every fan-out is parallel. No warning is emitted —
there is nothing for an author to change.

---

## 8. Join / merge semantics (reconvergence is a VALIDATED invariant we exploit)

**Decision:** the compiler already **validates** that every unconditional fan-out
re-converges at a single merge (the E073 guard in `normal_flow_plan`; the support
analyzer's `backbone_topologically_linearizable` / `step_branches_remerge`). Rather
than doing the work to relax that to a weaker "single `Finish` / multi-sink" rule, we
**keep the reconvergence guarantee and build parallelism on top of it.** Every parallel
fan-out therefore has a single, statically-known merge point `M` — which is exactly the
structure the launch → drain → assemble → merge window wants.

Reconvergence is not fundamentally required for DAG parallelism (a multi-sink DAG is
schedulable too), but since validation already enforces it, exploiting it is strictly
simpler and lower-risk than generalising: no scheduler needs to track arbitrary sinks,
and the shared continuation is always a single `merge_plan`.

- **E073 stays.** A fan-out must re-converge; `direct_find_merge_point` returns the merge
  `M`, and each branch ends in `Join` at `M`.
- **The merge `M` sees ALL branch outputs.** Assemble appends each branch's result to the
  `steps` context in declaration order, so `M` reads `steps.B.*`, `steps.C.*`, … exactly
  as the sequential linearisation would have produced. `merge_plan` runs once, after all
  branches (mirrors `Conditional`/`SwitchRoute`).
- **Failure policy:** a branch failing fatally (after its own onError) fails the workflow —
  but only after in-flight siblings **quiesce** to a safe point (the drain waits for all
  launched subtasks, as Split does), so there are no orphaned subtasks.

---

## 9. In-guest constraint (reaffirmed)

Nothing here introduces host-orchestrated fan-out. The host services the **same**
async imports Split already uses (agent invoke, host-io HTTP, timers); the branch
scheduler, slots, waitable-set, and merge all live in the emitted `workflow.wasm`.
`feedback_no_host_orchestrated_fanout` remains satisfied.

---

## 10. Risks

- **Determinism of assembly/debug** — fixed-order assemble keeps `steps.*` and summaries
  deterministic even though completion order varies. Must be enforced, not incidental.
  (Independent branches have no graph-specified order to preserve, §6/§8; but the emitted
  `steps.*` context and debug-event sequence must still be a stable function of the graph.)
- **Join at the validated merge** — every fan-out re-converges at a single merge `M` (§8),
  and the window drains all branches before `M` runs; no branch can be left in flight.
- **Nested parallel budget** (4c) — nested Split-in-branch multiplies in-flight subtasks;
  needs pool-sizing review (`PARALLEL_POOL_MAX`) and a cap.
- **Durable replay across interleaved branches** (4b/4c) — highest-risk. Key-addressed
  checkpoints make it *tractable*, but the double-fire adversarial battery (per Split
  backoff) must be extended to interleaved multi-branch replay before shipping durable
  4b/4c.
- **Wake-set with multiple simultaneous waits** (4b) — today's suspend path handles a
  single pending wait; the multi-wait wake-set path needs its own resume test.

---

## 11. Testing

Extend `crates/runtara-workflows/tests/direct_wasm_execute.rs`, mirroring the Split
battery:

- 4a: `parallel_branches_single_agent_overlap` (arrival-span proof, like
  `_parallel_split_http_overlap`), `parallel_branches_merge_reads_all`,
  `parallel_branches_branch_error_routes_via_onerror`,
  `parallel_branches_durable_resume`, `parallel_branches_rate_limited_backoff_overlaps`.
- 4b: `parallel_branches_multistep_chain`, `parallel_branches_wait_quiesce_resume`
  (multi-wait wake-set), `parallel_branches_durable_replay_no_double_fire`.
- Reconvergence (§8): `parallel_branches_merge_reads_all` — the merge `M` sees every
  branch's `steps.*` output regardless of completion order.
- Emitter `--lib` tests (`cargo test -p runtara-workflows`) per
  `feedback_verify_emitter_lib_tests`, plus a **live-server compile/execute** run
  through the production HTTP API per `feedback_always_e2e_verify`.

---

## 12. Recommendation

Build order is **4a → 4b → 4c**, each its own commit with unit + e2e + live-server
verification. 4a (single-Agent branches) is a small generalisation of the existing Split
window and covers the dominant "parallel independent calls then join" use at low durability
risk. 4b adds the resumable multi-step branch scheduler. 4c adds nested loops/splits inside
branches. The goal is **complete** parallelism: by 4c every DAG fan-out runs concurrently.

Parallelism is **on by default** — there is no opt-in flag. The only transitional
"fallback" is that, until a phase lands, shapes beyond its support still linearise so they
keep running (§6/§7); that path is dead code for fan-out once 4c lands. Correctness is
preserved because assemble/scheduler *is* the sequential per-branch lowering, and
independent DAG branches have no graph-specified ordering to disturb (§8).
