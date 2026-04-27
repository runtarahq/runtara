-- Enable pgvector + fuzzystrmatch (Tier 3 SMO object model).
-- pgvector backs the `vector(N)` column type and the COSINE_DISTANCE /
-- L2_DISTANCE / INNER_PRODUCT ExprFns; fuzzystrmatch backs LEVENSHTEIN.
-- Idempotent: safe to apply on databases that already have the extensions.
-- Privileges: fuzzystrmatch is in the trusted-extension list on AWS RDS, GCP
-- Cloud SQL, and Azure. pgvector availability varies — RDS/Cloud SQL/Supabase
-- support it on recent versions, but bare-metal installs may need the
-- `postgresql-NN-pgvector` package and a superuser to install it the first
-- time. After install, CREATE EXTENSION succeeds for non-superusers if
-- granted CREATE on the database.
CREATE EXTENSION IF NOT EXISTS "vector";
CREATE EXTENSION IF NOT EXISTS "fuzzystrmatch";
