# Server runtime tenant propagation

`RuntimeClient` and `EnvironmentClient` share environment state and pools without
storing a tenant. Every tenant operation takes `&TenantId` first after `self`.
Construction does not select an identity; every read, list, control, upload,
metric query, launch and polling operation requires explicit scope.

HTTP runtime handlers obtain scope from `RuntimeTenant`, which validates the
identity already supplied by authentication. Services that still accept host
identity strings validate them before entering the runtime client. Request
bodies, query parameters, resource IDs and the configured deployment tenant do
not establish runtime authority. Legacy internal options containing tenant fields
must agree with the explicit scope; conflicts fail before database or upload I/O.

MCP resolves the authenticated caller on each tool invocation and creates a
separate scoped server view. Direct repository/cache operations and internal HTTP
calls therefore use the same tenant. The internal bridge requires caller identity
and rejects scope mismatches; it has no synthetic-auth fallback. This does not
implement transport session ownership, replay isolation or deployment admission.

SSE streams, disconnect cleanup and execution observers retain explicit tenant
identity. Compilation, pending-input queries and report providers pass their
caller's scope. Background reconciliation uses the tenant on the durable
reservation; trigger execution uses its assigned event tenant. The runtime launch
sets both `TENANT_ID` and `RUNTARA_TENANT_ID` from the operation scope.

Missing and foreign instances propagate the same typed `InstanceNotFound` through
client reads and controls. Image lookup returns `None` for either case. Stop/resume
handlers preserve the missing-instance category, and execution control distinguishes
storage failures from missing resources. Conflicting internal option scope is a
validation error, never a fallback to another tenant.

## Verification

- Server unit tests cover authenticated scope validation, conflicting options
  before I/O, same-client A/B signals and checkpoint reads, and MCP caller identity.
- `runtime_tenancy` exercises a shared client and pool against PostgreSQL: owned
  reads/lists/images/metrics/signals/stop, concurrent A/B reads, and foreign versus
  missing instances across read and control operations.
- `tenant_launch_tests` checks that both tenants enqueue owned launches and store
  the correct guest tenant variables, while foreign image launches fail.
- `e2e/test_environment_tenancy.py` boots the actual server with disposable
  PostgreSQL and Valkey. Fresh throwaway API keys authenticate A and B against the
  same server, including concurrent HTTP reads and A→B→A MCP calls. It checks owned
  controls, foreign/missing responses, ineffective body/query tenant overrides,
  and preservation of B's work/files by workers assigned to A.

Use a separate `TEST_ENVIRONMENT_DATABASE_URL` for the client and launch tests.
They run the combined core/environment migrator; the core-only test database's
migration history must remain separate. CI provisions both databases.

```sh
cargo test -p runtara-server --lib
cargo test -p runtara-server --features db-integration-tests --test runtime_tenancy
cargo test -p runtara-server --features db-integration-tests --lib tenant_launch_tests
cargo build -p runtara-server
python3 e2e/test_environment_tenancy.py
```

## Remaining shared-deployment boundaries

This change does not admit a production shared deployment. Dedicated OIDC still
checks configured `TENANT_ID`; local and trust-proxy providers remain configured
for one tenant. The test uses the existing validated API-key row authority path,
not a new shared-mode auth policy. Assignment, membership and admission need the
[transition plan](shared-tenancy-transition.md)'s separate implementation.

Environment workers and the guest/core HTTP listener are still assigned one tenant.
The A/B launch test verifies durable acceptance, not execution of B's guest through
that listener. Tenant enumeration, per-instance capabilities, other server storage
and cache namespaces, RLS, quotas and scheduling fairness remain separate work.
