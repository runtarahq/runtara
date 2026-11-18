-- Row-level security that fails closed, on the server database, as the
-- runtime database's (store migration 056). Until now a session that named
-- no tenant was the system path and saw every tenant's rows, so work that
-- forgot its scope read across tenants without a sign. Reaching every tenant
-- is now asked for by name, and a session that asks for nothing is refused.

-- Whether this session may reach every tenant's rows. It may when it asked to
-- (`runtara.all_tenants`, set on the connections of work in the platform's own
-- scope) and its role is allowed to: any role, until a role named
-- `runtara_system` exists, and from then on only that role's members. A
-- session that names a tenant instead (`runtara.tenant_id`) may not, and
-- reaches that tenant's rows only. One that names neither is refused: a path
-- that forgot its scope fails where it runs instead of reading across tenants.
CREATE OR REPLACE FUNCTION runtara_all_tenants() RETURNS BOOLEAN
LANGUAGE plpgsql STABLE AS $$
BEGIN
    IF current_setting('runtara.all_tenants', true) = 'on' THEN
        IF coalesce(pg_has_role(current_user, to_regrole('runtara_system'), 'member'), true) THEN
            RETURN true;
        END IF;
        RAISE EXCEPTION 'runtara: role % may not work across tenants', current_user
            USING ERRCODE = 'insufficient_privilege';
    END IF;
    IF coalesce(current_setting('runtara.tenant_id', true), '') <> '' THEN
        RETURN false;
    END IF;
    RAISE EXCEPTION 'runtara: no tenant scope'
        USING ERRCODE = 'insufficient_privilege',
              HINT = 'Work on a tenant''s rows names the tenant, and work across tenants says so.';
END
$$;

-- Every policy that asked the old function, which showed a session naming no
-- tenant every row, asks this instead. The tenant's own rows are matched
-- first, by the column; the question above is asked once per statement, and
-- only when a row is another tenant's.
DO $$
DECLARE
    policed RECORD;
    tenant_column TEXT;
BEGIN
    FOR policed IN
        SELECT c.oid::regclass AS table_name,
               p.polname AS policy_name,
               pg_get_expr(p.polqual, p.polrelid) AS visible
          FROM pg_policy p
          JOIN pg_class c ON c.oid = p.polrelid
         WHERE pg_get_expr(p.polqual, p.polrelid) LIKE '%runtara_tenant_visible(%'
    LOOP
        tenant_column := substring(
            policed.visible FROM '^runtara_tenant_visible\(\(?"?([A-Za-z_][A-Za-z_0-9]*)"?\)?(?:::text)?\)$'
        );
        IF tenant_column IS NULL THEN
            RAISE EXCEPTION 'cannot tell the tenant column of policy % on %: %',
                policed.policy_name, policed.table_name, policed.visible;
        END IF;
        EXECUTE format('DROP POLICY %I ON %s', policed.policy_name, policed.table_name);
        EXECUTE format(
            'CREATE POLICY %I ON %s '
            'USING (%I = current_setting(''runtara.tenant_id'', true) OR (SELECT runtara_all_tenants())) '
            'WITH CHECK (%I = current_setting(''runtara.tenant_id'', true) OR (SELECT runtara_all_tenants()))',
            policed.policy_name, policed.table_name, tenant_column, tenant_column
        );
    END LOOP;
END
$$;

-- Nothing asks the old function any more, and nothing new should.
DROP FUNCTION IF EXISTS runtara_tenant_visible(TEXT);
