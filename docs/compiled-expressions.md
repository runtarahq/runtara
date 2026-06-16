# Compiled Expressions for the Direct WASM Emitter

Status: Phase 1 implemented (compiled conditions, runtime-compiled) — see Implementation status below.
Owner: Workflow runtime
Target file under change: `crates/runtara-workflow-stdlib/src/direct_json.rs` (plus `conditions.rs`, `switch_helpers.rs`, `template.rs`, `lib.rs`, and emit-side `crates/runtara-workflows/src/direct_wasm/*`)

---

## Implementation status

**Approach taken:** the doc recommends Option B (emit-time precompute). The entire
per-element win comes from compiling each expression **once per run** and reusing
it across all elements/iterations, which **Option A (compile at manifest parse,
cache on the manifest)** delivers identically with no emitter / WIT / manifest-
format / version risk and full local testability. Phase 1 therefore implements
the `CompiledExpr` design via **runtime compilation cached per run**, entirely in
`direct_json.rs`.

**Implemented & shipped (commits on `main`):**
- `CompiledExpr` IR + compiler + evaluator — a 1:1 mirror of
  `eval_condition_expression` / `apply_mapping_value` / `apply_reference`; leaf
  comparison/coercion delegate to the existing `values_equal`/`is_truthy`/
  `to_number`/`apply_type_hint`/`render_template`. Errors are deferred to
  `Error(msg)` nodes (message-parity).
- `compiled_condition_matches_interpreter` differential oracle: `compile==
  interpret` (incl. `Err` equality) across every operator + edge case.
- All **condition** evaluation sites route through the compiled path:
  `eval_condition` (Conditional + edge), `while_condition`, `filter` (per
  element), and Switch case conditions. Conditions are cached on the manifest
  (`c{id}`/`w{id}`/`f{id}`); Switch (cold) compiles per call.
- **Input mappings** for `apply_mapping` (agent / finish / embedWorkflow) compile
  to a cached `CompiledInputMapping` (keyed by mapping id) — agent input mappings
  inside a Split body run per iteration, so this is a per-iteration win too. The
  finish/agent post-passes are unchanged. Differential oracle
  `compiled_input_mapping_matches_interpreter`.
- W3: `lookup_segments` borrows through inline nodes (Cow) and clones only at a
  handle deref / the leaf; `apply_filter_compiled` consumes the input array and
  **moves** each element in/out of the scope instead of cloning it.
- **Perf:** `filter_perf_breakdown` per-element **9.4µs → 2.38µs (~4×)**;
  live e2e (`filter_wf.json`, 800-item Filter ×20) `find` **13.8ms → 9.55ms/call**
  (larger for prod-scale collections/records). **Verified:** 144 stdlib unit
  tests + differential oracle, 58 gated `RUNTARA_RUN_DIRECT_WASM_E2E` execute
  tests (filter/while/conditional/switch incl. interned-handle template +
  bounded large-scope accumulator), correctness unchanged.

**Deferred (mechanical; small/no perf delta):**
- **Remaining mapping sites** — Split iteration variables, `apply_log` /
  `apply_error` context, `delay_duration_ms`, `wait_timeout_ms` /
  `wait_action_mapping` — still use the interpreter `apply_input_mapping` /
  `apply_mapping_value`. These are small mappings; per-iteration only when the
  step sits in a Split/While body. They are mechanical extensions of the shipped
  `CompiledInputMapping` machinery (same compile + cache pattern as
  `apply_mapping`), left to keep the cutover low-risk against the byte-exact
  debug-shape tests.
- **Interpreter retained** (not the M5 full deletion): it is still the cold-path
  mapping evaluator and the differential-test oracle. A literal "delete the
  interpreter" cutover requires migrating all mapping sites first.
- **Emit-time precompute (Option B), value-store length metadata (W4), and the
  per-call source-parse-once WIT change** remain as future work (M1/M4 + §10.6).
  None are needed for the proven per-element win.

---

## 1. Problem & goal

The direct WASM emitter does not generate per-step native Rust anymore. Instead it ships the workflow DSL as a key-sorted JSON manifest and the workflow stdlib component **interprets that JSON at runtime**: every condition, mapping, and reference is re-walked as a `serde_json::Value` tree and every reference path string is re-parsed on **every** evaluation. This interpretation cost dominates large-`Filter`/`Split`/`While` workflows.

### What is re-done per evaluation today

For a single binary `Filter` condition referencing an item field, each element pays (all in `crates/runtara-workflow-stdlib/src/direct_json.rs`):

- `is_condition_operation` (`:4849`) re-reads `op`/`type` to classify the node — static.
- `eval_condition_operation` (`:4857`) re-reads `op.as_str()` + `arguments.as_array()` and re-matches the op string in a 20-arm `match` (`:4867`).
- `eval_comparison` (`:4973`) re-matches the op string a **second** time (`GT`/`GTE`/`LT`/`LTE`); `eval_string_match` (`:5004`) and `eval_array_match` (`:5023`) re-match a third place.
- `eval_condition_argument_as_value` (`:4945`) re-runs `is_condition_operation` per argument plus the `op == "LENGTH"` special-case check — static per argument.
- `apply_reference` (`:5119`) → `lookup_source_path` (`:5388`) → `path_to_json_pointer` (`:5416`): a full char-by-char re-parse of the path (4× `String::replace` at `:5417`, a dotted-form scan at `:5423`, then an escaped-pointer build at `:5448`), followed by a clone-on-traverse pointer walk (`.clone()` at `:5402`/`:5403`, `deref_handle().into_owned()` at `:5406`, final `materialize` at `:5408`).
- `apply_type_hint` (`:5310`) re-matches the type-hint string each call.
- `apply_filter` (`:3950`) additionally clones the **entire source envelope** once (`:3959`) and per element clones the item (`:3969`) and mutates `source["item"]` (`:3966`).

On top of every host call, the source envelope is re-deserialized from `list<u8>` (`serde_json::from_slice`) at the WIT boundary — e.g. `eval_condition` (`:386`), `apply_mapping` (`:366`), `filter` (`:832`), `while_condition` — and `while-condition`/`split-iteration` are re-invoked per iteration (`crates/runtara-workflows/src/direct_wasm/compile/while_loop.rs:306`).

### Goal

Introduce a **CompiledExpr** layer that parses each condition / mapping / reference **once** into a reusable evaluable form (operator enum, pre-split pointer segments, pre-classified argument kinds, hoisted literals, pre-parsed template) and **fully replaces** the runtime JSON interpreter for the evaluation hot path.

### Success metric

- `filter_perf_breakdown` (`direct_json.rs`, `#[ignore]` micro-bench at `:9688`) per-element cost drops from **~9.4 µs → sub-microsecond** for a single binary item-field condition. The path-string re-parse and op-string match (the ~10× interpretation overhead) drop to **zero** per element; only the genuine pointer walk + comparison remain.
- The input collection for `Filter`/`Split`/`GroupBy` is parsed/resolved **once** per step, not twice (today `apply_filter` resolves+clones the array at `:3955` and clones the whole source at `:3959`).
- Stacks on the already-shipped fixes: the **debug-end** fix (`filter_debug_end_reads_stored_output_without_recomputing`, `:9640` — debug-end no longer re-evaluates) and the workflow-level **SKU-index** optimization. Compiled expressions remove the *remaining* per-element interpretation cost those two changes do not touch.

---

## 2. Decisions (fixed)

1. **General, not Filter-only.** Build and release the compiled layer for **all** condition / mapping / reference evaluation across **every** step type (Conditional, While, Switch routing + case-match + case-output, Filter, GroupBy, Split value + iteration variables, Agent/AiAgent input mapping, EmbedWorkflow input mapping, Finish output mapping, Delay duration, Log/Error context, WaitForSignal timeout + action mappings, executionPlan edge conditions, and all debug/breakpoint recompute arms). Not a Filter special-case.

2. **Templates stay on the materialize/render path.** Model a template as a compiled `Template` node that delegates to the existing `render_template` (`crates/runtara-workflow-stdlib/src/template.rs:25`) over a `materialize`d (handle-free) source. The only compile-time win we take here is hoisting the minijinja parse out of the per-call path; we do **not** rewrite the renderer.

3. **Full replacement of the runtime interpreter.** No permanent production fallback. The compiled form is the only production evaluator. A **temporary interpreter-as-oracle** (the current `eval_*` / `apply_*` functions kept behind a host-only test feature) is fine — and required — for differential testing during development, but it must not ship in the `wasm32` build.

---

## 3. Current architecture — the eval hot path

All condition evaluation funnels through one function; all mapping/reference evaluation through a second family. The manifest is parsed once per run and held in a thread-local; each config's raw `serde_json::Value` is re-interpreted on every invocation.

### Manifest lifecycle

- `init_manifest` (`crates/runtara-workflow-stdlib/src/lib.rs:137`) is called **once per run**; it resets the value-store arena and calls `DirectJsonManifest::parse` (`direct_json.rs:320`), storing the result in `static MANIFEST: RefCell<Option<DirectJsonManifest>>` (`lib.rs:132`).
- The stdlib component is instantiated **per workflow run** (comment `direct_json.rs:60`), so the manifest — and any cache inside it — lives for the full run and is discarded with the instance. No cross-run staleness.
- Every per-kind config is addressed by a stable, manifest-wide-unique-per-kind `u32` id, assigned at emit time by a single shared allocator (`crates/runtara-workflows/src/direct_wasm/manifest.rs:592` `DirectManifestBuildState`, `allocate_*_id` `:607`). The runtime addresses configs by `(kind, u32 id)` via `self.<collection>.get(&id)` (e.g. `apply_mapping` `:368`, `eval_condition` `:388`, `filter` `:834`).
- All evaluation methods are `&self` (no `&mut self` in the impl); dispatch goes through an immutable borrow `MANIFEST.with(|slot| slot.borrow().as_ref()...)` (`lib.rs:162`). There is already an interior-mutable, lazily-populated field: `debug_start_ms: RefCell<BTreeMap<String, i64>>` (`direct_json.rs:300`, written via `&self` at `:2547`) — the precedent for adding a cache.

### Condition dispatch (one evaluator)

`eval_condition_expression` (`:4841`) → `is_condition_operation` gate (`:4849`) → `eval_condition_operation` (`:4857`, op-string match) → `eval_comparison`/`eval_equality`/`eval_string_match`/`eval_array_match`/`eval_length_as_value` (`:4973`–`:5041`). Non-operation nodes → `eval_condition_value` (`:4961`, unwraps `{type:"value"}` envelope) → `apply_mapping_value` → `is_truthy`.

### Mapping dispatch (one family)

`apply_mapping_value` (`:5086`, `valueType` match) → `apply_reference` (`:5119`) / immediate clone / `apply_composite` (`:5135`) / `render_template`. `apply_input_mapping` (`:5073`) iterates the top-level object and expands dotted keys via `insert_nested` (`:5355`). `apply_reference` → `lookup_source_path` (`:5388`) → `path_to_json_pointer` (`:5416`) → `apply_type_hint` (`:5310`).

### Two divergent reference resolvers (must be preserved)

- **Full** (`path_to_json_pointer` + `lookup_source_path`): bracket/quote syntax, `[N]` numeric indices, `~0`/`~1` JSON-pointer escaping, `$wfref` handle deref at root/mid/leaf (`deref_handle` `:245`), `materialize` of the leaf (`:260`), `default` fallback, `type` hint. Used by references, conditions, filter item lookups, GroupBy key (`path_to_json_pointer` at `:4145`).
- **Naive** (`switch_helpers.rs::resolve_dot_path` `:51`): `format!("/{}", path.replace('.', "/"))` — **no** bracket/index support, **no** `$wfref` deref, **no** default/type-hint. Used **only** by `process_switch_output` (`switch_helpers.rs:18`) for Switch case/default **output** references (`apply_switch` at `:3994`/`:4007`/`:4023`).

### Interning / value store

`build_source` (`:3007`) interns top-level scope entries ≥ 16 KiB (`WFREF_THRESHOLD_BYTES` `:55`) into `{"$wfref":<id>}` handles via `intern_if_large` (`:194`), content-deduped (`content_index` `:71`). `deref_handle` (`:245`) re-parses the stored bytes via `serde_json::from_slice` (`:249`) on **every** traversal with **no** memoization; `StoreEntry` (`:75`) holds only `bytes` + `nested` ids. `value_store_retain` (`:100`) GC requires handles to remain syntactically `{"$wfref":id}` in any onward-serialized scope.

---

## 4. Surface to cover (exhaustive — full replacement)

### 4.1 Condition operators

Runtime-evaluated (16) — `eval_condition_operation` (`:4867`):

| Operator | Semantics to preserve | Source |
|---|---|---|
| `AND` | short-circuit `try_fold(true)`; later args not evaluated once decided | `:4868` |
| `OR` | short-circuit `try_fold(false)` | `:4875` |
| `NOT` | first arg negated; **missing arg ⇒ true** | `:4882` |
| `GT`/`GTE`/`LT`/`LTE` | `eval_comparison`: arity<2 ⇒ false; both sides `to_number`, any `None` ⇒ false; f64 compare | `:4886`,`:4973` |
| `EQ`/`NE` | `eval_equality`: arity<2 ⇒ false; `values_equal` (NE negates) | `:4887`,`:4994` |
| `STARTS_WITH`/`ENDS_WITH` | both sides must be `as_str` else false; plain `str::starts_with/ends_with`, no regex/case-fold | `:4888`,`:5004` |
| `CONTAINS`/`IN`/`NOT_IN` | array membership via `values_equal` (coercion inside membership); `CONTAINS` = left.array contains right; `IN`/`NOT_IN` = right.array contains left | `:4889`,`:5023` |
| `LENGTH` | as bool: `len > 0`; as value: `eval_length_as_value` (String/Array/Object `.len()`, Null⇒0, scalar⇒1) | `:4890`,`:5041` |
| `IS_DEFINED` | `!value.is_null()`; **missing arg ⇒ false** | `:4897` |
| `IS_EMPTY` | empty Array/String/Object/Null ⇒ true, else false; **missing arg ⇒ true** | `:4901` |
| `IS_NOT_EMPTY` | inverse; **missing arg ⇒ false** | `:4913` |

Query-only (4) — **must ERROR, not evaluate** (`:4929`): `SIMILARITY_GTE`, `MATCH`, `COSINE_DISTANCE_LTE`, `L2_DISTANCE_LTE`. Validated/rejected up front by E027 (`crates/runtara-workflows/src/validation.rs:3837` `query_only_operator_name`); the runtime arm exists only to fail loudly for pre-validation workflows. Unknown op ⇒ Err (`:4933`).

Switch-only compound matchTypes that **desugar** at runtime (`switch_case_condition` `:4035`): `BETWEEN` ⇒ `AND(GTE lo, LTE hi)` (`build_between_condition` `:4085`; **bounds<2 ⇒ false**); `RANGE` ⇒ `AND` of present `gte`/`gt`/`lte`/`lt` keys (`build_range_condition` `:4108`; **0 keys ⇒ true**, 1 key ⇒ that single op); `EQ` with array match ⇒ `IN` (`:4047`).

**Compiled node mapping:** each operator → a `CompiledOp` enum variant. `AND`/`OR`/`NOT` hold `Vec<CompiledExpr>` and preserve short-circuit. Comparison/equality/string/array/length/existence variants hold pre-classified argument nodes. The four query-only operators compile to a `CompiledOp::QueryOnlyError(op)` variant that returns the **same `Err`** as today (never `false`). Switch `BETWEEN`/`RANGE`/array-`EQ` are desugared **at compile time** into the same compiled comparison nodes (preserving the empty/under-arity edge behavior above).

### 4.2 valueType kinds

`apply_mapping_value` (`:5086`) — canonical set `Reference`/`Immediate`/`Composite`/`Template` (`crates/runtara-dsl/src/schema_types.rs:1435` `MappingValue`):

| valueType | Compiled node | Notes |
|---|---|---|
| `reference` | `Reference { segments: Vec<PointerSegment>, default: Option<Value>, hint: TypeHint, resolver: Full }` | path pre-split; `Some(Null)\|None ⇒ default` collapse preserved (`:5126`) |
| `immediate` | `Immediate(Value)` | literal **hoisted once**, not re-cloned per call (today clones at `:5097`) |
| `composite` | `Composite(CompositeNode)` | object: `Vec<(String, CompiledExpr)>`; array: `Vec<CompiledExpr>` (`:5135`) |
| `template` | `Template { parsed: minijinja Template }` | delegates to `render_template` over `materialize(source)`; minijinja parse hoisted to compile time |

`eval_condition_value`'s `{type:"value"}` envelope unwrap (`:4961`) and the LENGTH-as-value argument carve-out (`:4947`) are modeled as compiled **argument kinds** (`Expression` vs `MappingValue` vs `LengthValue`), not re-detected per call.

### 4.3 Type hints

`apply_type_hint` (`:5310`) — `string`/`integer`/`number`/`boolean`/`json`/`file` (`schema_types.rs:1587` `ValueType`). The exact coercion table (incl. NaN/Inf ⇒ 0 via `Number::from_f64` returning `None`; fractional-only number ⇒ `boolean` false; `integer` null stays Null) is baked into the `TypeHint` enum applied to the resolved value. Non-string `string` hint serializes via `Value::to_string()`.

### 4.4 References / paths

`path_to_json_pointer` (`:5416`) is compiled **once** into `Vec<PointerSegment { Key(String) | Index(usize) }>` — eliminating the per-call 4× `String::replace`, dotted scan, and escaped-pointer build. The compiled segment walk must still **deref `$wfref` handles at every traversed segment** and `materialize` the leaf (lazy, traversal-time — never pre-materialize the whole source, or the large-scope OOM returns). Recognized roots (`is_qualified_workflow_path` `:5280`): `data`/`variables`/`workflow`/`steps`/`loop`/`item`.

### 4.5 Special cases — how each maps to a compiled node

- **Field-based operators naming Object Model columns** — `resolve_nested_references` (`:5168`) is the agent-`inputMapping`-only stateful rewrite. It is **reference evaluation embedded in agent payloads**, so it is in scope for a general compiled layer, but it is **not a pure expression** — it has structural skip rules. Map it to a compiled **`AgentInputMapping`** node whose children carry **per-node compile-time flags** ("resolve against workflow scope" vs "keep-as-column-literal"). The flags are computable from static structure: argument 0 of a field-argument operator (`is_field_argument_operator` `:5287`) and unqualified refs inside `fn`-call args (`is_unqualified_reference_envelope` `:5273`, `is_qualified_workflow_path` `:5280`) stay literal; everything qualified resolves. Resolved refs are re-wrapped as `{valueType:immediate,value:X}` and `unwrap_top_level_immediate_envelopes` (`:5245`) strips exactly one wrapper per top-level field — both modeled as compiled post-passes.
- **fn-call args (ExprFn — SIMILARITY/COSINE_DISTANCE/L2_DISTANCE)** — these never evaluate in the workflow runtime; they pass through to the agent as data. Compile to a `PassthroughFnCall` node that resolves only its qualified arguments and leaves unqualified column-name args verbatim. **Cannot be a pure compiled expr** that evaluates to a bool/value — it is a structural pass-through.
- **SIMILARITY / query-only operators as conditions** — `CompiledOp::QueryOnlyError`. **Cannot be a pure compiled expr** in the evaluating sense; it is an explicit error node.
- **Switch case OUTPUT references** — keep the divergence. Compile as a distinct `Reference { resolver: NaiveDot }` node that uses the naive dot→pointer grammar (no brackets, no `$wfref` deref, no default/hint). Do **not** silently route Switch outputs through the full resolver — that is a behavior change requiring explicit sign-off (see Open Questions).
- **GroupBy key** — compiled as a `KeyExtractor { segments }` (the `path_to_json_pointer` at `:4145` is already hoisted out of the item loop today; compile it to pre-split segments). Output `group_key_string` (`:4831`: Number/Bool→to_string, Null⇒`_null`, other⇒`_invalid`) preserved.

### 4.6 Step types / features that must route through the compiled evaluator

| Step / feature | Entry | Hotness | Compiled artifact |
|---|---|---|---|
| Conditional | `eval_condition` `:385` | per-step | condition |
| executionPlan edge conditions | `eval_condition` `:385` (lowered `compile/edge_route.rs:77`, onError `compile/agent_error.rs:474`) | **per transition** | condition (purpose `edge.condition`, `manifest.rs:1274`) |
| While condition | `while_condition` `:746` | **per iteration** | condition |
| Filter condition + value | `apply_filter` `:3950` (per-item `:3970`) | **per element** | condition + value mapping |
| Switch routing/value: case-match | `apply_switch` `:3987`, `switch_case_condition` `:4035` | per case | desugared condition per (switch_id, case_index) |
| Switch case OUTPUT | `process_switch_output` `switch_helpers.rs:18` | per case | naive-resolver mapping |
| GroupBy value + per-item key | `apply_group_by` `:4135` | value once; key **per element** | value mapping + key extractor |
| Split value + iteration variables | `split_items` `:3442`, `split_iteration_variables` `:3610` | value once; vars **per iteration** | value mapping + input mapping |
| Agent input mapping | `apply_mapping` purpose `agent.inputMapping` `:376` | per dispatch | `AgentInputMapping` (nested-ref) |
| AiAgent input mapping (tools/score/condition args) | same as Agent (resolved once, **not per turn**) | per dispatch | `AgentInputMapping` |
| EmbedWorkflow input mapping | `apply_mapping` purpose `embedWorkflow.inputMapping` | per step | input mapping |
| Finish output mapping | `apply_mapping` purpose `finish.inputMapping` + `unwrap_finish_outputs` `:5064` | per step | input mapping + single-key unwrap |
| Delay duration_ms | `delay_duration_ms` `:895` | per step | value mapping |
| Log message(literal)/level/context | `apply_log` `:4177` | per step | input mapping (context) |
| Error category/code/message/severity(literals)/context | `apply_error` `:4212` | per step | input mapping (context) |
| WaitForSignal timeoutMs + onWait/onTimeout actions | `wait_timeout_ms` `:1122`, `wait_action_mapping` `:4810` | per wait | value + action mappings (String-keyed) |
| All debug/track-events recompute | `debug_start_data` `:2597`, `debug_end_output` `:2738`, `breakpoint_event` `:962` | cold | reuse the same compiled artifacts (must produce identical values **and** error-tolerance) |

---

## 5. Design

### 5.1 CompiledExpr data model

A new shared, `wasm32`-clean type (location decided in §5.3):

```rust
enum CompiledExpr {
    Operation(CompiledOp),
    Reference(CompiledReference),
    Immediate(serde_json::Value),         // literal hoisted once
    Composite(CompiledComposite),
    Template(CompiledTemplate),
    ValueWrapper(Box<CompiledExpr>),       // {type:"value"} unwrap, compiled
}

enum CompiledOp {
    And(Vec<CompiledArg>), Or(Vec<CompiledArg>), Not(Box<CompiledArg>),
    Compare { kind: Cmp /* Gt|Gte|Lt|Lte */, left: CompiledArg, right: CompiledArg },
    Equality { negate: bool, left: CompiledArg, right: CompiledArg },
    StringMatch { kind: StrMatch /* Starts|Ends */, left: CompiledArg, right: CompiledArg },
    ArrayMatch { kind: ArrMatch /* Contains|In|NotIn */, left: CompiledArg, right: CompiledArg },
    Length(CompiledArg),
    IsDefined(Option<CompiledArg>),
    IsEmpty(Option<CompiledArg>),
    IsNotEmpty(Option<CompiledArg>),
    QueryOnlyError(&'static str),          // SIMILARITY_GTE/MATCH/COSINE_DISTANCE_LTE/L2_DISTANCE_LTE
}

// Pre-classified argument kind — eliminates per-call is_condition_operation + LENGTH probe
enum CompiledArg {
    Expression(Box<CompiledExpr>),         // nested op → Bool (eval_condition_argument_as_value default)
    LengthValue(CompiledArg2),             // nested LENGTH → numeric Value carve-out (:4947)
    Mapping(Box<CompiledExpr>),            // leaf value/mapping node
}

struct CompiledReference {
    segments: Vec<PointerSegment>,         // pre-parsed once from path_to_json_pointer
    default: Option<serde_json::Value>,
    hint: TypeHint,                        // string/integer/number/boolean/json/file/none
    resolver: Resolver,                    // Full (lookup_source_path) | NaiveDot (switch output)
}
enum PointerSegment { Key(String), Index(usize) }

enum CompiledComposite { Object(Vec<(String, CompiledExpr)>), Array(Vec<CompiledExpr>) }

struct CompiledTemplate { parsed: minijinja::Template<'static, 'static> } // env owned; parse hoisted

// Agent input mapping: structural rewrite with per-node resolve/keep flags
enum CompiledAgentNode {
    ResolveRef(CompiledReference),         // qualified → resolve, re-wrap immediate
    KeepLiteral,                           // field-arg-0 column name / unqualified fn arg → leave
    Object(Vec<(String, CompiledAgentNode)>),
    Array(Vec<CompiledAgentNode>),
    FnCall { args: Vec<CompiledAgentNode> },
    Operation { op: String, args: Vec<CompiledAgentNode> },
    Passthrough(serde_json::Value),
}
```

Leaf comparison/equality/membership/truthiness/coercion is delegated to the **existing** `conditions.rs` helpers (`values_equal` `:19`, `is_truthy` `:78`, `to_number` `:105`) — **not** reimplemented. This guarantees bit-identical numeric/string coercion (f64 `EPSILON` equality, String↔Number coercion, Bool→number) under the differential oracle.

### 5.2 Evaluation: borrow-not-clone + collection-resolved-once

- `eval` takes `&self_compiled, source: &Value` and returns `Result<bool, String>` (conditions) or `Result<Value, String>` (mappings). No op-string match, no path re-parse, no per-arg re-classification.
- `CompiledReference::resolve` returns `Cow<'a, Value>`: borrows inline subtrees, only allocating when it derefs a `$wfref` handle. Consumers that compare/coerce take `&Value` (the comparators already do); only the `type`-hint/coercion boundary and `default` substitution `.into_owned()`.
- **The Cow win requires the parsed source to outlive the borrow.** Today every WIT entry re-parses `source` into a function-local `Value` that drops at function end. The compiled layer therefore parses the source **once per host call** into a local owned `Value` and evaluates the compiled tree against `&that` — the borrow is valid for the whole call (and the whole per-element loop in `apply_filter`).
- **Collection resolved once:** `apply_filter`/`apply_group_by`/`split_items` resolve the input array a single time via the compiled value mapping, then iterate **borrowed** items. The per-element `source["item"]` insert is replaced with a lightweight scope push: evaluate the compiled condition against a small `EvalScope { base: &Value, item: &Value }` so `item.*` resolves by pointing the `item` root at the borrowed element — **no per-element source clone** (today `:3959` clones the whole source and `:3969` clones each item).
- **Value-store length metadata for bounded debug:** extend `StoreEntry` (`:75`) with `top_level_kind` + `length` + `byte_size` captured **at intern time** (near-free during the existing serialize at `intern_if_large:198`). A handle-aware bounded path can then answer `bounded_debug_value` (`:3496`) `{_truncated,_type,_length,_bytes}` for a bare handle **without** `deref_handle` + full materialize (today `debug_start_data` Filter arm materializes the whole array at `:2627` then truncates).
- **Handle memoization:** add an `OnceCell<Rc<Value>>` parsed-value cache to `StoreEntry` so a repeatedly-read large handle (e.g. an N-element Filter referencing a shared interned variable) parses **once per run** instead of N times (`deref_handle:249` re-parses every call today). Invalidated for free at `reset_value_store` (`:84`).

### 5.3 Where compilation happens — recommendation: **emit-time precompute (Option B)**

Two options were evaluated:

- **Option A (compile at runtime parse):** zero manifest-size growth, emitter untouched, no shared crate; but the compiler runs **in the wasm guest every run**, amortizes only within a run, adds cold-start, and is hard to unit-test outside the wasm harness.
- **Option B (compile at emit, ship compiled form in the manifest):** the build is paid **once at compile/deploy**, folds into the existing `sourceChecksum + templateMajor` cache (`crates/runtara-workflows/src/compile.rs:12`, `TEMPLATE_MAJOR_VERSION:32`), the compiler is **native, host-testable Rust**, and the guest pays neither the string re-parse nor the raw-`Value` walk.

**Recommendation: Option B.** The manifest is already decoded once per run (`DirectJsonManifest::parse:320`), so Option A buys nothing the interpreter doesn't already amortize. Both A and B must *build* a `CompiledExpr`; the only axis is *where*, and emit-time wins on caching (byte-identical artifacts via the SHA-256 checksum, `manifest.rs:586`), native testability, and removing the build from the hot guest.

Concretely: `crates/runtara-workflows/src/direct_wasm/manifest.rs` (where conditions/mappings are stored verbatim today via `canonical_json:1318`) produces the compiled form and serializes it into the manifest alongside (during differential testing) or instead of (final cutover) the raw `Value`. The `CompiledExpr` type lives in a **shared crate** that both `runtara-workflows` (emit) and `runtara-workflow-stdlib` (runtime) depend on, kept **off** the compiler feature gate (`crates/runtara-workflows/src/direct_wasm/mod.rs:19`) and `wasm32`-clean. Recommended location: a new `runtara-expr` crate (smallest dependency surface; avoids dragging the full DSL into the wasm guest). Bump `DIRECT_WORKFLOW_MANIFEST_VERSION` (`manifest.rs:32`, currently 2) + `templateMajor` to invalidate cached images cleanly on rollout.

> Pragmatic fallback if a fully-serializable `CompiledExpr` proves heavy (minijinja `Template` is not trivially serializable): ship the **normalized, validated** expression IR (operator enum, pre-split segments, classified args) and reconstruct the few non-serializable leaves (the minijinja `Template`) from the carried template **string** during a cheap `DirectJsonManifest::parse`-time finalize. This keeps the expensive parsing (path/op/arg classification) emit-time while keeping the manifest serializable and deterministic.

### 5.4 Caching / lifecycle

- **Storage:** the compiled form lives **inside `DirectJsonManifest`**, next to the raw `value: Value` in each config struct (`DirectJsonCondition:5659`, `DirectJsonMapping:5652`, `DirectJsonFilter`, `DirectJsonSwitch`, `DirectJsonWhile`, etc. `:5652`–`:5749`). The manifest already owns the source `Value` per id; the compiled form sits beside it.
- **Cache key:** the existing `(kind, u32 id)` tuple — no new identity scheme. The WIT boundary already passes the integer id (`eval_condition(condition_id, source)` `lib.rs:171`).
- **Second key space:** Wait/Delay/breakpoint step-body expressions are addressed by `step_id: &str` against `self.steps` (`wait_timeout_ms:1122` reads `step.body.get("timeoutMs")`; `breakpoint_key:933`). The cache needs a `String`-keyed side table (step_id + body-field discriminator) alongside the `u32` maps. *(Optional later: assign synthetic `u32` ids to these at emit time for a uniform keyspace — emitter change in `manifest.rs:592`.)*
- **Sub-expression keys:** Switch case outputs, GroupBy keys, and synthesized case conditions have no id of their own. Compile the **whole config node** into a compiled config object (`CompiledSwitch` holding `Vec<CompiledCase>`, `CompiledFilter` holding compiled condition + compiled value), so sub-expressions are addressed structurally within the parent's compiled form — no composite-id scheme needed.
- **No thread-local cache.** A separate `thread_local! EXPR_CACHE` is strictly worse: it would need its own reset hook in `init_manifest` (the manifest-owned form is reset for free on fresh parse), it would need the same `(kind,id)` discriminator the manifest already provides, and it splits ownership from the `Value` it was compiled from.
- **Eager vs lazy:** with Option B the heavy compilation is emit-time, so `DirectJsonManifest::parse` does only a cheap **finalize** (reconstruct non-serializable leaves). Do the finalize **eagerly in `parse`** — no interior mutability, matches the parse-once lifetime, avoids RefCell re-entrancy hazards in recursive condition/composite compilation, and runs are short-lived per instance so wasted finalize of never-taken branches is bounded. The existing `debug_start_ms: RefCell` precedent remains the model if any lazy field is later needed.
- The manifest **retains the raw `value: Value`** during differential testing (the interpreter-as-oracle reads it) and for the debug `input_mapping` echo (`step_debug_switch` asserts `input_mapping["cases"][0]["match"] == "active"`), so keep the raw `Value` until the oracle is retired (see §7).

---

## 6. Workstreams

### W1 — `CompiledExpr` IR + leaf evaluator (shared crate `runtara-expr`)

- New crate; `wasm32`-clean; depends only on `serde`/`serde_json`/`minijinja`. Off the compiler feature gate.
- Define the IR (§5.1) with deterministic serialization (no `HashMap`; canonical floats — guards the byte-identical-artifact invariant `manifest.rs:586`).
- Re-export / depend on the leaf helpers: `values_equal`, `is_truthy`, `to_number` (move from `conditions.rs` into `runtara-expr`, or keep in stdlib and have the IR call them — prefer moving so both emit and runtime share one vocabulary). `apply_type_hint` (`:5310`) coercion table → `TypeHint::apply`.
- `eval(&self, source) -> Result<bool|Value>` with `Cow`-borrowing `CompiledReference::resolve` (`Full` and `NaiveDot` resolvers), preserving handle deref at every segment + leaf `materialize`.

### W2 — Emit-time compiler (`crates/runtara-workflows/src/direct_wasm/manifest.rs`)

- New `compile_condition` / `compile_mapping` / `compile_reference` producing `CompiledExpr` from the same `Value` that `canonical_json` (`:1318`) stores today.
- Desugar Switch `BETWEEN`/`RANGE`/array-`EQ` at compile time (mirror `build_between_condition:4085`/`build_range_condition:4108`/`:4047` edge behavior exactly).
- Compile `AgentInputMapping` with the resolve/keep flags from `is_field_argument_operator`/`is_qualified_workflow_path` (port the rules from `resolve_nested_references:5168`).
- Emit the compiled form into the manifest; bump `DIRECT_WORKFLOW_MANIFEST_VERSION` (`:32`) + `TEMPLATE_MAJOR_VERSION` (`compile.rs:32`). Transport unchanged (sidecar / custom section / static segment — `compile.rs:678`, `static_data.rs:160`); watch manifest size (guest memory budget — use compact tags, store compiled-form-instead-of-raw at cutover).

### W3 — Runtime cutover (`crates/runtara-workflow-stdlib/src/direct_json.rs`, `lib.rs`)

- Add the compiled field to each config struct (`:5652`–`:5749`) and a `String`-keyed side table for step-body expressions.
- `DirectJsonManifest::parse` (`:320`): eager finalize of compiled forms (reconstruct minijinja `Template`s from carried strings).
- Replace each evaluation entry to call `compiled.eval(&source)` instead of `eval_condition_expression`/`apply_mapping_value`/`apply_filter` body: `eval_condition:385`, `apply_mapping:365`, `process_switch:396`/`value_switch:852`, `filter:831` (per-element compiled condition + scope-push, no source clone), `group_by:874`, `while_condition:746`, `delay_duration_ms:895`, `wait_timeout_ms:1122`, `wait_action_mapping:4810`, `log/error:2462`/`:2508`, `split_items:3442`/`split_iteration_variables:3610`.
- Wire `debug_start_data:2597` / `debug_end_output:2738` / `breakpoint_event:962` to the same compiled artifacts, preserving the `.ok()/unwrap_or(Null)` error-tolerance (step-error attribution invariant).
- Keep `process_switch_output` (`switch_helpers.rs:18`) as the `NaiveDot` compiled reference (preserve divergence).

### W4 — Interning fast paths (`direct_json.rs`)

- `StoreEntry` (`:75`): add `top_level_kind`/`length`/`byte_size` (captured at `intern_if_large:198`) + `OnceCell<Rc<Value>>` parsed-value memo.
- New handle-aware bounded-debug short-circuit before materialize (`debug_start_data:2627`, `step_debug_start:2558`).
- `deref_handle:245` reads through the `OnceCell` memo. Invalidate via `reset_value_store:84` (free — fresh manifest per run).

---

## 7. Migration — full replacement + differential testing

### Cutover

1. Land W1 + W2 + W3 with the manifest carrying **both** the raw `Value` and the compiled form (manifest version bumped). Production evaluates via the **compiled** form; the raw `Value` is retained only for the oracle and the debug `input_mapping` echo.
2. Keep the current interpreter functions (`eval_condition_expression`, `apply_mapping_value`, `apply_filter`, `apply_switch`, `apply_group_by`, `split_items`, `resolve_nested_references`, `lookup_source_path`) compiled **only under a host-only `compiled-oracle` cargo feature** (or `#[cfg(test)]`) — never in the `wasm32` build.
3. After the verification matrix is green, retire the oracle: delete the interpreter functions and the raw-`Value` retention (store compiled-form-only), shrinking the manifest. This is the final full-replacement state — **no permanent production fallback**.

### Differential-testing strategy (temporary interpreter-as-oracle)

There is **no** existing eval-vs-eval harness (the historical A/B suite compared against the now-deleted generated compiler and was retired — `fixture_smoke.rs` header). Build a fresh oracle inside `runtara-expr`/stdlib tests:

- Corpus of `(expr_json, source_json)` covering **every** operator arm incl. edge cases: arity-underflow (false), `NOT`/`IS_EMPTY` missing-arg defaults, query-only ⇒ **Err** (not false), String↔Number coercion, `NE` of null, type-hint edges (NaN/Inf ⇒ 0, fractional ⇒ boolean false, integer null stays Null), bracket + `~0/~1` escaped paths, `$wfref` handles at root/mid/leaf, Switch `BETWEEN`/`RANGE`/array-`EQ`, GroupBy `_null`/`_invalid` keys, agent-input column-ref preservation, Switch naive-output paths (incl. bracket paths that *must stay unresolved*).
- Plus every fixture under `crates/runtara-workflows/tests/fixtures`.
- A table/proptest asserting `compile(expr).eval(source) == interpret(expr, source)` — including **Err-equality** — for conditions (bool) and mappings (Value). Route the corpus **through the public `DirectJsonManifest` methods** (real id→cache lookup), not just internal fns.

### Verification matrix

| Layer | What | How |
|---|---|---|
| Unit (stdlib) | `eval_condition_handles_equality_against_source` (`:6784`), `eval_condition_errors_on_query_only_operators` (`:6896`), `eval_condition_handles_length_comparison` (`:6931`), `eval_condition_handles_truthy_value_expression` (`:6955`), `apply_mapping_handles_defaults_templates_and_composites` (`:6506`), `template_resolves_through_interned_loop_outputs` (`:6563`), `agent_mapping_*` (`:6637`–`:6760`), `filter_keeps_items_matching_condition` (`:7516`), `filter_supports_nested_boolean_conditions` (`:7548`), `filter_treats_non_array_input_as_empty_array` (`:7606`), `value_switch_*`/`routing_switch_*` (`:7631`–`:7792`), `group_by_*` (`:7793`), `split_*` (`:6968`), `while_helpers_match_generated_state_condition_and_output_shape` (`:7394`) | `cargo test -p runtara-workflow-stdlib` |
| Unit (debug shapes — byte-exact) | `filter_debug_payloads_are_bounded` (`:9573`), `filter_debug_end_reads_stored_output_without_recomputing` (`:9640`), `breakpoint_key_and_event_match_generated_shape` (`:7935`), `agent_breakpoint_event_uses_mapped_inputs_before_connection_injection` (`:8294`), `step_debug_switch_payloads_include_inputs_and_route` (`:10182`) | same |
| Parity harness | new `compile==interpret` differential corpus + fixtures | `cargo test -p runtara-expr` (oracle feature) |
| Structural compile | 122 wasmparser tests (import presence + stdlib host-call **ordering** e.g. apply-mapping before breakpoint-source `~:1397`) | `cargo test -p runtara-workflows` (per MEMORY `feedback_verify_emitter_lib_tests`) |
| Tier-A compile battery | `every_fixture_lowers_as_expected` (`fixture_smoke.rs:134`) — every fixture still lowers | `cargo test -p runtara-workflows --test fixture_smoke` |
| Perf bench | `filter_perf_breakdown` (`:9688`, `#[ignore]`) — assert sub-µs per element | `cargo test -p runtara-workflow-stdlib -- --ignored filter_perf_breakdown` |
| Gated behavioral e2e | filter/switch/groupby/while/conditional/edge-condition execute, step-error attribution (`*_records_step_error` `:1509`–`:1742`), `direct_wasm_execute_while_template_reads_interned_loop_outputs` (`:4441`) | `RUNTARA_RUN_DIRECT_WASM_E2E=1 cargo test -p runtara-workflows --test direct_wasm_execute` (per MEMORY `feedback_always_e2e_verify`) |
| Full e2e (Reconcile shape) | boot full server stack (embedded WASM runner), compile+register+execute a large-Filter "Reconcile"-shaped workflow, assert observable output + measure end-to-end | `e2e-verify` skill |
| WASM dead-code | host clippy cannot catch wasm32 `-D warnings` dead-code in the guest | `scripts/build-agent-components.sh` before release (MEMORY `wasm_deadcode_ci_gap`) |

After regenerating WIT bindings (only if the manifest WIT world changes), **revert unrelated `bindings.rs` churn** (MEMORY `bindings_regen_churn`).

---

## 8. Risks & mitigations

| Risk | Mitigation |
|---|---|
| **Two divergent resolvers accidentally unified** — routing Switch outputs through the full resolver changes behavior for bracketed/interned paths (`switch_helpers.rs:51` vs `lookup_source_path:5388`) | Model as distinct `Resolver::NaiveDot` vs `Full` compiled nodes; differential corpus must include Switch-output bracket/handle paths that **must stay unresolved**. |
| **`resolve_nested_references` mis-modeled** — collapsing a column-name literal into a resolved ref silently makes object-model queries match nothing (the original bug it fixed, comments `:5153`) | Compile per-node resolve/keep flags from `is_field_argument_operator:5287` + `is_qualified_workflow_path:5280`; cover with `agent_mapping_*` tests + corpus column-ref cases. |
| **Query-only ops silently defaulting to false** | `CompiledOp::QueryOnlyError` returns the same `Err` (`:4929`); guarded by `eval_condition_errors_on_query_only_operators:6896` + Err-equality in the oracle. |
| **Numeric/equality drift** | Reuse `conditions.rs` `values_equal`/`to_number`/`is_truthy` and the `apply_type_hint` table verbatim (move into `runtara-expr`); never reimplement with native `PartialEq`. |
| **`$wfref` handle regression** — pre-materializing the whole source reintroduces large-scope OOM (MEMORY `bump_allocator_leak`, `interning_template_handle_opacity`); leaking bare handles reintroduces the 8.0.19 minijinja-undefined regression | Compiled references deref **lazily at traversal time** + materialize leaf only; Template node receives `materialize(source)` (handle-free). Tests: `template_resolves_through_interned_loop_outputs:6563`, `while_template_reads_interned_loop_outputs:4441`. |
| **GC handle-shape breakage** — compiled refs must not strip `{"$wfref":id}` from onward-serialized scope (`value_store_retain:100`) | Compiled evaluation is read-only into a private result; never rewrites the scope that flows to the next step. |
| **Debug byte-exact shapes** (BTreeMap key ordering, `_truncated/_keys/_bytes`, finish unwrap, agent immediate-unwrap) | Compiled layer changes only *how* inputs are computed; feed identical values into the existing `bounded_debug_value:3496`/`debug_event_base` builders; guarded by the debug-shape unit tests above. |
| **Manifest size growth in guest linear memory** (`static_data.rs:160`) | Compact tags; at final cutover store compiled-form-**instead-of**-raw (drop the `Value`), net-shrinking the manifest. |
| **Non-deterministic serialization breaks SHA-256 cache** (`manifest.rs:586`) | No `HashMap`; canonical float formatting; covered by a "byte-identical for identical graph" test. |
| **Structural-test breakage if the WIT import surface changes** | Prefer keeping the existing `eval-condition`/`apply-mapping` WIT exports (compiled form is internal to the stdlib); if changed, update the 122 structural tests + emitter call sites in lockstep. |

---

## 9. Sequencing / milestones

| Milestone | Scope | Rough effort |
|---|---|---|
| **M0 — IR + leaf eval** | `runtara-expr` crate: `CompiledExpr` IR (§5.1), `TypeHint::apply`, `CompiledReference::resolve` (Full + NaiveDot, Cow-borrow + handle deref), move `values_equal`/`is_truthy`/`to_number`. Unit tests per node. | 3–4 d |
| **M1 — emit-time compiler** | `compile_condition`/`compile_mapping`/`compile_reference` in `manifest.rs`; Switch desugar; `AgentInputMapping` flags; deterministic serialization; manifest version bump. | 4–5 d |
| **M2 — differential oracle** | interpreter-as-oracle behind `compiled-oracle` feature; corpus (every operator arm + edge cases + fixtures); `compile==interpret` proptest incl. Err-equality, routed through public `DirectJsonManifest` methods. | 3–4 d |
| **M3 — runtime cutover** | W3: compiled fields on config structs + String-keyed step-body side table; eager finalize in `parse`; replace all evaluation entries; per-element scope-push in `apply_filter`/`split`/`group_by` (no source clone); debug/breakpoint rewire. Full unit + structural + Tier-A green. | 5–6 d |
| **M4 — interning fast paths** | W4: `StoreEntry` metadata + `OnceCell` memo; handle-aware bounded debug; bench `filter_perf_breakdown` sub-µs; gated `RUNTARA_RUN_DIRECT_WASM_E2E`; full e2e on Reconcile shape. | 3–4 d |
| **M5 — retire oracle / final replacement** | Delete interpreter functions + raw-`Value` retention; store compiled-form-only; final manifest version bump; release-gate via `scripts/build-agent-components.sh`. | 2 d |

Total ~20–25 engineer-days. M0–M2 are landable without changing production behavior (oracle off in prod); M3 is the behavior-affecting cutover; M4 delivers the headline perf; M5 is the cleanup that realizes the manifest-size win.

---

## 10. Open questions / decisions still needed

1. **Switch case-output resolver: preserve or unify?** Recommendation: **preserve** the `NaiveDot` divergence (compile as a distinct resolver node) for the initial cutover — unifying onto the full resolver is a deliberate behavior change (bracket indexing, `$wfref` deref, defaults, type hints in Switch outputs) that needs separate product sign-off and its own fixture coverage. Flagged for a follow-up decision.
2. **Shared-crate location:** new `runtara-expr` (recommended — smallest wasm32 dependency surface) vs. inside `runtara-dsl` (already shared by emitter + validator, but heavier in the guest). Decide before M0.
3. **`CompiledExpr` serializability vs. minijinja `Template`:** fully serialize the compiled form, or ship the normalized IR + reconstruct the minijinja `Template` from the carried string at parse-time finalize (the §5.3 fallback)? Recommendation: ship IR + reconstruct leaves — keeps the manifest serializable/deterministic while keeping the expensive parsing emit-time.
4. **One release of coexistence in production-shaped artifacts?** Should the manifest carry both raw + compiled for one shipped release (oracle-in-prod behind a flag) before deleting the interpreter, or retire the oracle entirely in dev? Recommendation: coexist for exactly **one** release behind the manifest-version gate, then delete in M5.
5. **Synthetic ids for step-body expressions?** Promote Wait `timeoutMs`/`pollIntervalMs` and breakpoint conditions to emit-time `u32` ids for a uniform keyspace (emitter change in `manifest.rs:592`), or keep the `String`-keyed side table? Recommendation: ship M3 with the side table; promote to synthetic ids only if the side table proves awkward.
6. **Source-parse-once at the WIT boundary:** the compiled layer removes expr-tree/path re-walk but **not** the per-call `serde_json::from_slice` of the source envelope (`lib.rs:171` and ~40 siblings; re-run per loop iteration). Removing it needs a WIT/protocol change to pass a stable scope identity/version so the parsed source + handle memo can be reused across the many host calls within one step/iteration. Out of scope for this work; recommended as a **follow-up** once the expression cost is gone and the source-parse becomes the next dominant term.
