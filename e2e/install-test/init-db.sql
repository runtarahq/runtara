-- Raw SQL (sql/execute, MCP execute_sql) runs as the object-model role, so it
-- is a non-superuser that owns only runtara_objects and cannot connect to the
-- server or runtime databases.
CREATE ROLE runtara_objects LOGIN PASSWORD 'runtara_objects'
  NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
CREATE DATABASE runtara_server OWNER runtara;
CREATE DATABASE runtara_objects OWNER runtara_objects;
REVOKE CONNECT, TEMPORARY ON DATABASE runtara_server, runtara FROM PUBLIC;

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
ALTER SCHEMA public OWNER TO runtara_objects;
REVOKE ALL ON SCHEMA public FROM PUBLIC;
