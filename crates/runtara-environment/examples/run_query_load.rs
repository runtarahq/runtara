// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Queries against the engine's own work (`docs/workflow-records.md`):
//! workers take runs through their lifecycle (registered, started, state
//! written with the store's `apply_state`, completed) without pause, while
//! more and more clients run the heaviest queries of `run_query_scale`
//! (over its `nested-1000000` tenant) at once. Both share one pool, of the
//! runtime pool's default size, as they do in the engine.
//!
//! ```text
//! SCALE_DATABASE_URL=postgres://…/runtara_scale \
//! RUNTARA_RUN_QUERY_TOTAL=4 RUNTARA_RUN_QUERY_PER_TENANT=100 \
//!   cargo run --release -p runtara-environment --example run_query_load -- [seconds] [clients…]
//! ```
//!
//! The query limits are read from the environment (`QueryLimits::from_env`);
//! every client queries one tenant's data, so set the per-tenant limit high
//! to see the total limit at work.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use runtara_core::persistence::run_state::{RunState, StatePatch};
use runtara_environment::instance_repository::InstanceRepository;
use runtara_environment::run_query::RunQuery;
use runtara_store_postgres::PostgresPersistence;
use serde_json::{Map, Value, json};
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::Mutex;

const TENANT: &str = "load-writes";
const WRITERS: usize = 8;

fn c(op: &str, arguments: Value) -> Value {
    json!({ "op": op, "arguments": arguments })
}

fn heavy() -> Vec<RunQuery> {
    [
        json!({"latestPerLabel": true, "groupBy": ["state.customer.country"],
            "aggregates": [{"fn": "count"}, {"fn": "sum", "path": "state.total"}, {"fn": "avg", "path": "state.total"}]}),
        json!({"latestPerLabel": true, "count": true, "select": ["label"],
            "condition": c("CONTAINS", json!(["state.tags", "gift"]))}),
        json!({"latestPerLabel": true, "unnest": "state.items",
            "groupBy": ["item.sku"], "aggregates": [{"fn": "sum", "path": "item.qty", "as": "qty"}],
            "orderBy": [{"path": "qty", "desc": true}], "limit": 10}),
    ]
    .into_iter()
    .map(|q| serde_json::from_value(q).unwrap())
    .collect()
}

fn patch(write: usize) -> StatePatch {
    let mut set = Map::new();
    set.insert("step".into(), json!(write));
    set.insert("status".into(), json!("paid"));
    set.insert(
        "customer".into(),
        json!({"name": "Load Test", "country": "DE"}),
    );
    set.insert("items".into(), json!([{"sku": "SKU-1", "qty": 2}]));
    StatePatch { set, clear: vec![] }
}

fn pct(times: &mut [Duration], p: f64) -> f64 {
    if times.is_empty() {
        return 0.0;
    }
    times.sort();
    times[((times.len() as f64 - 1.0) * p).round() as usize].as_secs_f64() * 1000.0
}

#[derive(Default)]
struct Phase {
    writes: Vec<Duration>,
    completes: Vec<Duration>,
    runs: usize,
    queries: Vec<Duration>,
    busy: usize,
    failed: usize,
}

async fn phase(pool: &PgPool, clients: usize, seconds: u64, next_run: Arc<AtomicUsize>) -> Phase {
    let stop = Arc::new(AtomicBool::new(false));
    let result = Arc::new(Mutex::new(Phase::default()));
    let store = Arc::new(PostgresPersistence::new(pool.clone()));
    let mut tasks = Vec::new();
    for _ in 0..WRITERS {
        let (pool, store, stop, result, next_run) = (
            pool.clone(),
            store.clone(),
            stop.clone(),
            result.clone(),
            next_run.clone(),
        );
        tasks.push(tokio::spawn(async move {
            let mut mine = Phase::default();
            while !stop.load(Ordering::Relaxed) {
                let n = next_run.fetch_add(1, Ordering::Relaxed);
                let id = format!("{TENANT}-{n}");
                let mut tx = pool.begin().await.unwrap();
                sqlx::query("INSERT INTO instances (instance_id, tenant_id, status, run_label) VALUES ($1, $2, 'pending', $3)")
                    .bind(&id).bind(TENANT).bind(format!("order-{}", n % 5000)).execute(&mut *tx).await.unwrap();
                sqlx::query("INSERT INTO instance_images (instance_id, image_id, tenant_id) VALUES ($1, $2, $2)")
                    .bind(&id).bind(TENANT).execute(&mut *tx).await.unwrap();
                tx.commit().await.unwrap();
                sqlx::query("UPDATE instances SET status = 'running', started_at = CURRENT_TIMESTAMP WHERE instance_id = $1")
                    .bind(&id).execute(&pool).await.unwrap();
                for w in 0..5 {
                    let t = Instant::now();
                    store.apply_state(TENANT, &id, &format!("{:064x}", n * 10 + w), &patch(w)).await.unwrap();
                    mine.writes.push(t.elapsed());
                }
                let t = Instant::now();
                sqlx::query("UPDATE instances SET status = 'completed', finished_at = CURRENT_TIMESTAMP WHERE instance_id = $1")
                    .bind(&id).execute(&pool).await.unwrap();
                mine.completes.push(t.elapsed());
                mine.runs += 1;
            }
            let mut all = result.lock().await;
            all.writes.extend(mine.writes);
            all.completes.extend(mine.completes);
            all.runs += mine.runs;
        }));
    }
    let repository = InstanceRepository::new(pool.clone());
    for client in 0..clients {
        let (repository, stop, result) = (repository.clone(), stop.clone(), result.clone());
        tasks.push(tokio::spawn(async move {
            let queries = heavy();
            let mut mine = Phase::default();
            let mut i = client;
            while !stop.load(Ordering::Relaxed) {
                let t = Instant::now();
                match repository
                    .query_runs(
                        "nested-1000000",
                        "orders",
                        queries[i % queries.len()].clone(),
                    )
                    .await
                {
                    Ok(_) => mine.queries.push(t.elapsed()),
                    Err(e) if e.to_string().contains(runtara_environment::run_query::BUSY) => {
                        mine.busy += 1
                    }
                    Err(e) => {
                        eprintln!("query failed: {e}");
                        mine.failed += 1
                    }
                }
                i += 1;
            }
            let mut all = result.lock().await;
            all.queries.extend(mine.queries);
            all.busy += mine.busy;
            all.failed += mine.failed;
        }));
    }
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    stop.store(true, Ordering::Relaxed);
    for task in tasks {
        task.await.unwrap();
    }
    Arc::try_unwrap(result).ok().unwrap().into_inner()
}

#[tokio::main]
async fn main() {
    let url = std::env::var("SCALE_DATABASE_URL").expect("SCALE_DATABASE_URL");
    let args: Vec<u64> = std::env::args()
        .skip(1)
        .map(|a| a.parse().unwrap())
        .collect();
    let seconds = args.first().copied().unwrap_or(30);
    let levels: Vec<usize> = if args.len() > 1 {
        args[1..].iter().map(|n| *n as usize).collect()
    } else {
        vec![0, 1, 5, 10, 20]
    };
    let pool = PgPoolOptions::new()
        .max_connections(32)
        .acquire_timeout(Duration::from_secs(60))
        .connect(&url)
        .await
        .unwrap();
    runtara_environment::migrations::run(&pool).await.unwrap();
    // A clean slate for the writers' tenant.
    sqlx::query("DELETE FROM instances WHERE tenant_id = $1")
        .bind(TENANT)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM run_keys WHERE tenant_id = $1")
        .bind(TENANT)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM images WHERE tenant_id = $1")
        .bind(TENANT)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO images (image_id, tenant_id, name, binary_path) VALUES ($1, $1, 'loadwf:1', '/x')")
        .bind(TENANT).execute(&pool).await.unwrap();

    let limits = runtara_environment::run_query::QueryLimits::from_env();
    println!(
        "{WRITERS} writers; queries limited to {} at once ({} a tenant); {seconds} s a step\n",
        limits.total, limits.per_tenant
    );
    println!(
        "| query clients | runs/s | state write p50/p99 ms | complete p50/p99 ms | queries/s | query p50/p99 ms | busy | failed |"
    );
    println!("| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    let next_run = Arc::new(AtomicUsize::new(0));
    for clients in levels {
        let mut p = phase(&pool, clients, seconds, next_run.clone()).await;
        println!(
            "| {clients} | {:.0} | {:.1} / {:.1} | {:.1} / {:.1} | {:.2} | {:.0} / {:.0} | {} | {} |",
            p.runs as f64 / seconds as f64,
            pct(&mut p.writes, 0.5),
            pct(&mut p.writes, 0.99),
            pct(&mut p.completes, 0.5),
            pct(&mut p.completes, 0.99),
            p.queries.len() as f64 / seconds as f64,
            pct(&mut p.queries, 0.5),
            pct(&mut p.queries, 0.99),
            p.busy,
            p.failed,
        );
    }
    sqlx::query("DELETE FROM instances WHERE tenant_id = $1")
        .bind(TENANT)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM run_keys WHERE tenant_id = $1")
        .bind(TENANT)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM images WHERE tenant_id = $1")
        .bind(TENANT)
        .execute(&pool)
        .await
        .unwrap();
}
