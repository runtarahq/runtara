# Parallel graph branches (heterogeneous fan-out) — detailed plan

Status: **4a LANDED** (4a.1 non-durable + 4a.2 durable single-Agent branches run
concurrently); **4b/4c pending**. Sibling to `docs/wasip3-parallelism.md`, which
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

A branch is now more than one async call, so it must become a **resumable coroutine**:
run until its next async yield point (agent invoke, delay, wait, embed), suspend,
resume on completion. This is the hand-emitted callback-ABI / segment scheduler.

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

## 5. Phase 4c — arbitrary subgraph branches (nested loops / splits / embeds)

**Scope:** a branch may contain a **nested While/Split/EmbedWorkflow** — a yield point
*inside a loop*. Now the resumption count is dynamic and loop state (counter,
accumulator, per-iteration scope) must be spilled and the loop re-entered at the right
iteration on resume. This is the **full CPS transform**: every live local across a yield
spilled to the branch state block, and the branch body restructured into a top-level
`br_table` dispatch over resumption points, including loop back-edges.

Nested Split inside a parallel branch also means **nested parallel windows** (a window
whose items are themselves launched from within a suspended branch), which multiplies
the in-flight subtask budget and the pool sizing (`PARALLEL_POOL_MAX`, currently 4).

**Recommendation:** defer 4c until 4a/4b demand proves it out. Most real graphs express
"parallel work with a loop inside" as a **Split with `parallelism > 1`** (already
shipped) rather than a hand-drawn fan-out of loop-containing branches. Ship 4a, measure,
then decide 4b/4c.

### 5.1 Effort

Large. This is the general in-guest async scheduler with full state spilling. Highest
risk area is durability correctness across nested loop resumption interleaved with
sibling branches — needs the adversarial replay/double-fire test battery that Split's
backoff work established, extended to nested scopes.

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
