# Raw SQL for workflows: `object-model:query-sql` / `object-model:execute-sql`

Status: **implemented** (2026-07-03, all four phases; e2e-verified incl. SIGTERM exactly-once, CRON trigger, and gated retry-semantics tests). Two deltas from the reviewed design, both noted inline: knob misconfiguration fails boot via `ConfigError` (the platform's stricter convention) instead of warn+default, and the advertised input wire name is `result_schema` (snake, macro alias-aware rule; camelCase accepted as alias).
Companion: [entitlements.md](entitlements.md), [reports-dsl-audit-2026-07-03.md](reports-dsl-audit-2026-07-03.md) (shares the no-concurrency-control root cause).

## Motivation

A daily CRON workflow needs a `rebuild_derived` step that rebuilds derived tables with real SQL — `FILTER (WHERE ...)` conditional aggregates, `array_agg(x ORDER BY d DESC)[1]` latest-row picks, `CROSS JOIN (VALUES ...)` window tables, `TRUNCATE` full-replace, computed-expression `UPDATE`, and `INSERT ... SELECT ... GROUP BY` write-back. The MCP layer already exposes `query_sql`/`execute_sql`, but the internal API the object-model WASM agent talks to has **zero** SQL routes, so no workflow step can run SQL today. This plan adds two capabilities to the existing object-model agent, wire-compatible with the MCP tools, reusing the existing `InstanceService` SQL methods. **No dedicated feature gate** (owner decision, 2026-07-03): the routes simply inherit the `database` entitlement that already covers every object-model route.

**Base design:** platform-consistency-first (2 of 3 judges), amended with the safety mechanisms every judge insisted on (read-only transaction on query, statement timeout, streaming row cap, audit line) and the future-proofing rules from the third design (additive-only wire evolution, `sql` stays a single statement forever). A post-synthesis adversarial review added the transport-retry fix, the response-byte cap, the concurrency guidance, and the rollout/test amendments — each marked **[review]** below.

---

## 1. Capability surface

Two capabilities on the existing `object-model` agent (`crates/agents/runtara-agent-object-model`). No new agent, no WIT change (invoke is string-dispatched), no templateMajor bump, no migrations.

| Capability id | fn | side_effects | idempotent (macro default) | Server path |
|---|---|---|---|---|
| `query-sql` | `pub fn query_sql` | `false` | `true` | `InstanceService::query_sql` (resultSchema present) / `query_sql_raw` (absent) |
| `execute-sql` | `pub fn execute_sql` | `true` | `false` | `InstanceService::execute_sql` |

Ids derive from fn names via the macro kebab default (`runtara-agent-macro/src/lib.rs:305`) — no explicit `id=`. Do NOT repeat `module_*` attrs (`create_instance` owns them).

**Excluded (consensus across all three designs):**
- `query-sql-one` — "exactly one row or error" is interactive-LLM ergonomics; workflows express it with a Conditional on `row_count`. MCP keeps its tool. Adding later is purely additive.
- Separate `query-sql-raw` — folded into `query-sql` via **optional `resultSchema`**: absent → `query_sql_raw` (generic JSON decoding), present → `query_sql` (typed decoding). The typed escape hatch is mandatory on the workflow surface because raw decoding 400s on PG arrays/bytea/custom enums (`runtara-object-store/src/query.rs:463-538`).

### 1.1 Input schemas (agent crate)

Conventions honored: field named `sql` — **never** `condition` (force-validated as ConditionExpression, `runtara-workflows/src/validation.rs:2858-2860`) and never `query`; `params: Vec<Value>` passthrough per the `aggregates` precedent (agent `lib.rs:1424-1439`) — emits truthful `array/items:any` metadata, skips coercion, engages the ArrayMappingEditor + the direct stdlib nested-ref pass so per-item `{valueType:"reference"}` envelopes resolve; `resultSchema` uses the `scoreExpression` rename+alias pattern; `_connection` stays hard-required via `require_connection_id` (matches all 12 siblings and the validator's `AgentMissingConnection` check — for the most dangerous capability in the catalog the author must name the target DB).

**QuerySqlInput**

| Field | Rust type | Wire name | Required | Notes |
|---|---|---|---|---|
| `sql` | `String` | `sql` | yes | One SELECT/read statement, `$1..$n` placeholders. Description states: runs in a READ ONLY transaction (writes rejected by Postgres); include LIMIT/OFFSET; result capped server-side (rows AND bytes); full result must fit workflow memory. |
| `params` | `Vec<Value>` | `params` | no (`#[serde(default)]`) | Typed positional params bound in array order. Item shape (documented in `#[field(description/example)]`): `{"type":"string|integer|decimal|boolean|timestamp|json|enum|vector","value":...}` (+ precision/scale, values, dimension). JSON null binds SQL NULL. Note: `bind_param` tolerates numeric/boolean strings and RFC3339 timestamp strings, but prefer immediate/reference values over templates. |
| `result_schema` | `Option<Vec<Value>>` | `resultSchema` (`#[serde(rename="resultSchema", alias="result_schema", default)]`) | no | `[{"name":..,"type":..,"nullable":true}]`. Omit for generic decoding; required for column types raw decoding rejects. |
| `_connection` | `Option<RawConnection>` | `_connection` | injected | `#[field(skip)]`, standard. |

**ExecuteSqlInput**

| Field | Rust type | Wire name | Required | Notes |
|---|---|---|---|---|
| `sql` | `String` | `sql` | yes | One statement: DML, `TRUNCATE`, `INSERT...SELECT`, DDL. Description carries verbatim: prepared-statement protocol = exactly one statement per call; **executes at least once** — write idempotent SQL (`ON CONFLICT`, WHERE guards); prefer the atomic single-statement full-replace `WITH del AS (DELETE FROM t) INSERT INTO t SELECT ...`; statements that cannot run in a transaction (`CREATE INDEX CONCURRENTLY`, `VACUUM`) are unsupported. **[review]** Also states DB-privilege prerequisites: `TRUNCATE` requires the TRUNCATE privilege or table ownership, DDL requires ownership — a scoped-down connection role fails with SQLSTATE 42501 → 400 → permanent step error. |
| `params` | `Vec<Value>` | `params` | no | Same as query-sql. |
| `_connection` | `Option<RawConnection>` | `_connection` | injected | — |

### 1.2 Output schemas

Crate convention wins over MCP camelCase (decision: `rows_affected`, not `rowsAffected` — every sibling output is a snake ident with no rename; workflow refs read `steps.X.outputs.rows_affected`). Every output carries `success` + `error` per crate envelope convention. No speculative `truncated`/`maxRows` fields (judges struck them).

| Output | Fields |
|---|---|
| `QuerySqlOutput` | `success: bool`, `rows: Vec<Value>` (JSON objects, column→value), `row_count: i64`, `error: Option<String>` |
| `ExecuteSqlOutput` | `success: bool`, `rows_affected: i64` (0 for DDL/TRUNCATE), `error: Option<String>` |

Declared `errors(...)` metadata: `OBJECT_MODEL_REQUEST_FAILED` (permanent — SQL/syntax/constraint/privilege/timeout/row-cap/byte-cap), `OBJECT_MODEL_UPSTREAM_ERROR` (transient, query only), `OBJECT_MODEL_HTTP_ERROR` (transient for query, permanent for execute — §2.4), `OBJECT_MODEL_PAYLOAD_TOO_LARGE` (permanent). Entitlement denial surfaces exactly like the sibling capabilities (§3) — no SQL-specific denial code.

---

## 2. Wire path and new routes

### 2.1 New internal routes (the only new server surface)

The internal router (`crates/runtara-server/src/server.rs:1963-2026`) has zero SQL routes. Add:

```
POST /api/internal/object-model/sql/query    -> internal_object_model::query_sql
POST /api/internal/object-model/sql/execute  -> internal_object_model::execute_sql
```

Register **inside `internal_object_model_routes`, above** the existing `.layer(DefaultBodyLimit 64MB)` / `.with_state` / `.route_layer(require_database)` calls, so they inherit for free: the 64 MB body limit (SYN-491), `ObjectModelState`, the `require_database` entitlement gate, and the drain-ordering guarantee (internal API outlives env drain, `server.rs:2428-2464`).

**[review] Trust dependency, stated explicitly:** these routes execute arbitrary SQL authenticated only by the `X-Org-Id` header, like every other internal route. That is safe *only* because of the boot-time loopback-bind enforcement (`enforce_internal_listener_safe`, `server.rs:2304-2330`) and the one-process-per-tenant model. The still-open "internal bind" hardening finding (F6, connection-proxy plan) now also covers these routes — add them to the internal-surface threat notes in `docs/entitlements.md` and to F6's scope.

### 2.2 Handlers (`crates/runtara-server/src/api/handlers/internal_object_model.rs`)

Follow sibling patterns: `extract_tenant_id` from `X-Org-Id`; reuse existing DTOs verbatim — `SqlParam` (`dto/object_model.rs:707`), `SqlResultColumn`, `SqlQueryResponse`, `SqlExecuteResponse` — so the typed-param wire shape (`{"type":"integer","value":42}` flattened ColumnType, `$1..$n` array-order binding) is byte-identical to MCP.

```rust
struct InternalSqlQueryRequest {
    sql: String,
    #[serde(default)] params: Vec<SqlParam>,
    #[serde(default, rename = "resultSchema", alias = "result_schema")]
    result_schema: Option<Vec<SqlResultColumn>>,
    #[serde(rename = "connectionId", alias = "connection_id")] connection_id: Option<String>,
}
// InternalSqlExecuteRequest: sql, params, connection_id
```

Dispatch: `result_schema.is_some()` → typed path, else raw path; execute → `execute_sql_workflow`.

**Error shape (decision — winner's approach, both winning judges):** **status-coded**, reusing/lifting `raw_sql_error_response` (`handlers/object_model.rs:1621`; Validation→400, NotFound→404, Conflict→409, DatabaseError→500) — NOT the sibling 200-envelope. Rationale: status codes flow through the agent's existing `check_status` into correct permanent/transient classification with zero envelope-parsing agent code; a 200-envelope would make every SQL failure a "successful" step invisible to onError. Document this as a second (deliberate) exception in `docs/entitlements.md` alongside the internal-agents 200 envelope.

**Audit line (kept from safety design, judge-3 must-keep):** one structured `tracing::info!` per request at target `runtara::raw_sql_audit`: tenant_id, connection_id, capability, sql_sha256, sql_prefix (first 256 chars — literals may carry data, so full text only at `debug`), param_count, duration_ms, outcome (ok|error), rows|rows_affected. **[review]** This is process-log-only and rotates away; that is accepted for v1 (a durable audit table is a listed future extension), but the emission itself must be pinned by a test (tracing capture asserting the line fires on ok and error paths) — an untested log line is not an audit mechanism.

### 2.3 Store/service layer: guarded execution (kept from safety/future-proof designs — all three judges must-keep)

Do not call the unguarded store primitives directly. Add to `crates/runtara-object-store/src/query.rs` (param binding via existing `bind_param`):

- `query_guarded(sql, params, schema: Option<..>, max_rows, max_bytes, timeout)` — `BEGIN; SET TRANSACTION READ ONLY; SET LOCAL statement_timeout = <ms>;` then **stream** rows via `fetch()`, decode with existing `row_to_typed_json`/`row_to_raw_json`, **abort with a validation error at max_rows + 1** ("result exceeded N rows; add LIMIT or aggregate"), rollback. Read-only txn is DB-level enforcement — strictly stronger than SQL parsing, closes the known write-via-read hole for this surface. Streaming abort (not fetch_all-then-count) so oversized results are never materialized. **[review]** The streaming loop also accumulates the serialized size of decoded rows and aborts past `max_bytes` ("result exceeded N bytes") — a row cap alone does not bound memory: one row with a large text/bytea/jsonb column can be hundreds of MB, and the 64 MB `DefaultBodyLimit` bounds the *request*, not the response.
- `execute_guarded(sql, params, timeout)` — transaction + `SET LOCAL statement_timeout` + statement + commit; returns `rows_affected()`.

New thin methods in `crates/runtara-server/src/api/services/object_model.rs`: `query_sql_workflow` / `execute_sql_workflow`, reusing `get_store` resolution (postgres-only check at `services/object_model.rs:101`, moka cache, per-connection pools). Runtime/MCP paths (`query_sql:661`, `query_sql_raw:703`, `execute_sql:722`) are **unchanged** — retrofit is listed future work.

### 2.4 Agent-side forwarding

Standard helpers: `http_post("sql/query", body)` / `http_post("sql/execute", body)` with `path_with_connection` + `with_connection_in_body`, `X-Org-Id` from `RUNTARA_TENANT_ID`, direct loopback (no proxy).

**Retry classification (the single most important semantic decision — all judges):** the shared `check_status` (agent `lib.rs:170-209`) classifies 429/5xx as transient, and the default policy retries retryable errors 3× — which would silently re-run a partially-applied mutation.

- `execute-sql`: apply a small `downgrade_transient(err)` after `check_status` — **5xx → permanent** (statement outcome on the tenant DB is unknown; never auto-retry a write; AiAgent "no re-billing" precedent), **429 stays transient** (rejected before reaching Postgres, safe).
- `query-sql`: **keeps stock status classification** — 5xx/429 transient. Reads are free to retry (READ ONLY txn guarantees it), and blanket-permanent would lose safe deadlock/serialization retries (judges 1+2 rejected design C's approach here).
- **[review] Transport-error reclassification (adversarial-review fix — this was a design bug):** `http_post` today maps every transport-level failure (connection refused/reset, wasi-http timeout) to `AgentError::permanent("OBJECT_MODEL_HTTP_ERROR")` (agent `lib.rs:211-233`). Left as-is, a network blip or a server restart mid-nightly-rebuild would *permanently* fail a read. Fix: the two SQL capabilities call the forwarding helper through a thin wrapper that reclassifies transport errors — **transient for `query-sql`** (retry is provably safe), **permanent for `execute-sql`, deliberately** (a reset-after-send has unknown statement outcome; today's behavior is the safe one but it is accidental — pin both directions with tests). Sibling capabilities are untouched.

---

## 3. Gating

**Decision (owner, 2026-07-03): no dedicated feature gate.** Earlier drafts carried a `RUNTARA_WORKFLOW_RAW_SQL` kill switch and weighed a 5th FeatureKey; both are dropped. A `database=true` tenant already runs identical SQL against the identical per-tenant DB via MCP `execute_sql`, so a workflow-only gate would add a knob without adding a boundary. What remains is all inherited, zero new code:

1. **Entitlement (inherited):** the routes pick up `require_database` from the internal router's `route_layer` (`server.rs:2026`) simply by living in `internal_object_model_routes` — same as all 12 sibling routes. Per repo doctrine (`docs/entitlements.md:471-486`) the runtime internal-API gate is THE choke point: object-model capabilities always round-trip loopback HTTP, so "stale workflows are unrunnable, not uncompilable". Reads the boot-resolved snapshot — no fail-open path.
2. **Real boundary:** per-tenant process + per-tenant DB + the Postgres connection role (least-privilege guidance in `docs/entitlements.md` — §7.11).
3. **Surfacing:** an entitlement 403 flows through `check_status` exactly like the sibling capabilities — permanent step error with the denial JSON visible in `steps.__error.*`. No SQL-specific denial code.
4. **Validation-time / catalog filtering:** none (no per-capability precedent; doctrine says UX-only anyway).
5. **[review] Rollout communication:** with no gate, every database-entitled tenant gains workflow raw SQL at the next upgrade — ship a CHANGELOG.md entry saying so.

## 4. Statement policy

- **`query-sql`: enforced read-only via `SET TRANSACTION READ ONLY`** — mechanism, not parsing. Postgres rejects any write/DDL (SQLSTATE 25006) → 400 → permanent step error.
- **`execute-sql`: no keyword classification** (permanent non-goal — classifiers are bypassable via `DO $$` / functions and accrete exceptions; the boundary is the per-tenant DB + connection role + entitlement). DML, `TRUNCATE`, `INSERT...SELECT`, and DDL all run, matching MCP `execute_sql` today. Wrapped in a transaction (needed for `SET LOCAL`); single-statement semantics unchanged.
- **Multi-statement:** structurally impossible via the sqlx extended protocol (one statement per Prepare) — pinned by a test, stated in the field description, never re-implemented as a splitter.
- **Rebuild pattern:** the canonical example recommends the atomic `WITH del AS (DELETE FROM t) INSERT INTO t SELECT ...` single statement. A two-step TRUNCATE→INSERT chain works but strands an empty table if the INSERT fails or the instance replays between steps — the docs say so explicitly (corrects the winner design's "naturally idempotent" claim).
- **[review] Concurrency:** the cron scheduler has no overlap/singleton protection, and a manual run or a retried instance can overlap the scheduled one. Two concurrent full-replace statements under READ COMMITTED each see the pre-delete snapshot and both append — silently duplicated derived rows, no error (this is the same no-concurrency-control root cause the reports audit flagged). The canonical rebuild example therefore takes a transaction-scoped advisory lock *inside the statement*, with the lock CTE force-referenced so it cannot be planned away:

  ```sql
  WITH lock AS (SELECT pg_advisory_xact_lock(hashtext('rebuild:derived_table'))),
       del  AS (DELETE FROM derived_table WHERE (SELECT true FROM lock) RETURNING 1)
  INSERT INTO derived_table
  SELECT ... FROM source WHERE (SELECT count(*) FROM del) >= 0 GROUP BY ...;
  ```

  Concurrent runs serialize on the lock; it releases at commit/rollback automatically. Docs state plainly: the platform does not serialize workflow instances — SQL-level locking is the author's job for rebuild-style writes.

  **[implementation finding, 2026-07-03]** Every CTE reference above is load-bearing: data-modifying CTEs execute *unordered* unless referenced. The lock CTE must be referenced from the DELETE's WHERE, and the DELETE must be referenced from the INSERT (`RETURNING 1` + `(SELECT count(*) FROM del) >= 0`), or the INSERT's primary-key check races the delete and fails with 23505 on every non-empty rebuild — verified empirically in the e2e (run 1 on an empty table passes, run 2 fails without the forced ordering).

## 5. Limits

Nothing bounds raw SQL today below the 300 s whole-instance SIGKILL. Ship bounded (all three judges), server-side so future surfaces inherit; env-knobbed, boot-resolved:

| Limit | Default | Knob | Failure |
|---|---|---|---|
| Statement timeout | 60 s | `RUNTARA_RAW_SQL_STATEMENT_TIMEOUT_MS` (clamped below the 300 s instance hard-timeout) | PG 57014 → 400 → permanent (a timed-out statement will time out again; retrying a write on timeout is the double-apply case) |
| Max result rows (query) | 10 000 | `RUNTARA_RAW_SQL_MAX_ROWS` | Streaming abort at N+1 → 400, **error never truncate** (silent truncation of a rebuild input is a correctness bug) |
| **[review]** Max result bytes (query) | 64 MB (symmetric with request limit) | `RUNTARA_RAW_SQL_MAX_RESPONSE_BYTES` | Streaming abort past cap → 400 ("result exceeded N bytes; select fewer/narrower columns") |
| Request body | 64 MB | inherited | 413 → `OBJECT_MODEL_PAYLOAD_TOO_LARGE` (existing mapping + test) |

**[review] Knob parsing policy (applies to all three numeric knobs):** parsed once at boot; invalid, zero, or negative values → startup `warn` + built-in default. Never "unset means unbounded". The timeout knob is additionally clamped to the instance hard-timeout. Each behavior gets a unit test.

**[review] When 60 s is not enough (the motivating workload itself may exceed it):** one process per tenant means the knob *is* per-tenant — raising `RUNTARA_RAW_SQL_STATEMENT_TIMEOUT_MS` for a tenant whose rebuild legitimately runs long affects only that tenant, and the docs say exactly that. A per-step `timeoutSeconds` input stays a reserved future extension (additive), not v1 scope.

Documented reality in field descriptions: one query's full JSON result must fit the 1 GiB bump-allocated guest heap as a single buffer (arena rewinds between, not within, Split/While iterations), and durable steps persist the full result envelope into core-DB checkpoints — mandate LIMIT/OFFSET; suggest `durable: false` for cheap large reads.

## 6. Durability, replay, retries — the contract

Stated verbatim in capability descriptions (no `ai_hints` attribute exists — descriptions + `get_workflow_authoring_schema` are the only guidance channels):

- Durable (default) checkpointed steps **skip re-invoke on replay** (checkpoint cache hit); failed attempts are never memoized.
- **At-least-once windows remain:** crash between tenant-DB commit and checkpoint save (two stores, no cross-commit atomicity); `durable:false`; mid-invoke force-stop at drain-grace expiry. Therefore: idempotent SQL mandated (`ON CONFLICT`, WHERE guards, CTE full-replace).
- Retries: `query-sql` retries transient errors (default 3×), including transport blips (§2.4); `execute-sql` never auto-retries server or transport errors — the only re-execution paths are the replay windows.
- Split/While: checkpoint keys carry `loop_indices`, per-iteration writes memoize independently; prefer one set-based statement over per-row loops.
- Step `timeout` is not enforced for agent steps (W071) — the statement_timeout is the real bound; say so instead of pretending.
- Add a canonical `rebuild_derived` example (CRON → single `execute-sql` advisory-locked CTE full-replace + typed params mixing immediate and reference items) to `get_workflow_authoring_schema` (`mcp/tools/workflows.rs:396-500`).
- **[review]** Add a second authoring example for reads above the row cap: a While loop paging `query-sql` with `LIMIT $1 OFFSET $2` reference params until `row_count < limit`. Without it, users hitting the cap get an error with no documented workflow-side recipe.

## 7. Implementation checklist (ordered)

**Phase 1 — store + server (mergeable alone):**
1. `crates/runtara-object-store/src/query.rs` — `query_guarded` (READ ONLY txn, `SET LOCAL statement_timeout`, streaming row-cap + byte-cap abort), `execute_guarded`; knobs + parsing policy in the server `config.rs`; store testcontainer tests.
2. `crates/runtara-server/src/api/services/object_model.rs` — `query_sql_workflow` / `execute_sql_workflow`.
3. `crates/runtara-server/src/api/handlers/internal_object_model.rs` — 2 handlers + request structs + audit line; lift `raw_sql_error_response` from `handlers/object_model.rs:1621` into a shared helper.
4. `crates/runtara-server/src/server.rs` — +2 routes inside `internal_object_model_routes` above the layer calls.
5. Unit tests: entitlement short-circuit clone of `database_gate_short_circuits_internal_object_model_path` (`middleware/entitlement.rs:975`); typed-vs-raw dispatch; knob-parsing edge cases; audit-line emission (ok/error).

**Phase 2 — agent:**
6. `crates/agents/runtara-agent-object-model/src/lib.rs` — 2 input + 2 output structs, 2 `#[capability]` fns, `downgrade_transient`, transport-error reclassification wrapper (§2.4), and **all 4 registration points**: `agent_info()` caps array (~1875), `input_types` (~1890), `output_types` (~1925), `Guest::invoke` arms (~2030). Misses are silent (absent from meta.json) or runtime-only (`UNKNOWN_CAPABILITY`).
7. `./scripts/build-agent-components.sh` (rebuild + emit-meta sidecars); **revert `bindings.rs` churn** (no WIT change); restart server; recompile probe workflow (components composed at compile time); `test_capability` smoke.

**Phase 3 — catalog, authoring, frontend, docs:**
8. `crates/runtara-workflows/tests/catalog/agent_catalog.json` — add both capabilities; validator tests (missing `sql` → required-input error; missing connection → `AgentMissingConnection`).
9. `crates/runtara-server/src/mcp/tools/workflows.rs` — `rebuild_derived` + pagination authoring examples.
10. Frontend: add `sql` to the multiline name-heuristic lists in `crates/runtara-server/frontend/src/features/workflows/components/WorkflowEditor/NodeForm/InputMappingField/MappingValueInput.tsx` and `crates/runtara-server/frontend/src/features/workflows/types/agent-metadata.ts` (note: paths are under `crates/runtara-server/frontend/`, not repo-root `frontend/`); run regen-frontend-api.
11. `docs/entitlements.md` — `database` now also covers workflow raw SQL (inherited); status-coded internal error exception; "bypasses per-schema authorization and soft-delete" warning; internal-surface/F6 note; least-privilege connection-role guidance (grant only DML + TRUNCATE on derived tables if DDL should be impossible). **[review]** CHANGELOG.md entry (§3.5).

**Phase 4 — verification:**
12. `e2e/test_obm_sql_workflow.sh` (clone `test_obm_query_by_id_workflow.sh`, wire into `e2e/run_all.sh`) — see §8.
13. Gated `direct_wasm_execute.rs` retry tests; SIGTERM drain leg; **[review]** CRON-trigger leg; full **e2e-verify** before declaring done (house rule).

Estimated diff: ~300 lines store, ~230 lines server, ~350 lines agent, plus tests. No migrations, no WIT, no DSL, no emitter, no MCP changes, no new env flags beyond the three limit knobs.

## 8. Test plan

**Unit:** agent metadata pins (`params` = `Vec<Value>`, `resultSchema` wire name + alias, snake outputs — `score_expression` pin pattern); `downgrade_transient` (500→permanent, 429→transient); **[review]** transport reclassification both directions (transport error → transient for query-sql, permanent for execute-sql); entitlement short-circuit; typed-vs-raw handler dispatch; knob parsing; audit-line capture.

**Integration (testcontainers — first-ever raw-SQL coverage; today only param-validation unit tests exist):** `crates/runtara-object-store/tests/integration.rs` — READ ONLY txn rejects UPDATE-spelled-as-query (SQLSTATE 25006); `statement_timeout` fires on `pg_sleep`; streaming row cap errors at N+1; **[review]** byte cap errors on a small-row-count/large-bytes result (one fat jsonb row); multi-statement `"DELETE FROM a; DROP TABLE b"` fails at the protocol (pins the guarantee); typed-param round-trip per ColumnType; `rows_affected` for UPDATE/TRUNCATE; **[review]** privilege-denied path (scoped role, SQLSTATE 42501 → validation error, permanent); unsupported-raw-type 400 with resultSchema guidance.

**Gated (`RUNTARA_RUN_DIRECT_WASM_E2E=1`, `--test-threads=1` — passes vacuously without the env var, don't be fooled):** mock object-model provider scripted to 500 — assert `execute-sql` performs **zero retries** and routes onError with `steps.__error.*` populated, while `query-sql` retries then succeeds; **[review]** mock connection-refused — `query-sql` retries, `execute-sql` fails permanently. Also `cargo test -p runtara-workflows` (structural compile tests; confirm zero emitter drift).

**e2e-verify shell (`e2e/test_obm_sql_workflow.sh`):** own DBs + Valkey; schema + postgres connection; graph exercising all six motivating features: create-instance ×N → `execute-sql` computed `UPDATE ... SET age_days = CURRENT_DATE - created::date WHERE id = $1` ($1 = reference to prior step output; assert `rows_affected == 1`) → `execute-sql` advisory-locked CTE full-replace with `INSERT...SELECT...GROUP BY` + `FILTER` aggregate → `query-sql` with `array_agg(... ORDER BY ...)` subscript + `CROSS JOIN (VALUES ...)`, one column via resultSchema → Finish. Run **twice** to prove idempotency. Legs:
- SIGTERM mid-run after the first checkpointed write → suspended → relaunch → completed with the write applied exactly once (checkpoint-cache hit, observed via a counter table);
- multi-statement string → permanent failure;
- **[review] CRON leg (the acceptance scenario itself):** create a cron trigger on the workflow, wait for the scheduler to fire it, assert the execute-sql step ran and the derived table changed. This exercises the trigger publisher's hardcoded-Valkey-stream path — the known e2e-isolation pitfall — which the direct-execute legs never touch.

Execute payload uses the `{"data":..., "variables":...}` input envelope.

**[review] Deliberately untested:** the SIGTERM-while-statement-in-flight window (commit server-side, suspend before checkpoint, replay re-runs) is documented in §6 but has no test — making it deterministic requires pausing inside a Postgres statement mid-drain, which is racy in CI. The idempotent-SQL mandate is the mitigation; revisit if a deterministic fault-injection hook ever lands in the e2e harness.

## 9. Regen steps

1. `./scripts/build-agent-components.sh` → rebuilt components + `runtara_agent_object_model.meta.json` sidecar; revert `bindings.rs` churn across agents.
2. Server restart (catalog loads from `RUNTARA_AGENT_COMPONENTS_DIR` at boot); recompile any probe workflows.
3. regen-frontend-api skill → `crates/runtara-server/frontend/src/generated/RuntaraRuntimeApi.ts`.
4. Refresh `crates/runtara-workflows/tests/catalog/agent_catalog.json` (hand-maintained; no drift test forces it).
5. `docs/entitlements.md`; CHANGELOG.md; authoring-schema examples.

## 10. Non-goals / future extensions (pre-reserved seats — additive only)

- **Multi-statement scripts / transactions** — future `execute-sql-script` capability (`statements: [{sql, params}]`, atomic). `sql` stays a single statement forever; this is the key wire pre-reservation.
- **Named parameters** — additive `"name"` key on the open param objects; `$n` only for now (MCP symmetry).
- **Dedicated `FeatureKey::RawSql`/`Sql`** — only if tier-level "CRUD without SQL" governance materializes; any convergence must handle `RUNTARA_ENTITLEMENTS_JSON`-override tenants (database:true would not imply the new key) before touching MCP gates.
- **Retrofit `query_guarded` bounds onto runtime/MCP SQL routes** — recommended, separate behavior-change PR (fixes the write-via-read hole platform-wide; behavior change for anyone (ab)using writes-via-query there).
- **Durable audit trail** — core-DB table or events-stream record per raw-SQL call, replacing the log-only audit line if governance requires queryable history.
- **Per-step `timeoutSeconds`/`maxRows`/`maxBytes` inputs** — additive; the env knobs are per-tenant already.
- **Non-Postgres backends** — new `integration_id` branch in `resolve_database_url`; optional `dialect` input reserved.
- **Streaming/chunked results, `format:"multiline"` metadata channel, per-capability entitlement/catalog filtering, `query-sql-one`, keyword statement classification (permanent non-goal)** — all additive if ever needed.
- **Workflow-level singleton/overlap control** — the platform-wide fix for the concurrency gap in §4; until then, advisory locks in the SQL are the documented pattern.

## 11. Open questions (owner's call)

~~Gating strength / kill-switch default / Step Picker visibility when gated off~~ — resolved 2026-07-03: no feature gating (§3).

1. **Defaults for the knobs** — 60 s statement timeout / 10 000 rows / 64 MB response are sized to checkpoint-blob and guest-heap realities but arbitrary within an order of magnitude; they become de-facto contracts.
2. **DDL through execute-sql** — plan allows it (MCP parity; boundary = per-tenant DB + connection role). If schema lifecycle should be forced through the object-model schemas API instead, that means adopting a keyword classifier with its known porosity.
3. **Retrofitting the guards onto runtime/MCP SQL routes** — recommended follow-up, but it's a behavior change for existing MCP users; separate go-ahead.

---

*Provenance: designed 2026-07-03 via a 26-agent review workflow — 6 subsystem maps, 3 independent designs (consistency/safety/future-proof), 3-judge panel (consistency design won 2:1), synthesis, then adversarial verification (10/12 load-bearing claims confirmed against the code; 2 refuted claims were wording, not substance) and a completeness critique whose 12 gaps are folded in above as **[review]** items. Same day, the owner dropped the proposed feature gating (kill switch + FeatureKey question); §3 and dependent sections were simplified accordingly.*
