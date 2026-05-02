---
name: add-migration
description: Use when adding a new SQL migration to the server or environment database — schema changes, new tables, indexes, extensions. Migrations are auto-applied by sqlx::migrate! at server startup; there is no manual migrate command.
---

# Add a database migration

Migrations are sqlx-managed `.sql` files, picked up at compile time via `sqlx::migrate!` and run at server startup.

## Which database?

Two separate migration directories — pick based on what you're changing:

- **Server tables** (workflows, instances, tenants, reports, vector store, ...) → [crates/runtara-server/migrations/](../../../crates/runtara-server/migrations/)
- **Environment tables** (image registry, runtime instance state) → [crates/runtara-environment/migrations/](../../../crates/runtara-environment/migrations/)

If unsure: changes touched by the server's HTTP handlers go in `runtara-server`; changes touched by the runtime/environment binary go in `runtara-environment`.

## Steps

### 1. Pick a timestamp

Use `YYYYMMDDHHMMSS` format. Pad to 14 digits, even if HHMMSS is `000000`. Examples from the repo:

- `20260430000000_workflow_metrics_daily_view.sql`
- `20260428000000_pgvector_extension.sql`

The timestamp must be **strictly greater** than the latest existing migration in that directory — sqlx applies them in lexical order.

```bash
# pick "now" as the timestamp
TS=$(date -u +%Y%m%d%H%M%S)
# or, if the latest migration is from the future, bump from that
LATEST=$(ls crates/runtara-server/migrations/ | tail -1 | cut -d_ -f1)
```

### 2. Write the SQL

```sql
-- crates/runtara-server/migrations/<TS>_<short_name>.sql

-- Forward-only migration (no down migration — sqlx::migrate! doesn't run them)
CREATE TABLE my_thing (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX my_thing_tenant_idx ON my_thing(tenant_id);
```

Conventions:

- Use `IF NOT EXISTS` for index/extension creation when reasonable, but **not** for `CREATE TABLE` of a new table — let it fail loudly if there's a name collision.
- Multi-tenant tables include `tenant_id TEXT NOT NULL` with an index.
- Timestamps as `TIMESTAMPTZ NOT NULL DEFAULT NOW()`.
- Use `gen_random_uuid()` (pgcrypto / Postgres 13+) for UUID PKs.

### 3. Run the server — migrations apply on startup

The runner is at [crates/runtara-server/src/main.rs:45](../../../crates/runtara-server/src/main.rs:45):

```rust
sqlx::migrate!("./migrations").set_ignore_missing(true).run(&pool).await
```

Just start the server. The migration runs once and is recorded in `_sqlx_migrations`.

To **bypass** migrations (rarely useful):

```bash
SKIP_MIGRATIONS=true cargo run -p runtara-server
```

### 4. Update sqlx query cache (if you use `sqlx::query!` macros)

If new code uses `sqlx::query!` / `query_as!` against the new schema, regenerate the offline cache so CI without a DB still compiles:

```bash
cargo sqlx prepare --workspace
```

(Skip if no compile-time-checked queries reference the new schema.)

### 5. Verify

Run `e2e-verify`. The test setup creates fresh `runtara_e2e_test` and `runtara_e2e_server` databases, so the migration runs in isolation. If it fails to apply cleanly there, it will fail in production.

If the migration breaks an existing e2e DB, drop and recreate:

```bash
psql "$DB_URL" -c "DROP DATABASE runtara_e2e_server; CREATE DATABASE runtara_e2e_server;"
```

## Gotchas

- **No down migrations.** If you need to roll back, write a follow-up forward migration that reverses the change.
- **No editing committed migrations.** sqlx records the migration's checksum; modifying a previously-applied file triggers `migration was previously applied but has been modified`. If you need to fix a bug in a fresh-but-merged migration, write a follow-up migration instead.
- **Server vs environment collision.** Both directories had a `20250101000000` migration historically — that's why e2e uses two separate DBs. Don't try to share schema between them.

## Files touched

- `crates/runtara-server/migrations/<TS>_<name>.sql` **or** `crates/runtara-environment/migrations/<TS>_<name>.sql` — new
- (optional) `.sqlx/` — regenerated query cache
