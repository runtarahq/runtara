# Frontend CSS & Styling Audit — 2026-07-24

Deep audit of styles and class usage in `crates/runtara-server/frontend` (Vite + React + Tailwind 3.4.17 + shadcn conventions). Eight audit dimensions were swept in parallel — dead CSS, config integrity, semantic-token drift, duplicated class strings, inline styles, arbitrary values, component reimplementation, dark mode — plus a completeness sweep. Every finding below survived an independent adversarial verification pass (74/75 raw findings confirmed; several were confirmed by recompiling the CSS with the project config and diffing the emitted rules).

**Scope:** `src/` (features, shared, lib, router), `index.html`, `tailwind.config.js`, `postcss.config.js`, `components.json`. Excluded: `node_modules`, `dist`, `coverage`, `storybook-static`, `src/generated`.

## Health snapshot

| Metric | Value |
| --- | --- |
| Semantic-token class usages vs raw palette | 2,315 vs 992 (~70/30) |
| Raw status-color usages vs status tokens | 448 vs 22 (~20:1) |
| Files with raw palette but zero `dark:` variants | 30 of 85 |
| Inline-style sites (justified / should-be-classes / duplicates-token) | 73 (40 / 21 / 12) |
| Arbitrary `[...]` values (excl. data/group variants) | 365 across 110 files |
| Hand-rolled `animate-pulse` skeletons vs shared `Skeleton` imports in features | 71 vs 0 |
| Inline lucide `Loader2` spinners (distinct class variants) | ~60 (25+) |
| Dead CSS variables (of 83 defined) | 13 |
| `@keyframes` referenced but never defined | 1 (`loading`) |

---

## 1. Broken or actively misleading (fix first)

These are not style nits — they change what users see or make the theme lie to its maintainers.

### 1.1 Radix toast system is headless — 7 features fire toasts that never render
`src/shared/hooks/useToast.ts` is the classic shadcn Radix-toast store, but the `toaster.tsx` component that would render its state **does not exist in the repo**, and nothing mounts `ToastProvider`/`ToastViewport`. Every `toast()` from `useToast` goes into an in-memory array and is never shown. Call sites include `StopButton/index.tsx:29,38,44` and `WorkflowEditor/index.tsx:936,1706` — i.e. stop/pause feedback is silently dropped. The real toast system is sonner (`main.tsx:44`).
**Fix:** migrate the 7 `useToast` call sites to sonner's `toast()` (already used by ~30 files), then delete `useToast.ts`, `ui/toast.tsx`, and the `@radix-ui/react-toast` dependency.

### 1.2 `toastOptions={{}}` in main.tsx wipes the entire tokenized sonner skin
`ui/sonner.tsx` styles toasts with theme tokens via `toastOptions.classNames`, then spreads `{...props}` **after** it. `main.tsx:44` renders `<Toaster richColors toastOptions={{}} />`, so the empty object replaces the classNames wholesale — every toast renders in sonner's default skin, not the app theme.
**Fix:** remove `toastOptions={{}}` (or deep-merge inside `ui/sonner.tsx`), and decide deliberately between `richColors` and the tokenized skin.

### 1.3 Global `.dark` overrides make the design tokens lie
`src/index.css:133–187` is the single most consequential styling hazard in the repo:

- `.dark .text-foreground { color: hsl(0 0% 75%) }` (`:149–151`) overrides the utility away from `--foreground` (0 0% 64%, `:84`). Body text (styled via `body { @apply text-foreground }`) renders 64%, explicitly-classed text renders 75%, and opacity-modified `text-foreground/80` (8 occurrences) bypasses the override and resolves from 64%. **The same token renders three different grays depending on how it's referenced**, and tuning `--foreground` appears to do nothing. Same pattern for `.text-muted-foreground` (977 usages ride through this).
- `.dark .font-bold, .dark .font-semibold { @apply text-gray-300 }` (`:143–146`) has specificity (0,2,0), beating any single color utility. Any element combining a weight with a color loses its color in dark mode: `font-semibold text-destructive` at `WorkflowHistory/index.tsx:641` and `ReportBlockHost.tsx:683` renders gray instead of red. ~23 call sites are visibly affected.
- `.dark table/th/td/li` element rules (`:160–187`) restyle underneath components: `TableHead` declares `font-semibold text-muted-foreground` but actually renders weight 500 in dark vs 600 in light, with its color decided by whichever of three competing global rules wins. The entire console table UI renders through this path.

**Fix:** fold the desired dark values into the `.dark` variable block itself (`--foreground: 0 0% 75%` etc.), delete all class/element overrides, and first sweep the ~18 raw `text-slate-900`-style headings that currently depend on the override rescue (see §5.2).

### 1.4 React Flow node registration bug behind dead CSS selectors
`index.css:345–365` sizes `.react-flow__node-EVENT_NODE/-WAIT_NODE/-GROUP_BY_NODE`, but `NODE_TYPES` (`features/workflows/config/workflow.ts:1–10`) defines no such keys. `WorkflowEditor/index.tsx:110–113` registers `[NODE_TYPES.EventNode]: EventNode` etc. — all three index expressions evaluate to **`undefined`** (the `Record<string,string>` type hides the typo), collapsing into a single `undefined` nodeTypes key. The imported `EventNode` component is unreachable and GroupBy/WaitForSignal steps silently render as `BASIC_NODE`.
**Fix:** remove the three dead selector groups and undefined-key registrations; type `NODE_TYPES` `as const` so a missing key is a compile error.

### 1.5 Printing any page except a report yields blank paper
The `@media print` block (`index.css:236–245`) is global: `body * { visibility: hidden !important }` with visibility restored only under `.report-print-root`, rendered solely by `ReportPage.tsx`. Cmd+P on any other route prints empty pages. The print CSS also selects by Tailwind utility names (`.my-5`, `.rounded-lg`, `.h-80` at `:319–341`), which breaks silently if classes change.
**Fix:** scope the hiding rules under a `body.printing-report` class toggled around `window.print()`, and switch utility-name selectors to data attributes.

### 1.6 Loading bar animates with an undefined keyframe
`HistoryPanelContent.tsx:284` uses `animate-[loading_1.5s_ease-in-out_infinite]`, but no `@keyframes loading` exists anywhere (only `glow-pulse`, `parked-pulse`, `row-flash-success`). The active-execution shimmer sits frozen.
**Fix:** define the keyframes in `tailwind.config.js` and use a named utility.

### 1.7 Invalid color definitions: `success/warning/error` foreground
`tailwind.config.js:89,93,97` use `hsl(var(--success-foreground), 0 0% 100%)` — the fallback belongs *inside* `var()`, and the three `*-foreground` variables are never defined in `index.css`. Any future `text-success-foreground` emits invalid-at-computed-value CSS that silently falls back to inherited color. (Compare the correct pattern used by `destructive.foreground` on line 85.)
**Fix:** define the vars in `:root`/`.dark` and use `hsl(var(--success-foreground))`, or delete the three entries.

### 1.8 `content` globs: two phantom dirs, one missing dir
`tailwind.config.js:7–14` scans `./src/pages/**` and `./src/components/**` (neither exists) and **omits `./src/router/**`**, which uses Tailwind classes (`PrivateRoute.tsx:11–12`, `index.tsx:151–152`). Those classes currently work only because other files happen to generate the same utilities — a change elsewhere can silently unstyle the router spinners.
**Fix:** replace the phantom globs with `./src/router/**/*.{ts,tsx}` (and `./index.html`), or use a single `./src/**/*.{ts,tsx}` glob.

### 1.9 Inter is declared but never loaded
`theme.fontFamily.sans` leads with Inter (the console design direction is blue/Inter), but there's no `<link>`, no `@font-face`, no `@fontsource/inter`, and no font files in `public/` or `src/assets`. The app silently renders in Roboto (where OS-installed) or system-ui — per-platform typography drift.
**Fix:** self-host Inter (`@fontsource/inter` imported in `main.tsx`) or remove it from the stack so config matches reality. Also note `fontFamily` sits at `theme.` level (replacing, not extending, defaults — `font-serif` is gone; currently unused, but should be intentional).

### 1.10 Theme is read non-reactively in canvas components
`ImprovedEdge.tsx:193` computes its entire 21-hex edge palette from `document.documentElement.classList.contains('dark')` **during render**, unsubscribed from the theme store — toggling the theme leaves rendered edges painted in the old theme until an unrelated re-render. Same pattern in `ReplayGraph.tsx:194–213`.
**Fix:** drive SVG strokes from CSS vars (`stroke: hsl(var(--destructive))` works) so no JS branch exists, or subscribe to `useThemeStore`.
Related plumbing gaps: theme-class logic is duplicated verbatim in `themeStore.ts:39–51` and `App.tsx:26–38`; nothing sets `color-scheme` or `<meta name="theme-color">`; no pre-paint theme bootstrap in `index.html` (light flash on load).

---

## 2. Unused / dead CSS

All "unused" claims below were re-verified against template-literal construction, `cn()` maps, tests, and the compiled CSS output.

| What | Where | Detail |
| --- | --- | --- |
| `--chart-1..5` (10 var definitions + config mapping) | `index.css:63–67,105–109`; `tailwind.config.js:102–108` | Zero consumers. Meanwhile **three independent hardcoded chart palettes** exist: `ChartBlock.tsx:29–36`, `ExecutionTimeline/index.tsx:60–102`, analytics charts. Either wire recharts to `hsl(var(--chart-N))` (gets dark theming for free) or delete the tokens. |
| `error` color family | `index.css:62,104`; config `:95–98` | Byte-identical to `--destructive` in both themes; 0 usages vs 227 for destructive. Delete. |
| Custom `animation`/`keyframes` config block | `tailwind.config.js:120–145` | `animate-in` is shadowed by the tailwindcss-animate plugin (both rules ship; plugin emitted last, wins — all 12 in-source `animate-in` usages are plugin-intent shadcn enter animations); `fade-in-slide-up` has 0 usages. Delete the whole block. |
| dvh/svh/lvh utilities ×3 definitions | config `:36–50`; `index.css:426–431` | Tailwind 3.4 core ships all of these natively; `lvh` variants have 0 usages. Delete both re-definitions. |
| Safe-area: 2 parallel systems | config `:30–35` (0 usages); `index.css:409–425` | Only `pt-safe` (3) and `pb-safe` (1) are used; `pl/pr/p-safe` dead; the entire config spacing convention dead. Keep one system. |
| `--accent-hover`, `--accent-dark`, `--border-light` | `index.css:119–122` | Dark-only, zero consumers; a trap — no `:root` fallback means first use silently breaks light mode. |
| `--sidebar-primary` pair | `index.css:71–72,112–113` | Only usage is an accidental `text-sidebar-primary-foreground` on an icon-only container (`Sidebar.tsx:51`). |
| `.scrollbar-gutter-stable` | `index.css:78–80` | Unused, and misfiled inside `@layer base` between `:root` and `.dark`. |
| `.report-print-brand` (~20 lines) | `index.css:190–192,287–303` | Styled print footer that **no component renders** — either a removed feature or a silently missing one (siblings `report-print-hidden` etc. are live). Decide intent. |
| `components.json` aliases | `components.json` | Point at `src/components` / `src/hooks`, which don't exist — `npx shadcn add` would scaffold a parallel tree. Update to `@/shared/components/...`. |
| `WorkflowCard` `style` pass-through prop | `WorkflowCard/index.tsx:30,80` | Sole caller never passes it. |

Confirmed **live** (do not delete): `animate-glow-pulse` (4), `animate-parked-pulse` (1), all `report-print-root/hidden/content` classes, `data-tiles-page-*` attrs (emitted by `tiles-page.tsx`), `data-sidebar` attrs, `success`/`warning` base color tokens.

---

## 3. Duplication (copy-paste that should be shared)

### 3.1 Loading states — the worst offender
- **Spinners:** ~60 inline `<Loader2 className="…animate-spin…">` across 50 files in **25+ distinct class variants** (`h-4 w-4 animate-spin` ×12, `mr-2 h-4 w-4 animate-spin` ×9, plus text-purple-600/text-slate-400 one-offs), 2 hand-rolled div spinners (`router/PrivateRoute.tsx:12`, `router/index.tsx:152`). A shared loader exists (`shared/components/loader.tsx`) but is imported by only 8 files — and it exports a component **also named `Loader2`**, colliding with the lucide icon name.
  **Fix:** one cva `<Spinner size>`; rename the shared `Loader2` export (`PageLoader`); codemod the two dominant variants (~40% of sites).
- **Skeletons:** `ui/skeleton.tsx` and `SkeletonTable` exist, yet **zero feature files import either** — 71 hand-rolled `animate-pulse` divs across 16 files, visually drifting from the primitive (`bg-muted/60`+`rounded` vs `bg-primary/10`+`rounded-sm`).
- **List-page state trio:** six console list pages (TriggersGrid, WorkflowsGrid, ObjectSchemasTable, ExistingConnections, ReportsListPage, Settings) copy-paste the same ~50-line skeleton/error/empty-state block (`'flex h-full flex-col items-center justify-center px-6 py-10 text-center'` ×12, error icon/title/subtitle strings ×8–18). `ConsoleTableShell` provides none of it.
  **Fix:** add `TableSkeletonRows` / `ErrorState` / `EmptyState` next to `ConsoleTableShell` — since 11 files already use the shell, a shell-level `loading`/`error` prop kills all three duplications at once.

### 3.2 Status pills
`console/StatusPill.tsx` centralizes five tones, yet its exact amber string is re-inlined verbatim at `InvocationHistoryColumns.tsx:130`, `ExecutionTimeline` keeps its own `statusColor` map, and green/red badge combos recur in 8+ files. Additionally there are **two parallel chip systems**: `ui/badge` (theme tokens, rounded-md) vs `StatusPill` (hardcoded emerald/amber palette, rounded-full), giving three different answers to "what does a warning chip look like."
**Fix:** export `TONE_CLASSES`/`executionStatusPill` for reuse; derive StatusPill tones from the same tokens Badge uses (or rebuild it on `badgeVariants`).

### 3.3 Repeated recipes (extract once)
| Concept | Occurrences | Files | Fix |
| --- | --- | --- | --- |
| Uppercase section label | ~60 in **20 spellings** (`text-[10px]`/`[11px]`/`xs`, `tracking-wide`/`wider`/`[0.08em]`) | 21+ | `<SectionLabel>` with one size pair per tier |
| Field error text | 21× `text-xs text-destructive` + 10× with `mt-1` + 5 literal clones of `FormMessage`'s class | 26+ | standalone `<FieldError>` |
| Row-action icon button (`h-7 w-7` muted / hover-destructive) | 27 in 3 variants | 10+ | Button `size="icon-sm"` + tone variant |
| Editor sidebar `<th>` cell string | 16× `'text-left p-2 text-sm font-medium text-muted-foreground'` | 5 | migrate to `ui/table` or an `EditorTable` set |
| Picker item row + mono type chip | 8× + 7× (VariablePickerModal alone ×6–7; copied into `condition-editor.tsx`) | 3 | `<PickerItem>`/`<PickerEmpty>`; also merge the two ConnectionPickerModal implementations |
| Mapping-mode toggle (incl. hardcoded green active state) | 20 strings across the Array/Object/Composite mapping editors | 3 | `<MappingModeToggle>` |
| Wizard-v2 native `<select>` with copied input skin (missing focus/disabled states) | 8 | 2 | `ui/select` or a `NativeSelect` primitive |
| Page-container wrapper `'w-full px-4 py-6 sm:px-6 lg:px-10'` + eyebrow label with `tracking-[0.08em]` | 5 + 5 | 5 | `<PageContainer>`/`<SectionEyebrow>` |
| Picker-modal geometry (`sm:max-w-[500px]`, `max-h-[400px]`, `max-w-[200px]`) | 16 | 5–6 | shared `PickerDialog` |
| Card-base recipe `'rounded-lg border bg-card'` with shadow drift (shadow-sm vs none) | 19 | reports blocks + analytics | `BlockFrame` on `Card` |
| Amber/blue notice banners in 3 inconsistent recipes (some missing dark:) | 12+ | 5+ | add `warning`/`info` variants to `ui/alert` |
| wizard-v2 `grid-cols-[…]` templates (header/row pairs must stay in sync) | 12+ | 7 | hoist to named constants |

### 3.4 React Flow node dimensions — three sources of truth
132×36 / 252px / 72×36 / 168×132 exist independently in: `index.css` `!important` blocks (`:345–405`), `NODE_TYPE_SIZES` (`config/workflow.ts:39–52`), and `graph.ts:11–14` fresh `BASE_*` literals — plus redundant per-node inline styles (`BasicNode.tsx:404`, `ConditionalNode.tsx:224`, `AiAgentNode.tsx:441`).
**Fix:** make `NODE_TYPE_SIZES` the single source; derive `graph.ts` constants from it; delete either the CSS blocks or the inline styles.

---

## 4. Inconsistencies

### 4.1 Status colors bypass the token system ~20:1
448 raw status-color usages (green 122, amber 125, red 100, yellow 73, orange 28) vs 22 semantic status-token usages; `*-error` has **zero** consumers. The token-based pattern is already proven in `ValidationMessageItem.tsx:41` and `RateLimitSection.tsx:82` — and it removes the need for `dark:` twins.
**Decide explicitly:** migrate status styling to tokens (start with StatusPill, which fans out everywhere) or delete the tokens. The current half-state is the worst option.

### 4.2 Warning is two colors
Warning-meaning UI is amber in some features (InvocationHistory, StatusPill, FolderDialogs) and yellow in others (WorkflowLogs level indicators `:54,:533`, RateLimitCard, FinishStepField) while `--warning` itself is amber. Exempt: NoteNode sticky-note yellow, search-highlight marks (intentional).

### 4.3 Neutral drift: slate vs gray vs tokens
Neutrals are expressed three ways: tokens (dominant — `text-muted-foreground` 977), raw slate (184, concentrated in features), raw gray (44, concentrated in shared). Six hotspot files hold ~⅓ of occurrences (`InvocationHistoryColumns` 14, `FolderDialogs` 12, `ObjectSchemaFormLayout` 9…). Mechanical mapping: slate-400/500→`text-muted-foreground`, 600–900→`text-foreground`, `border-slate-200`→`border-border`, `bg-slate-50/100`→`bg-muted` — and the paired `dark:` variants delete themselves.

### 4.4 Brand blue hardcoded 165× while `--primary` IS blue-500
`text-blue-600` links (19), `bg-blue-50` chips (17), `bg-blue-500` (14)… Buttons correctly use `bg-primary` (zero `bg-blue-600` exists), so drift concentrates in links/chips/icon accents. Consider adding an `--info` token for the legitimate info-tone blues.

### 4.5 Micro font sizes: two clashing unit systems
156 arbitrary font sizes below `text-xs`: `text-[10px]` ×82, `text-[11px]` ×44, plus a parallel rem family (`text-[0.8rem]` ×9, `[0.7rem]` ×5, `[0.65rem]` ×2) spelling the *same* visual sizes differently. Add `2xs`/`3xs` fontSize tokens and codemod.

### 4.6 Two disconnected z-index worlds
Page chrome: `z-10/20/50` plus escalations `z-[60]` (dropdown, to beat z-50 dialogs — while select/popover/tooltip stay z-50 and only work in dialogs by portal order) and `z-[100]` (toast). React Flow canvas: inline `zIndex` ladder −1/1/1000/1001/1002/9999, uncoordinated. Define a named zIndex scale in config; extract the canvas ladder into named constants.

### 4.7 Inline styles (73 sites: 40 justified, 21 should-be-classes, 12 duplicate tokens)
- `ExecutionTimeline/index.tsx` paints 14 step-type + 5 status colors from verbatim Tailwind-palette hexes at ~11 inline sites — theme-blind (also §1.10, §2 chart tokens).
- 21 static-value styles map 1:1 to utilities (`top: '30%'` handle offsets, `cursor: 'pointer'`, `tableLayout: 'fixed'`…).
- `ObjectInstancesTable/index.tsx:729` embeds the repo's only JSX `<style>` tag: global `row-flash-success` keyframes re-emitted every render with hardcoded green-500 rgba — move next to the other keyframes.
- `service-icon.tsx:80` sets an inline `boxShadow` identical to the `shadow-lg` already in its className.
- No CSS-in-JS, `setProperty`, or `cssText` anywhere — clean.

### 4.8 Cosmetics
- Icon sizing spelled three ways: `h-4 w-4` (131) vs `w-4 h-4` (13) vs `size-4` (6); same at 3.5. Pick one (Tailwind 3.4 supports `size-*`).
- `h-[1px]`/`w-[1px]` ×9 (including `ui/separator.tsx:20`) where `h-px`/`w-px` exist.
- Duplicated fixed dims: `w-[180px]` select triggers, `h-[350px]`/`h-[520px]` chart/replay placeholder pairs that drift independently.
- No `prettier-plugin-tailwindcss` (or eslint-plugin-tailwindcss) despite a full prettier/husky/lint-staged pipeline — the root enabler of the ordering/duplication noise. Also: both `.prettierrc` and `.prettierrc.json` exist at repo root.

---

## 5. Dark mode

(§1.3 covers the global-override hazard; these are the component-level issues.)

1. **Broken-in-dark components:** `maintenance-page.tsx` is a full `bg-white` page with `text-gray-900` and `bg-gray-900` buttons (blinding white sheet / near-black-on-black); `metric-card.tsx` trend text falls back to `text-gray-600` on `bg-card`; `FolderDialogs` `text-slate-400/500` is near-invisible. 36 raw-palette lines across 17 files have no `dark:` variant; several others are legible only because the global overrides rescue them — which is exactly why the overrides can't be deleted before this sweep.
2. **Divergent dark idioms:** panel backgrounds as `dark:bg-slate-800` (5), `dark:bg-white/10` (8), `dark:bg-gray-800/900`, `dark:bg-background` (5), one `dark:bg-card` — while `bg-muted`/`bg-card` (409 combined usages) need no variant at all. Warning split amber/yellow (§4.2) repeats in dark variants.
3. **74 hardcoded hexes in 10 files** can't react to the theme at all (ImprovedEdge ×21, ExecutionTimeline ×19, ChartBlock ×8, ReplayGraph ×6…) — §1.10 and the chart-token fix (§2) resolve most.

## 6. Accessibility & motion (sweep findings)

- **No `prefers-reduced-motion` handling anywhere** — the only awareness is `useReplayClock`. 69 `animate-spin`, 71+ `animate-pulse`, and two custom infinite box-shadow animations (glow/parked-pulse — box-shadow animation is also expensive) run unconditionally. Add a global reduce block + `motion-reduce:` variants.
- **Split focus idiom inside `ui/` itself:** Button/Input use `focus-visible:ring`, Select trigger and Dialog close use `focus:ring` (ring on every mouse click). Feature code inherits the split (45 vs 22). Standardize on `focus-visible:`.
- **Native `title=` instead of Tooltip in 66 files (174 occurrences)** including icon-only Stop/Pause/Resume buttons — invisible on touch and keyboard focus. Migrate icon-only controls at minimum.
- ~17 raw `<button>`s hand-duplicate ghost/icon Button **without its focus-visible ring** — invisible keyboard focus.
- `window.confirm` ×3 for destructive flows despite `ConfirmationDialog` existing for exactly this.

---

## 7. Recommended remediation sequence

Each phase is independently shippable; earlier phases unblock later ones.

1. **Correctness (small, high yield):** toast fixes (§1.1–1.2); content globs + components.json aliases (§1.8, §2); success/warning/error-foreground vars (§1.7); undefined `loading` keyframe (§1.6); NODE_TYPES registration (§1.4); print-scope fix (§1.5); load Inter or drop it (§1.9).
2. **Delete dead weight (one PR, pure deletion):** chart vars (or wire them — decide with §2), error family, custom animation block, dvh/safe-area duplicates, `--accent-*`/`--border-light`, `scrollbar-gutter-stable`, `report-print-brand` (after intent decision), sidebar-primary pair.
3. **Tooling:** add `prettier-plugin-tailwindcss` + one mechanical `npm run format` commit; delete `.prettierrc.json`; optionally eslint-plugin-tailwindcss.
4. **Dark-mode detox (ordered!):** first sweep raw light-palette + override-dependent files to tokens (§5.1, §4.3), **then** delete the global `.dark` overrides (§1.3), folding desired values into the `.dark` variable block. Fix non-reactive theme reads (§1.10), dedupe theme plumbing, add `color-scheme`.
5. **Shared-component consolidation:** loading trio (Spinner/Skeleton/ConsoleListState — §3.1), StatusPill/Badge unification (§3.2), Alert warning/info variants, FieldError, SectionLabel, icon-button size, picker primitives (§3.3), node-dimension single source (§3.4).
6. **Token migration campaign:** status colors → tokens (§4.1, via StatusPill), warning→amber (§4.2), slate/gray→tokens (§4.3), blue→primary/info (§4.4), micro font tokens (§4.5), z-index scale (§4.6).
7. **Polish:** reduced motion, focus idiom, tooltip migration, inline-style conversions, icon-size convention.

---

## Appendix: method

Audit ran as an orchestrated 84-agent workflow: 8 parallel dimension finders → 1 adversarial verifier per finding (each instructed to refute, checking dynamic class construction, cn()/clsx maps, compiled-CSS output via `npx tailwindcss` probe builds, and dist artifacts) → completeness critic. 74/75 finder claims confirmed; 1 refuted (an undercounted hex census — corrected figures used here); severity adjusted on 6. Duplicate findings discovered independently by multiple dimensions were merged in this document.
