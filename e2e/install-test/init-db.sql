CREATE DATABASE runtara_server OWNER runtara;
CREATE DATABASE runtara_objects OWNER runtara;

-- The server and object-model stores use pg_trgm / pgvector / fuzzystrmatch
-- for trigram, vector, and fuzzy-match schemas. The pgvector image ships these
-- extensions but does not install them per-database, and the runtime no longer
-- auto-creates them, so provision them up front on each application database.
\c runtara_server
CREATE EXTENSION IF NOT EXISTS "pg_trgm";
CREATE EXTENSION IF NOT EXISTS "vector";
CREATE EXTENSION IF NOT EXISTS "fuzzystrmatch";

\c runtara_objects
CREATE EXTENSION IF NOT EXISTS "pg_trgm";
CREATE EXTENSION IF NOT EXISTS "vector";
CREATE EXTENSION IF NOT EXISTS "fuzzystrmatch";
