# Graph Replay — animated historical execution over the workflow graph

**Status:** planned, not started. Handoff doc for a dedicated session.
**One line:** In the invocation view, add a **Timeline ⇄ Graph** switch where the Graph plays a *past* run as an
auto-laid-out animated DAG — nodes light up running → done / failed / suspended in the exact recorded order and timing,
concurrent branches glow concurrently — the same delight as the parallelism tutorial, but driven by **real historical
step-events**.

---

## 1. Why this works (the load-bearing insight)

We do **not** need to re-derive the scheduler's behavior to show parallelism. The recorded step-events already carry
per-step `timestamp_ms` + `duration_ms`. **Overlapping `[start, end]` intervals *are* concurrency.** So a replay that
places each step on a clock and lights the graph node during its interval reproduces the real parallel structure
exactly — where branches overlapped, where the run serialized, where it stalled/suspended, where it failed. The
animation is a faithful projection of recorded data, not a simulation.

This is the difference from the tutorial artifact: the tutorial *choreographs* an idealized run; Graph Replay
*plays back* a specific instance's actual timeline.

## 2. What already exists (reuse, don't rebuild)

Grounded in `crates/runtara-server/frontend`:

- **Graph lib:** `@xyflow/react` v12 (`package.json`) — already the Canvas renderer. Reuse it **read-only** for replay.
- **Timeline model:** `src/features/workflows/stores/timelineStore.ts` already builds a hierarchical step model with
  `minTimestamp` / `maxTimestamp` / `totalDuration`, an `expandedStepIds` set, and a per-`parentScopeId` children cache
  (`childrenCache`) — i.e. Split/While iteration nesting is **already modeled**. `types/timeline.ts::HierarchicalStep`
  is the per-step shape. **This is the replay clock's data source.** The Graph view is a *second renderer* of the same
  model + a shared clock; the Timeline view (`WorkflowEditor/TimelineView.tsx`) is the first.
- **Events source:** `getStepEvents` (`queries/index.ts:575`) → `GET /workflows/{wf}/instances/{inst}/step-events?sortOrder=asc&limit=1000`.
  Event fields (`types/step-events.ts`): `eventType` (`custom` | `completed` | `error` | …), `subtype`
  (`step_debug_start` | `step_debug_end` | `breakpoint_hit` | `suspended` | AI/tool subtypes), and `payload` carrying
  `step_id`, `step_name`, `step_type`, `duration_ms`, `timestamp_ms`, `scope_id`, `loop_indices`, `outputs`, and error info.
- **Node visuals:** `WorkflowEditor/CustomNodes/*` (BasicNode, ConditionalNode, SwitchNode, AiAgentNode, EventNode).
  Reuse them; add a `data.replayState` branch (same approach breakpoints use for the red dot in `BaseNode.tsx`).
- **Detail panel:** `components/DebugStepInspector.tsx` — reuse to show a node's recorded inputs/outputs/error on click.
- **View toggle host:** `WorkflowEditor/index.tsx` owns the Canvas / TimelineView switch — extend it.
- **Invocation entry points:** `features/invocation-history/*` + `pages/WorkflowHistory/index.tsx`, and
  `ValidationPanel/HistoryPanelContent.tsx`.

**Net:** the data pipeline (events → timeline model with bounds + scope nesting) already exists. This feature is mostly a
**new renderer of an existing model** + **auto-layout** + **a shared transport clock**.

## 3. New dependency

Auto-layout is the one genuinely new capability (no layout lib present today).
- **Recommend `@dagrejs/dagre`** (small, battle-tested layered/Sugiyama layout; the canonical React-Flow-auto-layout
  recipe). `elkjs` is the heavier alternative (better orthogonal routing, async worker) — pick it only if dagre's
  crossing-minimization proves insufficient on real graphs. Decide in P2 after trying dagre on the 9 tutorial workflows.

## 4. UX

- **Segmented switch** in the invocation view: **Timeline | Graph**. Both are driven by one replay clock; switching
  preserves the current playhead time.
- **Transport bar** (shared): `▶ Play` / `⏸ Pause`, `↻ Restart`, a **scrubber** (seek to any moment), **speed**
  (1× / 2× / 4× / 8× — real runs can be long), and an elapsed / total readout.
- **Graph view:** auto-laid-out DAG (topological, left→right). Node states animate through the recorded timeline:
  - `idle` (not yet reached) → faint.
  - `running` (t within its interval) → glow/pulse (reuse the tutorial's `is-run` treatment).
  - `done` (success end) → green tick. `failed` → red. `suspended` / `breakpoint_hit` → amber pulse.
  - `skipped` (branch never taken, e.g. false Conditional arm — no events) → dashed/greyed.
  - Edges flow when their upstream node completes and the downstream begins.
  - **Concurrency is automatic:** multiple nodes glow at once when their intervals overlap.
  - Split/While: the node shows an **iteration counter**; expanding drills into per-scope child intervals (the
    timelineStore `childrenCache` already provides these).
- **Timeline view:** the existing `TimelineView`, plus a **playhead line** at the clock's `t`; scrubbing one view moves
  the other; clicking a timeline row highlights + centers the graph node (and vice versa).
- **Inspector:** click a node → `DebugStepInspector` with that step's recorded inputs / outputs / error / timing.

## 5. Architecture (new files under `features/workflows/components/Replay/`)

- `useReplayModel(workflowId, instanceId)` — React Query hook. Fetches step-events (paginate past 1000 for long runs)
  + the **versioned** graph (see §6 gotcha), and folds them into a normalized model:
  `{ steps: Map<stepKey, {stepId, scopeId, loopIndices, startT, endT, status, stepType, hasChildren}>, edges, t0, tEnd,
  order }`. Reuse / share `timelineStore`'s bounds + children cache rather than re-deriving. `stepKey = stepId (+scopeId+loopIndices)`.
- `useReplayClock({ tEnd, speed, pacing })` — play/pause/seek/speed state machine driving `currentT` via
  `requestAnimationFrame`. Honors `prefers-reduced-motion` (snap to final, no tween). Two **pacings**: `real-time`
  (true `timestamp_ms`) and `even` (each event evenly spaced — default, legible); plus **idle-gap compression** for
  suspended runs (collapse multi-hour parked gaps to a fixed marker; `timelineStore.totalDuration` gives the span).
- `deriveFrame(model, t)` — pure function: given `t`, return `{ nodeStates: Map<nodeId, state>, activeEdges: Set }`.
  Both Graph and Timeline render from this. Unit-testable, no React.
- `layoutDag(nodes, edges)` — dagre wrapper → `{ id → {x,y} }`. Break While/loop back-edges for layering, draw them as
  curved back edges. Memoized per (graph version).
- `<ReplayGraph>` — `@xyflow/react`, read-only (`nodesDraggable/nodesConnectable/elementsSelectable=false`), positions
  from `layoutDag`, each node's `data.replayState` from `deriveFrame`. Edge flow via an `animated`/className toggle.
- `<ReplayTimeline>` — wraps existing `TimelineView` + a shared playhead.
- `<ReplayTransport>` — the play/scrub/speed/pacing controls.
- `<ReplayView>` — container: the Timeline|Graph toggle, the transport, the inspector; owns the clock and feeds both.
- Extend `CustomNodes` (or a thin `ReplayNodeChrome`) to render the five replay states; add replay-state CSS
  (mirror the tutorial's state palette so product + tutorial read as one system).

## 6. Hard parts / edge cases (call these out — they're where it gets real)

1. **Versioned graph, not the editor graph.** An instance ran `usedVersion` (on the instance record). Lay out **that
   version's** graph, not the current draft. Fetch the versioned execution graph; defensively handle event `step_id`s
   absent from the graph (render as orphan or skip).
2. **Instant steps.** Sync steps have ~0 `duration_ms`. Clamp to a **minimum visual duration** for legibility
   (visual-only; label honestly). Don't let 0-duration nodes flicker invisibly.
3. **Suspended runs with huge gaps.** A parked run can wait hours between suspend and resume. Default to **even pacing**
   / **idle-gap compression** with a "parked · 3h 12m" marker; offer a real-time toggle.
4. **`track_events = false` instances.** No step events → replay impossible. Detect and show an explicit empty state
   ("Event tracking was off for this run — nothing to replay"), not a blank graph. Replay is available only for
   event-tracked instances.
5. **Composites.** Split/While/Embed/Conditional/Switch — decide representation: annotated node with iteration counter
   (recommended MVP) vs expandable container. `scope_id` + `loop_indices` group per-iteration events (already in
   `childrenCache`). Loops make a node **pulse per iteration**.
6. **Big graphs / long runs.** `layoutDag` + React Flow handle large graphs; virtualize the timeline; the step-events
   endpoint is `O(events × payload)` and can be slow (see the `/steps` slow-query note) — fetch **compact events**
   (ids + ts + status) for the clock and lazy-load a node's full payload on click.
7. **Failed / partial runs.** Show where it stopped; unreached nodes stay idle/greyed; the failed node is red with its
   error in the inspector.

## 7. Backend

**MVP: no backend change** — the step-events endpoint already returns everything. *Optimization (optional):* add a lean
`GET …/instances/{id}/replay` that returns `{ graph(version), compactEvents[], status }` in one call with payloads
elided (lazy-fetched per node) — directly addresses gotcha #6. Defer unless the existing endpoint is too slow on real
runs.

## 8. Phasing

- **P1 — Model + clock (no UI).** `useReplayModel` + `deriveFrame` + `useReplayClock`. Unit tests with fixture events
  for: sequential, parallel-overlap, failed, suspended/resumed, split-with-iterations. Assert `deriveFrame` yields
  concurrent `running` states where intervals overlap.
- **P2 — Auto-layout.** `layoutDag` via dagre; render the versioned graph read-only in React Flow; verify layout is
  clean on the 9 tutorial workflows' real instances (they exist on :7001 with events).
- **P3 — Animation + transport.** Node replay states + edge flow driven by the clock; play/pause/scrub/speed; pacing
  toggle; reduced-motion.
- **P4 — Timeline ⇄ Graph toggle + shared playhead + inspector.** Wire both views to one clock; click-to-highlight both
  ways; `DebugStepInspector` on node click.
- **P5 — Polish.** Even-pacing vs real-time, idle-gap compression, empty/failed states, big-graph perf, light+dark,
  theme parity with the tutorial's state palette.

## 9. Verification

- Unit: `deriveFrame` / model builder against fixtures (each shape); `layoutDag` determinism.
- E2E on :7001: the 9 tutorial workflows already have real instances with events — open replay, assert nodes transition
  and the fan-out/wide/unbalanced runs show **overlapping running states**; verify a failed run and the durable
  suspend/resume run (rung 09).
- Playwright screenshots for the PR (light + dark).

## 10. Future / adjacent

- **Live mode:** the same graph animates a *currently-running* instance in real time (clock follows "now") — unifies
  live monitoring with historical replay from one component.
- **Breakpoint debugging reuses this graph:** pause overlay + the parked node highlighted (ties into the
  breakpoints-in-parallel-branches work already landed).
- Diff two runs; export replay as GIF/video.

---
*Reference: the parallelism/durability tutorial artifact (private) demonstrates the target animation language — node
state palette (running glow / done tick / checkpoint ring / parked pulse / memoized dashed), edge token flow, and the
transport feel. Match it so the product and the tutorial read as one visual system.*
