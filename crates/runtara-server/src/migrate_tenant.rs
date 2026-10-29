// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara-server migrate-tenant --tenant-id <id>`
//!
//! Copies one tenant from the databases and artifact store of a process that
//! served it alone into those of a process serving several (C11, M7.7):
//! workflows, connections, triggers, API keys and the like from the server
//! database, every run with its checkpoints, events, waits and images from the
//! runtime database, and the artifacts those images name. Run it with the
//! tenant suspended and its old process stopped, so nothing changes under it;
//! it refuses otherwise.
//!
//! Where from and to, as environment variables, never as arguments (they
//! carry credentials):
//!
//! - `MIGRATE_FROM_SERVER_DATABASE_URL`, `MIGRATE_FROM_RUNTIME_DATABASE_URL`;
//!   `MIGRATE_FROM_ARTIFACT_STORE_URL` or `MIGRATE_FROM_DATA_DIR`.
//! - `MIGRATE_TO_SERVER_DATABASE_URL`, `MIGRATE_TO_RUNTIME_DATABASE_URL`, each
//!   a role that may set `session_replication_role` (the copy runs as a
//!   replica, so no trigger fires and no foreign key is checked mid-copy);
//!   the artifacts go where this process keeps its own
//!   (`RUNTARA_ARTIFACT_STORE_URL`, else `DATA_DIR`).
//!
//! Each database's rows are copied in one transaction. Columns are copied by
//! the names both sides have; a column the source has and the target lacks
//! stops the copy, as it would lose data. Serial ids are given anew in the
//! source's order. Not copied: compilations (the first run compiles again,
//! against this process's release), OAuth states, rate-limit telemetry, the
//! live process registry and workflow-API receipts, and process bookkeeping.
//!
//! It refuses when the target already holds the tenant, when a connection
//! title is taken there, and when the tenant keeps object-model data in its
//! process's default database, which a shared process does not provide.

use std::ffi::OsString;

use futures::TryStreamExt;
use runtara_artifact_store::Digest;
use runtara_environment::artifacts::ArtifactStore;
use serde_json::{Value, json};
use sqlx::{PgPool, Postgres, Row, Transaction};

/// The argv[1] that selects this command.
pub const COMMAND: &str = "migrate-tenant";

const USAGE: &str = "Usage: runtara-server migrate-tenant --tenant-id <id>";

/// Rows sent to the target at once.
const BATCH: usize = 500;

#[derive(Debug, PartialEq, Eq)]
pub struct MigrateArgs {
    pub tenant_id: String,
}

/// Parse the process arguments. `None` when they don't select this command.
pub fn parse_args(mut args: impl Iterator<Item = OsString>) -> Option<Result<MigrateArgs, String>> {
    let _program = args.next();
    if args.next()?.to_str() != Some(COMMAND) {
        return None;
    }
    let rest: Vec<String> = match args.map(OsString::into_string).collect() {
        Ok(rest) => rest,
        Err(_) => return Some(Err(format!("arguments must be valid UTF-8\n{USAGE}"))),
    };
    Some(match rest.as_slice() {
        [flag, tenant] if flag == "--tenant-id" && !tenant.trim().is_empty() => Ok(MigrateArgs {
            tenant_id: tenant.clone(),
        }),
        [arg] if arg.starts_with("--tenant-id=") && arg.len() > "--tenant-id=".len() => {
            Ok(MigrateArgs {
                tenant_id: arg["--tenant-id=".len()..].to_owned(),
            })
        }
        _ => Err(USAGE.to_owned()),
    })
}

/// How a table's rows belong to a tenant, and how they are copied.
struct Table {
    name: &'static str,
    /// Rows of the tenant: a condition on `$1`, the tenant id.
    of_tenant: &'static str,
    /// A serial id given anew in the target, in the source's order.
    serial: Option<&'static str>,
}

const fn by(name: &'static str, of_tenant: &'static str) -> Table {
    Table {
        name,
        of_tenant,
        serial: None,
    }
}

const fn serial(name: &'static str, of_tenant: &'static str) -> Table {
    Table {
        name,
        of_tenant,
        serial: Some("id"),
    }
}

/// A row whose tenant column a later migration added may still be unset:
/// it is the tenant's when its instance is.
const INSTANCE_ROWS: &str = "tenant_id = $1 OR (tenant_id IS NULL AND instance_id IN (SELECT instance_id FROM instances WHERE tenant_id = $1))";

const RUNTIME_TABLES: &[Table] = &[
    by("instances", "tenant_id = $1"),
    serial("checkpoints", INSTANCE_ROWS),
    serial("instance_events", INSTANCE_ROWS),
    by("pending_signals", INSTANCE_ROWS),
    serial("pending_checkpoint_signals", INSTANCE_ROWS),
    by("invocation_root_leases", INSTANCE_ROWS),
    by("invocation_attempts", INSTANCE_ROWS),
    by("instance_input_requests", "tenant_id = $1"),
    by("instance_input_parks", INSTANCE_ROWS),
    by(
        "instance_control_receipts",
        "tenant_id = $1 OR (tenant_id IS NULL AND caller_instance_id IN (SELECT instance_id FROM instances WHERE tenant_id = $1))",
    ),
    by("instance_external_outcomes", "tenant_id = $1"),
    by("instance_waits", "tenant_id = $1"),
    by(
        "instance_wait_targets",
        "tenant_id = $1 OR (tenant_id IS NULL AND waiter_instance_id IN (SELECT instance_id FROM instances WHERE tenant_id = $1))",
    ),
    by("instance_agent_continuations", INSTANCE_ROWS),
    by("instance_state", INSTANCE_ROWS),
    by("instance_state_writes", INSTANCE_ROWS),
    by("images", "tenant_id = $1"),
    by("instance_images", "tenant_id = $1"),
    by("instance_launches", "tenant_id = $1"),
];

const SERVER_TABLES: &[Table] = &[
    by("workflows", "tenant_id = $1"),
    by("workflow_definitions", "tenant_id = $1"),
    by("workflow_dependencies", "parent_tenant_id = $1"),
    serial("workflow_metrics_hourly", "tenant_id = $1"),
    by("connection_data_entity", "tenant_id = $1"),
    by("connection_defaults", "tenant_id = $1"),
    by("invocation_trigger", "tenant_id = $1"),
    by("object_schema", "tenant_id = $1"),
    by("api_keys", "org_id = $1"),
    by("report_definitions", "tenant_id = $1"),
    by("audit_events", "tenant_id = $1"),
    by("execution_requests", "tenant_id = $1"),
    by(
        "execution_outbox",
        "request_id IN (SELECT request_id FROM execution_requests WHERE tenant_id = $1)",
    ),
    by("execution_admission_reservations", "tenant_id = $1"),
    by("channel_intake", "tenant_id = $1"),
    by("operations_views", "tenant_id = $1"),
];

fn env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| format!("{name} is required"))
}

async fn connect(name: &str) -> Result<PgPool, String> {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&env(name)?)
        .await
        .map_err(|error| format!("{name}: {error}"))
}

/// The source's artifact store.
fn source_store() -> Result<std::sync::Arc<dyn ArtifactStore>, String> {
    if let Ok(url) = std::env::var("MIGRATE_FROM_ARTIFACT_STORE_URL") {
        let cache = std::env::temp_dir().join("runtara-migrate-tenant-cache");
        return runtara_artifact_store::object::ObjectArtifactStore::from_url(&url, cache)
            .map(|store| std::sync::Arc::new(store) as _)
            .map_err(|error| format!("MIGRATE_FROM_ARTIFACT_STORE_URL: {error}"));
    }
    let dir = env("MIGRATE_FROM_DATA_DIR")
        .map_err(|_| "MIGRATE_FROM_ARTIFACT_STORE_URL or MIGRATE_FROM_DATA_DIR is required")?;
    Ok(std::sync::Arc::new(
        runtara_environment::image_registry::default_artifact_store(std::path::Path::new(&dir)),
    ))
}

/// This process's own artifact store, as it would keep artifacts:
/// `RUNTARA_ARTIFACT_STORE_URL`, else under `DATA_DIR`.
pub fn own_artifact_store() -> Result<std::sync::Arc<dyn ArtifactStore>, String> {
    let data_dir = crate::embedded_runtara::data_dir();
    match std::env::var("RUNTARA_ARTIFACT_STORE_URL")
        .ok()
        .filter(|url| !url.trim().is_empty())
    {
        Some(url) => runtara_artifact_store::object::ObjectArtifactStore::from_url(
            url.trim(),
            data_dir.join("artifact-cache"),
        )
        .map(|store| std::sync::Arc::new(store) as _)
        .map_err(|error| format!("RUNTARA_ARTIFACT_STORE_URL: {error}")),
        None => Ok(std::sync::Arc::new(
            runtara_environment::image_registry::default_artifact_store(&data_dir),
        )),
    }
}

/// Copy `args.tenant_id` into `target_store` and the target databases.
pub async fn run(
    args: MigrateArgs,
    target_store: std::sync::Arc<dyn ArtifactStore>,
) -> Result<Value, String> {
    let tenant = args.tenant_id.as_str();
    let from_server = connect("MIGRATE_FROM_SERVER_DATABASE_URL").await?;
    let from_runtime = connect("MIGRATE_FROM_RUNTIME_DATABASE_URL").await?;
    let to_server = connect("MIGRATE_TO_SERVER_DATABASE_URL").await?;
    let to_runtime = connect("MIGRATE_TO_RUNTIME_DATABASE_URL").await?;
    let from_store = source_store()?;

    preconditions(tenant, &from_server, &from_runtime, &to_server, &to_runtime).await?;

    let mut runtime_tx = replica(&to_runtime).await?;
    let mut server_tx = replica(&to_server).await?;
    let mut copied = serde_json::Map::new();
    for table in RUNTIME_TABLES {
        let rows = copy_table(table, tenant, &from_runtime, &mut runtime_tx).await?;
        copied.insert(table.name.to_owned(), rows.into());
    }
    for table in SERVER_TABLES {
        let rows = copy_table(table, tenant, &from_server, &mut server_tx).await?;
        copied.insert(table.name.to_owned(), rows.into());
    }
    // The admission count is derived: the tenant's open reservations.
    if table_exists(&to_server, "execution_admission_tenants").await? {
        sqlx::query(
            "INSERT INTO execution_admission_tenants (tenant_id, reserved_count) \
             SELECT $1, count(*) FROM execution_admission_reservations WHERE tenant_id = $1 AND released_at IS NULL \
             ON CONFLICT (tenant_id) DO UPDATE SET reserved_count = EXCLUDED.reserved_count, updated_at = now()",
        )
        .bind(tenant)
        .execute(&mut *server_tx)
        .await
        .map_err(|error| format!("admission count: {error}"))?;
    }

    // The artifacts the copied images name, before the rows that name them
    // are committed.
    let digests: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT binary_digest FROM images WHERE tenant_id = $1 AND binary_digest IS NOT NULL",
    )
    .bind(tenant)
    .fetch_all(&from_runtime)
    .await
    .map_err(|error| format!("image digests: {error}"))?;
    let mut artifacts = 0;
    for digest in &digests {
        let parsed =
            Digest::parse(digest).map_err(|_| format!("image digest {digest} is not a digest"))?;
        if target_store
            .exists(&parsed)
            .await
            .map_err(|error| format!("artifact {digest}: {error}"))?
        {
            continue;
        }
        let bytes = from_store
            .get(&parsed)
            .await
            .map_err(|error| format!("artifact {digest}: {error}"))?
            .ok_or_else(|| format!("artifact {digest} is missing from the source store"))?;
        let stored = target_store
            .put(&bytes)
            .await
            .map_err(|error| format!("artifact {digest}: {error}"))?;
        if stored != parsed {
            return Err(format!("artifact {digest} changed in transit"));
        }
        artifacts += 1;
    }

    runtime_tx
        .commit()
        .await
        .map_err(|error| format!("committing the runtime rows: {error}"))?;
    server_tx.commit().await.map_err(|error| {
        format!(
            "committing the server rows failed after the runtime rows were committed; \
             remove the tenant's runtime rows from the target before running again: {error}"
        )
    })?;
    Ok(json!({"tenant": tenant, "rows": copied, "artifacts": artifacts}))
}

async fn replica(pool: &PgPool) -> Result<Transaction<'static, Postgres>, String> {
    let mut tx = pool.begin().await.map_err(|error| error.to_string())?;
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *tx)
        .await
        .map_err(|error| {
            format!("the target role may not copy as a replica (session_replication_role): {error}")
        })?;
    Ok(tx)
}

async fn table_exists(pool: &PgPool, table: &str) -> Result<bool, String> {
    sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
        .bind(format!("public.{table}"))
        .fetch_one(pool)
        .await
        .map_err(|error| error.to_string())
}

async fn columns(pool: &PgPool, table: &str) -> Result<Vec<String>, String> {
    sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = $1 ORDER BY ordinal_position",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .map_err(|error| format!("{table}: {error}"))
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Copies `table`'s rows of `tenant` from `source` into the target, returning
/// how many.
async fn copy_table(
    table: &Table,
    tenant: &str,
    source: &PgPool,
    target: &mut Transaction<'static, Postgres>,
) -> Result<u64, String> {
    let source_columns = columns(source, table.name).await?;
    if source_columns.is_empty() {
        return Ok(0); // The source never had this table.
    }
    let target_columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = $1",
    )
    .bind(table.name)
    .fetch_all(&mut **target)
    .await
    .map_err(|error| format!("{}: {error}", table.name))?;
    if target_columns.is_empty() {
        return Err(format!("the target has no table {}", table.name));
    }
    if let Some(missing) = source_columns
        .iter()
        .find(|column| !target_columns.contains(column))
    {
        return Err(format!(
            "the target's {} has no column {missing}; migrate the target first",
            table.name
        ));
    }
    let copied: Vec<&String> = source_columns
        .iter()
        .filter(|column| Some(column.as_str()) != table.serial)
        .collect();
    let list = copied
        .iter()
        .map(|column| quoted(column))
        .collect::<Vec<_>>()
        .join(", ");
    let order = table.serial.map(quoted).unwrap_or_else(|| "1".to_owned());
    let select = format!(
        "SELECT row_to_json(r)::text FROM (SELECT {list} FROM {name} WHERE {of_tenant} ORDER BY {order}) r",
        name = quoted(table.name),
        of_tenant = table.of_tenant,
    );
    let insert = format!(
        "INSERT INTO {name} ({list}) SELECT {list} FROM json_populate_recordset(NULL::{name}, $1::json)",
        name = quoted(table.name),
    );
    let mut rows = sqlx::query(&select).bind(tenant).fetch(source);
    let mut batch: Vec<String> = Vec::with_capacity(BATCH);
    let mut total = 0u64;
    loop {
        let row = rows
            .try_next()
            .await
            .map_err(|error| format!("reading {}: {error}", table.name))?;
        let done = row.is_none();
        if let Some(row) = row {
            batch.push(row.get::<String, _>(0));
        }
        if batch.len() == BATCH || (done && !batch.is_empty()) {
            let json = format!("[{}]", batch.join(","));
            total += sqlx::query(&insert)
                .bind(json)
                .execute(&mut **target)
                .await
                .map_err(|error| format!("writing {}: {error}", table.name))?
                .rows_affected();
            batch.clear();
        }
        if done {
            return Ok(total);
        }
    }
}

/// The checks before anything is written.
async fn preconditions(
    tenant: &str,
    from_server: &PgPool,
    from_runtime: &PgPool,
    to_server: &PgPool,
    to_runtime: &PgPool,
) -> Result<(), String> {
    let count = |pool: &PgPool, sql: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(sql)
                .bind(tenant)
                .fetch_one(&pool)
                .await
                .map_err(|error| error.to_string())
        }
    };
    // Nothing of it may still run where it is.
    if table_exists(from_runtime, "container_registry").await?
        && count(
            from_runtime,
            "SELECT count(*) FROM container_registry WHERE tenant_id = $1",
        )
        .await?
            > 0
    {
        return Err(format!(
            "{tenant} still has running processes; stop its process first"
        ));
    }
    // A queued launch is only a row, which the target dispatches; one a
    // process holds (preparing, leased, starting, running) is not.
    let live = count(
        from_runtime,
        "SELECT count(*) FROM instance_launches WHERE tenant_id = $1 AND state IN ('preparing','leased','starting','running')",
    )
    .await?;
    if live > 0 {
        return Err(format!(
            "{tenant} has {live} launches in flight; stop its process first"
        ));
    }
    // The target must not hold it already.
    for (pool, sql, what) in [
        (
            to_server,
            "SELECT count(*) FROM workflows WHERE tenant_id = $1",
            "workflows",
        ),
        (
            to_runtime,
            "SELECT count(*) FROM instances WHERE tenant_id = $1",
            "runs",
        ),
    ] {
        if count(pool, sql).await? > 0 {
            return Err(format!("the target already has {tenant}'s {what}"));
        }
    }
    // A connection title the target has given another tenant.
    let titles: Vec<String> =
        sqlx::query_scalar("SELECT title FROM connection_data_entity WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_all(from_server)
            .await
            .map_err(|error| error.to_string())?;
    if !titles.is_empty() {
        let taken: Vec<String> = sqlx::query_scalar(
            "SELECT title FROM connection_data_entity WHERE title = ANY($1) AND tenant_id <> $2",
        )
        .bind(&titles)
        .bind(tenant)
        .fetch_all(to_server)
        .await
        .map_err(|error| error.to_string())?;
        if !taken.is_empty() {
            return Err(format!(
                "connection titles taken in the target by another tenant: {}; rename them first",
                taken.join(", ")
            ));
        }
    }
    // Object-model data in the process's own default database has nowhere
    // to go: a shared process keeps none for its tenants.
    let object_model = std::env::var("MIGRATE_FROM_OBJECT_MODEL_DATABASE_URL").ok();
    let object_pool = match object_model {
        Some(url) => Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .connect(&url)
                .await
                .map_err(|error| format!("MIGRATE_FROM_OBJECT_MODEL_DATABASE_URL: {error}"))?,
        ),
        None => None,
    };
    let object_pool = object_pool.as_ref().unwrap_or(from_server);
    if table_exists(object_pool, "__schema").await? {
        let schemas: i64 = sqlx::query_scalar("SELECT count(*) FROM \"__schema\"")
            .fetch_one(object_pool)
            .await
            .map_err(|error| error.to_string())?;
        if schemas > 0 {
            return Err(format!(
                "{tenant} keeps {schemas} object-model schemas in its process's default database, \
                 which a shared process does not provide; move them to a database of its own \
                 connection first"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Option<Result<MigrateArgs, String>> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn the_tenant_is_named_and_nothing_else_is_taken() {
        assert_eq!(parse(&["runtara-server"]), None);
        assert_eq!(
            parse(&["runtara-server", COMMAND, "--tenant-id", "acme"]),
            Some(Ok(MigrateArgs {
                tenant_id: "acme".into()
            }))
        );
        assert_eq!(
            parse(&["runtara-server", COMMAND, "--tenant-id=acme"]),
            Some(Ok(MigrateArgs {
                tenant_id: "acme".into()
            }))
        );
        assert!(matches!(parse(&["runtara-server", COMMAND]), Some(Err(_))));
        assert!(matches!(
            parse(&["runtara-server", COMMAND, "--from", "postgres://x"]),
            Some(Err(_))
        ));
    }

    #[test]
    fn every_table_names_its_tenant() {
        for table in RUNTIME_TABLES.iter().chain(SERVER_TABLES) {
            assert!(table.of_tenant.contains("$1"), "{}", table.name);
        }
    }
}
