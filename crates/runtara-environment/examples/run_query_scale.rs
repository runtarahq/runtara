// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! How `run_query` performs as a workflow's runs grow
//! (`docs/workflow-records.md`): order-shaped runs with nested state
//! (a customer object, an array of lines, tags, an optional fulfillment
//! object), queried as the engine queries them, without the engine's wire
//! between.
//!
//! ```text
//! SCALE_DATABASE_URL=postgres://… cargo run --release -p runtara-environment \
//!     --example run_query_scale -- 10000 100000 1000000
//! ```
//!
//! Each size gets a tenant of its own (`nested-<n>`), loaded once and kept.

use std::time::Instant;

use runtara_environment::instance_repository::InstanceRepository;
use runtara_environment::run_query::RunQuery;
use serde_json::{Value, json};
use sqlx::PgPool;

async fn load(pool: &PgPool, tenant: &str, runs: i64) {
    let loaded: i64 = sqlx::query_scalar("SELECT count(*) FROM instances WHERE tenant_id = $1")
        .bind(tenant)
        .fetch_one(pool)
        .await
        .unwrap();
    if loaded > 0 {
        eprintln!("{tenant}: {loaded} runs already loaded");
        return;
    }
    let started = Instant::now();
    for version in [1, 2] {
        sqlx::query(
            "INSERT INTO images (image_id, tenant_id, name, binary_path) VALUES ($1, $2, $3, '/x')",
        )
        .bind(format!("{tenant}-orders-{version}"))
        .bind(tenant)
        .bind(format!("orders:{version}"))
        .execute(pool)
        .await
        .unwrap();
    }
    // About 1.25 runs a label.
    let labels = (runs * 4 / 5).max(1);
    let batch = 50_000i64;
    let mut from = 0i64;
    while from < runs {
        let to = (from + batch).min(runs);
        let mut tx = pool.begin().await.unwrap();
        sqlx::query(
            "INSERT INTO instances (instance_id, tenant_id, status, run_label, created_at, started_at, finished_at) \
             SELECT $1 || '-' || g, $1, \
                    (CASE WHEN g % 50 = 0 THEN 'failed' WHEN g % 20 = 0 THEN 'suspended' ELSE 'completed' END)::instance_status, \
                    'order-' || (g % $4), \
                    timestamptz '2026-01-01' + g * interval '1 second', \
                    timestamptz '2026-01-01' + g * interval '1 second', \
                    timestamptz '2026-01-01' + g * interval '1 second' + interval '5 seconds' \
             FROM generate_series($2::bigint, $3::bigint - 1) g",
        )
        .bind(tenant)
        .bind(from)
        .bind(to)
        .bind(labels)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO instance_images (instance_id, image_id, tenant_id) \
             SELECT $1 || '-' || g, $1 || '-orders-' || (CASE WHEN g < $4 THEN 1 ELSE 2 END), $1 \
             FROM generate_series($2::bigint, $3::bigint - 1) g",
        )
        .bind(tenant)
        .bind(from)
        .bind(to)
        .bind(runs / 2)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO instance_state (instance_id, state, state_updated_at)
             SELECT $1 || '-' || g,
               jsonb_strip_nulls(jsonb_build_object(
                 'customer', jsonb_build_object(
                    'name', (ARRAY['Anna','Bogdan','Marta','Zoe','Mateo','Ines','Olaf','Priya','Kenji','Lea'])[1 + (g * 7) % 10]
                            || ' ' || (ARRAY['Nowak','Smith','Garcia','Kim','Rossi','Berg'])[1 + (g * 11) % 6],
                    'country', (ARRAY['DE','PL','US','FR','GB','ES','IT'])[1 + g % 7],
                    'tier', (ARRAY['basic','plus','pro'])[1 + (g * 3) % 3],
                    'vip', g % 5 = 0),
                 'total', ((g * 37) % 100000) / 100.0,
                 'status', (ARRAY['new','paid','shipped','delivered','cancelled'])[1 + (g * 13) % 5],
                 'items', (SELECT jsonb_agg(jsonb_build_object(
                              'sku', 'SKU-' || ((g * i * 7) % 500),
                              'qty', 1 + (g + i) % 5,
                              'price', ((g * i) % 10000) / 100.0))
                           FROM generate_series(1, 1 + (g % 4)) i),
                 'tags', CASE g % 4 WHEN 0 THEN '[]'::jsonb WHEN 1 THEN '["rush"]'::jsonb
                                    WHEN 2 THEN '["gift","rush"]'::jsonb ELSE '["gift"]'::jsonb END,
                 'fulfillment', CASE WHEN g % 3 <> 0 THEN jsonb_build_object(
                      'carrier', (ARRAY['dhl','ups','dpd'])[1 + g % 3],
                      'trackingId', 'T' || g) END,
                 'notifiedAt', CASE WHEN g % 3 <> 0 THEN to_char(timestamptz '2026-01-01' + g * interval '1 second', 'YYYY-MM-DD"T"HH24:MI:SS"Z"') END)),
               timestamptz '2026-01-01' + g * interval '1 second' + interval '3 seconds'
             FROM generate_series($2::bigint, $3::bigint - 1) g"#,
        )
        .bind(tenant)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await
        .unwrap();
        tx.commit().await.unwrap();
        from = to;
    }
    for table in ["instances", "instance_state", "run_keys", "instance_images"] {
        sqlx::query(&format!("ANALYZE {table}"))
            .execute(pool)
            .await
            .unwrap();
    }
    eprintln!(
        "{tenant}: loaded {runs} runs in {:.1}s",
        started.elapsed().as_secs_f64()
    );
}

fn c(op: &str, arguments: Value) -> Value {
    json!({ "op": op, "arguments": arguments })
}

fn queries(runs: i64) -> Vec<(&'static str, Value)> {
    let label = format!("order-{}", runs / 3);
    vec![
        (
            "one label, latest",
            json!({"latestPerLabel": true,
            "condition": c("EQ", json!(["label", label]))}),
        ),
        (
            "one label, last completed run",
            json!({
            "condition": c("AND", json!([c("EQ", json!(["label", label])), c("EQ", json!(["status", "completed"]))])),
            "orderBy": [{"path": "createdAt", "desc": true}], "limit": 1}),
        ),
        (
            "nested eq: country DE and paid",
            json!({"latestPerLabel": true, "count": true,
            "select": ["label", "state.total"],
            "condition": c("AND", json!([c("EQ", json!(["state.customer.country", "DE"])), c("EQ", json!(["state.status", "paid"]))]))}),
        ),
        (
            "[*] eq: lines with SKU-42",
            json!({"latestPerLabel": true, "count": true, "select": ["label"],
            "condition": c("EQ", json!(["state.items[*].sku", "SKU-42"]))}),
        ),
        (
            "nested range: total > 990",
            json!({"latestPerLabel": true, "count": true, "select": ["label", "state.total"],
            "condition": c("GT", json!(["state.total", 990]))}),
        ),
        (
            "[*] range + bool: qty >= 5, vip",
            json!({"latestPerLabel": true, "count": true, "select": ["label"],
            "condition": c("AND", json!([c("GTE", json!(["state.items[*].qty", 5])), c("EQ", json!(["state.customer.vip", true]))]))}),
        ),
        (
            "missing field + IN: not notified",
            json!({"latestPerLabel": true, "count": true, "select": ["label"],
            "condition": c("AND", json!([c("IS_DEFINED", json!(["state.notifiedAt", false])), c("IN", json!(["state.status", ["paid", "shipped"]]))]))}),
        ),
        (
            "CONTAINS tag 'gift'",
            json!({"latestPerLabel": true, "count": true, "select": ["label"],
            "condition": c("CONTAINS", json!(["state.tags", "gift"]))}),
        ),
        (
            "STARTS_WITH name 'Mar'",
            json!({"latestPerLabel": true, "count": true, "select": ["label"],
            "condition": c("STARTS_WITH", json!(["state.customer.name", "Mar"]))}),
        ),
        (
            "deep path eq: carrier ups, failed",
            json!({"latestPerLabel": true, "count": true, "select": ["label"],
            "condition": c("AND", json!([c("EQ", json!(["state.fulfillment.carrier", "ups"])), c("EQ", json!(["status", "failed"]))]))}),
        ),
        (
            "group: by country, count/sum/avg",
            json!({"latestPerLabel": true, "groupBy": ["state.customer.country"],
            "aggregates": [{"fn": "count"}, {"fn": "sum", "path": "state.total"}, {"fn": "avg", "path": "state.total"}]}),
        ),
        (
            "group: by state.status x run status",
            json!({"latestPerLabel": true, "groupBy": ["state.status", "status"],
            "aggregates": [{"fn": "count"}]}),
        ),
        (
            "group: filtered, by tier",
            json!({"latestPerLabel": true, "groupBy": ["state.customer.tier"],
            "aggregates": [{"fn": "count"}, {"fn": "max", "path": "state.total"}],
            "condition": c("EQ", json!(["state.customer.country", "PL"]))}),
        ),
        (
            "unnest: top 10 SKUs by qty",
            json!({"latestPerLabel": true, "unnest": "state.items",
            "groupBy": ["item.sku"], "aggregates": [{"fn": "sum", "path": "item.qty", "as": "qty"}],
            "orderBy": [{"path": "qty", "desc": true}], "limit": 10}),
        ),
        (
            "all runs: group by version",
            json!({"groupBy": ["version"], "aggregates": [{"fn": "count"}]}),
        ),
        (
            "page 101 by total desc",
            json!({"latestPerLabel": true, "select": ["label", "state.total"],
            "orderBy": [{"path": "state.total", "desc": true}], "limit": 100, "offset": 10000}),
        ),
        (
            "first page, whole state",
            json!({"latestPerLabel": true, "limit": 100}),
        ),
    ]
}

async fn index_scans(pool: &PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT COALESCE(sum(idx_scan), 0)::bigint FROM pg_stat_user_indexes \
         WHERE indexrelname IN ('run_keys_state_idx', 'instance_state_state_path_idx')",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::main]
async fn main() {
    let url = std::env::var("SCALE_DATABASE_URL").expect("SCALE_DATABASE_URL");
    let sizes: Vec<i64> = std::env::args()
        .skip(1)
        .map(|a| a.parse().expect("a size"))
        .collect();
    let pool = PgPool::connect(&url).await.unwrap();
    runtara_environment::migrations::run(&pool).await.unwrap();
    let repository = InstanceRepository::new(pool.clone());
    for runs in sizes {
        let tenant = format!("nested-{runs}");
        load(&pool, &tenant, runs).await;
        println!("\n## {runs} runs ({} labels)\n", runs * 4 / 5);
        println!("| query | rows | total | median ms | max ms | bytes | GIN |");
        println!("| --- | ---: | ---: | ---: | ---: | ---: | :---: |");
        for (name, query) in queries(runs) {
            let parsed: RunQuery = serde_json::from_value(query.clone()).unwrap();
            let mut times = Vec::new();
            let mut last = None;
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            let before = index_scans(&pool).await;
            for _ in 0..5 {
                let started = Instant::now();
                let result = repository
                    .query_runs(&tenant, "orders", parsed.clone())
                    .await
                    .unwrap_or_else(|e| panic!("{name}: {e}"));
                times.push(started.elapsed().as_secs_f64() * 1000.0);
                last = Some(result);
            }
            // Statistics reach the view a second or so after the queries.
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
            let used_index = index_scans(&pool).await > before;
            times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let result = last.unwrap();
            let bytes = serde_json::to_vec(&result).unwrap().len();
            println!(
                "| {name} | {} | {} | {:.1} | {:.1} | {bytes} | {} |",
                result.rows.len(),
                result.total.map(|t| t.to_string()).unwrap_or_default(),
                times[2],
                times[4],
                if used_index { "yes" } else { "" }
            );
        }
    }
}
