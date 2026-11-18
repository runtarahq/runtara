-- What the intake role's database role may do in the server database, for a
-- server whose intake is a process of its own (RUNTARA_ROLE=intake,
-- docs/roles.md, docs/splitting-the-engine.md).
--
-- Intake receives what anyone may send: it looks up the trigger a webhook
-- names, and queues the run it asks for. It is given the triggers to read,
-- the tables a queued run is written to, and nothing else: no connection,
-- no workflow's definition, no key, and it may not work across tenants.
--
-- Run as an administrator, on the server database, once the engine has run
-- the migrations, and again after an upgrade that adds a table:
--
--   psql -d <server database> -v intake_role=<role> \
--        -f scripts/roles/intake-database-role.sql
--
-- The role exists already, as a plain login role (NOSUPERUSER NOBYPASSRLS).
-- Intake starts with SKIP_MIGRATIONS=true. `runtara_system` is made by
-- api-database-role.sql; intake is not a member of it.

\set ON_ERROR_STOP on

DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'runtara_system') THEN
        CREATE ROLE runtara_system NOLOGIN;
    END IF;
END
$$;
REVOKE runtara_system FROM :"intake_role";

REVOKE ALL ON ALL TABLES IN SCHEMA public FROM :"intake_role";
REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM :"intake_role";
GRANT USAGE ON SCHEMA public TO :"intake_role";

-- The trigger a webhook names, with what checks it.
GRANT SELECT ON invocation_trigger TO :"intake_role";

-- Whose a workflow is, for the synchronous trigger, which names one.
GRANT SELECT (tenant_id, workflow_id) ON workflows TO :"intake_role";

-- A queued run: the request, its place in the outbox, and its admission.
GRANT SELECT, INSERT ON execution_requests TO :"intake_role";
GRANT SELECT, INSERT ON execution_outbox TO :"intake_role";
GRANT SELECT, INSERT ON execution_admission_reservations TO :"intake_role";
-- The admission count, where it is kept here and not in Valkey.
GRANT SELECT, INSERT, UPDATE ON execution_admission_tenants TO :"intake_role";

GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO :"intake_role";

-- Nothing is granted on: workflow_definitions, workflow_compilations,
-- connection_data_entity, connection_defaults, oauth_state,
-- platform_oauth_app, seal_wrapped_key, api_keys, audit_events,
-- or anything in the runtime database.
