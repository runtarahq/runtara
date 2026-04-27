-- Enable pgvector + fuzzystrmatch (Tier 3 SMO object model).
-- pgvector backs the `vector(N)` column type and the COSINE_DISTANCE /
-- L2_DISTANCE / INNER_PRODUCT ExprFns; fuzzystrmatch backs LEVENSHTEIN.
--
-- Both `CREATE EXTENSION` calls are wrapped in DO blocks that swallow the
-- failure and emit a WARNING. This keeps the migration idempotent across
-- environments where the extension package isn't installed yet (some
-- managed Postgres providers ship pgvector only on recent versions; bare-
-- metal installs need the `postgresql-NN-pgvector` package). Runtime soft-
-- fails in `ObjectStore::ensure_extensions` mirror this for per-tenant DBs
-- bootstrapped via `from_pool`. Privileges: fuzzystrmatch is in the trusted-
-- extension list on AWS RDS / Cloud SQL / Azure; pgvector availability
-- varies by provider.
DO $$
BEGIN
    CREATE EXTENSION IF NOT EXISTS "vector";
EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'pgvector extension is not available (%); vector columns / COSINE_DISTANCE / L2_DISTANCE / INNER_PRODUCT will not work until it is installed', SQLERRM;
END
$$;

DO $$
BEGIN
    CREATE EXTENSION IF NOT EXISTS "fuzzystrmatch";
EXCEPTION WHEN OTHERS THEN
    RAISE WARNING 'fuzzystrmatch extension is not available (%); LEVENSHTEIN will not work until it is installed', SQLERRM;
END
$$;
