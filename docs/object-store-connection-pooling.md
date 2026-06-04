# Object Store Connection Pooling & Caching

How the object model reaches customer PostgreSQL databases (typically our cloud →
the customer's Azure), why that path is slow today, and the plan to make it fast and
bounded. All changes are **server-side** and transparent to the `object_model` agent,
to reports, and to the databases view — none of their contracts change.

> Status: **proposal — not yet implemented.** Phasing is in
> [§11](#11-phasing); start with Phase 1.

## 1. Background — the problem

The object model stores schemas and instances in a customer-provided PostgreSQL
database. A request resolves a `connection_id` to a `database_url`, gets (or builds)
an `ObjectStore`, and runs SQL through its `PgPool`. Over a high-latency cross-cloud
link this is slow today for three reasons:

| # | Issue | Where | Effect |
|---|-------|-------|--------|
| 1 | Bare connect, zero pool tuning | `crates/runtara-object-store/src/store.rs:55` | Inherits stock sqlx defaults (see below) |
| 2 | Pool cache grows unbounded; check-then-insert race | `crates/runtara-server/src/api/repositories/object_model.rs:114` | Leaks a pool/socket per distinct URL forever; concurrent first-hits build duplicate pools; credential rotation orphans old pools |
| 3 | Connection metadata re-resolved every request | `crates/runtara-server/src/api/services/object_model.rs:126` → `crates/runtara-connections/src/repository/connections.rs:678` | An internal DB query **plus** an `unseal()` decrypt on **every** call, even when the pool is already cached |

`ObjectStore::new` calls `PgPool::connect(&url)` with **no** `PgPoolOptions`, so each
customer pool runs on sqlx 0.8.6 defaults (verified in
`sqlx-core-0.8.6/src/pool/options.rs`):

| Default | Value | Why it hurts a cross-cloud link |
|---------|-------|---------------------------------|
| `min_connections` | **0** | No warm spare. After idle, the pool drains to zero and the next request pays a full cross-cloud TCP + TLS + auth handshake (~3–5 RTTs; Azure forces SSL). |
| `idle_timeout` | **10 min** | Warm connections torn down after 10 min idle → cold reconnect on the next call. |
| `max_lifetime` | **30 min** | Even a busy connection is recycled every 30 min, re-paying the handshake. |
| `test_before_acquire` | **true** | A liveness **ping round-trip before every checkout** → an extra RTT on *every* query, even when warm. |
| `max_connections` | **10** | Usually fine; relevant only under burst concurrency. |

The two that dominate perceived latency: `test_before_acquire=true` (a per-query RTT
tax) and `min_connections=0` + idle/lifetime recycling (periodic full TLS reconnects).

`StoreConfig` (`crates/runtara-object-store/src/config.rs`) exposes no pool knobs at
all, so today there is no way to tune any of this.

## 2. Scope — who is affected

Every customer-facing PostgreSQL path funnels through `ObjectStore::new`. A grep of
the whole server confirms it is the **only** customer-DB connect site; every other
`PgPool` is our internal server DB or our own object-model DB. So all three consumers
ride the same pool and caches and improve together:

- **The `object_model` agent** (`crates/agents/runtara-agent-object-model`) — a thin
  HTTP wrapper over `RUNTARA_OBJECT_MODEL_URL` with no database dependency. Fully
  transparent: nothing in the agent or its WIT changes.
- **Reports** — `ReportService` holds the `Arc<ObjectStoreManager>`
  (`crates/runtara-server/src/api/services/reports.rs:136`) and runs its SQL through
  it. Each report block / dataset / join carries its own `connection_id`
  (`reports.rs:154`, defaulted at `reports.rs:353`), and `aggregate_instances_by_schema`
  + `get_schema_by_name` are called **per block/join** (`reports.rs:981`, `:1106`,
  `:1358`), aggregates capped at 1000 rows (`reports.rs:41`). A single render is an
  **N+1 cross-cloud fan-out**, so reports are the heaviest beneficiary.
- **The "databases" view** — the Objects feature at `/objects/types`, gated by
  `EntitlementRoute feature="database"` (`crates/runtara-server/frontend/src/router/index.tsx:319`).
  Picking a database selects a `connectionId`; `useObjectSchemaDtos` →
  `getAllSchemas(token, connectionId)`
  (`crates/runtara-server/frontend/src/features/objects/hooks/useObjectSchemas.ts:29`)
  calls `list_schemas` keyed by `connection_id`. The schemas table reads field counts
  from the DTO (no per-row fan-out), so a database load is a single `list_schemas`
  query; "Manage instances" issues `instances/query`; report authoring fans
  `list_schemas` across several connections at once (`useObjectSchemas.ts:54`).

Because nothing bypasses `ObjectStore::new`, no slow SQL path is left outside this
plan's reach.

## 3. Design overview — three cooperating caches

```
request (tenant_id, connection_id?)
        │
        ▼
[L3] connection-metadata cache  ── miss ──▶ ConnectionsFacade.get_with_parameters (DB + decrypt)
        │ hit: {integration_id, database_url}        TTL ~60s
        ▼
   url_fingerprint = hash(database_url)
        │
        ▼
[L2] pool/store cache  ── miss ──▶ ObjectStore::connect(StoreConfig + PoolConfig)   ◀── single-flight
        │ key = (tenant_id, connection_id, url_fingerprint)
        │ idle-TTL evicts unused pools, LRU caps total
        ▼
[L1] tuned PgPool (per customer DB)  ── acquire ──▶ warm connection (no ping), query Azure
```

Each layer is independently shippable and testable.

## 4. Layer 1 — Per-database pool tuning

Replace the bare connect with an explicit options path. `PgConnectOptions` implements
`FromStr`, so we parse the URL then layer settings on top:

```rust
// crates/runtara-object-store/src/store.rs
let mut connect_opts: PgConnectOptions = config.database_url.parse()
    .map_err(|e| ObjectStoreError::Connection(format!("bad database_url: {e}")))?;
connect_opts = connect_opts
    .application_name("runtara-object-model")              // visible to customer DBAs
    .statement_cache_capacity(pool.statement_cache_capacity);
// ssl_mode is left to the URL (Azure URLs carry sslmode=require); an optional env
// can force a minimum mode if we ever need to.

let pool = PgPoolOptions::new()
    .max_connections(pool.max_connections)
    .min_connections(pool.min_connections)
    .acquire_timeout(pool.acquire_timeout)
    .idle_timeout(pool.idle_timeout)
    .max_lifetime(pool.max_lifetime)
    .test_before_acquire(pool.test_before_acquire)
    .connect_with(connect_opts)
    .await?;
```

The two changes that fix the reported slowness:

- **`test_before_acquire = false`** — removes the per-checkout liveness ping. On a WAN
  link that ping roughly doubles the round-trips of a small op. sqlx 0.8 exposes **no
  TCP-keepalive knob**, so dead-connection safety comes from keeping `idle_timeout` and
  `max_lifetime` **below** Azure's server-side / PgBouncer idle cutoff, so we recycle
  before the server kills a socket. Residual risk (a connection killed server-side
  mid-idle is handed out and fails on first use) is bounded by that, and the knob is
  env-toggleable to flip back to `true` instantly if stale-connection errors appear. A
  custom `before_acquire` hook is the middle-ground option if ever needed.
- **`min_connections = 1`** — keeps a warm connection so the hot path never pays a cold
  cross-cloud handshake. Total warm sockets are bounded by L2's cache cap.

`StoreConfig` gains a nested `PoolConfig` with builder methods and a `Default` holding
these values; the object-store crate owns the defaults and the server overrides from
env. `ObjectStore::from_pool` is unchanged.

## 5. Layer 2 — Pool/store cache & lifecycle

Rework `ObjectStoreManager` so the cache is keyed by stable identity, bounded,
self-evicting, and race-free.

- **Key:** `StoreKey::Connection { tenant_id, connection_id, url_hash }` for customer
  DBs; `StoreKey::Default` for the env default (never evicted). Folding `url_hash` into
  the key makes **credential rotation free** — a rotated URL yields a new key/pool and
  the old one idles out. No raw credentials sit in map keys anymore (today the key is
  the full URL string).
- **Single-flight:** N concurrent first-hits for the same key build **one** pool, not
  N. This fixes the current check-then-insert race.
- **Eviction:** idle-TTL (evict pools unused for ~15 min, closing their `PgPool` when
  the last `Arc` drops) + LRU cap on total cached pools. This bounds file descriptors
  and Azure-side connection slots — the current map never shrinks.
- **Negative cache (fast-fail a dead DB):** a successful build is cached; a *failed*
  build (unreachable / down customer DB) is remembered in a separate short-TTL cache
  (~5s) so subsequent requests fast-fail instead of re-attempting a slow cross-cloud
  connect on every call. The entry expires automatically, so one request then retries
  and recovery is hands-off. Only the build/first-connect path is covered; a store that
  built fine and later breaks stays cached and is bounded per-request by `acquire_timeout`
  (sqlx keeps the pool rather than rebuilding).

**Implementation choice (see [§12](#12-open-decisions)):** use the `moka` async cache
(`moka::future::Cache` gives `time_to_idle`, `max_capacity`, and `get_with`
single-flight out of the box) **or** hand-roll it (`RwLock<HashMap>` + per-key build
`Mutex` + a reaper task, no new dependency). Recommendation: `moka`; it removes the
most error-prone concurrency code. In-flight requests hold an `Arc<ObjectStore>`, so
eviction never closes a pool mid-query.

## 6. Layer 3 — Connection-metadata cache

Today `resolve_database_url` hits the internal DB and decrypts params on every request.
Add a short-TTL cache keyed by `(tenant_id, connection_id)` (and the default-connection
lookup) returning `{ integration_id, database_url }`:

- TTL ~60s (write-based), so rotation/deletion is picked up within a minute without any
  explicit invalidation.
- Collapses request bursts to one DB round-trip + one decrypt per minute per connection.
- Produces the `url_hash` used as L2's key component, so L2 and L3 share one resolve
  step.
- Optional later nicety: explicit `invalidate` hook on connection update/delete for
  instant propagation.

## 7. Configuration

New knobs mirror the existing `OBJECT_MODEL_*` style in
`crates/runtara-server/src/config.rs` (struct field → `parse_*_or` → module getter),
all with safe defaults so we can ship dark and tune by env:

| Knob | Env var | Default | Why |
|------|---------|---------|-----|
| max connections / DB | `OBJECT_MODEL_POOL_MAX_CONNECTIONS` | 10 | matches today's default |
| min connections / DB | `OBJECT_MODEL_POOL_MIN_CONNECTIONS` | 1 | keep 1 warm → no cold reconnect |
| acquire timeout | `OBJECT_MODEL_POOL_ACQUIRE_TIMEOUT_SECS` | 10 | fail fast |
| idle timeout | `OBJECT_MODEL_POOL_IDLE_TIMEOUT_SECS` | 300 | recycle below Azure/PgBouncer cutoff |
| max lifetime | `OBJECT_MODEL_POOL_MAX_LIFETIME_SECS` | 1800 | bound staleness |
| test before acquire | `OBJECT_MODEL_POOL_TEST_BEFORE_ACQUIRE` | false | drop per-query ping RTT |
| stmt cache / conn | `OBJECT_MODEL_POOL_STMT_CACHE` | 100 | sqlx default; helps repeated queries |
| pool idle TTL | `OBJECT_MODEL_POOL_CACHE_TTL_SECS` | 900 | evict unused customer pools |
| pool cache max | `OBJECT_MODEL_POOL_CACHE_MAX` | 256 | LRU bound on warm sockets |
| metadata TTL | `OBJECT_MODEL_CONN_META_TTL_SECS` | 60 | collapse per-request DB lookups |

The existing `OBJECT_MODEL_MAX_CONNECTIONS` (default pool, `config.rs:115`) stays.

## 8. Code changes by file

- `crates/runtara-object-store/src/config.rs` — add `PoolConfig` struct + builder
  methods on `StoreConfig`; `Default` holds the §7 values.
- `crates/runtara-object-store/src/store.rs:54` — `ObjectStore::new` builds via
  `PgPoolOptions::connect_with(PgConnectOptions)` (§4). `from_pool` unchanged.
- `crates/runtara-server/src/config.rs` — new env vars + getters (mirror
  `object_model_max_connections` at lines 115 / 477).
- `crates/runtara-server/src/api/repositories/object_model.rs` — rework
  `ObjectStoreManager` cache (§5): new keying, single-flight, eviction.
- `crates/runtara-server/src/api/services/object_model.rs:120` — metadata cache in the
  resolve path (§6).
- `crates/runtara-server/src/server.rs:932` — pass `PoolConfig` from env into the
  manager; extend the pool-pressure monitor at `server.rs:903` to iterate cached pools.
  Also **unify the default object-model pool under the same `PoolConfig`** — today it
  sets only `max_connections` and still inherits `test_before_acquire=true` /
  `min_connections=0`. It is in our cloud (low latency, so the ping is cheap), but
  unifying keeps reports that fall back to the default connection on warm pools too.
- Cargo manifests — add `moka` if chosen (§5).

## 9. Observability

- Extend the existing pool monitor (already ticks every 5s at `server.rs:903`) to
  report aggregate across cached pools: pool count, total/idle/active connections, and
  a per-pool warn at ≥75% usage.
- `tracing` fields for: cold pool build (with `elapsed_ms`), L2 store cache hit/miss,
  L3 metadata cache hit/miss — so "is it warm?" is answerable from logs.
- Optional: Prometheus gauges (acquire latency, pool count, cache hit ratio) if a
  metrics stack is wired.

## 10. Testing & verification

- **Unit (object-store):** `StoreConfig` → `PgPoolOptions` mapping; pool builds with
  options applied.
- **Unit (manager):** keying; single-flight (spawn N concurrent `get`, assert exactly
  one build); idle-TTL eviction; rotation (changed URL → new pool, old reaped);
  metadata cache hit/miss.
- **e2e (`e2e-verify` skill):** boot the full stack against a Postgres connection, run
  object-model CRUD, and assert behavior is unchanged while logs show **warm reuse**
  (second op: no cold-connect, no per-acquire ping) and metadata-cache hits.
- **Consumer coverage:** add a **report-render** latency check (first vs. warm) and a
  **databases-view** schema-list check, since those are the best demonstrations of the
  win — reports for the N+1 fan-out, the databases view for repeated single-query loads
  and the multi-connection authoring browse (which exercises L2 single-flight).

## 11. Phasing

1. **L1 pool tuning** — biggest latency win, smallest blast radius (object-store crate
   + env wiring). Fixes the ping RTT and cold-start. Keep the existing URL-keyed cache
   for now.
2. **L2 cache rework** — bounds resources, fixes the race and rotation handling.
3. **L3 metadata cache** — removes the per-request internal DB hit + decrypt.
4. **Observability polish** + optional per-connection param overrides.

Each phase lands independently with its own `e2e-verify`.

## 12. Open decisions

1. **`min_connections`:** `1` (warm, kills cold reconnect — *recommended*) vs `0`
   (fewer idle sockets if there are many rarely-used customer DBs).
2. **`test_before_acquire`:** `false` (fast, *recommended*, with idle/lifetime guard +
   env revert) vs `true` (safe, keeps the RTT tax).
3. **Cache impl:** add `moka` (*recommended*, removes hand-rolled concurrency) vs
   no-new-dep hand-rolled cache + reaper.
4. **Per-connection overrides** (e.g. `pool_max_connections` in connection params):
   Phase 1 vs defer to Phase 4 (*recommended defer*).

## 13. Backwards compatibility

The `object_model` agent and the internal API contract are unchanged → fully
transparent. All knobs have safe defaults; `test_before_acquire` is env-toggleable for
instant revert. The default (`__default__`) store path keeps working; unifying its
config (§8) is additive. Per repo convention this lands directly on `main`, in phases.
