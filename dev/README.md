# Local dev stack

Self-contained Docker Compose for local development and the e2e suite. Replaces the previous dependency on `smo-runtime`.

## Up / down

```sh
docker compose -f dev/docker-compose.yml up -d
docker compose -f dev/docker-compose.yml down          # keep volumes
docker compose -f dev/docker-compose.yml down -v       # also wipe DB
```

## What it runs

- `runtara-dev-postgres` (`pgvector/pgvector:pg18`) on `localhost:5432`. Pre-installs `pg_trgm`, `vector`, and `fuzzystrmatch` on `smo_container`, `smo_object_model`, and `runtara`.
- `runtara-dev-valkey` (`valkey/valkey:8-alpine`) on `localhost:6379`.

Credentials match what the e2e scripts already expect: user `smo_worker`, password `GueUkDKea0CjKP4Rn5Bk0FDV`. Override via env if you need to: `POSTGRES_HOST`, `POSTGRES_PORT`, `POSTGRES_USER`, `POSTGRES_PASSWORD`, `VALKEY_HOST`, `VALKEY_PORT`.

## If you used `smo-runtime` before

Stop the old containers first — same ports:

```sh
docker stop smo-dev-postgres smo-dev-valkey
docker compose -f dev/docker-compose.yml up -d
```

The data volumes are separate, so DB state from `smo-runtime` won't carry over.
