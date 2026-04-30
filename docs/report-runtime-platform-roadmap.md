# Object Model Report Runtime Roadmap Spec

Status: planning spec for follow-up implementation sessions.

Reference baseline: commit `53ff9ac` (`Implement MCP authoring and runtime fixes`).

## Purpose

This document narrows the remaining work to practical Object Model reporting
scenarios. It intentionally excludes execution queue visibility, wakeup
lifecycle, and deployment-gateway OpenAPI concerns.

The current implementation should focus on reports that read Object Model
schemas such as:

- `StockSnapshot` fact rows.
- `TDProduct` product/dimension rows.
- Category filters backed by Object Model data.
- Tables that need product fields next to stock rows.
- Aggregate reports that need to filter or group fact rows by product/category
  fields without denormalizing everything up front.
- A stock delta style report that may need aggregate-then-aggregate behavior.

## In Scope

- Report runtime planning over Object Model sources.
- Report source joins between Object Model schemas.
- Scalar joined value columns in report tables.
- Report-native dynamic condition values from report filters.
- Same-store condition subqueries to avoid very large inline `IN` lists.
- SQL-level joined aggregate planning for Object Model schemas where broadcast
  joins stop scaling.
- Two-stage aggregation only for concrete report needs such as first/last daily
  sums.
- Backend, MCP authoring schema, validation, tests, and frontend report builder
  support where a public authoring shape is exposed.

## Out Of Scope

- Workflow execution queue visibility.
- Wakeup prompt lifecycle.
- Tenant-prefixed OpenAPI or gateway routing.
- Arbitrary SQL authoring.
- Cross-database joins.
- Multi-hop joins.
- right/full outer joins.
- Report scheduling/email delivery.

## Current State

### Report Engine

Relevant files:

- `crates/runtara-server/src/api/dto/reports.rs`
- `crates/runtara-server/src/api/services/reports.rs`
- `crates/runtara-server/src/mcp/tools/reports.rs`
- `crates/runtara-server/frontend/src/features/reports/types.ts`
- `crates/runtara-server/frontend/src/features/reports/components/ReportDefinitionBuilder.tsx`
- `docs/report-block-joins-future-work.md`
- `docs/reporting-module-spec.md`

Known current behavior:

- `ReportSource.join` exists on block-level sources.
- `ReportSource.join` is implemented only on aggregate paths as a
  broadcast-hash join:
  - dimension rows are resolved first,
  - primary aggregate query is pushed down with `parent_field IN [keys]`,
  - aggregate rows are enriched afterward with `<alias>.<field>` columns.
- Filter-mode table rendering currently ignores `block.source.join`.
- Qualified conditions are split only for simple shapes; full `AND`/`OR`/`NOT`
  post-filtering is not implemented.
- `ReportTableColumnType::Value` already exists, but scalar joined lookup
  behavior does not.
- `ReportTableColumnSource` has per-cell chart joins but lacks `select` for
  scalar value lookup.
- Frontend report types know `type: "value" | "chart"` for columns, but do not
  yet model block-level `source.join`.
- Report condition validation rejects workflow-style `MappingValue` objects.
- `validate_report` rejects many unknown keys and gives similar-key hints.

### Object Model Query Engine

Relevant files:

- `crates/runtara-object-store/src/sql/condition.rs`
- `crates/runtara-object-store/src/sql/aggregate.rs`
- `crates/runtara-server/src/api/dto/object_model.rs`
- `crates/runtara-server/src/api/services/object_model.rs`

Known current behavior:

- Object Model conditions compile to SQL from JSON condition shapes.
- Condition errors are path-aware.
- Aggregate order-by supports vector distance expressions.
- There is no Object Store SQL-level joined aggregate request.
- There is no condition subquery syntax.

## Design Principles

1. Keep this roadmap about Object Model report querying only.
2. Prefer one report query planner over more ad hoc render-path patches.
3. Keep report condition semantics report-native; do not reuse workflow
   `MappingValue` in report source conditions.
4. Preserve direct Object Model service paths for simple filters/aggregates.
5. Use broadcast joins only as a bounded compatibility path.
6. Push large joins, subqueries, and staged aggregation into SQL when result sets
   can be large.
7. Prefer same-store SQL features over cross-store emulation. Cross-store
   subqueries/joins should fail clearly unless a bounded fallback is explicitly
   implemented.
8. Public authoring shapes require MCP schema, validation, tests, and frontend
   type/builder support.

## Decisions

- Reuse the existing `ReportTableColumnType::Value`; Phase 2 adds runtime
  behavior, `source.select`, and validation.
- Include frontend report type/builder updates when exposing `source.join`,
  `source.select`, or scalar value column behavior.
- Broadcast join post-filtering should be correct within a hard cap. If a query
  needs more than the cap, fail with a targeted error rather than returning a
  misleading short page or approximate `totalCount`.
- Add structured report diagnostics at the API layer. The frontend can render
  diagnostics as a bottom-of-report callout/note.
- Scalar `cardinality: "first"` should be deterministic. Prefer explicit
  `orderBy`, use stable fallback ordering when necessary, and warn when the
  lookup may be ambiguous.
- Add `kind` to column-level joins. Default scalar value lookup behavior should
  be `left`, returning `null` for missing matches. `inner` can drop unmatched
  parent rows when explicitly requested.
- Dynamic filter paths should cover current filter types:
  - `value` for scalar filters,
  - `values` for multi-select filters,
  - `from` and `to` for time ranges,
  - `min` and `max` for number ranges.
- SQL subqueries are same-store only at first. If the parent source and subquery
  source resolve to different physical Object Stores/connections, reject with a
  clear error.

## Target Authoring Shapes

### Block Source Join

Keep the existing shape backward-compatible:

```json
{
  "source": {
    "schema": "StockSnapshot",
    "mode": "aggregate",
    "join": [
      {
        "schema": "TDProduct",
        "alias": "product",
        "parentField": "sku",
        "field": "sku",
        "kind": "inner"
      }
    ],
    "condition": {
      "op": "IN",
      "arguments": ["product.category_leaf_id", ["leaf_a", "leaf_b"]]
    },
    "groupBy": ["sku", "product.part_number"],
    "aggregates": [{ "alias": "qty", "op": "sum", "field": "qty" }]
  }
}
```

Initial support should stay single-hop and single-key. Multi-key joins can be
designed later with:

```json
{
  "schema": "TDProduct",
  "alias": "product",
  "keys": [
    { "parentField": "sku", "field": "sku" },
    { "parentField": "tenant_sku_scope", "field": "tenant_sku_scope" }
  ],
  "kind": "inner"
}
```

Compatibility rule:

- Continue accepting `parentField` + `field`.
- Do not serialize canonical `keys` until a versioned API decision is made.

### Scalar Joined Value Column

Use existing `column.type: "value"` for row-level scalar lookups.

```json
{
  "type": "table",
  "source": {
    "schema": "StockSnapshot",
    "mode": "filter",
    "condition": { "op": "GT", "arguments": ["qty", 0] }
  },
  "table": {
    "columns": [
      { "field": "sku", "label": "SKU" },
      {
        "field": "part_number",
        "label": "Part Number",
        "type": "value",
        "source": {
          "schema": "TDProduct",
          "select": "part_number",
          "join": [
            { "parentField": "sku", "field": "sku", "kind": "left" }
          ],
          "orderBy": [{ "field": "createdAt", "direction": "asc" }]
        }
      }
    ]
  }
}
```

Semantics:

- `source.select` is required when `type: "value"` has a `source`.
- Lookup is evaluated for visible rows, batched by join key.
- No N-query-per-N-row implementation.
- Missing left-join lookups return `null`.
- Explicit `kind: "inner"` can remove unmatched parent rows.
- Default cardinality is `first`.
- Add optional `cardinality: "first" | "single" | "array"` only when needed.
- `single` errors if more than one matching row exists.
- `first` must be deterministic. Prefer explicit `orderBy`; otherwise use a
  stable fallback and emit a warning when uniqueness is unknown.

Shortcut when block-level `source.join` already enriched the row:

```json
{ "field": "product.part_number", "label": "Part Number", "type": "value" }
```

### Dynamic Report Condition Values

Do not put workflow `MappingValue` inside report conditions. Add report-native
condition argument references:

```json
{
  "op": "EQ",
  "arguments": [
    "category_leaf_id",
    { "filter": "selected_category", "path": "value" }
  ]
}
```

```json
{
  "op": "IN",
  "arguments": [
    "category_leaf_id",
    { "filter": "selected_categories", "path": "values" }
  ]
}
```

Supported initial paths:

- `{ "filter": "<filterId>", "path": "value" }`
- `{ "filter": "<filterId>", "path": "values" }`
- `{ "filter": "<filterId>", "path": "from" }`
- `{ "filter": "<filterId>", "path": "to" }`
- `{ "filter": "<filterId>", "path": "min" }`
- `{ "filter": "<filterId>", "path": "max" }`

Resolution timing:

- Resolve dynamic values in the report service before calling Object Model
  filter/aggregate APIs.
- After resolution, Object Model receives only literal condition values or
  internal subquery representations.
- Validation checks that referenced filters exist.
- `IN` requires an array-like resolved value.
- Empty optional filters should prune their condition term where safe.
- Missing required filter values should produce a validation/runtime error.

### Same-Store Subquery Conditions

Prefer subqueries over very large inline `IN` lists.

```json
{
  "op": "IN",
  "arguments": [
    "sku",
    {
      "subquery": {
        "schema": "TDProduct",
        "select": "sku",
        "condition": {
          "op": "IN",
          "arguments": [
            "category_leaf_id",
            { "filter": "selected_categories", "path": "values" }
          ]
        }
      }
    }
  ]
}
```

Object Store SQL target:

```sql
sku IN (
  SELECT product.sku
  FROM td_product product
  WHERE product.category_leaf_id = ANY($1)
)
```

Rules:

- `subquery.schema` and `subquery.select` are required.
- `subquery.connectionId` may be added; default it to the parent source
  connection.
- Parent source and subquery source must resolve to the same physical Object
  Store/database at first.
- Subquery conditions use the same dynamic-value resolution before SQL
  compilation.
- Subquery select field type must be compatible with the left-hand field type.
- Reject nested subqueries until there is a concrete use case.
- Reject unbounded subqueries in user-facing filter option endpoints unless the
  parent query has a limit or aggregate grouping.

### Two-Stage Aggregation

Keep staged aggregation late and focused on concrete report needs, such as
computing first/last over daily `SUM(qty)` rather than arbitrary warehouse rows.

```json
{
  "source": {
    "schema": "StockSnapshot",
    "mode": "aggregate",
    "stages": [
      {
        "id": "daily",
        "groupBy": ["sku", "snapshot_date"],
        "aggregates": [
          { "alias": "qty_sum", "op": "sum", "field": "qty" }
        ]
      },
      {
        "id": "sku_summary",
        "groupBy": ["sku"],
        "aggregates": [
          {
            "alias": "first_qty",
            "op": "first_value",
            "field": "qty_sum",
            "orderBy": [{ "field": "snapshot_date", "direction": "asc" }]
          },
          {
            "alias": "last_qty",
            "op": "last_value",
            "field": "qty_sum",
            "orderBy": [{ "field": "snapshot_date", "direction": "asc" }]
          }
        ]
      }
    ]
  }
}
```

Rules:

- `stages` and top-level `groupBy`/`aggregates` are mutually exclusive at first.
- Every stage after the first can reference only fields emitted by the prior
  stage.
- `first_value`/`last_value` over a prior aggregate must order by a prior-stage
  emitted dimension.
- Implement with SQL subqueries, not unbounded in-memory transforms.

## Phase Plan

### Phase 0: Planner Boundary And Fixtures

Goal: make later report changes additive instead of scattering more logic
through render paths.

Tasks:

1. Introduce an internal report query planner module, for example
   `crates/runtara-server/src/api/services/reports/query_plan.rs`.
2. Define internal structs only where they clarify existing behavior:
   - `ReportQueryPlan`
   - `ReportSourcePlan`
   - `JoinPlan`
   - `ProjectionPlan`
   - `ReportDiagnostic`
3. Move existing join helpers behind planner functions without changing public
   behavior:
   - `aggregate_with_optional_joins`
   - `resolve_join`
   - `enrich_aggregate_result`
   - condition splitting helpers
4. Add reusable test fixtures:
   - `StockSnapshot`
   - `TDProduct`
   - category ids
   - daily stock quantities

Acceptance criteria:

- No public DTO changes.
- Existing report tests still pass.
- New planner unit tests cover current aggregate broadcast-join behavior.
- `cargo test -p runtara-server mcp::tools::reports::tests --lib`
- `cargo test -p runtara-server api::services::reports --lib`

### Phase 1: Dynamic Report Condition Values

Goal: allow simple filter substitution directly inside report source
conditions, without workflow `MappingValue` and without overusing
`filterMappings`.

Tasks:

1. Add report-native condition value detection and resolution.
2. Support `value`, `values`, `from`, `to`, `min`, and `max` paths.
3. Validate filter references in service validation and MCP
   `validate_report`.
4. Resolve dynamic values before:
   - block render,
   - block data endpoint,
   - dataset query endpoint,
   - filter options endpoint when applicable.
5. Preserve `filterMappings` for broad report-level filter application.
6. Update MCP authoring schema and examples.

Acceptance criteria:

- This works:

  ```json
  {
    "op": "EQ",
    "arguments": ["category_leaf_id", { "filter": "category", "path": "value" }]
  }
  ```

- Multi-select filters can drive `IN`.
- Range filters can drive `GTE`/`LT` or similar condition terms.
- `validate_report` rejects unknown filter ids.
- Workflow `MappingValue` objects are still rejected with a hint to use
  `{ "filter": ... }`.

Tests:

- Scalar filter resolution.
- Multi-select filter resolution.
- Time/number range path resolution.
- `IN` with scalar mismatch.
- Unknown filter id error path.

### Phase 2: Complete Bounded Broadcast Join Coverage

Goal: make existing `ReportSource.join` useful for common Object Model report
tables and aggregate filters.

Tasks:

1. Add filter-mode table support for `block.source.join`.
2. Add frontend `ReportSource.join` typing and builder preservation/editing.
3. Support qualified condition refs in full condition trees:
   - `AND`
   - `OR`
   - `NOT`
4. For broadcast joins, perform bounded two-step evaluation:
   - push down a safe over-approximation to the primary query,
   - fetch primary candidate rows up to `MAX_JOIN_POST_FILTER_ROWS`,
   - enrich rows,
   - post-filter rows against the original condition,
   - paginate after post-filtering.
5. Support qualified `orderBy` after enrichment only when result size is within
   the cap.
6. Add structured diagnostics when SQL limit pushdown is disabled.
7. Add targeted validation/runtime errors when a requested qualified operation
   cannot be evaluated safely.
8. Update `get_report_authoring_schema` with working examples.

Implementation notes:

- Keep `MAX_BROADCAST_JOIN_DIM_ROWS`.
- Add `MAX_JOIN_POST_FILTER_ROWS`.
- Do not return approximate `totalCount` for post-filtered joins. Return exact
  count within cap or fail with a clear message.

Acceptance criteria:

- A table block can show `StockSnapshot` rows with `TDProduct.part_number`
  using `source.join`.
- A report can filter `StockSnapshot` by `TDProduct.category_leaf_id` without a
  denormalization workflow.
- `OR`/`NOT` qualified conditions either work within the cap or fail with a
  targeted message.
- Qualified sort returns exact results within the cap.

Tests:

- Unit tests for condition alias collection and post-filter evaluation.
- Service tests for table filter-mode joins.
- MCP `validate_report` tests for supported and unsupported join shapes.
- Frontend type/builder tests where practical.

### Phase 3: Scalar Joined Value Columns

Goal: support scalar lookups next to table rows without requiring denormalized
fields.

DTO changes:

- Reuse `ReportTableColumnType::Value`.
- Add `select: Option<String>` to `ReportTableColumnSource`.
- Add `kind: Option<ReportJoinKind>` to `ReportTableColumnJoin` or a compatible
  column-level join kind.
- Optionally add `cardinality: Option<ReportValueCardinality>` if needed.

Tasks:

1. Validate `type: "value"` columns:
   - `source.select` is required when `source` is present,
   - `source.aggregates` must be empty,
   - `source.groupBy` must be empty unless cardinality semantics require it,
   - join fields and select field must exist.
2. Implement visible-row lookup.
3. Batch lookups by join key instead of querying once per row.
4. Support block-level enriched fields like `product.part_number` as a cheap
   projection when the block source already joined `product`.
5. Add deterministic `first` behavior.
6. Add diagnostics for ambiguous `first` when uniqueness is unknown and no
   explicit `orderBy` is present.
7. Update frontend report types/builder.
8. Update authoring schema and examples.

Acceptance criteria:

- A table can show `sku`, `qty`, and `TDProduct.part_number` from
  `StockSnapshot` without backfilling `part_number`.
- The implementation does not issue N queries for N rows.
- Missing lookup returns `null` for left semantics.
- Explicit inner semantics can drop unmatched rows.
- Multi-match behavior is deterministic and documented.

Tests:

- Projection from block-level join.
- Column-level source join lookup.
- Left missing lookup returns `null`.
- Inner missing lookup removes the row.
- Multi-match deterministic first lookup.
- `single` validation/error if cardinality is added.

### Phase 4: Same-Store Condition Subqueries

Goal: replace huge inline `IN` lists with SQL-backed subqueries for Object Model
schemas in the same store.

Tasks:

1. Add report DTO parsing/validation for `{ "subquery": ... }` condition values.
2. Add internal Object Store condition representation for subquery operands.
3. Add SQL generation for:
   - `field IN (SELECT select FROM schema WHERE condition)`
   - optionally `EXISTS` only if a concrete use case appears.
4. Ensure tenant/schema scoping is preserved.
5. Reject cross-store subqueries with a clear error.
6. Add validation for field compatibility.
7. Reject nested subqueries until there is a concrete use case.

Acceptance criteria:

- A report can express:

  `StockSnapshot.sku IN (SELECT TDProduct.sku WHERE category_leaf_id IN filters.category.values)`

- No massive value list is serialized through MCP or report JSON at render time.
- Query plans remain parameterized.
- Cross-store cases fail clearly.

Tests:

- SQL generation unit tests in `runtara-object-store`.
- Report service tests with category-filtered product subquery.
- MCP validation tests for malformed subqueries.
- Cross-store rejection test if connection resolution can be exercised.

### Phase 5: SQL-Level Joined Aggregate Planner

Goal: move beyond broadcast joins for large fact/dimension workloads and enable
joined aggregate fields safely.

Scope:

- Same Object Store only.
- Single-hop joins.
- Inner and left joins.
- Single-key joins at first.
- Internal request shape first; expose public Object Model joined query API only
  if needed.

Tasks:

1. Add an internal Object Store joined aggregate request shape:
   - base schema,
   - joins,
   - condition,
   - group_by,
   - aggregates,
   - order_by,
   - limit/offset.
2. Emit SQL with explicit table aliases.
3. Support qualified refs in:
   - group_by,
   - condition,
   - order_by,
   - aggregate field where safe.
4. Preserve broadcast path for smaller legacy requests.
5. Have the report planner choose SQL joins when qualified conditions/grouping
   or row counts make broadcast impractical.
6. Add structured diagnostics that explain whether a report used broadcast or
   SQL join planning.

Acceptance criteria:

- Report can aggregate `StockSnapshot` grouped by
  `TDProduct.category_leaf_id`.
- Report can filter fact rows by dimension fields without materializing a large
  key list in memory.
- Qualified `orderBy` does not require unbounded in-memory sorting.

Tests:

- SQL rendering tests for joins.
- Integration tests against Postgres/pgvector container if available.
- Regression tests for old broadcast join behavior.

### Phase 6: Two-Stage Aggregation

Goal: express "aggregate raw rows, then aggregate the aggregate result" for
current stock summary needs.

Dependency:

- Prefer Phase 5 first, because SQL subqueries make staged aggregation natural.

Tasks:

1. Add `source.stages` DTOs.
2. Validate stage references and stage output fields.
3. Implement SQL subquery generation:

   ```sql
   SELECT sku,
          (array_agg(qty_sum ORDER BY snapshot_date ASC))[1] AS first_qty
   FROM (
     SELECT sku, snapshot_date, SUM(qty) AS qty_sum
     FROM stock_snapshot
     GROUP BY sku, snapshot_date
   ) daily
   GROUP BY sku
   ```

4. Support joins only in stage 0 at first unless a concrete report requires
   more.
5. Update frontend, authoring schema, and examples.

Acceptance criteria:

- The stock report can compute `first_qty` as `first_value` over daily
  `SUM(qty)`, not over arbitrary warehouse rows.
- First/last values are deterministic and require an order field.
- Stage output columns drive table/chart/metric rendering normally.

Tests:

- Unit validation for missing prior-stage fields.
- SQL generation for two-stage aggregate.
- Service-level test with multiple warehouse rows per day.

## Cross-Phase Validation Requirements

Every phase should update all of these where applicable:

- DTO structs and serde aliases.
- Frontend report types and builder preservation/editing.
- `get_report_authoring_schema` examples.
- `validate_report` unknown-key and semantic validation.
- MCP tool descriptions when new authoring shapes are exposed.
- Service-layer tests for actual render/query behavior.
- Object Store SQL tests when SQL generation changes.
- Local smoke when HTTP/MCP surfaces change.

## Suggested PR Breakdown

1. `report-planner-boundary`
   - Phase 0 only.
2. `report-dynamic-condition-values`
   - Phase 1.
3. `report-joins-broadcast-v2`
   - Phase 2.
4. `report-scalar-value-columns`
   - Phase 3.
5. `object-store-condition-subqueries`
   - Phase 4.
6. `object-store-joined-aggregate-planner`
   - Phase 5.
7. `report-staged-aggregation`
   - Phase 6 only if the current stock scenario still needs it after earlier
     phases.

## Initial Task Prompt For A New Session

Use this prompt to start the next implementation session:

```text
Read docs/report-runtime-platform-roadmap.md. Start with Phase 0 only:
introduce an internal report query planner boundary for Object Model report
queries without changing public report behavior. Preserve existing tests, add
planner-focused tests around current aggregate broadcast join behavior, and do
not implement later phases yet.
```

For a more aggressive session:

```text
Read docs/report-runtime-platform-roadmap.md. Implement Phase 0 and Phase 1:
refactor current report query planning behind an internal planner module, then
add report-native dynamic condition values for Object Model report filters.
Keep workflow MappingValue unsupported in report source conditions, update MCP
authoring validation/examples, and add service/unit tests.
```
