-- The engines sharing this database (docs/multiple-engines-plan.md, E1).
-- Each engine writes its row when it starts, renews `last_seen` while it
-- runs and removes the row when it stops. A row not renewed within its
-- lease is an engine that died, and any engine may remove it.
--
-- It is no tenant's: it names processes, not anyone's work.
CREATE TABLE engine_members (
    id                   TEXT        PRIMARY KEY,
    release              TEXT        NOT NULL,
    run_channel_endpoint TEXT,
    started_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen            TIMESTAMPTZ NOT NULL DEFAULT now()
);
