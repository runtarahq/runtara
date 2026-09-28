// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Boot-time check of the role the object-model pool connects as.
//!
//! Raw SQL (`sql/execute`, MCP `execute_sql`) runs with whatever role
//! `OBJECT_MODEL_DATABASE_URL` logs in as, so that role is the real privilege
//! boundary for caller-supplied SQL. A superuser role, or an object-model
//! database that is the server's own database, lets raw SQL reach far more than
//! object tables. This only warns: CI smoke runs and many e2e scripts share one
//! database on purpose. Nothing here logs a URL or a credential.

use sqlx::PgPool;

/// What the boot check learned about the object-model connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectModelPrivilegeFacts {
    /// The object-model login role is a Postgres superuser.
    pub superuser: bool,
    /// The object-model pool and the server pool reach the same database on
    /// the same Postgres server.
    pub shares_server_database: bool,
}

/// One warning per unsafe fact, worded for operators.
pub fn privilege_warnings(facts: ObjectModelPrivilegeFacts) -> Vec<&'static str> {
    let mut warnings = Vec::new();
    if facts.superuser {
        warnings.push(
            "The object-model database role is a Postgres superuser. Raw SQL \
             (sql/execute, MCP execute_sql) runs with that role, so it can reach \
             every database on the server. Point OBJECT_MODEL_DATABASE_URL at a \
             dedicated non-superuser role that owns only the object-model database.",
        );
    }
    if facts.shares_server_database {
        warnings.push(
            "The object-model database is the server's own database. Raw SQL \
             (sql/execute, MCP execute_sql) runs there with the object-model role, \
             so it can read and change server tables (workflows, connections, API \
             keys). Use a dedicated object-model database and a non-superuser role \
             that cannot connect to the server database.",
        );
    }
    warnings
}

/// Where a pool is connected: database name plus server address and port.
/// The address is `None` over a Unix socket, which still compares correctly.
type DatabaseIdentity = (String, Option<String>, Option<i32>);

async fn database_identity(pool: &PgPool) -> Result<DatabaseIdentity, sqlx::Error> {
    sqlx::query_as("SELECT current_database()::text, host(inet_server_addr()), inet_server_port()")
        .fetch_one(pool)
        .await
}

/// Collect [`ObjectModelPrivilegeFacts`] from the two live pools.
pub async fn collect_facts(
    object_model_pool: &PgPool,
    server_pool: &PgPool,
) -> Result<ObjectModelPrivilegeFacts, sqlx::Error> {
    let superuser: bool =
        sqlx::query_scalar("SELECT rolsuper FROM pg_roles WHERE rolname = current_user")
            .fetch_optional(object_model_pool)
            .await?
            .unwrap_or(false);
    let object_model = database_identity(object_model_pool).await?;
    let server = database_identity(server_pool).await?;
    Ok(ObjectModelPrivilegeFacts {
        superuser,
        shares_server_database: object_model == server,
    })
}

/// Run the check and log each finding as a `WARN`. A failed check is itself
/// only a warning; it never stops the server.
pub async fn warn_if_overprivileged(object_model_pool: &PgPool, server_pool: &PgPool) {
    match collect_facts(object_model_pool, server_pool).await {
        Ok(facts) => {
            for warning in privilege_warnings(facts) {
                tracing::warn!(
                    superuser = facts.superuser,
                    shares_server_database = facts.shares_server_database,
                    "{warning}"
                );
            }
        }
        Err(error) => tracing::warn!(
            error = %error,
            "Could not check the object-model database role privileges"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(superuser: bool, shares_server_database: bool) -> ObjectModelPrivilegeFacts {
        ObjectModelPrivilegeFacts {
            superuser,
            shares_server_database,
        }
    }

    #[test]
    fn dedicated_non_superuser_role_is_quiet() {
        assert!(privilege_warnings(facts(false, false)).is_empty());
    }

    #[test]
    fn superuser_role_warns_and_names_the_raw_sql_surface() {
        let warnings = privilege_warnings(facts(true, false));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("superuser"));
        assert!(warnings[0].contains("sql/execute"));
        assert!(warnings[0].contains("execute_sql"));
        assert!(warnings[0].contains("non-superuser role"));
    }

    #[test]
    fn shared_server_database_warns() {
        let warnings = privilege_warnings(facts(false, true));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("server's own database"));
        assert!(warnings[0].contains("dedicated object-model database"));
    }

    #[test]
    fn both_findings_warn_separately() {
        assert_eq!(privilege_warnings(facts(true, true)).len(), 2);
    }
}
