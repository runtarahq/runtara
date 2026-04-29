# Report block joins — v2 design notes

## Background

`ReportSource.join` shipped in v1 as a broadcast-hash join executed
client-side: dimensions are queried first, the primary aggregate is
filtered by `parent_field IN [resolved_keys]`, and result rows are
enriched with `<alias>.<field>` columns from the dim lookup. See
[crates/runtara-server/src/api/services/reports.rs](../crates/runtara-server/src/api/services/reports.rs)
(`aggregate_with_optional_joins`, `resolve_join`, `enrich_aggregate_result`).

This works for the common dimensional case (small dim, large fact, 1:1
key on the dim side, AND-shaped condition tree, qualified refs only in
`groupBy` and `condition`). It does not yet cover several adjacent
shapes that callers will eventually want. This doc records what's
deferred, why, and how to add each item.

## V1 limits, sized

### Cheap (a few hours each)

#### 1. Filter-mode joins on tables

Currently only the aggregate path routes through `aggregate_with_optional_joins`.
The `render_table_block` filter path
([reports.rs:1035](../crates/runtara-server/src/api/services/reports.rs))
ignores `block.source.join`.

**Plan:**
1. Mirror `resolve_join` and qualified-condition splitting before
   building `FilterRequest`.
2. AND `parent_field IN [...]` into the request's condition.
3. After `filter_instances_by_schema` returns instances, post-process
   each flattened row to add `<alias>.<field>` keys via the dim
   lookup.
4. Reject qualified `sort_by` (or buffer + in-memory sort if the result
   set is small — gated by `MAX_TABLE_PAGE_SIZE`).

**Risk:** none, the broadcast-hash plumbing is already proven by the
aggregate path. Add the same `MAX_BROADCAST_JOIN_DIM_ROWS` guard.

**Cost:** ~150 lines + 2-3 unit tests.

#### 2. OR / NOT condition trees with qualified refs

`split_qualified_condition` only handles top-level AND today. Callers
who want `(p.category IN [A] AND status='active') OR p.priority='hot'`
get a "mixes references across joins" validation error.

**Plan:**
1. Walk the full condition tree. For each subtree, collect its
   per-alias term groupings.
2. For pushdown, push the **union** of all aliased keys per join (an
   over-approximation — the SQL filter is broader than strictly
   needed).
3. After the primary aggregate / filter returns, post-filter rows in
   memory by re-evaluating the original (un-rewritten) condition tree
   against the enriched row.

**Risk:** the post-filter step adds CPU on big result sets. Limit
applies *before* post-filter, so caller-visible row counts may drop
unexpectedly. Document this explicitly. Cap with the same
`MAX_BROADCAST_JOIN_DIM_ROWS` per-join.

**Cost:** ~200 lines + a condition-evaluator helper + 4-5 unit tests.

### Medium (a day each)

#### 3. Qualified refs in `orderBy.column` (aggregate path)

After enrichment, sort over enriched rows in memory when `orderBy`
references a qualified column.

**Plan:**
1. Run the primary aggregate without the qualified portion of
   `orderBy`.
2. Enrich rows.
3. In-memory sort over the full enriched-row set.
4. Apply `limit` after the sort.

**Risk:** `limit` no longer pushes to SQL — fetching unbounded then
sorting + truncating is the only correct way. Document the trade-off,
or add a soft cap (e.g. fetch up to `limit * 10`, sort, truncate) with
an explicit warning when the cap is hit.

**Cost:** ~100 lines + a doc note + 2 unit tests.

#### 4. Compound (multi-key) joins

Today `parent_field` and `field` are single columns. Multi-key joins
need `Vec<{parent, field}>` per join entry and `(a, b) IN ((...))` SQL
emission.

**Plan:**
1. Change `ReportSourceJoin.parent_field`/`field` to a single
   `keys: Vec<{parentField, field}>` array (with backward-compat alias
   for the singular form).
2. Update `resolve_join` to build a tuple-key lookup (string-join the
   key columns).
3. The IN filter on the primary becomes either:
   - tuple-IN: extend the object-store condition layer to emit
     `(parent_field_a, parent_field_b) IN ((...), (...))`, or
   - one-shot OR-of-ANDs: `OR_i (parent_a EQ a_i AND parent_b EQ b_i)`
     — works without object-store changes but scales poorly past a few
     hundred keys.

**Risk:** the object-store extension for tuple-IN is the real cost.
Without it, OR-of-ANDs is a workable v2 first step.

**Cost:** ~300 lines if extending the object-store; ~150 if using the
OR-of-ANDs fallback.

### Expensive (a week+ — defer past v2)

#### 5. Qualified refs in `aggregates[].field`

`aggregates: [{alias: "max_unit_cost", op: "max", field: "p.unit_cost"}]`
needs aggregation to happen *after* dim enrichment — the dim row is
where `p.unit_cost` lives. Two ways:

- **Two-pass:** primary aggregate first, enrich, then re-aggregate
  including dim columns. Breaks when the inner aggregate folds away
  the parent_field needed to look up the dim row.
- **SQL-level join:** drop the broadcast-hash approach for aggregate
  blocks and emit a real SQL join. Requires a new code path in
  `runtara-object-store` and a way for the report layer to express
  joined queries (a `JoinedAggregateRequest`?).

The right answer is SQL-level joins. That's a substantial rewrite of
the broadcast-hash helper and the object-store query builder. Defer
until a concrete use case demands it.

#### 6. Multi-hop joins (Stock → Product → Category)

The current alias resolution assumes one hop. Multi-hop needs:
- Topological resolution order across joins.
- Allowing `<alias_a>.<field>` to resolve against another alias's
  primary, not the block's source.
- Either chained broadcast (resolve A keys → resolve B keys keyed by
  A's relevant column → push down to primary) or full SQL-level
  multi-join.

The **denormalize one hop closer** workaround (e.g. add
`category_top_name` to `TDProduct`) covers ~95% of cases without
this. Defer until that workaround clearly stops scaling.

#### 7. RIGHT / FULL OUTER joins

Broadcast-hash inverts: query the dim, then for each dim row fetch
matching primary rows, then merge null-padding for unmatched primary
keys. Different code path entirely. Inner+left covers analytics; OUTER
joins are mostly an ETL pattern. Skip unless ETL becomes a target use
case for reports.

## Recommended v2 scope

Ship items 1 and 2 (filter-mode joins, OR/NOT conditions) together —
they round out the feature for typical reporting use without
architectural cost. Item 3 (qualified `orderBy`) is a small follow-up
once item 2 lands. Items 4-7 stay deferred.

## Out of scope: audit log coalescing

The original Request #1 proposal included an audit-log entry per
bulk call. The exploration found no audit infrastructure on
single-row updates either, so there's nothing to coalesce *into*.
If audit logging is added to `update_object_instance` later, the
coalesced-bulk variant is straightforward: emit one event per
`bulk_update_instances` call carrying the matched/updated counts
and a digest of the condition.

## Reference

- v1 implementation: `aggregate_with_optional_joins` in
  [crates/runtara-server/src/api/services/reports.rs](../crates/runtara-server/src/api/services/reports.rs)
- DTO shape:
  [crates/runtara-server/src/api/dto/reports.rs:170-237](../crates/runtara-server/src/api/dto/reports.rs)
- Per-cell join precursor (chart columns):
  `ReportTableColumnJoin` in the same DTO file
- Existing reporting spec: [reporting-module-spec.md](reporting-module-spec.md)
