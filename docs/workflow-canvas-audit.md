# Workflow canvas UX/UI audit

Audit date: 2026-10-11. Scope: the workflow editor's Canvas view
(`crates/runtara-server/frontend/src/features/workflows/components/WorkflowEditor`),
checked against live workflows on a local server: a linear chain (Landing Demo),
an AI Agent with tools and memory (AI Assistant Showcase), a 20-step branching
workflow (tenant-provision) and nested fan-out with a While container.

Goal: make the canvas the default way to view a workflow, and make step
configuration readable on it without opening each step's editor.

## Summary

The canvas looked broken for one root cause: **the layout and the cards
disagreed about how big a card is**. The layout trusted sizes saved with the
workflow (132×36 from an older card design) and a grid rule that snapped 52px
cards to 48px. Meanwhile, the cards always drew themselves at 216×52. So cards
overflowed their layout boxes, covered each other, and edges ran underneath them.
The AI Agent card had the same problem: its content was about twice as tall as
its box. Separately, fit-to-view could never zoom out far enough to show a long
workflow, and it ran before nodes were measured. The canvas also showed a step's
name and type only. Seeing its inputs meant opening the editor.

This change fixes the alignment and fit problems, makes Canvas the default view,
and adds step details to the canvas. The remaining findings are listed under
[Open findings](#open-findings), in priority order.

## Fixed in this change

| # | Severity | Finding | Fix |
|---|----------|---------|-----|
| F1 | Critical | Cards overlapped and edges ran under them. `applyStoredNodeVisualState` applied saved width/height to every node, but only containers and notes are resizable. The layout packed 216px cards into 132px slots. | Only containers and notes keep saved sizes. The layout now gets every other node's size from `CustomNodes/node-size.ts`, the same function the card components render from. |
| F2 | High | Cards were 52px tall, which breaks the grid rule in `workflow.ts`: heights must be odd multiples of 12 so node centres line up. The layout snapped cards to 48px, so handles and edges sat 2px off. | Compact cards are 60px. Every computed height (detail lines, Switch cases, AI Agent tools) is rounded up to an odd multiple of 12, and a unit test enforces this. |
| F3 | High | The AI Agent card drew ~186px of content in a 96px box. Its Memory section spilled out, its onError handle sat in the middle of the card, and it covered the nodes below. | The card height is computed from the tool count, and each row has a fixed height. While a run is executing, the "Add tool" and "Add memory" rows are hidden but keep their space, so the card's height doesn't change. |
| F4 | Medium | A Switch in value mode rendered 36px tall while the layout reserved 52px. | The Switch takes its size from the shared sizing function. |
| F5 | High | Fit-to-view could not fit long workflows. React Flow's default minimum zoom of 0.5 can't fit tenant-provision (~4,600px wide), so both ends were clipped and the Fit button did nothing. The initial fit also ran on a 100ms timer before nodes were measured. A two-step workflow was zoomed to 1.9×. | Minimum zoom is 0.1 and fitting never zooms past 100%. Nodes get their size up front (`measured`), so React Flow knows they're initialized and the fit runs on real bounds. |
| F6 | Medium | Fitting a long workflow made its text unreadable (~0.25 zoom). | The first view fits the whole workflow only if the zoom stays at 0.8 or above. Otherwise it opens at 0.8, anchored on Start. The Fit button still shows everything. |
| F7 | Low | The view was re-fitted whenever any node moved or changed size, so it would jump while a step was being edited. | The view is re-fitted only when steps or edges are added or removed, or the detail level changes. |
| F8 | Product | Timeline was the default view. | Canvas is the default. Each viewer's last choice is remembered (`editor-preferences.ts`). |
| F9 | High | The canvas showed only a step's name and type, plus agent/capability. Its inputs were visible only in the editor, and when shown they echoed the DSL (`instance_id data.id`, `data { … }`). | **Details** mode (the default) shows each card's inputs as a small two-column table, at most three rows plus "+N more". Labels come from the capability's display names. References name their source ("Input › id", or the source step's name), and an object lists the fields it sets. Conditions read as sentences (`Input · stage is "intake"`), and the header is just the name plus an icon-only "Agent disabled" warning. **Compact** keeps cards name-only; there, hovering a step shows all its inputs. The toggle is at the bottom right of the canvas. |
| F10 | Medium | Moving between step editors needed a close and a double-click each time. | While an editor is open, a single click on another step switches the editor to that step. Unsaved edits stay staged on the step where they were made. |
| F11 | Medium | Card memo comparators ignored some data changes: a Basic card ignored `description`, and a Conditional card ignored `inputMapping`. Cards could show stale text after an edit. | The comparators now include those fields, plus the fields the detail rows read. |

Verification:

- Unit tests: `node-size.test.ts` includes a regression test for the Landing
  Demo graph with its stale 132×36 sizes. `step-details.test.ts` covers the
  detail rows. The full workflow suite passes.
- Live check: on all four workflows above, every card's DOM box equals its
  layout box.

## Open findings

Ordered by impact.

1. **Editing is hard to discover and has no keyboard path.** Double-click is
   the only way to open a step's editor, and `disableKeyboardA11y` is on. The
   hover preview now says "Double-click to edit", but that's a hint, not a fix.
   Recommend: Enter or Space on a focused card opens its editor, with a visible
   focus ring, and arrow keys move between connected steps.
2. **Long linear workflows become very wide.** The left-to-right layout puts a
   20-step chain about 4,600px across, so only ~4 steps are readable at once,
   and there is no minimap. Recommend a `MiniMap`, then an option to wrap long
   chains or lay them out top to bottom.
3. **The header overlaps the canvas.** The view tabs (top left), the title
   (centre) and the toolbar are absolutely positioned over the canvas, and they
   collide below ~1,100px of canvas width. Recommend a real header row.
4. **The docked editor covers the graph.** By design it overlays the canvas,
   because shrinking React Flow clips nodes. Opening it does not pan the
   selected step into the visible area, and it can hide that step. Recommend
   centring the edited node in the uncovered part of the canvas when the panel
   opens.
5. **Agent steps don't show which connection they use.** Detail rows skip
   `connectionId` because it's an opaque id. Show the connection's name (needs a
   lookup from the connections query).
6. **EmbedWorkflow cards show the slug, not the child workflow.** Show the child
   workflow's name and link to it. Today Ctrl/Cmd-click navigates, but nothing
   on the card says so.
7. **The Conditional condition is truncated to one line.** Details mode could
   wrap it to two lines on the card; today it's complete only in the hover
   preview.
8. **Containers look selected at rest.** Split and While containers draw a
   primary-coloured border with corner handles, which is the same treatment as
   selection.
9. **Inserting a step between two others is hidden.** On the canvas, the "+" on
   an edge appears only after you select the edge, and handle buttons appear
   only on unconnected handles. The timeline shows "Add step", "Add parallel
   branch" and "Add error" everywhere. Recommend a hover "+" on each edge
   midpoint.
10. **Card text is small.** It's 9–12px at 100% zoom, so typically 7–10px after
    fitting. Consider a larger base size and fewer chips per card.
11. **Notes probably can't be resized.** `handleNodesChange` drops every
    `dimensions` change, which may include the note resizer's output. Needs
    verification.
12. **"Start" is ambiguous.** A step named "Start" sits next to the virtual
    Start indicator (seen in Nested Fan-Out).
13. **Stale positioning constant.** The Switch's pending-node placement still
    assumes the old card centre (`nodeCenter = 36`). This only affects where a
    new step first appears, because the canvas is auto-laid out.
14. **No hover preview on AI Agent cards or containers.** Both render their own
    bodies. A preview of the AI Agent's prompt and model would help.

## Design notes

- `CustomNodes/node-size.ts` is the only source of rendered sizes for
  fixed-size nodes. If you add a node type or change a card's body, update it
  there, and keep heights at odd multiples of 12.
- `step-details.ts` turns node data into label/value rows. It stays pure
  because the layout calls it, before render, to size the cards.
- `CanvasDisplayContext` defaults to compact cards with no previews, so the
  replay graph (which reuses `BaseNode`) is unchanged.
