// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! What concurrency per run label costs (`docs/workflow-records.md`):
//! labelled runs are admitted with the launch queue's own `claim_initial`,
//! started, and completed, from many workers at once, with the trigger that
//! hands a label on (`label_released`) in place or dropped, and with
//! `labelConcurrency: queue`, which also takes the label's lock and looks
//! for a holder at admission.
//!
//! ```text
//! SCALE_DATABASE_URL=postgres://…/any cargo run --release -p runtara-environment \
//!     --example label_release_cost -- [runs] [workers]
//! ```
//!
//! Makes its own databases (`label_cost_*`) next to the one named, and drops
//! them after.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use runtara_dsl::LabelConcurrency;
use runtara_environment::launch_queue::{
    EnqueueRequest, InitialLaunchOutcome, InitialLaunchRequest, LaunchKind, LaunchRepository,
};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use tokio::sync::Mutex;

fn pct(times: &mut [Duration], p: f64) -> f64 {
    if times.is_empty() {
        return 0.0;
    }
    times.sort();
    times[((times.len() as f64 - 1.0) * p).round() as usize].as_secs_f64() * 1000.0
}

async fn database(admin: &PgPool, url: &str, name: &str, trigger: bool) -> PgPool {
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
    if !trigger {
        sqlx::query("DROP TRIGGER label_released ON instances")
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query(
        "INSERT INTO images (image_id, tenant_id, name, binary_path) VALUES ('img', 't', 'orders:1', '/x')",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool
}

#[derive(Default)]
struct Times {
    admit: Vec<Duration>,
    complete: Vec<Duration>,
    waited: usize,
}

async fn run(pool: PgPool, runs: usize, workers: usize, policy: LabelConcurrency) -> (f64, Times) {
    let repository = LaunchRepository::new(pool.clone());
    let next = Arc::new(AtomicUsize::new(0));
    let all = Arc::new(Mutex::new(Times::default()));
    // About 1.25 runs a label, so labels are reused while earlier runs live.
    let labels = (runs * 4 / 5).max(1);
    let started = Instant::now();
    let mut tasks = Vec::new();
    for _ in 0..workers {
        let (pool, repository, next, all) =
            (pool.clone(), repository.clone(), next.clone(), all.clone());
        tasks.push(tokio::spawn(async move {
            let mut mine = Times::default();
            loop {
                let n = next.fetch_add(1, Ordering::Relaxed);
                if n >= runs {
                    break;
                }
                let id = format!("run-{n}");
                let request = InitialLaunchRequest {
                    run_label: Some(format!("order-{}", n % labels)),
                    parent: None,
                    launch: EnqueueRequest::immediate(
                        format!("launch-{n}"),
                        &id,
                        "t",
                        "img",
                        LaunchKind::Start,
                        Duration::from_secs(3600),
                    )
                    .with_workflow_scope("orders", false)
                    .with_label_policy(policy, false),
                    input: None,
                    env: None,
                    timeout_seconds: None,
                    shared_child_steps: Default::default(),
                };
                let t = Instant::now();
                let outcome = repository.claim_initial(request).await.unwrap();
                mine.admit.push(t.elapsed());
                if matches!(outcome, InitialLaunchOutcome::Waiting(_)) {
                    // A waiting run is handed the label when its holder ends;
                    // here it simply ends too, as a cancelled wait would.
                    mine.waited += 1;
                }
                sqlx::query(
                    "UPDATE instances SET status = 'running', started_at = NOW() WHERE instance_id = $1",
                )
                .bind(&id)
                .execute(&pool)
                .await
                .unwrap();
                let t = Instant::now();
                sqlx::query(
                    "UPDATE instances SET status = 'completed', finished_at = NOW() WHERE instance_id = $1",
                )
                .bind(&id)
                .execute(&pool)
                .await
                .unwrap();
                mine.complete.push(t.elapsed());
            }
            let mut all = all.lock().await;
            all.admit.extend(mine.admit);
            all.complete.extend(mine.complete);
            all.waited += mine.waited;
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
    let seconds = started.elapsed().as_secs_f64();
    (seconds, Arc::try_unwrap(all).ok().unwrap().into_inner())
}

#[tokio::main]
async fn main() {
    let url = std::env::var("SCALE_DATABASE_URL").expect("SCALE_DATABASE_URL");
    let args: Vec<usize> = std::env::args()
        .skip(1)
        .map(|a| a.parse().unwrap())
        .collect();
    let runs = args.first().copied().unwrap_or(8000);
    let workers = args.get(1).copied().unwrap_or(16);
    let admin = PgPool::connect(&url).await.unwrap();

    println!("{runs} labelled runs, about 1.25 a label, {workers} workers\n");
    println!("| variant | runs/s | admit p50 / p99 ms | complete p50 / p99 ms | waited |");
    println!("| --- | ---: | ---: | ---: | ---: |");
    for round in 0..2 {
        for (name, trigger, policy) in [
            ("allow, no release trigger", false, LabelConcurrency::Allow),
            ("allow, release trigger", true, LabelConcurrency::Allow),
            ("queue, release trigger", true, LabelConcurrency::Queue),
        ] {
            let db = format!("label_cost_{round}_{}", name.len());
            let pool = database(&admin, &url, &db, trigger).await;
            let (seconds, mut t) = run(pool.clone(), runs, workers, policy).await;
            println!(
                "| {name} | {:.0} | {:.1} / {:.1} | {:.1} / {:.1} | {} |",
                runs as f64 / seconds,
                pct(&mut t.admit, 0.5),
                pct(&mut t.admit, 0.99),
                pct(&mut t.complete, 0.5),
                pct(&mut t.complete, 0.99),
                t.waited,
            );
            pool.close().await;
            sqlx::query(&format!("DROP DATABASE {db}"))
                .execute(&admin)
                .await
                .unwrap();
        }
    }
}
