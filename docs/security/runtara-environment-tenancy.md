# Environment tenant isolation

This implementation extends the [core tenant contract](runtara-core-tenancy-api.md)
into `runtara-environment`. It is the first environment slice of the
[shared-tenancy transition](shared-tenancy-transition.md), not approval to admit
multiple tenants through the current server.

## API and behavior

Tenant identity comes from the trusted embedding host. `TenantId` validates its
representation; it does not authenticate or authorize a caller. Repositories and
pools remain reusable. Tenant operations take a mandatory `&TenantId` as the first argument after the
`self` receiver for methods, and as the first argument for free functions and
handlers, before shared state or database dependencies. Tenant-bound constructors
likewise take tenant identity first.

| Boundary | Implemented behavior | Missing, foreign, and failure handling |
| --- | --- | --- |
| Instance repository and direct database queries | Detail, image binding, lists, counts, metrics, stderr and resource writes apply the supplied tenant. Joins also constrain ownership. Optional tenant filters are removed. | Foreign IDs have the existing missing result; no foreign metadata or diagnostic writes. Database failures propagate. |
| Image registry | ID/name lookup, registration, name claiming, listing, artifact checks and deletion are scoped. Registration checks payload tenant before I/O. The process-global ID-only cache is removed. | Foreign lookup returns `None`; foreign deletion returns `false`. Global ID collisions are sanitized and cannot overwrite a foreign image. |
| Launch queue | Initial claim/replay, enqueue, claims, promotion, renewal, recovery, reconciliation, expiry, cancellation and terminal transitions require a tenant. Selection filters precede batch limits. Image and instance ownership are checked inside the write transaction. | Foreign work is unavailable. Existing guarded `false`/`None`/empty-batch outcomes remain; a foreign initial instance collision exposes no launch. Storage errors remain errors. |
| Container registry | Registration requires an owned instance and matching launch. Reads, control delivery, leases and cleanup are scoped; physical handle identity remains checked. | A mismatched supplied handle is rejected before I/O. A foreign ID cannot return, abort, replace or delete a handle. |
| Workers and recovery | Dispatch, wake, heartbeat, startup recovery, database retention, image cleanup, run cleanup and shutdown use an explicit configured tenant. | A worker for A leaves B's eligible work and artifacts untouched. Tenant enumeration and assignment are still a separate host responsibility. |
| Server adapters | Shared clients take `&TenantId` per operation. HTTP/MCP propagate authenticated identity; background callers carry assigned scope. Conflicting legacy option tenants fail before I/O. | Missing and foreign instances retain typed missing errors and HTTP 404 responses. Omitted optional filters remain within the mandatory operation scope. See [server propagation](runtara-server-tenant-propagation.md). |

Instance, image and launch IDs remain globally unique. This change adds no SQL
migration or RLS policy. Database roles, trusted host code and filesystem access
remain privileged; application scoping is not a database or OS security boundary.
The artifact-inventory CLI remains an explicitly invoked operator diagnostic.

## Artifact compatibility

New artifacts use SHA-256 encoded identity components under:

- `DATA_DIR/tenants/<tenant hash>/images/<image hash>/`
- `DATA_DIR/tenants/<tenant hash>/runs/<instance hash>/<launch hash>/`

Opaque IDs, including path separators or `..`, cannot change that layout. Tenant
image cleanup visits only its assigned namespace, and stale image deletion locks
and rechecks database references before removing rows. Run cleanup likewise scans
only its assigned tenant. Directory entries that are symlinks are not traversed.

Existing image rows retain their registered binary paths and remain readable
through scoped registry lookup. Legacy flat image directories and old run layouts
are deliberately retained: automatic cleanup cannot safely infer ownership from
those paths. Operators need a separate ownership-verified migration/retention
procedure to reclaim them. No bulk file migration occurs here.

## Verification

`tests/tenancy_test.rs` exercises known foreign IDs across reads, writes, launch
replays/transitions, batch limits, container control and uploaded artifacts using
PostgreSQL. Worker tests cover preservation of another tenant's files and active
images. Existing database and composed-runner suites exercise positive lifecycle
behavior after API migration. Server unit tests cover typed error propagation and
indistinguishable checkpoint 404 responses.

Run database-backed tests only against an isolated test database:

```sh
cargo test -p runtara-environment --features db-integration-tests -- --test-threads=1
cargo test -p runtara-environment --features scoped-workflow-integration-tests \
  --test scoped_runner_test --test cooperative_stop_test -- --test-threads=1
```

The latter requires `TEST_ENVIRONMENT_DATABASE_URL` and built components. The local
HTTP test starts its own disposable PostgreSQL with pgvector and Valkey, boots the
actual server with A configured and A/B authenticated using throwaway API keys. It
checks HTTP/MCP reads and controls, worker cleanup, dispatch isolation and restart recovery:

```sh
scripts/build-agent-components.sh
cargo build -p runtara-server
python3 e2e/test_environment_tenancy.py
```

Authenticated A/B request switching is covered by the [server propagation slice](runtara-server-tenant-propagation.md). Tenant directory orchestration,
authenticated instance capabilities, resource fairness, RLS and shared-environment
admission still require the transition plan's later work.
