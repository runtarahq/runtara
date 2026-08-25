-- Drop columns left behind by the OCI and native runners (deleted in 4ba33fcb).
--
-- Every column below is provably unread and holds only its default:
--
--   images.bundle_path      -- OCI bundle dir; written as NULL since the OCI
--                              runner went away, and nothing reads it.
--   images.runner_type      -- selected between OCI/native/wasm backends. Only
--                              the embedded wasm runner exists, so every row
--                              carries a value no code branches on.
--   container_registry.bundle_path
--                           -- same OCI bundle dir, always NULL.
--   container_registry.pid  -- the embedded runner is in-process and never
--                              reports a PID, so this is always NULL. Its only
--                              readers (a /proc liveness probe and a SIGKILL
--                              path) were unreachable and are gone.
--   container_registry.process_killed
--                           -- only ever read by get_unkilled_containers, which
--                              filtered on `pid IS NOT NULL` and so always
--                              returned nothing.
--
-- Earlier migrations are deliberately left in place; sqlx::migrate! validates
-- applied migrations by checksum, so editing them would break boot.

ALTER TABLE images
    DROP COLUMN IF EXISTS bundle_path,
    DROP COLUMN IF EXISTS runner_type;

ALTER TABLE container_registry
    DROP COLUMN IF EXISTS bundle_path,
    DROP COLUMN IF EXISTS pid,
    DROP COLUMN IF EXISTS process_killed;
