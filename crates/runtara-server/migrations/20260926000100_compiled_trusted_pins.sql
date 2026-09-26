-- Record the approved trusted built-in versions each compiled artifact pins.
--
-- A workflow that uses a trusted built-in (S3 or Azure presigning) imports a
-- content-bound `runtara:trusted-artifacts/<agent>-h<wasm>-h<meta>@0.1.0` pin.
-- After an operator upgrades that built-in, an artifact compiled against the
-- old version can no longer run its trusted calls. Readiness therefore
-- requires every recorded pin to be installed on this server, so an upgrade
-- makes the affected workflows recompile. An empty array is an artifact with
-- no trusted dependency. A failed row carries pins only when the compile
-- produced versions this server does not run; that failure stays terminal
-- until they are all installed.
-- Keep this nullable: rows written before this column existed carry unknown
-- pins, must not claim that every pin is installed, and recompile once.

ALTER TABLE workflow_compilations
    ADD COLUMN IF NOT EXISTS trusted_pins TEXT[];
