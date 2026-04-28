-- Enable pgvector + fuzzystrmatch (Tier 3 SMO object model).
-- pgvector backs the `vector(N)` column type and the COSINE_DISTANCE /
-- L2_DISTANCE / INNER_PRODUCT ExprFns; fuzzystrmatch backs LEVENSHTEIN.
-- Both are required — provisioning must use a Postgres image that ships
-- pgvector (e.g. `pgvector/pgvector:pg16+`). The migration fails hard if
-- either extension is unavailable so the runtime never silently degrades.
CREATE EXTENSION IF NOT EXISTS "vector";
CREATE EXTENSION IF NOT EXISTS "fuzzystrmatch";
