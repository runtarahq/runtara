// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! What keeping `run_keys` costs the write path
//! (`docs/workflow-records.md`): runs go through the lifecycle the
//! engine takes them through (registered with a label and an image,
//! started, state written several times with the store's own
//! `apply_state`, completed), from many workers at once, in a database
//! with `run_keys`' triggers and in one without them.
//!
//! ```text
//! SCALE_DATABASE_URL=postgres://…/any cargo run --release -p runtara-environment \
//!     --example run_keys_write_cost -- [runs] [writes a run] [workers]
//! ```
//!
//! Makes its own databases (`write_cost_*`) next to the one named, and
//! drops them after.

use std::sync::Arc;
use std::time::{Duration, Instant};

use runtara_core::persistence::run_state::{RunState, StatePatch};
use runtara_store_postgres::PostgresPersistence;
use serde_json::{Map, Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio::sync::Mutex;

const TRIGGERS: &[(&str, &str)] = &[
    ("run_keys_on_image", "instance_images"),
    ("run_keys_on_state", "instance_state"),
    ("run_keys_on_label", "instances"),
    ("run_keys_on_run", "instances"),
];

#[derive(Default)]
struct Times {
    register: Vec<Duration>,
    start: Vec<Duration>,
    write: Vec<Duration>,
    complete: Vec<Duration>,
}

fn pct(times: &mut [Duration], p: f64) -> f64 {
    if times.is_empty() {
        return 0.0;
    }
    times.sort();
    let i = ((times.len() as f64 - 1.0) * p).round() as usize;
    times[i].as_secs_f64() * 1000.0
}

/// A patch of about `bytes`, nested as run state tends to be.
fn patch(run: usize, write: usize, bytes: usize) -> StatePatch {
    let mut set = Map::new();
    set.insert("step".into(), json!(write));
    set.insert(
        "status".into(),
        json!((["new", "paid", "shipped", "done"])[write % 4]),
    );
    set.insert(
        "customer".into(),
        json!({"name": format!("Customer {run}"), "country": (["DE", "PL", "US"])[run % 3]}),
    );
    set.insert(
        "items".into(),
        json!(
            (0..3)
                .map(|i| json!({"sku": format!("SKU-{}", (run + i) % 500), "qty": i + 1}))
                .collect::<Vec<_>>()
        ),
    );
    // Text that does not compress away, as real state seldom does.
    let mut seed = (run as u64) * 7919 + write as u64 + 1;
    let notes: String = (0..bytes.saturating_sub(250))
        .map(|_| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            char::from(b"abcdefghijklmnopqrstuvwxyz0123456789 "[(seed >> 33) as usize % 37])
        })
        .collect();
    set.insert("notes".into(), json!(notes));
    StatePatch { set, clear: vec![] }
}

async fn database(admin: &PgPool, url: &str, name: &str, variant: &str) -> PgPool {
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {name}"))
        .execute(admin)
        .await;
    sqlx::query(&format!("CREATE DATABASE {name}"))
        .execute(admin)
        .await
        .unwrap();
    let options: PgConnectOptions = url.parse().unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(40)
        .connect_with(options.database(name))
        .await
        .unwrap();
    runtara_environment::migrations::run(&pool).await.unwrap();
    if variant == "off" {
        for (trigger, table) in TRIGGERS {
            sqlx::query(&format!("DROP TRIGGER {trigger} ON {table}"))
                .execute(&pool)
                .await
                .unwrap();
        }
    }
    sqlx::query("INSERT INTO images (image_id, tenant_id, name, binary_path) VALUES ('img', 't', 'orders:1', '/x')")
        .execute(&pool)
        .await
        .unwrap();
    pool
}

async fn lsn(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT pg_wal_lsn_diff(pg_current_wal_lsn(), '0/0')::bigint")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn size(pool: &PgPool, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT pg_total_relation_size('{table}')"))
        .fetch_one(pool)
        .await
        .unwrap()
}

struct Outcome {
    seconds: f64,
    times: Times,
    wal: i64,
    sizes: Vec<(&'static str, i64)>,
}

async fn run(pool: PgPool, runs: usize, writes: usize, workers: usize, bytes: usize) -> Outcome {
    let store = Arc::new(PostgresPersistence::new(pool.clone()));
    let times = Arc::new(Mutex::new(Times::default()));
    let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let labels = (runs * 4 / 5).max(1);
    sqlx::query("CHECKPOINT").execute(&pool).await.unwrap();
    let wal_before = lsn(&pool).await;
    let started = Instant::now();
    let mut tasks = Vec::new();
    for _ in 0..workers {
        let (pool, store, times, next) = (pool.clone(), store.clone(), times.clone(), next.clone());
        tasks.push(tokio::spawn(async move {
            let mut mine = Times::default();
            loop {
                let n = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if n >= runs {
                    break;
                }
                let id = format!("run-{n}");
                let label = format!("order-{}", n % labels);

                let t = Instant::now();
                let mut tx = pool.begin().await.unwrap();
                sqlx::query("INSERT INTO instances (instance_id, tenant_id, status, run_label) VALUES ($1, 't', 'pending', $2)")
                    .bind(&id).bind(&label).execute(&mut *tx).await.unwrap();
                sqlx::query("INSERT INTO instance_images (instance_id, image_id, tenant_id) VALUES ($1, 'img', 't')")
                    .bind(&id).execute(&mut *tx).await.unwrap();
                tx.commit().await.unwrap();
                mine.register.push(t.elapsed());

                let t = Instant::now();
                sqlx::query("UPDATE instances SET status = 'running', started_at = CURRENT_TIMESTAMP WHERE instance_id = $1")
                    .bind(&id).execute(&pool).await.unwrap();
                mine.start.push(t.elapsed());

                for w in 0..writes {
                    let t = Instant::now();
                    store
                        .apply_state("t", &id, &format!("{:064x}", n * 1000 + w), &patch(n, w, bytes))
                        .await
                        .unwrap();
                    mine.write.push(t.elapsed());
                }

                let t = Instant::now();
                sqlx::query("UPDATE instances SET status = 'completed', finished_at = CURRENT_TIMESTAMP, output = $2 WHERE instance_id = $1")
                    .bind(&id).bind(b"{\"ok\":true}".to_vec()).execute(&pool).await.unwrap();
                mine.complete.push(t.elapsed());
            }
            let mut all = times.lock().await;
            all.register.extend(mine.register);
            all.start.extend(mine.start);
            all.write.extend(mine.write);
            all.complete.extend(mine.complete);
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let seconds = started.elapsed().as_secs_f64();
    let wal = lsn(&pool).await - wal_before;
    sqlx::query("VACUUM ANALYZE").execute(&pool).await.unwrap();
    let mut sizes = Vec::new();
    for table in [
        "instances",
        "instance_state",
        "instance_state_writes",
        "run_keys",
    ] {
        sizes.push((table, size(&pool, table).await));
    }
    let times = Arc::try_unwrap(times).ok().unwrap().into_inner();
    Outcome {
        seconds,
        times,
        wal,
        sizes,
    }
}

#[tokio::main]
async fn main() {
    let url = std::env::var("SCALE_DATABASE_URL").expect("SCALE_DATABASE_URL");
    let args: Vec<usize> = std::env::args()
        .skip(1)
        .map(|a| a.parse().unwrap())
        .collect();
    let runs = args.first().copied().unwrap_or(4000);
    let writes = args.get(1).copied().unwrap_or(5);
    let workers = args.get(2).copied().unwrap_or(16);
    let admin = PgPool::connect(&url).await.unwrap();

    println!("{runs} runs, {writes} state writes each, {workers} workers\n");
    println!(
        "| state | run_keys | runs/s | register p50/p99 ms | start p50/p99 | write p50/p99 | complete p50/p99 | WAL MB | run_keys MB | instance_state MB |"
    );
    println!("| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
    for bytes in [300usize, 16_000] {
        // Each case twice, alternating, so neither always runs on a warmer server.
        for (round, variant) in [(0, "off"), (0, "on"), (1, "off"), (1, "on")] {
            let name = format!("write_cost_{variant}_{round}");
            let pool = database(&admin, &url, &name, variant).await;
            let mut o = run(pool.clone(), runs, writes, workers, bytes).await;
            let mb = |table: &str| {
                o.sizes
                    .iter()
                    .find(|(t, _)| *t == table)
                    .map(|(_, b)| *b as f64 / 1_048_576.0)
                    .unwrap_or(0.0)
            };
            let (run_keys, state) = (mb("run_keys"), mb("instance_state"));
            println!(
                "| {bytes} B | {} | {:.0} | {:.1} / {:.1} | {:.1} / {:.1} | {:.1} / {:.1} | {:.1} / {:.1} | {:.0} | {:.1} | {:.1} |",
                variant,
                runs as f64 / o.seconds,
                pct(&mut o.times.register, 0.5),
                pct(&mut o.times.register, 0.99),
                pct(&mut o.times.start, 0.5),
                pct(&mut o.times.start, 0.99),
                pct(&mut o.times.write, 0.5),
                pct(&mut o.times.write, 0.99),
                pct(&mut o.times.complete, 0.5),
                pct(&mut o.times.complete, 0.99),
                o.wal as f64 / 1_048_576.0,
                run_keys,
                state,
            );
            pool.close().await;
            sqlx::query(&format!("DROP DATABASE {name}"))
                .execute(&admin)
                .await
                .unwrap();
        }
    }
    let _ = Value::Null;
}
