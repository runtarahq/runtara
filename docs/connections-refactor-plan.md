# Connections Refactor Plan

## Objective

Make `runtara-connections` the single implementation of connection management in the workspace, and have `runtara-server` consume it as a pluggable module.

The primary architectural goal is not only deduplication. It is to create a stable boundary where:

- credential storage can be replaced without rewriting connection consumers
- auth injection and token refresh remain reusable
- per-connection rate limiting remains reusable
- host applications can mount HTTP routes from the crate or use exported services directly

This refactor should happen before implementing at-rest encryption so encryption lands in one place only.

## Current State

There are currently two implementations of connection logic:

- `crates/runtara-connections/src/...`
- `crates/runtara-server/src/api/{dto,handlers,repositories,services}/...`

The server does not mount the routers from `runtara-connections`. Instead it uses its own duplicated handlers and services in `crates/runtara-server/src/server.rs`.

The live server-side implementation is used by:

- connection CRUD and type discovery
- OAuth authorize/callback
- rate limit endpoints and tracking
- runtime credential resolution
- internal proxy auth injection
- file storage connection resolution
- object model external database resolution
- webhook verification and channel consumers
- agent testing and agent execution connection loading

Because of this duplication, any security or behavioral change currently requires patching both codepaths.

## Scope

### In Scope

- define the responsibility boundary of `runtara-connections`
- make `runtara-connections` self-contained and host-configurable
- move shared connection logic out of `runtara-server`
- make `runtara-server` mount and consume `runtara-connections`
- replace direct SQL access to `connection_parameters` with exported crate APIs

### Out of Scope

- implementing at-rest encryption in this refactor
- redesigning file storage, object model, or channels as separate crates
- broad cleanup of unrelated server architecture

## Responsibility Boundary

The core rule is:

If the code answers "how do I store, resolve, refresh, inject, or throttle a connection?", it belongs in `runtara-connections`.

If the code answers "what business operation do I perform once I have a connection?", it belongs outside the crate.

### `runtara-connections` Owns

- connection metadata lifecycle
- credential lifecycle
- OAuth state and token refresh logic
- rate limit policy and analytics for connections
- resolution of a connection ID into runtime-ready auth/configuration
- auth strategy selection for each integration type
- optional HTTP adapters for CRUD, OAuth callback, runtime resolve, and proxying

### `runtara-connections` Does Not Own

- file storage business logic
- object model business logic
- scenario business logic
- trigger/channel lifecycle policy
- webhook registration policy
- agent testing/execution orchestration
- host auth middleware
- host OpenAPI composition

Those systems should depend on `runtara-connections` through exported services.

## Design Principles

### 1. Storage Swappability

The crate must not let most consumers read raw `connection_parameters` directly from SQL.

Instead, consumers should ask for higher-level resolved data through crate services. That keeps the storage backend replaceable:

- PostgreSQL JSONB
- encrypted PostgreSQL payload
- split metadata + secret store
- Vault or KMS-backed store
- cloud secret manager

### 2. Auth Proxy Is a Consumer, Not the Owner

The internal auth proxy approach stays, but the proxy should become a transport adapter over `runtara-connections` resolution logic.

The proxy should not know:

- where credentials are stored
- how OAuth refresh is implemented
- how SigV4 works internally
- how rate limit preflight is computed

It should ask the connections crate to prepare the outbound request.

### 3. Keep Provider-Specific Details Internal

A shape like this is not a good long-term boundary:

```rust
ResolvedProxyAuth {
    base_url: Option<String>,
    headers: HashMap<String, String>,
    aws_signing: Option<AwsSigningParams>,
    rate_limit: Option<ResolvedRateLimit>,
}
```

`aws_signing` is a leaky abstraction. It privileges one auth mode in a supposedly generic API.

Prefer one of these designs:

- a generic auth plan:

```rust
enum AuthStep {
    SetHeader { name: String, value: SecretString },
    SetQueryParam { name: String, value: SecretString },
    ApplySigner(SigningMethod),
}
```

- or better, a prepared-request API where signing stays internal:

```rust
async fn prepare_outbound_request(...) -> Result<PreparedRequest, ConnectionsError>
```

The prepared-request API is the stronger boundary and is preferred for the proxy path.

## Target Architecture

### Core Domain Layer

`runtara-connections` should expose a core domain that does not depend on Axum handlers to be useful.

Suggested core components:

- `ConnectionCatalog`
  Owns non-secret metadata such as `id`, `tenant_id`, `title`, `integration_id`, `connection_subtype`, `status`, `valid_until`, `rate_limit_config`, and flags like `is_default_file_storage`.

- `CredentialStore`
  Owns secret or opaque credential payloads for a connection. This is the main future swap point.

- `OAuthStateStore`
  Owns temporary OAuth authorization state records.

- `RateLimitStore`
  Owns real-time bucket state and historical usage analytics.

- `ConnectionResolver`
  Main service that combines catalog, credentials, auth strategies, token refresh, and rate-limit policy.

- `AuthStrategyRegistry`
  Maps `integration_id` to auth resolution behavior.

### Adapter Layer

The crate should also expose optional adapters:

- Axum tenant router
- Axum public OAuth callback router
- Axum internal runtime router
- optionally Axum internal proxy router
- SQLx PostgreSQL implementations
- Redis/Valkey-backed rate limit implementation

### Host Layer

`runtara-server` should:

- construct the module with host config
- inject tenant/auth context before mounted tenant routes
- mount the module's routers
- call its services from file storage, object model, channels, agent execution, and other host features

## Proposed Public API Shape

The exact names can vary, but the responsibilities should look roughly like this.

### Module Construction

```rust
pub struct ConnectionsModule {
    pub services: ConnectionsServices,
}

pub struct ConnectionsServices {
    pub catalog: Arc<dyn ConnectionCatalog>,
    pub credentials: Arc<dyn CredentialStore>,
    pub resolver: Arc<ConnectionResolver>,
    pub rate_limits: Arc<RateLimitService>,
}

pub struct ConnectionsModuleConfig {
    pub db_pool: PgPool,
    pub redis_url: Option<String>,
    pub public_base_url: String,
    pub http_client: reqwest::Client,
}
```

### Router Construction

```rust
impl ConnectionsModule {
    pub fn tenant_router(&self) -> Router;
    pub fn public_router(&self) -> Router;
    pub fn internal_router(&self) -> Router;
    pub fn proxy_router(&self) -> Router;
}
```

If `proxy_router()` feels too opinionated, the crate can instead expose a `ProxyRequestAuthorizer` service and keep the HTTP endpoint in `runtara-server`.

### Resolution API

Use two resolution modes.

#### A. Proxy/Auth Preparation

Preferred path for HTTP-like integrations.

```rust
pub struct PreparedOutboundRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub rate_limit: Option<ResolvedRateLimit>,
}

pub trait ProxyRequestAuthorizer {
    async fn prepare_request(
        &self,
        tenant_id: &str,
        connection_id: &str,
        request: OutboundRequest,
    ) -> Result<PreparedOutboundRequest, ConnectionsError>;
}
```

#### B. Runtime Resolution

Escape hatch for integrations that truly need raw parameters in memory.

```rust
pub struct RuntimeResolvedConnection {
    pub integration_id: String,
    pub connection_subtype: Option<String>,
    pub parameters: serde_json::Value,
    pub rate_limit: Option<RuntimeRateLimitState>,
}
```

### Validation API

The server still needs scenario validation against existing connection IDs.

Expose a small catalog-oriented helper:

```rust
async fn list_existing_ids(
    &self,
    tenant_id: &str,
    connection_ids: &[String],
) -> Result<HashSet<String>, ConnectionsError>;
```

## Data Ownership Model

The current schema stores `connection_parameters` as one JSONB payload in `connection_data_entity`.

For this refactor:

- keep the schema as-is
- move all access behind crate ports and services
- stop letting consumers query it directly

After consolidation, future work can split the payload conceptually into:

- config: non-secret settings
- secrets: secret material

That split should be an implementation detail behind `CredentialStore` and resolver APIs, not a contract leaked to consumers.

## Integration With the Auth Proxy Approach

The current internal proxy should be modeled as a thin consumer of `runtara-connections`.

### Desired Flow

1. caller sends internal proxy request with `tenant_id` and `connection_id`
2. proxy calls `prepare_request(...)` on the connections crate
3. the connections crate:
   - loads metadata
   - loads credentials
   - refreshes OAuth tokens if needed
   - applies integration-specific auth logic
   - performs rate-limit preflight
   - returns a prepared outbound request
4. proxy sends the upstream request and returns the response
5. the connections crate records rate-limit telemetry

### Why This Boundary Is Good

- proxy code no longer cares where credentials are stored
- provider-specific auth details stay inside the crate
- encryption later lands in one place only
- non-proxy consumers can still use the same auth resolution logic

## Detailed Migration Plan

This work should be done in phases. Each phase should keep the system working.

### Phase 0: Freeze the Boundary

Goal:

- agree on what moves and what stays

Actions:

- treat `runtara-server/src/api/{dto,handlers,repositories,services}/connections*`, `oauth*`, and `rate_limits*` as deprecated duplicates
- declare `runtara-connections` the only future owner for connection domain logic
- avoid adding new connection logic to `runtara-server`

Output:

- approved architecture and migration sequence

### Phase 1: Make `runtara-connections` Self-Contained

Goal:

- remove hidden host assumptions from the crate

Actions:

- replace direct `std::env::var("PUBLIC_BASE_URL")` reads in `crates/runtara-connections/src/handler/oauth.rs`
- use crate-owned shared state instead of only `PgPool` as Axum state
- thread `public_base_url`, `redis_url`, and shared `reqwest::Client` through crate config/state
- ensure all handlers/services in the crate depend only on crate-owned config and traits

Output:

- the crate can be instantiated by a host without env-based hidden coupling

### Phase 2: Move Shared Auth Resolution Into `runtara-connections`

Goal:

- centralize reusable auth injection logic

Actions:

- move `crates/runtara-server/src/api/services/proxy_auth.rs` into `runtara-connections`
- redesign the exported API away from `aws_signing` as a top-level special case
- implement either:
  - `prepare_request(...)`, or
  - a generic auth-plan API
- make token refresh and persisted token updates happen through the crate only

Output:

- one reusable auth resolution path shared by proxy and other consumers

### Phase 3: Export a Service Facade

Goal:

- make the crate usable without going through HTTP

Actions:

- expose repository/service facade from `runtara-connections`
- include:
  - metadata lookup
  - runtime connection resolution
  - proxy request preparation
  - default file storage lookup
  - existing-ID lookup for validation
  - rate-limit service access

Output:

- other crates can depend on `runtara-connections` directly instead of copying logic

### Phase 4: Mount Crate Routers in `runtara-server`

Goal:

- stop using duplicated server HTTP handlers

Actions:

- replace server route registration in `crates/runtara-server/src/server.rs` for:
  - `/api/runtime/connections*`
  - `/api/runtime/rate-limits`
  - `/api/runtime/connections/{id}/rate-limit-*`
  - `/api/runtime/connections/{id}/oauth/authorize`
  - `/api/oauth/{tenant_id}/callback`
  - `/api/connections/{tenant_id}/{connection_id}`
- mount routers returned by `runtara-connections`
- add a small adapter in `runtara-server` that converts `AuthContext` into `runtara_connections::TenantId`

Output:

- live HTTP traffic for connections flows through the crate

### Phase 5: Migrate Internal Consumers

Goal:

- eliminate direct dependency on server-local connection code

Actions:

- update `crates/runtara-server/src/api/services/file_storage.rs`
  - use the crate resolver/service facade
- update `crates/runtara-server/src/api/services/object_model.rs`
  - use crate runtime resolution for external DB URLs
- update `crates/runtara-server/src/api/services/agent_testing.rs`
  - stop direct SQL reads of `connection_parameters`
- update `crates/runtara-server/src/api/services/agent_execution.rs`
  - stop direct SQL reads of `connection_parameters`
- update:
  - `webhook_manager.rs`
  - `webhook_verification.rs`
  - channel files in `crates/runtara-server/src/channels/`
  so they use exported services from `runtara-connections`
- update scenario services/handlers to use crate `list_existing_ids(...)`

Output:

- all server consumers use the crate as the single source of truth

### Phase 6: Decide Proxy Endpoint Ownership

There are two valid end states.

#### Option A: Proxy HTTP Endpoint Lives in `runtara-connections`

Use when:

- the proxy is considered part of the connection runtime plane
- you want host reuse with minimal extra composition

Pros:

- strongest encapsulation
- fewer server-specific wrappers

Cons:

- the connections crate includes more HTTP behavior

#### Option B: Proxy HTTP Endpoint Stays in `runtara-server`

Use when:

- you want the crate to expose only services, not transport

Pros:

- keeps the crate smaller at the HTTP layer

Cons:

- server still owns a thin but important adapter

Recommendation:

- keep the proxy behavior in `runtara-connections`
- if necessary, let `runtara-server` own only host-specific mounting and internal-only exposure

### Phase 7: Remove Duplicated Server Implementation

Goal:

- delete the dead copy

Actions:

- remove duplicated DTOs, handlers, repositories, and services in `runtara-server` related to:
  - connections
  - oauth
  - rate limits
- remove duplicate imports/usages in server code
- update server OpenAPI registration to point at crate types and handlers

Output:

- one implementation remains in the workspace

### Phase 8: Add Contract Tests

Goal:

- lock the new boundary down before encryption work

Tests to add:

- CRUD contract tests for connection lifecycle
- OAuth authorize/callback tests
- runtime resolution tests
- proxy request preparation tests for:
  - bearer
  - API key
  - basic auth
  - OAuth refresh
  - SigV4-backed integrations
- rate-limit preflight and telemetry tests
- consumer integration tests for:
  - file storage
  - object model external DB lookup
  - agent testing/execution connection loading

Output:

- a stable contract for future credential storage replacement

### Phase 9: Implement Replaceable Credential Storage

Only after consolidation:

- add `CredentialStore` abstraction-backed encryption or external secret storage
- migrate SQL-backed implementation behind the trait
- add backfill/migration logic once only one implementation remains

## Concrete File Movement Plan

### Code to Consolidate Into `runtara-connections`

- `crates/runtara-server/src/api/dto/connections.rs`
- `crates/runtara-server/src/api/dto/rate_limits.rs`
- `crates/runtara-server/src/api/handlers/connections.rs`
- `crates/runtara-server/src/api/handlers/oauth.rs`
- `crates/runtara-server/src/api/handlers/rate_limits.rs`
- `crates/runtara-server/src/api/repositories/connections.rs`
- `crates/runtara-server/src/api/services/connections.rs`
- `crates/runtara-server/src/api/services/oauth.rs`
- `crates/runtara-server/src/api/services/rate_limits.rs`
- `crates/runtara-server/src/api/services/proxy_auth.rs`
- optionally `crates/runtara-server/src/api/handlers/internal_proxy.rs`

### Code That Should Remain in `runtara-server` But Depend on the Crate

- `crates/runtara-server/src/api/services/file_storage.rs`
- `crates/runtara-server/src/api/services/object_model.rs`
- `crates/runtara-server/src/api/services/agent_testing.rs`
- `crates/runtara-server/src/api/services/agent_execution.rs`
- `crates/runtara-server/src/api/services/webhook_manager.rs`
- `crates/runtara-server/src/api/services/webhook_verification.rs`
- `crates/runtara-server/src/channels/*`
- scenario handlers/services that validate connection existence

## Risks

### Risk 1: Hidden Consumer Paths

There are multiple connection consumers outside the obvious CRUD handlers.

Mitigation:

- migrate consumers before deleting duplicated code
- use repo-wide search to ensure no direct SQL access remains

### Risk 2: OpenAPI Drift

Server OpenAPI derives currently reference server-local DTOs and handlers.

Mitigation:

- enable `utoipa` support in `runtara-connections`
- switch references incrementally during route migration

### Risk 3: Tenant Context Mismatch

`runtara-server` currently uses `middleware::tenant_auth::OrgId`, while `runtara-connections` expects `TenantId` in request extensions.

Mitigation:

- create a host adapter layer that inserts `runtara_connections::TenantId`
- keep auth ownership in the host

### Risk 4: Overloading the Crate

If file storage or channels are moved wholesale into `runtara-connections`, the crate boundary becomes muddy.

Mitigation:

- move only connection-domain logic
- keep feature-domain business logic in the host

## Acceptance Criteria

This refactor is complete when all of the following are true:

- `runtara-server` mounts routers from `runtara-connections` instead of local duplicates
- there is no server-local connection repository/service/handler implementation remaining
- no code outside `runtara-connections` reads `connection_parameters` directly from SQL
- auth proxy preparation is provided by `runtara-connections`
- file storage, object model, channel, and agent services use exported crate services
- the crate can be configured fully by host-provided state, not hidden env reads
- encryption can be added by changing only the crate implementation, not consumers

## Recommended Delivery Order

1. finalize the crate boundary and public API shape
2. make `runtara-connections` self-contained
3. move auth resolution logic into the crate
4. export a service facade
5. mount crate routers in `runtara-server`
6. migrate internal consumers
7. delete duplicated server implementation
8. add contract/integration tests
9. implement credential storage replacement and encryption

## Follow-Up After This Refactor

Once this plan is complete, `SYN-390` becomes much simpler:

- encryption is implemented once in `runtara-connections`
- runtime consumers do not change
- the auth proxy keeps working through the same service boundary
- storage can evolve from JSONB to encrypted JSONB to external secrets without changing downstream business logic
