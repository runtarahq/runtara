-- The runs waiting in process, as the engine that runs each records them
-- (docs/multiple-engines-plan.md, E7): every engine shows a run as
-- `waiting_in_process` from here, not only the one that runs it. A row's
-- owner is an engine in engine_members; a row whose owner is no longer a
-- live member is no run's wait, and is ignored and removed.
CREATE TABLE in_process_waits (
    instance_id TEXT        PRIMARY KEY,
    owner       TEXT        NOT NULL,
    since       TIMESTAMPTZ NOT NULL DEFAULT now()
);
