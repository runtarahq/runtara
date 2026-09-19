# Object Model over native SQL host calls

Status: implemented and verified locally; see [validation notes](object-model-host-calls-validation.md). This replaces the earlier
14-operation host-interface proposal. Based on the trusted-capability and native
connection branch `codex/trusted-capabilities-presigning`.

## Decision

Expose exactly three database operations to WASM:

- `query`: one read statement, returning bounded typed rows.
- `execute`: one write/DDL statement, returning affected rows and optionally
  bounded `RETURNING` rows.
- `execute-batch`: ordered statements on one connection, with explicit commit
  semantics and per-statement results.

Use a versioned `runtara:database/sql@0.1.0` interface. The Object Model agent
implements schema-aware CRUD, filters, aggregation, bulk behavior, and memory
on top of it. The host implements the database driver and execution policy.
There are no CRUD/schema/memory host methods and no `trusted` credential handoff.

```text
workflow mappings / transforms
  -> Object Model agent + shared pure schema/SQL library
  -> query | execute | execute-batch
  -> native tenant/connection/policy + PostgreSQL driver
  -> PostgreSQL
```

Public HTTP/MCP Object Model APIs remain. Their native implementations reuse the
same pure schema/SQL library so the agent does not become a second implementation
of validation and query generation. Internal Object Model HTTP routes are removed.
Outbound HTTP proxy migration remains separate.

## What we already have, and what is missing

| Area | Current code | Required work |
| --- | --- | --- |
| Workflow data mapping | `runtara-workflow-stdlib` applies mappings; transform/CSV agents handle data transformation | Reuse these; SQL host does not evaluate workflow expressions |
| Typed parameters | `runtara-object-store/src/query.rs`: positional `$1` parameters, nullable string/integer/decimal/boolean/timestamp/JSON/enum/vector values | Extract a driver-independent wire contract and make null/precision semantics explicit |
| Result decoding | Raw and schema-directed SQLx-to-JSON conversion | Replace lossy numeric conversion in the new wire path, preserve column identity, and specify unsupported types |
| Query guardrails | Read-only transaction, statement timeout, row/byte caps | Reuse in native SQL adapter |
| Writes | Guarded single-statement execution returns affected-row count | Add `RETURNING` support; collect/validate results before commit |
| Batches | No generic guarded batch API in `query.rs` | Add transaction ownership, bounded batch execution, and explicit partial outcomes |
| SQL generation | Pure condition/expression/aggregation/DDL helpers under `runtara-object-store/src/sql` | Extract from SQLx crate, adapt to typed bindings, and share with WASM |
| Object semantics | Schema metadata, validation, CRUD/bulk planning and normalization spread across store/server | Extract pure logic; move workflow orchestration into agent |

This is enough foundation for the architecture, but the existing raw SQL API is
not yet a complete replacement. In particular, simply moving the current JSON
helpers behind WIT would retain precision loss and omit required transaction and
write-result behavior.

## Contract

Canonical WIT lives in `runtara-workflow-wit/wit/database`, following the native
connection-resolver client/host-world pattern. Add interface constants, WIT
parsing tests, compiler/composer registration, and real component linking tests.

Each function takes a required opaque `connection-id` separately from the
request. Tenant, credentials, database URL, and transaction handles are absent
from guest arguments. Reject empty IDs; no default-store or HTTP fallback.

A small shared `runtara-database-contract` crate defines versioned request/result
schemas, typed SQL values, and errors. Use JSON bytes for the envelopes in WIT,
with tagged values where ordinary JSON cannot represent database values exactly.
No SQLx, server DTO, OpenAPI, socket, or filesystem dependencies.

Conceptual signatures:

```text
query(connection_id, { sql, params, result_schema? })
  -> RowSet

execute(connection_id, { sql, params, returning?: ResultSpec })
  -> { rows_affected, returned?: RowSet }

execute_batch(connection_id, { mode, statements: [Statement] })
  -> BatchResult
```

`ResultSpec` selects raw supported decoding, selected named columns with their
actual SQL types, or a typed projection. Selected columns let the agent apply
legacy Object Model type families without changing driver codecs. Returning is
explicit so commands that discard rows cannot accidentally bypass row limits.
`Statement` has the same SQL/parameters/optional result specification as
`execute`. A batch is a write-capable operation; only `query` promises read-only
execution. SQL is one prepared statement per entry, not a semicolon-delimited
script.

Use ordered column descriptors and positional cells for `RowSet`. This retains
column types and duplicate column labels, unlike the current object-map decoder
which can overwrite duplicate names. The agent projects rows into its existing
JSON output shapes, requires aliases where a named-object result is ambiguous,
and handles empty results using descriptors rather than guessing from row one.

### Data representation and mapping

Keep three layers distinct:

1. Workflow mapping resolves references, expressions, defaults, and transforms
   before capability invocation. It already runs in WASM.
2. Object Model mapping resolves logical schema/field names, validates object
   properties, chooses SQL types, handles missing fields/defaults, and builds
   typed positional parameters. This moves to the shared pure library/agent.
3. Database encoding binds those values into PostgreSQL and decodes driver rows.
   This remains native; it is a driver responsibility, not an ORM.

The new SQL wire format must provide:

- Exact decimal strings and signed 64-bit integer strings, with explicit tags;
  no conversion through `f64`. Keep floating-point values distinct.
- Typed SQL NULL distinct from a JSON value containing `null`. Current
  `SqlParam { type: json, value: null }` means SQL NULL, so it cannot express both.
- Distinct missing property, explicit NULL, and SQL DEFAULT during object
  planning. Missing update properties must not erase existing values.
- Explicit text, boolean, JSON, timestamp/timezone, and vector representations.
  Separate SQL value types from Object Model column definitions: generated
  `tsvector` columns are schema features, not writable parameter types.
- Clear supported-type errors. UUID results already have a raw decoding path;
  add typed UUID binding for generic SQL. Keep date/time forms unambiguous.
  Arrays, binary values, and arbitrary PostgreSQL extension types are not
  automatically supported by a result schema; require explicit SQL casts or
  JSON/base64 conversion until codecs are intentionally added.

Object Model adapters preserve existing capability output schemas. Exact values
must survive the new host boundary; legacy numeric presentation belongs in an
explicit compatibility adapter, with tests documenting any remaining precision
loss. Do not silently change every existing numeric capability field to a string
as part of the transport migration. Raw SQL's new host contract remains lossless
for its declared exact numeric types.

Examples of abstractions built without additional host methods:

- create: schema lookup -> validate/normalize -> `execute(INSERT ... RETURNING)`;
- query: schema lookup -> compile conditions/order/projection -> `query` -> map
  SQL column names back to existing instance fields;
- update/delete: generate the current partial-update/soft-delete SQL -> execute;
- aggregate: existing expression/aggregate builder -> query -> columns/rows;
- memory: schema bootstrap plus query/insert/update through these same methods.

Bind data values, and quote/validate identifiers separately. Identifiers cannot
be value parameters. Audit existing builders that render validated literals;
retain their checks and prefer typed bindings where feasible. Never introduce
unvalidated SQL fragments from workflow mappings.

### Batch and transaction semantics

Provide two explicit modes:

- `atomic` (default): one connection and transaction; ordered execution; any
  failure rolls back the entire batch. Results are exposed only after commit.
- `independent`: each statement gets its own bounded transaction; report each
  committed/rejected result. Continue after deterministic per-statement errors;
  stop on cancellation, deadline, exhausted result budgets, connection loss, or
  unknown commit outcome.
  Remaining entries are marked not started. This supports partial-progress bulk
  semantics without pretending the whole batch was atomic.

Use one database connection per batch. No guest-visible begin/commit/rollback
handles and no transaction retained across host calls or durable suspension.
Host-owned transaction setup and limits cannot be overridden by guest SQL:
reject transaction-control and policy-changing session commands using parsed
statement classification rather than string-prefix matching. Keep database
permissions as the authority for SQL effects; arbitrary SQL is not made safe
by a superficial keyword filter.

Read results needed within write transactions use a statement's explicit result
specification, including SELECT or command RETURNING. These entries have the
same write authority as the enclosing batch; they are not guarded read-only
queries. Use the separate `query` function for read-only work and its retry
classification.

Bound statement count, total request/response bytes, rows, concurrency, each
statement deadline, and total batch duration. Initially cap batches at 1,000
statements (operator may lower); retain the 64 MiB request and effective 8 MiB
guest-response ceilings. SQL's configured limits may be stricter. Large bulk
requests are chunked by the agent, respecting parameter limits. Chunking an
atomic operation across host calls is not atomic: reject oversize atomic work
or use fewer set-based statements rather than silently splitting it.

No result-to-parameter substitution language in v1. Generate identifiers before
a batch where existing semantics permit, or use SQL CTEs/RETURNING to express
related work in one statement. Arbitrary guest computation between transactional
statements is deliberately outside these three primitives.

Errors include a safe code/SQLSTATE where applicable, statement index, and
transaction outcome (`not-started`, `rolled-back`, `committed`, `unknown`). An
unknown commit outcome is never auto-retried. Decode/size failures in a command's
RETURNING rows occur before commit and roll back. A lost response after commit
can still be indeterminate to the workflow; this does not provide exactly-once
mutations across durable replay.

## Move Object Model behavior into the agent

Extract `runtara-object-model-core` from current store/server code. It contains
schema/column/condition types, validation, name mapping, identifier quoting,
DDL/filter/aggregate builders, bulk normalization, and statement/result plans.
It must compile for `wasm32-wasip2` without SQLx or network access. Inject values
such as time/IDs into pure planning functions rather than reading them there.

`runtara-object-store` reuses this core for native public API behavior and keeps
its pools, driver code, and native orchestration. The Object Model agent uses
it with an async SQL client. Avoid copying implementations into the agent or
pulling the native object-store crate into WASM.

Preserve the existing physical data model and `__schema` records. Resolve logical
schema names and read metadata with ordinary query calls. Agent-managed schema
bootstrap must update metadata, create tables, and create indexes in an atomic
batch, preserving existing conflict/tombstone behavior. Keep database unique
constraints and test concurrent memory bootstrap. Do not replace these semantics
with only `CREATE TABLE IF NOT EXISTS`.

Native SQL pool creation must not issue Object Model DDL. The native public API
initializes metadata explicitly; the WASM agent bootstraps its metadata table,
schema record, physical table, and indexes within its SQL batch. A shared
transaction-scoped advisory lock serializes initial metadata creation, including
concurrent native API and agent callers. Raw SQL works without metadata or DDL
privileges.

Store layout options such as metadata-table name, auto columns and soft-delete
mode must be explicit. Supply a safe versioned Object Model layout descriptor
through the existing native connection metadata interface; derive it from the
same configuration used by native ObjectStoreManager. It contains no credentials
or database URL. This is configuration discovery, not a fourth database method.
Do not trust guest-provided layout hints for host policy decisions.

Port all existing capability semantics, including:

- generated/default/system fields, timestamp naming, typed JSON/vector values;
- schema-name-to-table-name mapping, deleted-row filtering, uniqueness;
- structured/subquery conditions, score expressions, sorting, pagination/counts;
- bulk object/columnar inputs, skip/stop validation, conflict skip/upsert,
  partial-update presence, original input error indices and affected counts;
- create-if-not-exists's current read-then-create behavior (not guaranteed
  atomic), memory load/save output, and schema-exists-as-success behavior.

Build a compatibility matrix from implementation/tests before porting, including
current batch/chunk transaction boundaries. Choose atomic or independent mode
per operation to preserve behavior, rather than making every bulk call atomic
without documenting the change. Keep schema metadata fresh enough for public
API mutations; do not introduce indefinite agent schema caches.

## Host responsibilities and policy boundary

Add `DatabaseHost` to component-host and inject a native adapter backed by
ConnectionsFacade, ObjectStoreManager and the guarded SQL driver. Reuse pools
and tenant-aware connection lookup; credentials never enter WASM.

Wire concurrent async bindings into workflow execution, embedded runners,
interactive agent testing, scoped children, and published workflows-as-agents.
Restricted trusted instances deny database imports. Missing service/tenant is a
configuration error; unrelated components do not require a database backend.

Apply database entitlement and generic execution limits on every host call.
Preserve statement timeout, read-only query enforcement, request/response limits,
audit attribution, cancellation, and pool recovery. Do not log bound values,
credentials, database URLs, or new full SQL payloads. Budget WIT allocations and
native row decoding, not only the final serialized output.

Schema validation in the agent is a convenience contract, not a security boundary.
Raw SQL already permits bypassing Object Model abstractions. In particular,
Object Model row/schema quotas cannot be made authoritative merely by retaining
checks in the agent. Keep existing database-enforced constraints; inventory
higher-level quotas and explicitly distinguish convenience limits from enforced
host limits. If a quota must constrain arbitrary SQL, enforce it in the database
or restrict SQL privileges in a separate policy change—do not claim the generic
host can infer every higher-level operation from SQL.

Current quota inventory: `maxObjectSchemas` remains enforced by the public
schema-create handler; it is not a generic SQL limit. The effective
`objectModelBulkRequestLimit` (infrastructure/tier minimum) is supplied in the
safe layout descriptor and checked by the shared bulk planner. Arbitrary SQL can
bypass both Object Model conveniences. Database entitlement, connection ownership,
SQL time/row/byte/batch bounds, and database role privileges remain authoritative
host/database restrictions.

The request-byte limit is checked on the lifted WASM slice before copying it
into native memory. Decoded PostgreSQL rows are streamed, with raw cell sizes
checked before allocating decoded values. SQLx still receives each protocol row
before this check; the response cap is not a hard cap on the driver's protocol
buffer or total process RSS. Guest memory and pool/task concurrency have their
separate execution limits.

Tenant authority controls connection ownership; rows are isolated according to
the selected database/role. Sharing a database between tenant connections does
not introduce automatic row isolation. Public HTTP authentication/authorization
and current public/MCP SQL contracts remain in their existing paths.

## Implementation sequence

1. **Characterize behavior.** Capture current object/SQL type conversions,
   validation, metadata layout, bulk commit boundaries, mapping and memory
   behavior as compatibility fixtures. Include numeric/null edge cases.
2. **Extract pure core and SQL contract.** Make native code reuse the extracted
   logic first. Verify native public API parity and WASM compilation before
   porting orchestration. Add exact-value codecs for the new wire path without
   silently changing existing API serialization.
3. **Implement SQL host service.** Add query, execute with RETURNING, and both
   batch modes; enforce authority, limits, errors, and transaction outcomes.
   Test on isolated PostgreSQL, then through real WIT imports with fake/native
   adapters in all execution paths.
4. **Port the Object Model agent.** Add typed client wrappers and safe layout
   discovery; port schemas/CRUD, conditions/aggregation, bulk, then memory.
   Preserve capability IDs and user-facing inputs/outputs. Keep workflow mapping
   and transforms in their existing WASM layer.
5. **Remove internal transport.** Delete internal Object Model routes/handlers,
   `runtara-http` from this agent, `RUNTARA_OBJECT_MODEL_URL`, dispatcher/runner
   URL fields and injection, and obsolete HTTP fixtures/docs. Retain native
   database connection configuration and public APIs.
6. **Rebuild and release.** Build agents and recompose affected workflows using
   existing generators. No retained legacy internal endpoints. Drain old
   running/suspended artifacts on the previous release or explicitly restart
   them; never silently replace suspended code. Roll back matching host/bundles.

## Verification and acceptance

- Exact codec round trips: i64 boundaries, decimals, typed NULL versus JSON null,
  UTC/local timestamps, vectors, UUIDs, empty results, duplicate column labels,
  unsupported types, defaults, and omitted update fields.
- Native-versus-WASM abstraction parity against separate identical test databases:
  schema metadata/tables, rows, CRUD, bulk modes, aggregates, conditions, mapping,
  soft deletes and memory. Never shadow live writes to compare paths.
- Transactions: command RETURNING, atomic rollback on a later error, independent
  partial commits, schema/DDL rollback, bootstrap races, read-only enforcement,
  forbidden transaction commands, batch caps and unknown commit outcomes.
- Authority/limits: forged tenant/env/metadata, cross-tenant/wrong-type connections,
  entitlement denial, restricted instances, injection attempts, byte/row/statement
  limits, safe errors, and cancellation during lookup/acquire/query/commit.
- Real components: standalone tests, composed workflows, parallel branches,
  scoped children and workflow-as-agent. Verify cancellation/reuse at each memory
  stage and no leaked transactions or pool resources.
- Public Object Model HTTP/MCP regression tests; removed internal routes return
  unavailable. Run migrated Object Model workflows with no internal listener.
  Inspect actual component imports: the SQL interface is the database contract.

Run pinned-toolchain format/clippy, focused contract/core/agent tests, isolated
server/object-store PostgreSQL suites, `scripts/build-agent-components.sh`, and
CI's feature-gated component-host, direct-WASM, deadline and scoped-runner suites.
Do not claim unrun checks. Regenerate public API clients only if those contracts
actually change.

Acceptance: existing Object Model capabilities and memory work via three native
SQL functions; higher-level workflow orchestration runs in WASM; shared pure
logic prevents semantic drift; data mapping and declared SQL types are defined
end to end; the internal Object Model HTTP dependency is removed.

This document supersedes the Object Model portion of
[the broader host-interface plan](wasm-host-interfaces-plan.md).
