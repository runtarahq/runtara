-- What the API role's database role may do in the server database, for a
-- server run as two processes (RUNTARA_ROLE, docs/roles.md).
--
-- The API keeps workflows' definitions and its own records. It is given
-- nothing of what starts or admits a run, nothing of what a compile
-- recorded but to read it, no connection, no key, and it may not work
-- across tenants: only members of `runtara_system` may, and it is not one.
--
-- Run as an administrator, on the server database, once the engine has run
-- the migrations, and again after an upgrade that adds a table:
--
--   psql -d <server database> -v api_role=<role> -v engine_role=<role> \
--        -f scripts/roles/api-database-role.sql
--
-- Both roles exist already, as plain login roles (NOSUPERUSER NOBYPASSRLS).
-- The engine's owns the tables. The API starts with SKIP_MIGRATIONS=true.

\set ON_ERROR_STOP on

-- Who may work across tenants. Until this role exists any role may; from
-- then on only its members.
DO $$
BEGIN
    IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'runtara_system') THEN
        CREATE ROLE runtara_system NOLOGIN;
    END IF;
END
$$;
GRANT runtara_system TO :"engine_role";
REVOKE runtara_system FROM :"api_role";

REVOKE ALL ON ALL TABLES IN SCHEMA public FROM :"api_role";
REVOKE ALL ON ALL SEQUENCES IN SCHEMA public FROM :"api_role";
GRANT USAGE ON SCHEMA public TO :"api_role";

-- Its own records: definitions, saved views, API keys, and the audit trail
-- it adds to.
GRANT SELECT, INSERT, UPDATE, DELETE
    ON workflows, workflow_definitions, operations_views, api_keys
    TO :"api_role";
GRANT SELECT, INSERT ON audit_events TO :"api_role";

-- A deleted workflow's triggers are switched off with it. Triggers are
-- otherwise the engine's, which serves them.
GRANT SELECT, UPDATE ON invocation_trigger TO :"api_role";

-- What the engine recorded, to show it.
GRANT SELECT
    ON workflow_compilations, workflow_dependencies,
       workflow_metrics_hourly, workflow_metrics_daily
    TO :"api_role";

GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO :"api_role";

-- Nothing is granted on: execution_requests, execution_outbox,
-- execution_admission_reservations, execution_admission_tenants,
-- connection_data_entity, connection_defaults, oauth_state,
-- rate_limit_events, platform_oauth_app, seal_wrapped_key, object_schema,
-- report_definitions, metadata, conversations, conversation_messages,
-- conversation_events. Nothing is granted in the runtime database,
-- which the API is given no address of.
