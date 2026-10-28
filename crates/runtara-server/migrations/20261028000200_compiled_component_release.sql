-- Record the component release each compilation was composed against.
--
-- A workflow artifact composes the agents and the workflow stdlib of the
-- release the server compiles by (the index digest under
-- `<DATA_DIR>/releases/`). New agent or stdlib bytes are a new release and a
-- different artifact, even when the template, lowering mode and definition are
-- unchanged, so a success or a failure recorded against another release no
-- longer describes this server's compile. NULL is a compile without a release
-- (a raw components directory) or a row written before this column existed;
-- under a release it misses, and the workflow recompiles once.

ALTER TABLE workflow_compilations
    ADD COLUMN IF NOT EXISTS component_release TEXT;
