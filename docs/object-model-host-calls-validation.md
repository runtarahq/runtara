# Object Model native SQL migration validation

The implementation follows [the host-call plan](object-model-host-calls-plan.md).
The database ABI has exactly `query`, `execute`, and `execute-batch`. Workflow
Object Model semantics execute in the agent, using a shared pure Rust core.

## Acceptance evidence

| Requirement | Implementation and verification |
| --- | --- |
| Three native imports, no internal HTTP fallback | Canonical database WIT; component-host linker; compiler registration. Built Object Model component imports database SQL and safe connection metadata, with no HTTP/socket/filesystem imports. Internal Object Model handlers/routes and guest URL configuration are removed. |
| Authoritative tenant and opaque connection | Five component-host database tests cover forged environment, missing authority, restricted instances, oversized requests, and scoped child tasks. Server PostgreSQL connection E2E verifies tenant/type/unknown-ID denial after a successful lookup. Native SQL rechecks connection ownership on every call. |
| Credentials, pools, native driver remain host-owned | NativeDatabase resolves through ConnectionsFacade and shares the bounded pool cache. Guest requests contain no tenant, credentials, or database URL. Connection errors are redacted. |
| SQL has no implicit Object Model DDL | ObjectStore::connect and the manager's SQL pool path do no metadata setup. Server and actual WASM tests first query a fresh database and verify the metadata table is absent. The agent then bootstraps metadata and memory with its own atomic SQL batch. |
| Mapping and abstraction parity | Shared core covers schema validation, SQL planning, defaults, field presence, bulk normalization, DDL and result mapping. Eight agent/PostgreSQL tests compare native results and exercise fresh/concurrent/tombstoned schemas, CRUD, nulls, bulk modes, aggregate conditions and legacy SQL schemas. |
| Exact generic SQL values | Ten native driver tests include i64 limits, high-precision decimals, SQL NULL versus JSON null, date/time/UUID values, duplicate columns and empty row metadata. Legacy numeric presentation is isolated in the agent adapter. |
| Transactions and bounds | Driver tests cover RETURNING validation before commit, atomic rollback, independent partial commits/rejected entries, shared batch row budgets, statement caps, read-only CTE enforcement, DDL rollback, deadlines and pool reuse with one connection. Unknown write outcomes are never retried by the agent. |
| All existing capabilities in actual WASM | The real component-to-PostgreSQL test invokes all 14 exported capabilities, including bulk updates/deletes, raw reads/writes, fresh memory bootstrap/save/reload and SQL NULL property omission. No internal HTTP listener is used. |
| Cancellation and execution paths | Full component-host suite passed (one explicitly manual soak ignored). Five Object Model cancellation/reuse cases and eight memory deadline cases passed again after the final bootstrap changes. Five native database authority tests include scoped child Stores. The composed workflow suite passed 400 tests; three explicitly manual measurements were ignored. |
| Embedded runner and public API/MCP regression | All 13 scoped-runner/cooperative-stop PostgreSQL tests passed. All 73 native ObjectStore integration tests passed. Server Object Model unit selection passed 39 tests, including DTO/MCP schemas, entitlement middleware and public service helpers; the real connection-routing E2E passed. |
| Build, lint and formatting | Built all 26 agents and both shared workflow components with the pinned toolchain. Strict all-target Clippy passed across contract/core/store/agent/host/environment/workflows/server with affected integration features; focused follow-ups covered the final batch and deadline changes. Formatting and diff whitespace checks passed. |

The full deadline run passed 117 tests and exposed two existing 200 ms fixtures
that timed out before their first HTTP request, also reproducible in a focused
run. Their positive startup allowance is now one second, consistent with nearby
pending-I/O tests; the request-count, retry, timeout and output assertions remain.
Both focused reruns passed. Memory's eight cases also passed on the final component.

The duplicate push CI run exposed another timing-sensitive fixture: early Split
replay had only 1 ms of live execution budget to reach its suspension. It now
replays 500 ms before expiry, preserving the unchanged stored-deadline assertion
and the exact-expiry failure check. The parallel PR CI run passed all functional
suites on the same implementation.

## Reproduction

Use `RUSTC_WRAPPER=` if the local wrapper does not support the pinned compiler.
Build with `scripts/build-agent-components.sh` and set
`RUNTARA_AGENT_COMPONENTS_DIR` to `target/wasm32-wasip2/release`.

The new database targets require an isolated `TEST_DATABASE_URL`:

- `cargo test -p runtara-object-store --features db-integration-tests --test integration --test native_database`
- `cargo test -p runtara-agent-object-model --features db-integration-tests --test native_sql_model`
- `cargo test -p runtara-component-host --features component-integration-tests,db-integration-tests --test object_model_database`

The existing server connection E2E owns its Docker fixture. The environment
runner tests require a separate isolated `TEST_ENVIRONMENT_DATABASE_URL` and
`--test-threads=1`. CI includes the new targets and their required features.

## Rollout and limits

Deploy the host with rebuilt agents and recomposed workflows. Drain old artifacts
on their previous release, or explicitly restart them after recompilation; do
not replace suspended code silently. There are no retained legacy internal
Object Model endpoints. Public HTTP/MCP APIs and ordinary outbound HTTP remain.

The standalone running-server E2E script passed against an isolated local server:
removed native dispatch/presign/Object Model endpoints return 404 and outbound
HTTP remains available. Live public schema/CRUD/default/null mapping and
parameterized SQL checks passed. Interactive WASM agent execution through the
server's native SQL service preserved i64 precision. No production deployment,
artifact draining, or manual capacity soak was performed.
GitHub checks for the new revision are reported separately from these local runs.

The SQL response budget bounds decoded rows and serialized results. SQLx receives
a protocol row before its raw cell sizes can be checked; this is not a hard
process-RSS cap. Exact SQL values are preserved at the ABI; compatibility
presentation can retain existing JSON-number precision limits. Generic SQL does
not enforce Object Model schema/row quotas, and does not provide automatic row
isolation or exactly-once writes across lost responses/durable replay.
