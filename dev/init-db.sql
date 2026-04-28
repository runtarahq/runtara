-- Local-dev DB bootstrap. Runs once on first container start (via the
-- `docker-entrypoint-initdb.d` mount). Creates the databases that
-- runtara-server / start.sh / e2e tests expect, and pre-installs the
-- Postgres extensions Tier 1–3 require so first-run migration succeeds.

-- Object-model database (per-tenant data lives here in single-tenant dev).
CREATE DATABASE smo_object_model;
GRANT ALL PRIVILEGES ON DATABASE smo_object_model TO smo_worker;

-- Embedded runtara environment server (workflow metadata).
CREATE DATABASE runtara;
GRANT ALL PRIVILEGES ON DATABASE runtara TO smo_worker;

\c smo_container
GRANT ALL ON SCHEMA public TO smo_worker;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO smo_worker;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON SEQUENCES TO smo_worker;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON FUNCTIONS TO smo_worker;
CREATE EXTENSION IF NOT EXISTS "pg_trgm";
CREATE EXTENSION IF NOT EXISTS "vector";
CREATE EXTENSION IF NOT EXISTS "fuzzystrmatch";

\c smo_object_model
GRANT ALL ON SCHEMA public TO smo_worker;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO smo_worker;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON SEQUENCES TO smo_worker;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON FUNCTIONS TO smo_worker;
CREATE EXTENSION IF NOT EXISTS "pg_trgm";
CREATE EXTENSION IF NOT EXISTS "vector";
CREATE EXTENSION IF NOT EXISTS "fuzzystrmatch";

\c runtara
GRANT ALL ON SCHEMA public TO smo_worker;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON TABLES TO smo_worker;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON SEQUENCES TO smo_worker;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT ALL ON FUNCTIONS TO smo_worker;
CREATE EXTENSION IF NOT EXISTS "pg_trgm";
CREATE EXTENSION IF NOT EXISTS "vector";
CREATE EXTENSION IF NOT EXISTS "fuzzystrmatch";
