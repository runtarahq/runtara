-- What the engine has approved to run, kept here because this database is
-- the engine's alone: the API's process is given no way to it. A workflow's
-- definition lives in the server database, which the API writes; what is
-- written there runs only as far as it agrees with these rows.

-- A version of a workflow someone with the authority to run it asked to have
-- compiled or run, as the definition was then (`source_checksum`,
-- `track_events`), and the image the engine compiled from it. A definition
-- that no longer matches is not run until it is approved again.
CREATE TABLE workflow_approvals (
    tenant_id       TEXT        NOT NULL,
    workflow_id     TEXT        NOT NULL,
    version         INTEGER     NOT NULL,
    source_checksum TEXT        NOT NULL,
    track_events    BOOLEAN     NOT NULL,
    image_id        TEXT,
    approved_by     TEXT        NOT NULL,
    approved_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, workflow_id, version)
);

-- The version of a workflow that runs when a run names none, where one was
-- chosen. A workflow with no row runs its latest version.
CREATE TABLE workflow_current_versions (
    tenant_id   TEXT        NOT NULL,
    workflow_id TEXT        NOT NULL,
    version     INTEGER     NOT NULL,
    set_by      TEXT        NOT NULL,
    set_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, workflow_id)
);

-- What was carried over once from before these tables existed.
CREATE TABLE engine_carried_over (
    name TEXT        PRIMARY KEY,
    at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE workflow_approvals ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_approvals FORCE ROW LEVEL SECURITY;
CREATE POLICY runtara_tenant ON workflow_approvals
    USING (tenant_id = current_setting('runtara.tenant_id', true) OR (SELECT runtara_all_tenants()))
    WITH CHECK (tenant_id = current_setting('runtara.tenant_id', true) OR (SELECT runtara_all_tenants()));

ALTER TABLE workflow_current_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE workflow_current_versions FORCE ROW LEVEL SECURITY;
CREATE POLICY runtara_tenant ON workflow_current_versions
    USING (tenant_id = current_setting('runtara.tenant_id', true) OR (SELECT runtara_all_tenants()))
    WITH CHECK (tenant_id = current_setting('runtara.tenant_id', true) OR (SELECT runtara_all_tenants()));
