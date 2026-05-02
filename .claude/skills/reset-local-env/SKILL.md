---
name: reset-local-env
description: Use when local dev state has drifted into a weird shape — failing migrations, half-applied schema changes, stale cached binaries, suspended instances clogging the system. Wipes the e2e DBs and data dir and gives you a clean slate. Distinct from e2e-verify which assumes a clean env.
---

# Reset the local e2e environment

For when "the migrations look applied but a fresh column is missing" or "I have 200 suspended instances from yesterday's SIGTERM tests" or "wasm runner cache is acting weird".

This skill is destructive — it deletes the e2e databases and data dir. It does **not** touch your prod / dev DBs unless you point it at them.

## Pre-flight

Confirm what you're about to wipe:

```bash
DB_URL="postgres://user:pass@localhost:5432/postgres"
psql "$DB_URL" -c "SELECT datname FROM pg_database WHERE datname LIKE 'runtara_e2e%';"
ls -la /tmp/runtara_e2e_data 2>/dev/null
```

If those names don't match what your local server uses, **stop** and adjust the env variables below before running.

## Reset

### 1. Stop the server

```bash
pkill -x runtara-server
# wait for it to exit cleanly
while pgrep -x runtara-server > /dev/null; do sleep 0.5; done
```

### 2. Drop and recreate the DBs

The two databases must be reset together — the server and environment migrations are versioned independently and a partial reset leaves them out of sync (manifests as `migration was previously applied but is missing`).

```bash
DB_URL="postgres://user:pass@localhost:5432/postgres"

psql "$DB_URL" -c "DROP DATABASE IF EXISTS runtara_e2e_test;"
psql "$DB_URL" -c "DROP DATABASE IF EXISTS runtara_e2e_server;"
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_test;"
psql "$DB_URL" -c "CREATE DATABASE runtara_e2e_server;"
```

### 3. Wipe the data dir

```bash
rm -rf /tmp/runtara_e2e_data
mkdir -p /tmp/runtara_e2e_data
```

### 4. (Optional) wipe the WASM build cache

If you suspect stale compiled stdlib (rare — usually only after a stdlib bump):

```bash
rm -rf target/native_cache_wasm target/wasm32-wasip2
```

Then rebuild from `e2e-verify` step 1.

### 5. Restart the server

The server runs migrations on startup, so a fresh DB will be populated automatically. Use the env block from `e2e-verify` step 4.

```bash
RUST_LOG="sqlx=info,runtara_server=info" \
DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_server" \
OBJECT_MODEL_DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_server" \
RUNTARA_DATABASE_URL="postgres://user:pass@localhost:5432/runtara_e2e_test" \
RUNTARA_ENVIRONMENT_ADDR="127.0.0.1:18002" \
DATA_DIR="/tmp/runtara_e2e_data" \
SERVER_PORT=17001 \
INTERNAL_PORT=17002 \
  target/debug/runtara-server &

curl --retry 20 --retry-delay 1 --retry-connrefused http://127.0.0.1:8004/api/v1/health
```

`sqlx=info` confirms migrations applied cleanly. If a migration fails, fix it (see `add-migration` skill) before continuing — running `e2e-verify` against a half-migrated DB wastes time.

## When NOT to reset

- **You just need to clear suspended instances** — usually a connection or workflow update fixes the underlying issue without nuking everything. Use `trace-instance` to diagnose first.
- **You want to clear images** — delete via `/api/v1/images/{id}` instead of dropping the DB.
- **You want to test a migration** — yes, but reset only **one** DB (the one with the migration), and only after committing your migration file. Resetting both wipes more state than needed.

## After reset

Workflows, instances, connections, and registered images are all gone. Re-register what you need via `e2e-verify` step 5–6 or the UI.
